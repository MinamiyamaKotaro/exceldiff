# `cli/` Design Document

*[日本語](cli.md)*

Design document for `cli/` (the `xlsxdiff` binary crate) — the CLI that actually runs as a process to generate the `.xlsx` diff preview comment `.github/workflows/xlsx-diff.yml` posts to a changed PR ([Issue #22](https://github.com/MinamiyamaKotaro/exceldiff/issues/22), [Issue #32](https://github.com/MinamiyamaKotaro/exceldiff/issues/32)).

It originally lived as `examples/xlsx_diff_cli.rs`. Even after the Markdown-formatting logic moved out into [`markdown.rs`](markdown.en.md) ([Issue #31](https://github.com/MinamiyamaKotaro/exceldiff/issues/31)), the CLI still did the parse/diff orchestration itself and mapped the outcome onto `FileStatus`. Issue #32 moved that orchestration into [`markdown.rs::diff_file_section_from_paths`](markdown.en.md), shrinking the CLI to "turn argv into five arguments and write the result to stdout," and relocated it into its own workspace member, `cli/`.

## Responsibilities / Scope

- Parses argv (`[--max-rows-per-sheet <N>] [--diff-mode <auto|coordinate>] [--grid-html-dir <dir>] <display_path> <A|M|D> [base_file] [head_file]`), maps it onto [`exceldiff::diff_file_section_from_paths`](markdown.en.md)'s arguments (`display_path`/`status`/`base_path`/`head_path`/`MarkdownOptions`), calls it, and writes the returned Markdown string to stdout (`main.rs`)
- Feeds the leading `--max-rows-per-sheet <N>`/`--diff-mode <auto|coordinate>`/`--grid-html-dir <dir>` flags (all optional, recognized only as `--flag value` pairs consumed from the front, not in arbitrary order) into `MarkdownOptions::max_rows_per_sheet`/`diff_mode` and into an [`exceldiff::grid_sections_from_paths`](grid.en.md) call — the bridge for [`action.yml`'s inputs of the same names](action.en.md) ([Issue #24](https://github.com/MinamiyamaKotaro/exceldiff/issues/24)). An invalid value (a non-numeric `--max-rows-per-sheet`, a `--diff-mode` that's neither `auto` nor `coordinate`, any of the flags with no value following it) prints the usage message to stderr and exits non-zero — no argument-parsing crate like `clap` was added for this (see "Relationship to the `exceldiff` crate" below); a hand-written loop (`parse_options`) is enough
- When `--grid-html-dir <dir>` is given, writes one standalone HTML page per changed sheet (`<dir>/sheet-{N}.html`, wrapped via `exceldiff::wrap_grid_page`) plus a `manifest.tsv` (`sheet_name\thtml_path` per line, the same TSV convention `git diff --name-status` already uses) into `dir` — entirely additive, with zero effect on the stdout Markdown contract (`write_grid_sections`). A grid-rendering failure (a bad path, say) only logs a warning to stderr; the process still exits successfully, since the stdout Markdown — this binary's primary output — is already written by that point
- Treats the workflow's own convention for "this revision has no file" — an empty-string argument — as `None` when calling `diff_file_section_from_paths` (`.filter(|s| !s.is_empty())`). `.github/workflows/xlsx-diff.yml` initializes `base_file`/`head_file` to the empty string and simply never runs `git show` for whichever side doesn't apply (e.g. `base_file` for status `A`, `head_file` for status `D`), passing that empty string straight through to `xlsxdiff` — this isn't a fallback for a failed `git show`
- Prints a usage message to stderr and exits non-zero when fewer than 2 positional arguments remain after flag parsing (`display_path` + `status`)
- **Explicitly out of scope**: parsing `.xlsx`, computing the diff, or Markdown formatting itself (all of it [`exceldiff::diff_file_section_from_paths`](markdown.en.md)'s responsibility), the GitHub Actions workflow itself (`.github/workflows/xlsx-diff.yml`, [`action.yml`](action.en.md)), and distributing this as an installable binary (see "Relationship to the `exceldiff` crate" below)

## Key types / functions

```rust
// cli/src/main.rs
struct Options {
    markdown: MarkdownOptions,
    grid_html_dir: Option<String>,
}
fn parse_options(args: &[String]) -> Option<(Options, &[String])>;
fn write_grid_sections(
    dir: &str,
    status: &str,
    base_path: Option<&str>,
    head_path: Option<&str>,
    diff_mode: DiffMode,
) -> std::io::Result<()>;
fn main() -> std::process::ExitCode;
```

`parse_options` consumes the three `--` flags and returns an `Options` plus the remaining positional arguments (`None` on an invalid value); `main` uses that to call `diff_file_section_from_paths` (always) and `write_grid_sections` (only when `--grid-html-dir` was given). See [`cli/src/main.rs`](../../cli/src/main.rs) for the actual implementation.

## Relationship to the `exceldiff` crate: why a separate crate instead of `examples/` or `src/bin/`

Three options were considered for where the CLI should live (see Issue #32's PR review):

1. **Stay under `examples/`** (the pre-migration state): excluded from `cargo install`/a plain `cargo build`, but requires an explicit `cargo build --example xlsx_diff_cli` invocation and can't have its own crate-level integration tests (a `tests/` directory the way a real package has one)
2. **Promote to `src/bin/xlsxdiff.rs`** (what [Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23) originally assumed): Cargo's `autobins` default behavior would automatically fold it into the `exceldiff` library crate's own `cargo install`/plain `cargo build`/`cargo package` surface — unintentionally widening the library's published binary surface
3. **A separate workspace member, `cli/`** (the option taken): a separate package that only references `exceldiff` via a `path` dependency, so it never appears in `cargo build`/`cargo install`/`cargo package` run against `exceldiff` alone. It also gets its own `Cargo.toml` and `tests/` directory, making it possible to write real-process integration tests of `main.rs`'s own argv handling (see "Test plan" below)

Option 3 keeps option 1's benefit (the library's published surface stays untouched) while also gaining what option 1 couldn't offer: real-process integration tests. The composite action from [Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23) (see the [`action.yml` design document](action.en.md)) also builds on this crate directly, from source — `cli/Cargo.toml`'s `publish = false` is unchanged.

## Dependencies

- Depends on (normal): [`exceldiff`](lib.en.md) (a `path` dependency; uses `diff_file_section_from_paths`, `MarkdownOptions`, [`grid_sections_from_paths`/`wrap_grid_page`](grid.en.md), and `DiffMode`)
- Depends on (dev only): `zip` (used only by `cli/tests/cli.rs` to build a controlled test `.xlsx` pair in-memory — see "Test plan" below; never part of the shipped binary)
- Depended on by: [`action.yml`](action.en.md) (builds it with `cargo build --release -p xlsxdiff`, then runs `target/release/xlsxdiff` once per `.xlsx` file changed in the PR, concatenating each invocation's output into the comment body; also passes `--grid-html-dir` when `visual: true`, screenshotting the HTML it writes there with Playwright). [`.github/workflows/xlsx-diff.yml`](../../.github/workflows/xlsx-diff.yml) itself is now a thin workflow that calls `action.yml` via `uses: ./` and no longer builds this crate directly

## Error handling policy

`main` returns an `ExitCode` — only argv validation failure (an invalid flag value, or fewer than 2 positional arguments left after flag parsing) returns `ExitCode::FAILURE` with a usage message on stderr. Everything else, including a parse error or an unrecognized git status letter, is represented *as data* by `exceldiff::diff_file_section_from_paths`, which returns an ordinary Markdown string for it (see [`markdown.rs`'s error handling policy](markdown.en.md)) — so the CLI itself never panics or exits non-zero for those cases. This is deliberate: one file failing to parse should never stop the workflow from posting a comment for the rest of the PR's `.xlsx` files. `write_grid_sections` (`--grid-html-dir`) follows the same spirit: `main` only logs its failure to stderr, never turning it into `ExitCode::FAILURE` — the stdout Markdown, this binary's primary deliverable, is already written by the time it runs.

## Test plan

- [`markdown.rs`'s own unit tests](markdown.en.md) already cover every `FileStatus` branch of `diff_file_section_from_paths` itself (normal cases, parse errors, which revision) with no process spawn needed, so `cli/tests/cli.rs` doesn't re-verify those
- `cli/tests/cli.rs` instead covers what's specific to this crate: **argv handling when actually run as a process** (via `env!("CARGO_BIN_EXE_xlsxdiff")`, checking exit code and stdout/stderr):
  - fewer than 3 arguments prints a usage message to stderr and exits non-zero
  - an empty-string `base_file`/`head_file` argument is treated the same as the argument being omitted (verifying the workflow's own convention of passing an empty string straight through for whichever side doesn't apply — see "Responsibilities / Scope" above)
  - each git status (`A`/`D`/`M`, and an unrecognized letter) produces output with the matching heading badge — the fine-grained formatting itself is already [verified on the `markdown.rs` side](markdown.en.md), so this only confirms the right arguments actually reach it
  - `--max-rows-per-sheet`/`--diff-mode` actually reach `MarkdownOptions` and change the output, and an invalid value (non-numeric, an unrecognized mode name) produces the usage error — what the flags themselves *mean* (the hunk cap, the diffing algorithm each `DiffMode` picks) is already [unit-tested on the `markdown.rs` side](markdown.en.md), so this only confirms argv wiring is correct
  - `--grid-html-dir` actually writes `manifest.tsv` and a per-sheet HTML file when given, an empty `manifest.tsv` when there's nothing to render, and a flag with no value following it (same as the other two) produces the usage error
- Tests that just need *some* real `.xlsx` (confirming a real file parses/errors as expected) use the existing fixtures under `tests/fixtures/` (`normal/basic_types.xlsx`, `error/corrupted_xml.xlsx`), referenced by a path relative to this crate's own `CARGO_MANIFEST_DIR` — matching the existing convention of crate-level integration tests using `tests/fixtures/` directly (e.g. [`tests/error.rs`](../../tests/error.rs))
- The test confirming "a changed cell renders as an `@@` hunk," though, builds a minimal `.xlsx` pair in-memory inside `cli/tests/cli.rs` itself (differing in exactly one cell's value, via the `zip` crate as a dev-dependency) rather than picking two unrelated files under `tests/fixtures/`. Which specific cells differ between two unrelated real files isn't something a test controls or guarantees — this one initially did use two unrelated fixtures, passed locally, then failed in CI with zero hunks once dependency resolution came out differently (this library crate doesn't commit `Cargo.lock`). Building the pair in the test is the same fix [`tests/fixtures/diff.rs`'s `cell_modified()`](../../tests/fixtures/diff.rs) and [`src/markdown.rs`'s own unit tests](markdown.en.md) already apply, for the same reason

## Open questions

1. **Distributing this as an installable binary / a composite action**: the theme [Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23) was exploring. The composite action itself is now implemented ([`action.yml`](action.en.md), building this repo directly from source). Whether this crate also gets published to crates.io (dropping `publish = false`) stays open until pre-built binary distribution ([Issue #22](https://github.com/MinamiyamaKotaro/exceldiff/issues/22)/[Issue #28](https://github.com/MinamiyamaKotaro/exceldiff/issues/28)) actually needs it.
2. **Versioning policy**: `cli/Cargo.toml`'s version starts independently at `0.1.0`, unlinked to `exceldiff`'s own version. Revisit if the two ever need to move in lockstep (e.g. once `cli` starts depending on specific public-API guarantees tied to an `exceldiff` version).
