# `cli/` Design Document

*[日本語](cli.md)*

Design document for `cli/` (the `xlsxdiff` binary crate) — the CLI that actually runs as a process to generate the `.xlsx` diff preview comment `.github/workflows/xlsx-diff.yml` posts to a changed PR ([Issue #22](https://github.com/MinamiyamaKotaro/exceldiff/issues/22), [Issue #32](https://github.com/MinamiyamaKotaro/exceldiff/issues/32)).

It originally lived as `examples/xlsx_diff_cli.rs`. Even after the Markdown-formatting logic moved out into [`markdown.rs`](markdown.en.md) ([Issue #31](https://github.com/MinamiyamaKotaro/exceldiff/issues/31)), the CLI still did the parse/diff orchestration itself and mapped the outcome onto `FileStatus`. Issue #32 moved that orchestration into [`markdown.rs::diff_file_section_from_paths`](markdown.en.md), shrinking the CLI to "turn argv into five arguments and write the result to stdout," and relocated it into its own workspace member, `cli/`.

## Responsibilities / Scope

- Parses argv (`<display_path> <A|M|D> [base_file] [head_file]`), maps it onto [`exceldiff::diff_file_section_from_paths`](markdown.en.md)'s arguments, calls it, and writes the returned Markdown string to stdout (`main.rs`)
- Treats the workflow's own convention for "this revision has no file" — an empty-string argument (the workflow redirects `git show`'s output to an empty file when the revision doesn't have the path) — as `None` when calling `diff_file_section_from_paths` (`.filter(|s| !s.is_empty())`)
- Prints a usage message to stderr and exits non-zero when fewer than 3 arguments are given (program name + `display_path` + `status`)
- **Explicitly out of scope**: parsing `.xlsx`, computing the diff, or Markdown formatting itself (all of it [`exceldiff::diff_file_section_from_paths`](markdown.en.md)'s responsibility), the GitHub Actions workflow itself (`.github/workflows/xlsx-diff.yml`), and distributing this as an installable binary or composite action (a separate, still-open question — see "Relationship to the `exceldiff` crate" below)

## Key types / functions

```rust
// cli/src/main.rs
fn main() -> std::process::ExitCode;
```

A thin binary crate with a single `main` function. See [`cli/src/main.rs`](../../cli/src/main.rs) for the actual implementation.

## Relationship to the `exceldiff` crate: why a separate crate instead of `examples/` or `src/bin/`

Three options were considered for where the CLI should live (see Issue #32's PR review):

1. **Stay under `examples/`** (the pre-migration state): excluded from `cargo install`/a plain `cargo build`, but requires an explicit `cargo build --example xlsx_diff_cli` invocation and can't have its own crate-level integration tests (a `tests/` directory the way a real package has one)
2. **Promote to `src/bin/xlsxdiff.rs`** (what [Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23) originally assumed): Cargo's `autobins` default behavior would automatically fold it into the `exceldiff` library crate's own `cargo install`/plain `cargo build`/`cargo package` surface — unintentionally widening the library's published binary surface
3. **A separate workspace member, `cli/`** (the option taken): a separate package that only references `exceldiff` via a `path` dependency, so it never appears in `cargo build`/`cargo install`/`cargo package` run against `exceldiff` alone. It also gets its own `Cargo.toml` and `tests/` directory, making it possible to write real-process integration tests of `main.rs`'s own argv handling (see "Test plan" below)

Option 3 keeps option 1's benefit (the library's published surface stays untouched) while also gaining what option 1 couldn't offer: real-process integration tests. It also leaves a natural home to build on later if `cargo install xlsxdiff` or an `action.yml`-based composite action ([Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23)) happens — just drop `cli/Cargo.toml`'s `publish = false` and publish to crates.io if/when needed.

## Dependencies

- Depends on: [`exceldiff`](lib.en.md) (a `path` dependency; uses only `diff_file_section_from_paths` and `MarkdownOptions`)
- Depended on by: `.github/workflows/xlsx-diff.yml` (builds it with `cargo build --release -p xlsxdiff`, then runs `target/release/xlsxdiff` once per `.xlsx` file changed in the PR, concatenating each invocation's output into the comment body)

## Error handling policy

`main` returns an `ExitCode` — only argv validation failure (fewer than 3 arguments) returns `ExitCode::FAILURE` with a usage message on stderr. Everything else, including a parse error or an unrecognized git status letter, is represented *as data* by `exceldiff::diff_file_section_from_paths`, which returns an ordinary Markdown string for it (see [`markdown.rs`'s error handling policy](markdown.en.md)) — so the CLI itself never panics or exits non-zero for those cases. This is deliberate: one file failing to parse should never stop the workflow from posting a comment for the rest of the PR's `.xlsx` files.

## Test plan

- [`markdown.rs`'s own unit tests](markdown.en.md) already cover every `FileStatus` branch of `diff_file_section_from_paths` itself (normal cases, parse errors, which revision) with no process spawn needed, so `cli/tests/cli.rs` doesn't re-verify those
- `cli/tests/cli.rs` instead covers what's specific to this crate: **argv handling when actually run as a process** (via `env!("CARGO_BIN_EXE_xlsxdiff")`, checking exit code and stdout/stderr):
  - fewer than 3 arguments prints a usage message to stderr and exits non-zero
  - an empty-string `base_file`/`head_file` argument is treated the same as the argument being omitted (verifying the workflow's own "pass an empty string when `git show` fails" convention)
  - each git status (`A`/`D`/`M`, and an unrecognized letter) produces output with the matching heading badge — the fine-grained formatting itself is already [verified on the `markdown.rs` side](markdown.en.md), so this only confirms the right arguments actually reach it
- Real files come from the existing fixtures under `tests/fixtures/` (`normal/basic_types.xlsx`, `other/date.xlsx`, `error/corrupted_xml.xlsx`), referenced by a path relative to this crate's own `CARGO_MANIFEST_DIR` — matching the existing convention of crate-level integration tests using `tests/fixtures/` directly (e.g. [`tests/error.rs`](../../tests/error.rs)), a different layer from [`src/`'s own unit tests, which stick to in-memory data](markdown.en.md)

## Open questions

1. **Distributing this as an installable binary / a composite action**: the theme [Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23) was exploring. Whether this crate gets published to crates.io (dropping `publish = false`) or an `action.yml` just builds this repo directly is still open.
2. **Versioning policy**: `cli/Cargo.toml`'s version starts independently at `0.1.0`, unlinked to `exceldiff`'s own version. Revisit if the two ever need to move in lockstep (e.g. once `cli` starts depending on specific public-API guarantees tied to an `exceldiff` version).
