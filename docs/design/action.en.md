# `action.yml` Design Document

*[日本語](action.md)*

Design document for the repo-root `action.yml` (`runs: using: composite`). It factors the steps [`.github/workflows/xlsx-diff.yml`](../../.github/workflows/xlsx-diff.yml) used to inline directly (this-repo-only) into a reusable composite action that other repositories can call via `uses:` ([Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23)).

As noted in [`cli.md`](cli.en.md)'s open question 1, [Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23) originally assumed "promote the CLI to `src/bin/xlsxdiff.rs`, then turn it into a composite action." In practice [Issue #31](https://github.com/MinamiyamaKotaro/exceldiff/issues/31)/[Issue #32](https://github.com/MinamiyamaKotaro/exceldiff/issues/32) instead adopted a separate workspace member, `cli/`. This design takes that current state as given and builds `cli/` from source inside the composite action — there is no need to publish the `cli` crate to crates.io (see "Open questions" below).

## Responsibilities / Scope

- Encapsulates resolving the `xlsxdiff` binary (downloading a pre-built release, falling back to Rust toolchain setup + building `cli/` from source only when that download isn't available — [Issue #28](https://github.com/MinamiyamaKotaro/exceldiff/issues/28), see "Pre-built binary distribution" below), computing the diff for each changed `.xlsx` file, and posting/updating the Markdown comment, all as composite-action `steps`.
- Removes the need for a calling workflow to duplicate these steps itself — this repo's own `.github/workflows/xlsx-diff.yml` dogfoods it by calling this `action.yml` via `uses: ./` (see "Test plan" below).
- When `visual: true`, collects [`grid.rs`'s Excel-like grid HTML](grid.en.md) page for each changed sheet, uploads every page from a single job run as one GitHub Actions artifact (`actions/upload-artifact@v4`), and appends its download link under the text diff (see "Visual mode" below). Actually collecting and publishing that HTML is [explicitly outside `grid.rs`'s own scope](grid.en.md), so this action is where that wiring lives.
- **Explicitly out of scope**: parsing `.xlsx`, computing the diff, Markdown formatting, or grid HTML rendering itself (all [`exceldiff::diff_file_section_from_paths`](markdown.en.md)'s/[`grid_sections_from_paths`](grid.en.md)'s and, by extension, [`cli/`](cli.en.md)'s responsibility); a `changed-cells-count` output (carved out as follow-up work under [Issue #24](https://github.com/MinamiyamaKotaro/exceldiff/issues/24), still unstarted as of [Issue #43](https://github.com/MinamiyamaKotaro/exceldiff/issues/43) — see "Open questions" below). Commit-scoped diffing is implemented as `diff-scope: commit` (see "Inputs / outputs" below). Pre-built binary distribution is implemented as `release.yml` plus `action.yml`'s "Resolve xlsxdiff binary" step (see "Pre-built binary distribution" below).

## Inputs / outputs ([Issue #24](https://github.com/MinamiyamaKotaro/exceldiff/issues/24))

| input | type/default | what it does |
|---|---|---|
| `github-token` | string, `${{ github.token }}` | token used to post the comment |
| `files` | string, `*.xlsx` | a **git pathspec** passed straight to `git diff -- <files>` — not a shell glob. The default already matches at any depth without a leading `**/` |
| `comment` | bool string, `'true'` | post/update a PR comment |
| `job-summary` | bool string, `'false'` | also write to `$GITHUB_STEP_SUMMARY`. Independent of `comment` — a caller without `pull-requests: write` (e.g. a fork PR) can set `comment: false`/`job-summary: true` to see the diff without hitting a permission error |
| `max-rows-per-sheet` | numeric string, `'30'` | passed to [`MarkdownOptions::max_rows_per_sheet`](markdown.en.md) via `cli/`'s `--max-rows-per-sheet` flag |
| `diff-mode` | string enum `auto`\|`coordinate`, `'auto'` | passed to [`MarkdownOptions::diff_mode`](markdown.en.md) via `cli/`'s `--diff-mode` flag. `auto` is the current `diff_workbooks_best_effort` (auto-picks coordinate/row/column alignment); `coordinate` forces plain coordinate comparison, skipping alignment detection |
| `diff-scope` | string enum `pr`\|`commit`, `'pr'` | `pr` (default) diffs the PR's cumulative `base.sha`⇔`head.sha` as one section per changed file, same as always. `commit` instead enumerates every commit the PR introduces via `git log --reverse base.sha..head.sha` and renders one subsection per commit, diffed against its immediate parent (`<commit>^1`), under a `## Commit <short-sha> — <subject>` heading — fixes a file added and then modified within the same PR always showing as `Added` ([Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23), [Issue #43](https://github.com/MinamiyamaKotaro/exceldiff/issues/43)). Doesn't affect `has-changes`/`changed-files-count`, which always stay cumulative. Combined with `visual: true`, the grid HTML destination is also namespaced by commit (see "Visual mode" below). A `push` scope (the immediately-preceding push's `before`/`after`) isn't implemented yet (see "Open questions") |
| `visual` | bool string, `'false'` | render an Excel-like grid view (a standalone HTML page) for each changed sheet and attach them to the comment as a downloadable workflow artifact. Needs no extra `permissions:` (see "Preconditions" below) |

| output | type | what it is |
|---|---|---|
| `has-changes` | bool string | whether any file matching `files` changed in the PR |
| `changed-files-count` | numeric string | how many files matching `files` changed |

Both outputs are computable from `git diff --name-status`'s result (`$changed`) alone, so the "Compute diffs" step (`id: diff`) writes them straight to `$GITHUB_OUTPUT` with no change to `cli/` needed. A `changed-cells-count` output isn't implemented yet — `xlsxdiff` currently only emits a Markdown string, with no machine-readable added/modified/deleted count, so that would need a `cli/`-side change too (see "Open questions" below).

## Preconditions this action requires from its caller

Unlike an ordinary workflow job, a composite action cannot declare or perform two things on its own — the caller's own workflow is expected to supply them (documented in a comment at the top of `action.yml` itself):

- **A `permissions:` block**: composite action metadata has no `permissions:` key (only workflow/job level can declare it).
  - **`contents: read` is always required, regardless of `comment`/`visual`** — it's what lets `actions/checkout@v4` itself fetch the repository at all. GitHub Actions sets every scope you don't list to `none` (not the repo's default) as soon as you write a `permissions:` block at all, so a caller who writes only `permissions: pull-requests: write` silently loses `contents` access, and `actions/checkout` fails with a generic "repository not found" — a real bug hit while verifying this action from an external repo via `uses:` ([Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23)), and one that doesn't obviously look like a permissions problem from that error alone.
  - With `comment` left at its default `true`, the comment-posting step below fails on insufficient token scope unless the caller has also granted `permissions: pull-requests: write` (set `comment: false`/`job-summary: true` to avoid needing that permission at all — see "Inputs / outputs" above).
  - `visual: true` needs no additional permission — the grid HTML pages are uploaded via `actions/upload-artifact@v4`, authorized through a separate `ACTIONS_RUNTIME_TOKEN` outside the `GITHUB_TOKEN` permissions model (see "Visual mode" below).
- **Checkout**: a composite action does not check out the calling repository on its own. The diff-computation step runs `git show <sha>:<path>` against both the PR's base and head revisions, so the caller must have already run `actions/checkout@v4` with `fetch-depth: 0` (a shallow checkout only has the merge commit, not the other revisions).

Both follow from this action being `pull_request`-event-only in the first place (the diff step reads `github.event.pull_request.base.sha`/`head.sha`) — calling it from, say, `workflow_dispatch` produces no meaningful result.

## Branding / Marketplace category ([Issue #27](https://github.com/MinamiyamaKotaro/exceldiff/issues/27))

`action.yml`'s `branding` is `icon: grid` (from Feather v4.28.0 — matches this action's own `visual: true` mode, which renders an actual Excel-like "grid" view of each changed sheet, see [`grid.rs`](grid.en.md)) and `color: green` (a product decision).

The Marketplace listing category has no corresponding field in this repo (it's picked in the listing UI at publish time), so it's recorded here as a decision instead: primary category **Code review** (posting a diff-preview comment on a PR is this action's main function), secondary category **Utilities**.

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
  diff-scope:                   # default 'pr' ('pr' | 'commit')
  visual:                        # default 'false'
outputs:
  has-changes:             # steps.diff.outputs.has-changes
  changed-files-count:   # steps.diff.outputs.changed-files-count
runs:
  using: composite
  steps:
    - id: resolve_binary      # if github.action_ref is non-empty, downloads + checksum-verifies
                               # the matching pre-built release binary; sets found=true/bin-path
                               # on success (Issue #28)
    - if: steps.resolve_binary.outputs.found != 'true'
      uses: dtolnay/rust-toolchain@stable
    - if: steps.resolve_binary.outputs.found != 'true'
      uses: Swatinem/rust-cache@v2   # workspaces: rooted at this action's own path
    - id: build_fallback      # only when found != true: cargo build --release -p xlsxdiff
      if: steps.resolve_binary.outputs.found != 'true'
    - id: diff               # BIN = resolve_binary's (on success) or build_fallback's bin-path.
                               # writes has-changes/changed-files-count to $GITHUB_OUTPUT from the
                               # cumulative base..head diff, always. diff-scope: pr (default) then
                               # runs git show + xlsxdiff once per changed file; diff-scope: commit
                               # instead runs it once per (PR commit x file changed in that commit),
                               # nested under "## Commit <short-sha> — <subject>" subsections. If
                               # visual: true, also collects each sheet's HTML page into
                               # ${{ runner.temp }}/xlsx-diff-visuals/ (namespaced under
                               # commit-short-sha/ in commit mode) and writes has-visuals to
                               # $GITHUB_OUTPUT
    - if: inputs.visual && steps.diff.outputs.has-visuals == 'true'
      id: upload_visuals      # actions/upload-artifact@v4 uploads xlsx-diff-visuals/ as one artifact
    - if: inputs.visual && steps.diff.outputs.has-visuals == 'true'
                               # appends upload_visuals.outputs.artifact-url to the comment Markdown
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

## Visual mode design (`visual: true`)

GitHub sanitizes `style=` attributes out of PR comment HTML, so [`grid.rs`](grid.en.md)'s colored, bordered grid HTML can't be pasted into the comment body directly.

**Current delivery path** (see [the Issue #23 discussion](https://github.com/MinamiyamaKotaro/exceldiff/issues/23) for the original decision, and [Issue #47](https://github.com/MinamiyamaKotaro/exceldiff/issues/47) for why it replaced the push-based scheme described below):

1. **Collecting the combined HTML page**: `xlsxdiff --grid-html-dir <dir>` writes every changed sheet combined onto one page (`<dir>/grid.html`, wrapped via `exceldiff::wrap_grid_page` — self-contained: inline `<style>` only, no external assets), which is copied as-is to `${{ runner.temp }}/xlsx-diff-visuals/{sanitize(file path)}.html` (`sanitize` collapses anything outside alphanumerics/`._-` to `_`) — one copy per changed file, not per sheet. Never committed to `git`. Sheet names collected from `manifest.tsv` are joined with commas into `$VISUALS_LIST` (`path\tsheet1,sheet2,...`) for the comment's bullet list below. The diff step's `has-visuals` output flips to `true` once at least one page has been collected.
2. **Uploaded as a single artifact**: once every changed file has been processed, and only if `has-visuals == 'true'`, `actions/upload-artifact@v4` uploads the whole `xlsx-diff-visuals/` directory as one artifact (`xlsx-diff-grids`) — once per job run, not once per changed file.
3. **Download link appended**: the upload step's `artifact-url` output (shape: `https://github.com/{owner}/{repo}/actions/runs/{run_id}/artifacts/{artifact_id}`) is appended to the comment Markdown the diff step already wrote, in a separate later step (the URL only exists once the artifact does, so the diff step itself can't know it yet). Downloading that URL requires being logged in to GitHub — in practice, only someone with read access to this repository can download it (documented in [`actions/upload-artifact`'s README](https://github.com/actions/upload-artifact)).

**Best-effort per changed file**: if copying the HTML fails, only that one changed file's page is skipped (a warning goes to stderr) — processing continues, the same "one failure doesn't stop everything else" policy [`cli/`'s error handling](cli.en.md) already follows. The upload itself only happens once per job, so the push-race handling the old design needed is gone.

**Combined with `diff-scope: commit` (Issue #43)**: if the destination path were keyed on file path alone (`{sanitize(file path)}.html`), the same file changed by more than one commit would have the later commit's page silently overwrite the earlier one's — reproduced directly in the Issue #43 PoC before deciding on a fix. To avoid that, `diff-scope: commit` namespaces the destination as `{commit-short-sha}/{sanitize(file path)}.html` and adds a leading commit-label column to `$VISUALS_LIST` (`commit_label\tpath\tsheet1,sheet2,...`), so the comment's closing bullet list is also grouped per commit under its own bolded "Commit" subheading naming that commit's short SHA. In `diff-scope: pr` (default) this column is fixed to a literal `-`, never an empty string — even with `IFS` set to just a tab, tab still counts as "IFS whitespace" to `read`, which silently drops a leading empty field and shifts every later field left by one. Setting that column to an empty string was tried first and genuinely broke `diff-scope: pr`'s own output; switching to the non-empty placeholder fixed it.

### History: replacing the push-based delivery with an artifact (Issue #47)

The original design: PNGs were committed and pushed to `xlsx-diff-images`, a branch with no shared history with the code (via a parallel `git worktree` — never touching the caller's own working tree). Path scheme: `pr-{PR number}/{first 7 chars of head's SHA}/{sanitize(file path)}/sheet-{sanitize(sheet name)}.png`. The actual commit SHA the push produced (not the branch's moving tip, so a later push couldn't break an earlier, already-posted URL) was used to build `https://raw.githubusercontent.com/{owner}/{repo}/{commit_sha}/{path}`, embedded as a Markdown image right after that file's text diff. Push races (multiple PRs touching `.xlsx` files concurrently, more than one job pushing to the same branch at once) were handled by `push_image` retrying up to 5 times on a failed `git push`, fetching and rebasing between attempts (linear `sleep $attempt` backoff). No GitHub Pages involved — just a plain push.

This design had a real defect: **screenshots were unviewable on private repositories**. `raw.githubusercontent.com` is a different domain from `github.com`, and a browser's `github.com` session cookie doesn't automatically carry over — so even a user with real read access to the repo saw a broken image ([reported and investigated in Issue #47](https://github.com/MinamiyamaKotaro/exceldiff/issues/47)). Two alternatives were tried and rejected after hands-on verification:

- **Base64 data-URI embedding** (`![](data:image/png;base64,...)`): confirmed live, by posting to an actual issue comment and inspecting the GitHub REST API's `body_html`, that GitHub's comment sanitizer strips the `data:`-scheme `img src` entirely — both in Markdown and raw-HTML form.
- **Uploading via `uploads.github.com`'s user-attachments endpoint**: the image itself rendered correctly in a live test (rewritten to a signed `private-user-images.githubusercontent.com`/JWT URL), but an unauthenticated, cookieless request to the generated attachment URL (`https://github.com/user-attachments/assets/<uuid>`) still received a 302 to a valid signed URL — suggesting the access control is obscurity (an unguessable URL), not a real repository-permission check. Given this tool handles business data, that didn't meet the bar. It's also an undocumented, unofficial API, and whether it works with a composite action's `GITHUB_TOKEN` (`ghs_`) was never confirmed either.

The final choice, `actions/upload-artifact@v4` + `artifact-url`, rides GitHub's own permission model (repository read access) instead of reinventing one. The trade-off is losing the inline PR-timeline preview in exchange for an extra download click — a deliberate choice of "only people with real repo access can see this" over "visible, but actually visible to anyone with the link." This also incidentally resolved the "`xlsx-diff-images` branch growth" open question below, since there's no longer a branch to grow. See [Issue #47](https://github.com/MinamiyamaKotaro/exceldiff/issues/47)'s comments for the raw verification logs.

### History, part 2: replacing the screenshot PNG with the HTML page itself (Issue #47, right after the artifact switch)

The artifact switch above was implemented and verified end-to-end in PR #48, including a private-repo access-control test. Immediately after that, testing against a genuinely large sheet on a private repo (a real "skill sheet" fixture, 25,517 cells on one sheet) surfaced feedback that the resulting screenshot PNG was too large to make out — Playwright's element screenshot (`page.locator(".page-content").screenshot()`) renders at the sheet's actual pixel dimensions, so a sheet with more cells produces a physically larger image, and most image viewers shrink an oversized image to fit the window — the net effect being that the biggest sheets became the *least* legible.

The fix: drop the screenshot-rendering step entirely and attach `wrap_grid_page`'s standalone HTML page directly to `xlsx-diff-visuals/` instead. Since it has no external dependencies, downloading and opening it in a browser behaves like any ordinary web page — scroll and zoom (the browser's own zoom) work regardless of how large the sheet is.

This also removed the `action-scripts/` directory (`screenshot.mjs`, `package.json`) and `action.yml`'s `Install Node.js`/`Install screenshot dependencies` steps (Node.js setup, `npm install`, `npx playwright install --with-deps chromium`) — Playwright/Chromium is no longer needed at all, which incidentally cuts `visual: true`'s job runtime significantly too.

### History, part 3: one combined page per changed file instead of one page per sheet (Issue #47)

Right after "History, part 2" replaced the PNG with HTML, further feedback asked for one combined view instead of separate files per sheet. At that point, `write_grid_sections` (`cli/src/main.rs`) called `wrap_grid_page` once *per sheet*, writing separate `sheet-{i}.html` files — a changed file with several changed sheets produced that many separate downloads inside the artifact.

`wrap_grid_page` was already designed to accept one or more concatenated fragments in a single call (`examples/xlsx_diff_grid.rs` already does this), so the fix was simply to concatenate every sheet's fragment first and call `wrap_grid_page` once, instead of once per sheet. The output file name changed from `sheet-{i}.html` to a fixed `grid.html` (one per changed file), and every line of `manifest.tsv` now points at that same `grid.html`.

`action.yml`'s collection loop was updated to match: instead of copying once per sheet, it now takes the `html_path` from `manifest.tsv`'s first line (they're all identical), copies it once to a fixed `sanitize(path).html` (no more `sanitize(path)/sheet-sanitize(name).html` subdirectory), and joins every sheet name from `manifest.tsv` with commas into `$VISUALS_LIST` instead of one line per sheet. The comment's bullet list changed accordingly, from one line per sheet (`- path — sheet1` / `- path — sheet2`) to one line per file (`- path — sheet1,sheet2`).

Added a dedicated test to `cli/tests/cli.rs`, `grid_html_dir_combines_every_changed_sheet_into_one_page`, passing a two-sheet `.xlsx` pair (built with a new `xlsx_zip_multi_sheet` helper — an in-memory minimal pair rather than reusing two unrelated real fixtures, same reasoning [[feedback_test_fixture_determinism]] gives elsewhere) and confirming both `manifest.tsv` lines point at the same `grid.html`, exactly one HTML file is actually written, and it contains both sheets' `class="sheet"` sections.

## Pre-built binary distribution ([Issue #28](https://github.com/MinamiyamaKotaro/exceldiff/issues/28))

The composite action (P0) always required a `cargo build` on every invocation, which was Issue #28's original concern about slowness. A PoC done before implementing (see Issue #28's comments) measured this and found the concern partly right, but for a different reason than assumed: `dtolnay/rust-toolchain` takes ~0.6s and `Swatinem/rust-cache`'s restore ~0.4s, both cheap — but **`Swatinem/rust-cache` never actually gets a hit, because `xlsx-diff.yml` is `pull_request`-only (never runs on a push to `master`), so the default branch never gets its own cache entry for this key, and every new PR's first run starts cold (`No cache found.`)**. `cargo build --release -p xlsxdiff` itself measured at ~14 seconds in that state.

### Design: download-first, transparent fallback to a source build

`action.yml`'s first step, "Resolve xlsxdiff binary," does the following (see [`action.yml`](../../action.yml) for the implementation):

1. If `${{ github.action_ref }}` (the ref from the caller's `uses: owner/repo@ref`) is non-empty and `runner.os`/`runner.arch` map to a known combination (the supported-targets table below), it fetches `https://github.com/MinamiyamaKotaro/exceldiff/releases/download/{action_ref}/xlsxdiff-{action_ref}-{target}.tar.gz` and its `SHA256SUMS` via `curl -fsSL`, verifies the checksum (`sha256sum -c`, falling back to `shasum -a 256 -c` since macOS's BSD userland has no `sha256sum` at all), and only extracts + `chmod +x`'s a binary that passed verification.
2. If any of the above doesn't pan out (empty ref, unsupported platform, download failure, checksum mismatch), it **unconditionally** falls back to the original source-build path — `dtolnay/rust-toolchain` + `Swatinem/rust-cache` + `cargo build --release -p xlsxdiff` — simply by attaching `if: steps.resolve_binary.outputs.found != 'true'` to those three steps (a composite action's per-step `if:` makes skipping a whole step this cleanly possible).
3. **Deliberately no shape-based pre-filter on `action_ref`** — any non-empty value triggers a download attempt. An early PoC draft gated this behind a `^v[0-9]+\.[0-9]+` regex to only attempt version-tag-shaped refs, but that missed this repo's own README-documented usage, `uses: MinamiyamaKotaro/exceldiff@v1` (major-version-only), and would have silently skipped the download and fallen back to a source build every time — a real bug caught and fixed live in Issue #28's comments. Since a failed `curl` (404) already falls through to the source build correctly, the shape check turned out to add nothing.

This design means this repo's own `.github/workflows/xlsx-diff.yml` (`uses: ./`, `action_ref` always empty) **automatically** keeps taking the source-build path — the mechanism that lets pre-built binaries roll out at all is, by construction, the same one that guarantees this repo's own self-dogfooding never breaks.

### `release.yml` (new)

`.github/workflows/release.yml` triggers on a `v*`-shaped tag push, matrix-builds on `ubuntu-latest`/`macos-latest` (twice — native `aarch64-apple-darwin` and a same-OS cross-build to `x86_64-apple-darwin`)/`windows-latest`, packages each target's binary as `xlsxdiff-{tag}-{target}.tar.gz` via plain `tar` (every target uses `.tar.gz`, Windows included — `windows-latest` has a working `tar` too, so a separate `.zip`/`Compress-Archive` path wasn't needed), and a final aggregation job generates a `SHA256SUMS` covering every asset and attaches everything to a GitHub Release via `gh release create`.

`xlsxdiff`'s dependency graph (checked with `cargo tree`: `quick-xml`/`serde`/`serde_json`/`thiserror`/`zip` — even `zip`'s `deflate` feature is pure Rust via `flate2` → `zlib-rs`, not C zlib) has zero C/native dependencies (`exceldiff`'s optional `diff-storage` feature, which pulls in `rusqlite`'s bundled-C-SQLite build, is never enabled here since `cli/Cargo.toml`'s `exceldiff = { path = ".." }` requests no features). So every target builds by compiling natively on that OS's own GitHub-hosted runner — no `cross`/Docker needed at all; a real local test even confirmed a same-machine cross-build (arm64 macOS → x86_64 macOS) succeeding in 7 seconds.

Supported targets, in priority order:

| target | priority | notes |
|---|---|---|
| `x86_64-unknown-linux-gnu` | P0 | matches `ubuntu-latest`; the overwhelming majority of real callers |
| `aarch64-apple-darwin` | P1 | `macos-latest` is an arm64 host today |
| `x86_64-apple-darwin` | P1 | same-OS cross-build |
| `x86_64-pc-windows-msvc` | P1 | `windows-latest` |
| `aarch64-unknown-linux-gnu` | P2 (not implemented) | likely needs a cross-linker; out of scope for now — see "Open questions" below |

### Live verification (`v0.14.0-rc1`)

After the local checks — (a) mocking the download-success path with a local HTTP server, (b) the checksum-mismatch fallback, (c) target-triple resolution across every `runner.os`/`runner.arch` combination — a verification-only pre-release tag `v0.14.0-rc1` was actually cut and `release.yml` run for real, confirming (full detail in "Open questions" item 6 below):

- `release.yml` successfully built all 4 targets and published them to a GitHub Release with a `SHA256SUMS`. Downloaded an asset locally with a plain `curl`, verified its checksum (`OK`), extracted it, and confirmed the result is a genuine ELF executable.
- `action.yml`'s download-success path was verified by calling `uses: MinamiyamaKotaro/exceldiff@v0.14.0-rc1` from the existing throwaway external repo (`MinamiyamaKotaro/exceldiff-action-verify`) — the job log shows `dtolnay/rust-toolchain`, `Swatinem/rust-cache`, and `cargo build` never ran at all; "Resolve xlsxdiff binary"'s `curl` download + checksum check alone resolved the binary (`found=true`), and the run completed straight through to posting the comment and attaching the grid artifact (about 6 seconds from checkout start to the comment-posting step starting).

## Dependencies

- Depends on: [`cli/`](cli.en.md) (run once per changed file, with the `--max-rows-per-sheet`/`--diff-mode` flags, plus `--grid-html-dir` when `visual: true`. The positional-argument contract — `<display_path> <A|M|D> [base_file] [head_file]` — is unchanged. See "Pre-built binary distribution" above for how the binary itself is obtained). ~~`action-scripts/`~~ (a Playwright-dependent Node package) was removed, see "History, part 2" above — `visual: true` now runs on the Rust toolchain alone.
- Depended on by: [`.github/workflows/xlsx-diff.yml`](../../.github/workflows/xlsx-diff.yml) — the only caller so far, referencing this action via `uses: ./` within this same repository. External repositories calling it via `uses: MinamiyamaKotaro/exceldiff@<tag>` is an intended future use, but no such external caller exists yet. [`.github/workflows/release.yml`](../../.github/workflows/release.yml) — new dependent, publishing `xlsxdiff`'s pre-built binaries to a GitHub Release on a tag push.

## Error handling policy

Following the same design as `cli/` itself (see [`main`'s error handling policy](cli.en.md)) — one file's parse error should never stop the rest of the PR's comment from posting — this action doesn't add any new explicit failure points beyond a build failure. For PRs from a fork, GitHub Actions forces `GITHUB_TOKEN` to read-only regardless of the caller's `permissions:` block, so the comment-posting step silently fails there (documented directly in `action.yml`). Nothing depends on that step, but since it's also this job's last step, a non-zero exit from the `peter-evans/*` actions there is still reported as the job failing — unchanged behavior carried over from the original inline `xlsx-diff.yml`. The `visual: true` grid HTML upload (`actions/upload-artifact@v4`) itself is authorized through a path separate from `GITHUB_TOKEN`'s permissions (`ACTIONS_RUNTIME_TOKEN`), so it's expected to keep working even on a fork PR — only the comment-posting step still fails there, for the reason above (unverified so far, see "Open questions" below).

## Test plan

A composite action is a YAML definition, not something `cargo test` exercises, so it's verified as follows:

1. **Static validation**: `action.yml` is checked as valid YAML (`actionlint` only understands workflow files under `.github/workflows/` and doesn't support composite action metadata files, so plain YAML parsing — e.g. Python's `yaml.safe_load` — is used instead). The calling workflow side (`.github/workflows/xlsx-diff.yml`) is additionally checked with `actionlint`.
2. **Unit-level check of the shell logic**: the "for each changed file, `git show` both revisions, run `xlsxdiff`, concatenate the Markdown, and write `has-changes`/`changed-files-count` to `$GITHUB_OUTPUT`" script runs as plain `bash` once `${{ github.action_path }}`/`${{ runner.temp }}`/`$GITHUB_OUTPUT` are substituted with local paths. This was verified directly: a disposable local git repository was built with all three statuses (A/M/D) present in one diff, running the script against it produced the expected Markdown output, and both the changed and no-changes cases produced the correct `has-changes`/`changed-files-count` values. That `--max-rows-per-sheet`/`--diff-mode` actually reach `MarkdownOptions` is verified on the `cli/` side instead ([`cli/tests/cli.rs`](../../cli/tests/cli.rs), see "Dependencies" below) — from this script's own point of view, the flag values are just passed straight through to `"$BIN"`, so their meaning isn't re-verified here.
3. **Integration check on real GitHub Actions**: `.github/workflows/xlsx-diff.yml` itself now calls this action via `uses: ./` (see "Dependencies" above). This turns every future PR that touches an `.xlsx` file into a regression test of the whole action — toolchain setup, building rooted at `github.action_path`, `rust-cache`'s workspace setting, and comment posting — without needing a separate external test repository; this repository dogfoods itself. **This is the only layer that caught a real bug** (see "Found after the fact" above) — the single-changed-file case didn't reproduce in the local shell-script check (which used a diff with multiple statuses mixed together) or in static validation; only repeated trials on a real GitHub Actions runner reproduced and pinned it down. A concrete example of why an actual integration test can't be skipped when validating a composite action.
4. **`visual`-mode-specific checks**: the diff step's shell logic was run standalone against a disposable git repo with `VISUAL=true`, confirming `xlsxdiff --grid-html-dir`'s output lands unmodified under `xlsx-diff-visuals/` (its contents inspected directly) and `has-visuals` comes out correctly. Whether `actions/upload-artifact@v4`'s `artifact-url` actually renders as a clickable download link in the comment, and the private-repo access control itself (unauthenticated → HTTP 404, authenticated → downloadable), were both confirmed on real GitHub Actions runners as part of PR #48 (see "Open questions" below) — this change (PNG → HTML) only touches what the artifact contains, not the delivery mechanism's own permission model, so that part wasn't re-verified.
5. **`diff-scope: commit` verification (Issue #43)**: before implementing, a PoC (`poc/issue43-poc/`, never committed) built an "Added → Modified → Added" commit sequence in a disposable git repo, confirming the per-commit loop actually fixes the Issue #23 problem and that, combined with `visual: true`, a naive file-path-only destination name collides (a later commit's grid HTML silently overwrites an earlier one's) — then confirmed the commit-namespaced fix resolves that collision (both recorded as GitHub issue comments: [first comment](https://github.com/MinamiyamaKotaro/exceldiff/issues/43#issuecomment-5448294124), [follow-up comment](https://github.com/MinamiyamaKotaro/exceldiff/issues/43#issuecomment-5448338647)). After implementing, both affected shell scripts were extracted straight out of `action.yml` (parsed via `yaml.safe_load`, each step's `run:` field written to its own file), syntax-checked with `bash -n`, and then actually run — against the real built `xlsxdiff` binary and a disposable git repo — for `diff-scope=pr`/`commit` and `visual=true`/`false`, including a commit that touches no `.xlsx` file (expected to be skipped). The resulting comment Markdown, `$VISUALS_DIR` directory layout, `$VISUALS_LIST` contents, and `$GITHUB_OUTPUT` were all inspected directly. This caught a real bug: with the leading (commit-label) column in `$VISUALS_LIST` left as an empty string, `diff-scope: pr` (the default mode!) came out corrupted — a bash quirk where tab, even as the sole `IFS` character, still counts as "IFS whitespace" to `read`, so a leading empty field is silently dropped and every later field shifts left by one (see "Visual mode" above). Fixed by switching the placeholder to `-` and re-verified. **That said, all of this verification was local shell-script execution only — per item 3's own lesson above (some bugs only reproduce on a real GitHub Actions run, never locally), an actual GitHub Actions integration check (dogfooding via a real multi-commit PR) has not been done yet** (see "Open questions" below).

## Open questions

1. **Publishing the `cli` crate to crates.io**: this action builds `cli/` from source, and `cli/Cargo.toml`'s `publish = false` is unchanged. Pre-built binary distribution ([Issue #28](https://github.com/MinamiyamaKotaro/exceldiff/issues/28), see "Pre-built binary distribution" above) ended up shipping via direct GitHub Release assets instead, so `crates.io` publication still isn't needed — [Issue #22](https://github.com/MinamiyamaKotaro/exceldiff/issues/22)'s assumed reason to publish never materialized. Leave it as-is until some other reason to publish comes up.
2. **Generalizing inputs/outputs (continued)**: `files`/`comment`/`job-summary`/`max-rows-per-sheet`/`diff-mode`/`visual` inputs and `has-changes`/`changed-files-count` outputs are implemented ([Issue #24](https://github.com/MinamiyamaKotaro/exceldiff/issues/24)). ~~Commit-scoped diffing (`diff-scope`)~~ ([Issue #43](https://github.com/MinamiyamaKotaro/exceldiff/issues/43), `commit` mode only): added a `diff-scope` input (`pr` default/as before, or `commit`). `commit` mode enumerates every commit the PR introduces via `git log --reverse base.sha..head.sha` and diffs each against its immediate parent (`<commit>^1`) as its own Markdown subsection — fixing a file added and later modified within the same PR always reporting as `Added` ([the Issue #23 discussion](https://github.com/MinamiyamaKotaro/exceldiff/issues/23)). The `visual: true` grid HTML artifact is also namespaced per commit (see "Visual mode" above). Verified beforehand with a PoC (`poc/issue43-poc/`) and, after implementing, with a local shell-script check — but **an actual GitHub Actions integration check (dogfooding) hasn't happened yet** (see "Test plan" item 5). Still left as follow-up work:
   - A `changed-cells-count` output: `xlsxdiff` currently only emits a Markdown string, with no machine-readable added/modified/deleted count. Needs a `cli/`-side change (e.g. a summary line to stderr like `added=N modified=M deleted=D`) that `action.yml` sums across files.
   - `diff-scope: push` (switching to the immediately-preceding push's `before`/`after`) is still unimplemented. The `pull_request` event's `synchronize`-action payload does carry `before`/`after` fields ([confirmed against octokit/webhooks' JSON Schema](https://github.com/octokit/webhooks)), but other actions like `opened` don't have them — a fallback (e.g. to `base.sha`/`head.sha`) needs its own design and verification.
   - Customizing the comment wording/marker (`<!-- xlsx-diff-comment -->`) itself stays out of scope until there's a concrete need for it.
   - `files` was implemented as a git pathspec (not a shell glob) — distinct from GitHub Actions' own `paths:` trigger-filter syntax; this action doesn't control the calling workflow's trigger at all.
3. ~~**Real-world verification from an external repository**~~([Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23), resolved): cut a pre-release tag `v0.1.0-rc1` and called it from a separate throwaway repository (`MinamiyamaKotaro/exceldiff-action-verify`) via `uses: MinamiyamaKotaro/exceldiff@v0.1.0-rc1`, confirming checkout → diff computation → PR comment posting worked end-to-end. This test itself caught a real bug: a caller workflow declaring only `permissions: pull-requests: write` (exactly this README's own basic example) silently lost `contents` access, breaking `actions/checkout` (see "A `permissions:` block" above; fixed in PR #45). Self-dogfooding alone (which always writes `contents: write` for `visual: true`) could never have caught this — a concrete case for why external verification isn't optional.
4. ~~**`xlsx-diff-images` branch growth**~~ (resolved by the Issue #47 redesign): the push-based scheme (commits growing without bound) is retired in favor of artifact uploads (auto-expire after 90 days by default, shorter via `retention-days`), so this risk no longer applies.
5. ~~**Live GitHub Actions verification of `visual` mode**~~ (resolved, [Issue #47](https://github.com/MinamiyamaKotaro/exceldiff/issues/47), [PR #48](https://github.com/MinamiyamaKotaro/exceldiff/pull/48)): a throwaway `.xlsx` fixture, added in a temporary commit, triggered this repo's own `uses: ./` workflow for real ([run 33122403504](https://github.com/MinamiyamaKotaro/exceldiff/actions/runs/33122403504)); confirmed via the GitHub REST API that `actions/upload-artifact@v4` succeeded, the comment carried a working `artifact-url` link, and the artifact itself (`xlsx-diff-screenshots`, id `9666997603`) exists and isn't expired. The fixture was removed afterward (same throwaway-then-delete approach as Issue #23/#24). **Private-repo access was verified separately, after merge**: a new PR on the existing throwaway external verification repo (`MinamiyamaKotaro/exceldiff-action-verify`, private) called this action via `uses:` pinned to the merge commit (`0c4f571`), and the GitHub API confirmed both directions — (a) an unauthenticated request to the `artifact-url` returns HTTP 404 (401 from the API), and (b) an authenticated request with real repo access successfully downloads the zip, containing the expected screenshot PNG. Unlike the old `raw.githubusercontent.com` scheme (broken even for an authorized viewer) and the rejected `uploads.github.com` route (reachable even without authentication), this is direct evidence the new design actually restricts viewing to people with real repository access.
6. ~~**GitHub Actions integration check for pre-built binary distribution**~~ ([Issue #28](https://github.com/MinamiyamaKotaro/exceldiff/issues/28), resolved): cut a verification-only pre-release tag `v0.14.0-rc1`, confirming `release.yml` actually builds all 4 targets (`x86_64-unknown-linux-gnu`/`aarch64-apple-darwin`/`x86_64-apple-darwin`/`x86_64-pc-windows-msvc`) and publishes them plus a `SHA256SUMS` to a GitHub Release. Downloaded a release asset locally with plain `curl`, verified its checksum (`OK`), extracted it, and confirmed it's a genuine ELF executable. **`action.yml`'s download-success path itself** can't be exercised by this repo's own `uses: ./` (`action_ref` is always empty there), so it was verified the same way as Issue #23 — calling `uses: MinamiyamaKotaro/exceldiff@v0.14.0-rc1` (with `visual: true`) from the existing throwaway external repo (`MinamiyamaKotaro/exceldiff-action-verify`). The job log confirms `dtolnay/rust-toolchain`, `Swatinem/rust-cache`, and `cargo build` never ran at all — "Resolve xlsxdiff binary"'s `curl` download + checksum check alone resolved the binary (`found=true`), and the run went straight through to posting the comment and attaching the grid artifact (about 6 seconds from checkout start to the comment-posting step starting). The verification PR/branch were closed/deleted afterward; the `v0.14.0-rc1` tag/Release itself is kept, same as `v0.1.0-rc1`.
7. **Pre-built binary distribution's remaining follow-up work** ([Issue #28](https://github.com/MinamiyamaKotaro/exceldiff/issues/28)): what item 6's live verification didn't resolve:
   - `aarch64-unknown-linux-gnu` (ARM64 Linux) isn't implemented yet — likely needs a cross-linker, and whether `ubuntu-latest` has a generally-available native ARM64 variant needs its own look.
   - Independent of `changed-cells-count` (item 2 above), whether `release.yml`'s `SHA256SUMS` format/asset naming convention is reusable for that or other future tooling integrations hasn't been considered.
   - `v0.14.0-rc1` is a verification-only pre-release; when and under what version number this action cuts its first "real" stable tag (e.g. `v1`) is a separate decision.
