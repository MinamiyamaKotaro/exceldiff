# `action.yml` Design Document

*[日本語](action.md)*

Design document for the repo-root `action.yml` (`runs: using: composite`). It factors the steps [`.github/workflows/xlsx-diff.yml`](../../.github/workflows/xlsx-diff.yml) used to inline directly (this-repo-only) into a reusable composite action that other repositories can call via `uses:` ([Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23)).

As noted in [`cli.md`](cli.en.md)'s open question 1, [Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23) originally assumed "promote the CLI to `src/bin/xlsxdiff.rs`, then turn it into a composite action." In practice [Issue #31](https://github.com/MinamiyamaKotaro/exceldiff/issues/31)/[Issue #32](https://github.com/MinamiyamaKotaro/exceldiff/issues/32) instead adopted a separate workspace member, `cli/`. This design takes that current state as given and builds `cli/` from source inside the composite action — there is no need to publish the `cli` crate to crates.io (see "Open questions" below).

## Responsibilities / Scope

- Encapsulates Rust toolchain setup, building `cli/` (package `xlsxdiff`), computing the diff for each changed `.xlsx` file, and posting/updating the Markdown comment, all as composite-action `steps`.
- Removes the need for a calling workflow to duplicate these steps itself — this repo's own `.github/workflows/xlsx-diff.yml` dogfoods it by calling this `action.yml` via `uses: ./` (see "Test plan" below).
- **Explicitly out of scope**: parsing `.xlsx`, computing the diff, or Markdown formatting itself (all [`exceldiff::diff_file_section_from_paths`](markdown.en.md)'s and, by extension, [`cli/`](cli.en.md)'s responsibility); pre-built binary distribution ([Issue #22](https://github.com/MinamiyamaKotaro/exceldiff/issues/22)/[Issue #28](https://github.com/MinamiyamaKotaro/exceldiff/issues/28), P2); a `changed-cells-count` output and commit-scoped diffing (both carved out as follow-up work under [Issue #24](https://github.com/MinamiyamaKotaro/exceldiff/issues/24) — see "Open questions" below).

## Inputs / outputs ([Issue #24](https://github.com/MinamiyamaKotaro/exceldiff/issues/24))

| input | type/default | what it does |
|---|---|---|
| `github-token` | string, `${{ github.token }}` | token used to post the comment |
| `files` | string, `*.xlsx` | a **git pathspec** passed straight to `git diff -- <files>` — not a shell glob. The default already matches at any depth without a leading `**/` |
| `comment` | bool string, `'true'` | post/update a PR comment |
| `job-summary` | bool string, `'false'` | also write to `$GITHUB_STEP_SUMMARY`. Independent of `comment` — a caller without `pull-requests: write` (e.g. a fork PR) can set `comment: false`/`job-summary: true` to see the diff without hitting a permission error |
| `max-rows-per-sheet` | numeric string, `'30'` | passed to [`MarkdownOptions::max_rows_per_sheet`](markdown.en.md) via `cli/`'s `--max-rows-per-sheet` flag |
| `diff-mode` | string enum `auto`\|`coordinate`, `'auto'` | passed to [`MarkdownOptions::diff_mode`](markdown.en.md) via `cli/`'s `--diff-mode` flag. `auto` is the current `diff_workbooks_best_effort` (auto-picks coordinate/row/column alignment); `coordinate` forces plain coordinate comparison, skipping alignment detection |

| output | type | what it is |
|---|---|---|
| `has-changes` | bool string | whether any file matching `files` changed in the PR |
| `changed-files-count` | numeric string | how many files matching `files` changed |

Both outputs are computable from `git diff --name-status`'s result (`$changed`) alone, so the "Compute diffs" step (`id: diff`) writes them straight to `$GITHUB_OUTPUT` with no change to `cli/` needed. A `changed-cells-count` output isn't implemented yet — `xlsxdiff` currently only emits a Markdown string, with no machine-readable added/modified/deleted count, so that would need a `cli/`-side change too (see "Open questions" below).

## Preconditions this action requires from its caller

Unlike an ordinary workflow job, a composite action cannot declare or perform two things on its own — the caller's own workflow is expected to supply them (documented in a comment at the top of `action.yml` itself):

- **A `permissions:` block**: composite action metadata has no `permissions:` key (only workflow/job level can declare it). With `comment` left at its default `true`, the comment-posting step below fails on insufficient token scope unless the caller has granted `permissions: pull-requests: write` (set `comment: false`/`job-summary: true` to avoid needing that permission at all — see "Inputs / outputs" above).
- **Checkout**: a composite action does not check out the calling repository on its own. The diff-computation step runs `git show <sha>:<path>` against both the PR's base and head revisions, so the caller must have already run `actions/checkout@v4` with `fetch-depth: 0` (a shallow checkout only has the merge commit, not the other revisions).

Both follow from this action being `pull_request`-event-only in the first place (the diff step reads `github.event.pull_request.base.sha`/`head.sha`) — calling it from, say, `workflow_dispatch` produces no meaningful result.

## Key structure

```yaml
# action.yml
inputs:
  github-token:         # default ${{ github.token }}
  files:                 # default '*.xlsx' (a git pathspec)
  comment:                # default 'true'
  job-summary:              # default 'false'
  max-rows-per-sheet:         # default '30'
  diff-mode:                   # default 'auto'
outputs:
  has-changes:             # steps.diff.outputs.has-changes
  changed-files-count:   # steps.diff.outputs.changed-files-count
runs:
  using: composite
  steps:
    - dtolnay/rust-toolchain@stable
    - Swatinem/rust-cache@v2         # workspaces: rooted at this action's own path
    - cargo build --release -p xlsxdiff --manifest-path ...
    - id: diff               # for each changed file: git show + run xlsxdiff, assembling
                               # the Markdown; writes has-changes/changed-files-count to
                               # $GITHUB_OUTPUT
    - if: inputs.job-summary  # append to $GITHUB_STEP_SUMMARY
    - if: inputs.comment
      uses: peter-evans/find-comment@v3
    - if: inputs.comment
      uses: peter-evans/create-or-update-comment@v4
```

See [`action.yml`](../../action.yml) for the actual implementation.

## Design point: the caller's checkout and this action's own checkout live in different directories

When called from an external repository via `uses: owner/repo@ref`, the GitHub Actions runner fetches this action's own repository into a directory **separate** from wherever the caller's workflow has already checked itself out (the caller's `$PWD`/`GITHUB_WORKSPACE`). That path is exposed via the `${{ github.action_path }}` context. This separation matters in two places:

1. **Where the build output ends up**: specifying `--manifest-path "${{ github.action_path }}/Cargo.toml"` makes Cargo's default `target` directory land under the root of *that* manifest's workspace — i.e. `${{ github.action_path }}` — rather than the CWD (unless overridden by the `CARGO_TARGET_DIR` env var or `.cargo/config.toml`'s `build.target-dir`). So a later step reading the built binary back out must also anchor at `${{ github.action_path }}/target/release/xlsxdiff` — a directory unrelated to the caller's own `target/` (a side benefit: even a caller that's itself a Rust repo never has its `target/` polluted or raced against).
2. **`Swatinem/rust-cache`'s workspace setting**: the `workspaces` input defaults to `". -> target"`, where `.` means the caller's repository root (`GITHUB_WORKSPACE`) — unrelated to this action's own `Cargo.lock`. Left at the default, caching simply doesn't engage (especially when the caller isn't even a Rust repository and has no `Cargo.lock` at all). Explicitly setting `workspaces: "${{ github.action_path }} -> target"` (syntax: `$workspace -> $target`, `$target` defaults to `target` if omitted) roots the cache at this action's own workspace instead.

A couple of smaller adjustments:

- The scratch comment-body file is written under `${{ runner.temp }}` (the per-job scratch directory the runner provides) instead of the caller's working tree, so nothing is left behind there.
- The token used for posting is exposed as a `github-token` input, defaulting to `${{ github.token }}` (the calling workflow's own `GITHUB_TOKEN`) so a caller can override it with a custom PAT if needed. Both `peter-evans/find-comment` and `peter-evans/create-or-update-comment` already have their own `token` input (also defaulting to `${{ github.token }}`), so it's passed straight through to those.

### Found after the fact: a trailing `while read ... done <<< "$var"` can fail unexpectedly under `set -e`

Discovered and fixed during live validation of the [Issue #24](https://github.com/MinamiyamaKotaro/exceldiff/issues/24) work (PR #38). The "Compute diffs" step ends with `if [ -z "$changed" ]; ... else; while IFS=$'\t' read -r file_status path; do ...; done <<< "$changed"; fi` — and when exactly **one** file had changed, the step would sometimes fail with `Process completed with exit code 1` (reproduced repeatedly on real GitHub Actions runners; never locally under plain `bash`). The apparent mechanism: the `while` loop is the last statement the script runs, and the loop's own termination (`read` returning non-zero once the herestring is exhausted) ended up becoming the whole script's exit status. The loop body itself ran correctly and `$OUT` was already written in full — the step was reported as failed despite having actually succeeded.

The fix: add a no-op `:` (always exit 0) as the script's actual last statement, right after the `if`/`fi` block — a standard defensive pattern that decouples the script's overall exit status from the loop's own trailing exit code. Notably, local `bash 3.2` never reproduced this pattern in the first place (per POSIX, a `while` loop's exit status should be that of the last command in its *body*, not a failing condition check) — so this is most likely specific to the GitHub Actions runner's `bash` (Ubuntu, 5.x), though the exact root cause wasn't fully pinned down. Three consecutive CI successes after adding `:` were treated as sufficient confirmation in practice.

This mattered because it risked breaking the action for exactly the single-changed-file case — the most common real-world shape a PR takes — and multi-file diff testing alone never caught it (self-dogfooding during [Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23) happened to always involve multiple changed files or coincidentally two-or-more, never a deliberate single-file case).

## Dependencies

- Depends on: [`cli/`](cli.en.md) (built via `cargo build -p xlsxdiff`; the `xlsxdiff` binary is run once per changed file, with the `--max-rows-per-sheet`/`--diff-mode` flags. The positional-argument contract — `<display_path> <A|M|D> [base_file] [head_file]` — is unchanged)
- Depended on by: [`.github/workflows/xlsx-diff.yml`](../../.github/workflows/xlsx-diff.yml) — the only caller so far, referencing this action via `uses: ./` within this same repository. External repositories calling it via `uses: MinamiyamaKotaro/exceldiff@<tag>` is an intended future use, but no such external caller exists yet.

## Error handling policy

Following the same design as `cli/` itself (see [`main`'s error handling policy](cli.en.md)) — one file's parse error should never stop the rest of the PR's comment from posting — this action doesn't add any new explicit failure points beyond a build failure. For PRs from a fork, GitHub Actions forces `GITHUB_TOKEN` to read-only regardless of the caller's `permissions:` block, so the comment-posting step silently fails there (documented directly in `action.yml`). Nothing depends on that step, but since it's also this job's last step, a non-zero exit from the `peter-evans/*` actions there is still reported as the job failing — unchanged behavior carried over from the original inline `xlsx-diff.yml`.

## Test plan

A composite action is a YAML definition, not something `cargo test` exercises, so it's verified as follows:

1. **Static validation**: `action.yml` is checked as valid YAML (`actionlint` only understands workflow files under `.github/workflows/` and doesn't support composite action metadata files, so plain YAML parsing — e.g. Python's `yaml.safe_load` — is used instead). The calling workflow side (`.github/workflows/xlsx-diff.yml`) is additionally checked with `actionlint`.
2. **Unit-level check of the shell logic**: the "for each changed file, `git show` both revisions, run `xlsxdiff`, concatenate the Markdown, and write `has-changes`/`changed-files-count` to `$GITHUB_OUTPUT`" script runs as plain `bash` once `${{ github.action_path }}`/`${{ runner.temp }}`/`$GITHUB_OUTPUT` are substituted with local paths. This was verified directly: a disposable local git repository was built with all three statuses (A/M/D) present in one diff, running the script against it produced the expected Markdown output, and both the changed and no-changes cases produced the correct `has-changes`/`changed-files-count` values. That `--max-rows-per-sheet`/`--diff-mode` actually reach `MarkdownOptions` is verified on the `cli/` side instead ([`cli/tests/cli.rs`](../../cli/tests/cli.rs), see "Dependencies" below) — from this script's own point of view, the flag values are just passed straight through to `"$BIN"`, so their meaning isn't re-verified here.
3. **Integration check on real GitHub Actions**: `.github/workflows/xlsx-diff.yml` itself now calls this action via `uses: ./` (see "Dependencies" above). This turns every future PR that touches an `.xlsx` file into a regression test of the whole action — toolchain setup, building rooted at `github.action_path`, `rust-cache`'s workspace setting, and comment posting — without needing a separate external test repository; this repository dogfoods itself. **This is the only layer that caught a real bug** (see "Found after the fact" above) — the single-changed-file case didn't reproduce in the local shell-script check (which used a diff with multiple statuses mixed together) or in static validation; only repeated trials on a real GitHub Actions runner reproduced and pinned it down. A concrete example of why an actual integration test can't be skipped when validating a composite action.

## Open questions

1. **Publishing the `cli` crate to crates.io**: this action builds `cli/` from source, and `cli/Cargo.toml`'s `publish = false` is unchanged. Leave it as-is until there's an actual reason to publish (e.g. distributing pre-built binaries to cut a caller's build time — [Issue #22](https://github.com/MinamiyamaKotaro/exceldiff/issues/22)/[Issue #28](https://github.com/MinamiyamaKotaro/exceldiff/issues/28)).
2. **Generalizing inputs/outputs (continued)**: `files`/`comment`/`job-summary`/`max-rows-per-sheet`/`diff-mode` inputs and `has-changes`/`changed-files-count` outputs are implemented ([Issue #24](https://github.com/MinamiyamaKotaro/exceldiff/issues/24)). Left as follow-up work:
   - A `changed-cells-count` output: `xlsxdiff` currently only emits a Markdown string, with no machine-readable added/modified/deleted count. Needs a `cli/`-side change (e.g. a summary line to stderr like `added=N modified=M deleted=D`) that `action.yml` sums across files.
   - Commit-scoped diffing (`diff-scope`): today the diff is always the PR's cumulative `base.sha`⇔`head.sha` comparison — a file added and later modified within the same PR always reports as `Added` (see [the Issue #23 discussion](https://github.com/MinamiyamaKotaro/exceldiff/issues/23)). Switching to a `push` scope (the immediately-preceding push's `before`/`after`) or a `commit` scope (each commit in the PR diffed against its predecessor) changes the comment's own shape — from one section per PR to potentially several — so it's deprioritized to a separate P2 task.
   - Customizing the comment wording/marker (`<!-- xlsx-diff-comment -->`) itself stays out of scope until there's a concrete need for it.
   - `files` was implemented as a git pathspec (not a shell glob) — distinct from GitHub Actions' own `paths:` trigger-filter syntax; this action doesn't control the calling workflow's trigger at all.
3. **Real-world verification from an external repository**: as of this design, only the self-dogfooding path (`uses: ./`) has been exercised. Actually calling it from a separate repository via `uses: MinamiyamaKotaro/exceldiff@<tag>` hasn't been tried yet — do that once a tagged release exists.
