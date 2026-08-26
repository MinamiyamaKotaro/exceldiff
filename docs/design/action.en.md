# `action.yml` Design Document

*[日本語](action.md)*

Design document for the repo-root `action.yml` (`runs: using: composite`). It factors the steps [`.github/workflows/xlsx-diff.yml`](../../.github/workflows/xlsx-diff.yml) used to inline directly (this-repo-only) into a reusable composite action that other repositories can call via `uses:` ([Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23)).

As noted in [`cli.md`](cli.en.md)'s open question 1, [Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23) originally assumed "promote the CLI to `src/bin/xlsxdiff.rs`, then turn it into a composite action." In practice [Issue #31](https://github.com/MinamiyamaKotaro/exceldiff/issues/31)/[Issue #32](https://github.com/MinamiyamaKotaro/exceldiff/issues/32) instead adopted a separate workspace member, `cli/`. This design takes that current state as given and builds `cli/` from source inside the composite action — there is no need to publish the `cli` crate to crates.io (see "Open questions" below).

## Responsibilities / Scope

- Encapsulates Rust toolchain setup, building `cli/` (package `xlsxdiff`), computing the diff for each changed `.xlsx` file, and posting/updating the Markdown comment, all as composite-action `steps`.
- Removes the need for a calling workflow to duplicate these steps itself — this repo's own `.github/workflows/xlsx-diff.yml` dogfoods it by calling this `action.yml` via `uses: ./` (see "Test plan" below).
- **Explicitly out of scope**: parsing `.xlsx`, computing the diff, or Markdown formatting itself (all [`exceldiff::diff_file_section_from_paths`](markdown.en.md)'s and, by extension, [`cli/`](cli.en.md)'s responsibility); generalized inputs/outputs design ([Issue #24](https://github.com/MinamiyamaKotaro/exceldiff/issues/24)'s scope — this action just ports the current `xlsx-diff.yml`'s fixed behavior as-is: a fixed `**/*.xlsx` target, a fixed comment marker); pre-built binary distribution ([Issue #22](https://github.com/MinamiyamaKotaro/exceldiff/issues/22)/[Issue #28](https://github.com/MinamiyamaKotaro/exceldiff/issues/28), P2).

## Preconditions this action requires from its caller

Unlike an ordinary workflow job, a composite action cannot declare or perform two things on its own — the caller's own workflow is expected to supply them (documented in a comment at the top of `action.yml` itself):

- **A `permissions:` block**: composite action metadata has no `permissions:` key (only workflow/job level can declare it). If the caller hasn't granted `permissions: pull-requests: write`, the comment-posting step below fails on insufficient token scope.
- **Checkout**: a composite action does not check out the calling repository on its own. The diff-computation step runs `git show <sha>:<path>` against both the PR's base and head revisions, so the caller must have already run `actions/checkout@v4` with `fetch-depth: 0` (a shallow checkout only has the merge commit, not the other revisions).

Both follow from this action being `pull_request`-event-only in the first place (the diff step reads `github.event.pull_request.base.sha`/`head.sha`) — calling it from, say, `workflow_dispatch` produces no meaningful result.

## Key structure

```yaml
# action.yml
inputs:
  github-token:  # default ${{ github.token }}
runs:
  using: composite
  steps:
    - dtolnay/rust-toolchain@stable
    - Swatinem/rust-cache@v2         # workspaces: rooted at this action's own path
    - cargo build --release -p xlsxdiff --manifest-path ...
    - for each changed .xlsx file: git show + run xlsxdiff, assembling the Markdown
    - peter-evans/find-comment@v3
    - peter-evans/create-or-update-comment@v4
```

See [`action.yml`](../../action.yml) for the actual implementation.

## Design point: the caller's checkout and this action's own checkout live in different directories

When called from an external repository via `uses: owner/repo@ref`, the GitHub Actions runner fetches this action's own repository into a directory **separate** from wherever the caller's workflow has already checked itself out (the caller's `$PWD`/`GITHUB_WORKSPACE`). That path is exposed via the `${{ github.action_path }}` context. This separation matters in two places:

1. **Where the build output ends up**: specifying `--manifest-path "${{ github.action_path }}/Cargo.toml"` makes Cargo's default `target` directory land under the root of *that* manifest's workspace — i.e. `${{ github.action_path }}` — rather than the CWD (unless overridden by the `CARGO_TARGET_DIR` env var or `.cargo/config.toml`'s `build.target-dir`). So a later step reading the built binary back out must also anchor at `${{ github.action_path }}/target/release/xlsxdiff` — a directory unrelated to the caller's own `target/` (a side benefit: even a caller that's itself a Rust repo never has its `target/` polluted or raced against).
2. **`Swatinem/rust-cache`'s workspace setting**: the `workspaces` input defaults to `". -> target"`, where `.` means the caller's repository root (`GITHUB_WORKSPACE`) — unrelated to this action's own `Cargo.lock`. Left at the default, caching simply doesn't engage (especially when the caller isn't even a Rust repository and has no `Cargo.lock` at all). Explicitly setting `workspaces: "${{ github.action_path }} -> target"` (syntax: `$workspace -> $target`, `$target` defaults to `target` if omitted) roots the cache at this action's own workspace instead.

A couple of smaller adjustments:

- The scratch comment-body file is written under `${{ runner.temp }}` (the per-job scratch directory the runner provides) instead of the caller's working tree, so nothing is left behind there.
- The token used for posting is exposed as a `github-token` input, defaulting to `${{ github.token }}` (the calling workflow's own `GITHUB_TOKEN`) so a caller can override it with a custom PAT if needed. Both `peter-evans/find-comment` and `peter-evans/create-or-update-comment` already have their own `token` input (also defaulting to `${{ github.token }}`), so it's passed straight through to those.

## Dependencies

- Depends on: [`cli/`](cli.en.md) (built via `cargo build -p xlsxdiff`; the `xlsxdiff` binary is run once per `.xlsx` file changed in the PR. This action does not change `cli/`'s argv contract — `<display_path> <A|M|D> [base_file] [head_file]`)
- Depended on by: [`.github/workflows/xlsx-diff.yml`](../../.github/workflows/xlsx-diff.yml) — the only caller so far, referencing this action via `uses: ./` within this same repository. External repositories calling it via `uses: MinamiyamaKotaro/exceldiff@<tag>` is an intended future use, but no such external caller exists yet.

## Error handling policy

Following the same design as `cli/` itself (see [`main`'s error handling policy](cli.en.md)) — one file's parse error should never stop the rest of the PR's comment from posting — this action doesn't add any new explicit failure points beyond a build failure. For PRs from a fork, GitHub Actions forces `GITHUB_TOKEN` to read-only regardless of the caller's `permissions:` block, so the comment-posting step silently fails there (documented directly in `action.yml`). Nothing depends on that step, but since it's also this job's last step, a non-zero exit from the `peter-evans/*` actions there is still reported as the job failing — unchanged behavior carried over from the original inline `xlsx-diff.yml`.

## Test plan

A composite action is a YAML definition, not something `cargo test` exercises, so it's verified as follows:

1. **Static validation**: `action.yml` is checked as valid YAML (`actionlint` only understands workflow files under `.github/workflows/` and doesn't support composite action metadata files, so plain YAML parsing — e.g. Python's `yaml.safe_load` — is used instead). The calling workflow side (`.github/workflows/xlsx-diff.yml`) is additionally checked with `actionlint`.
2. **Unit-level check of the shell logic**: the "for each changed `.xlsx` file, `git show` both revisions and run `xlsxdiff`, concatenating the Markdown" script runs as plain `bash` once `${{ github.action_path }}`/`${{ runner.temp }}` are substituted with local paths. This was verified directly: a disposable local git repository was built with all three statuses (A/M/D) present in one diff, and running the script against it produced the expected Markdown output.
3. **Integration check on real GitHub Actions**: `.github/workflows/xlsx-diff.yml` itself now calls this action via `uses: ./` (see "Dependencies" above). This turns every future PR that touches an `.xlsx` file into a regression test of the whole action — toolchain setup, building rooted at `github.action_path`, `rust-cache`'s workspace setting, and comment posting — without needing a separate external test repository; this repository dogfoods itself.

## Open questions

1. **Publishing the `cli` crate to crates.io**: this action builds `cli/` from source, and `cli/Cargo.toml`'s `publish = false` is unchanged. Leave it as-is until there's an actual reason to publish (e.g. distributing pre-built binaries to cut a caller's build time — [Issue #22](https://github.com/MinamiyamaKotaro/exceldiff/issues/22)/[Issue #28](https://github.com/MinamiyamaKotaro/exceldiff/issues/28)).
2. **Generalizing inputs/outputs**: customizing the target path (currently fixed to `**/*.xlsx`) or the comment wording/marker is [Issue #24](https://github.com/MinamiyamaKotaro/exceldiff/issues/24)'s scope; this action only ports the current workflow's fixed behavior.
3. **Real-world verification from an external repository**: as of this design, only the self-dogfooding path (`uses: ./`) has been exercised. Actually calling it from a separate repository via `uses: MinamiyamaKotaro/exceldiff@<tag>` hasn't been tried yet — do that once a tagged release exists.
