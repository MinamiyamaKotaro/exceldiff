# `markdown.rs` Design Document

*[日本語](markdown.md)*

Design document for `src/markdown.rs`. It sits downstream of the 5-phase pipeline [architecture.md](architecture.en.md) defines, formatting the `WorkbookDiff` that [`diff/`](diff/mod.en.md) (Issue #3) computes into the GitHub Flavored Markdown `.github/workflows/xlsx-diff.yml` posts as a PR comment ([Issue #22](https://github.com/MinamiyamaKotaro/exceldiff/issues/22), [Issue #31](https://github.com/MinamiyamaKotaro/exceldiff/issues/31)).

This is the Markdown-formatting logic that originally lived inline in `examples/xlsx_diff_cli.rs` (`print_added`/`print_deleted`/`print_modified`/`print_diff`/`format_value`/`escape_table_cell`), extracted into the library. The CLI used to write directly to stdout via `println!`, so the only way to check the formatted output was to actually run the compiled binary and capture stdout. Every function here returns a `String` instead, so a unit test can pass a `WorkbookDiff` in and check the string it gets back (the testability [Issue #31](https://github.com/MinamiyamaKotaro/exceldiff/issues/31) asks for).

## Responsibilities / Scope

- Takes a [`diff::WorkbookDiff`](diff/model.en.md) (plus the enclosing file's git status A/M/D and its display path) and returns a GitHub Flavored Markdown string (`format_file_section`, see below)
- Renders the list of changed cells as ` ```diff ` fence content — one `@@ <A1 coord> @@` hunk plus `-`/`+` lines per cell (`format_cell_hunk`). See "Design decision: why a diff fence instead of a Markdown table" below for why this shape was chosen over a table
- Renders merged-region changes ([`diff::MergeDiff`](diff/model.en.md), Issue #8) into the same ` ```diff ` fence as the cell hunks, as `@@ <start>:<end> (merge) @@` hunks (`format_merge_hunk`). The summary line (`{added} added, {modified} modified, {deleted} deleted`) counts every merge change — added, removed, or resized — toward "modified": a merge change isn't really the addition or removal of a specific cell, it's a change in how existing cells are grouped, which reads as a modification either way you look at it
- Lets the caller cap how many cell hunks render per sheet via `MarkdownOptions::max_rows_per_sheet` (feeds into [Issue #24](https://github.com/MinamiyamaKotaro/exceldiff/issues/24)'s action input design). Merge hunks are never capped — see the doc comment in the code for why
- Tags the file heading (`### <badge> · \`path\``) with an emoji status badge (🆕/✏️/🗑️/❓) so added/modified/deleted is obvious at a glance (`file_status_badge`) — the path alone doesn't say what happened to a file once a PR touches several `.xlsx` files at once
- **Explicitly out of scope**: parsing `.xlsx` or computing the diff itself ([`parse_workbook`](lib.en.md), [`diff::diff_workbooks`](diff/engine.en.md) — the caller's responsibility), the GitHub Actions workflow itself (`.github/workflows/xlsx-diff.yml`), and rendering a grid-paper `.xlsx` as an actual Excel-looking HTML grid (a separate, still-open design question: GitHub sanitizes a `style=` attribute out of HTML pasted into a PR comment, so colored borders/fills can never be embedded directly in the comment body — that visual grid view needs a different output path entirely, e.g. a screenshot image plus a link to a page hosted on GitHub Pages, which is out of this module's scope)

## Key types / functions (draft)

```rust
use crate::diff::{CellDiff, CellPos, DiffStatus, MergeDiff, SheetDiff, WorkbookDiff};
use crate::json::JsonCellValue;
use crate::model::CellRef;

#[derive(Debug, Clone, Copy)]
pub struct MarkdownOptions {
    pub max_rows_per_sheet: usize, // default 30
}

#[derive(Debug, Clone, Copy)]
pub struct AddedSummary {
    pub sheet_count: usize,
    pub cell_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionSide { Base, Head }

pub enum FileStatus<'a> {
    Added(Option<AddedSummary>),
    AddedParseError(&'a str),
    Deleted,
    Modified(&'a WorkbookDiff),
    ModifiedMissingContent,
    ModifiedParseError(RevisionSide, &'a str),
    Unrecognized(&'a str),
}

pub fn format_file_section(display_path: &str, status: &FileStatus, options: &MarkdownOptions) -> String;
pub fn format_workbook_diff(diff: &WorkbookDiff, options: &MarkdownOptions) -> String;
```

See [`src/markdown.rs`](../../src/markdown.rs) for the actual implementation. `format_sheet_diff`, `format_cell_hunk`, `format_merge_hunk`, and `format_value` are private helpers.

## Design decision: why a diff fence instead of a Markdown table

The CLI's original implementation rendered a `| | Cell | Before | After |` Markdown table, with `➕`/`✏️`/`➖` emoji embedded as status markers. This has a real problem: GitHub sanitizes a `style=` attribute out of any HTML/Markdown pasted into a PR comment, so a table cell can never carry a background color — there's no way to get the red/green highlighting a real `git diff` shows, no matter how the table is built.

A ` ```diff ` fence sidesteps this entirely: GitHub's own syntax highlighting applies a red/green background to any line starting with `-`/`+` inside such a fence — the exact same mechanism a real `git diff` output gets, no extra styling required. So this module renders value changes as `-`/`+` lines inside a ` ```diff ` fence instead. The `@@ <A1 coord> @@` hunk header echoes a unified diff's own hunk header (`@@ -a,b +c,d @@`), which GitHub also highlights (in a purple-ish tone). A merged-region change (`format_merge_hunk`) tags that same header line with `(merge)` to keep it visually distinct from a cell-value hunk — `@@ B1:F1 @@` alone could otherwise read as plain cell-range notation.

## Dependencies

- Depends on: [`diff/model.rs`](diff/model.en.md) (`CellDiff`, `CellPos`, `DiffStatus`, `MergeDiff`, `SheetDiff`, `WorkbookDiff`), [`json.rs`](json.en.md) (`JsonCellValue` — reused as the conversion target for `CellDiff::old_value`/`new_value`/`format_value`, keeping the tagged-value representation consistent with [json.md's design decision](json.en.md) across both the diff and JSON worlds), [`model/cell.rs`](model/cell.en.md) (`CellRef::to_a1` — converts a coordinate to A1 notation)
- Depended on by: [`lib.rs`](lib.en.md) (re-exports `FileStatus`/`MarkdownOptions`/`AddedSummary`/`RevisionSide`/`format_file_section`/`format_workbook_diff` as public crate API), `examples/xlsx_diff_cli.rs` (a thin wrapper that parses args, calls `parse_workbook`/`diff_workbooks`, and maps the outcome onto `FileStatus` before calling into this module — further thinning the CLI itself is [Issue #32](https://github.com/MinamiyamaKotaro/exceldiff/issues/32)'s scope)

## Error handling policy

None of this module's functions return a `Result` — the `WorkbookDiff`/`FileStatus` it receives is already resolved, valid data from the caller (`parse_workbook`/`diff_workbooks`); this module never performs an operation that can fail (parsing a file, computing a diff). A parse failure is instead represented *as data*, via `FileStatus::AddedParseError`/`ModifiedParseError` — the caller supplies the error message, and this module just formats it into the Markdown string like anything else (the same "carry the outcome, including the error case, as data" convention [`CellDiff::old_value`/`new_value`](diff/model.en.md) already follows).

## Test plan

- Build a `WorkbookDiff` directly and pass it to `format_workbook_diff`/`format_file_section`, checking the returned string — no process spawn, no fixture file needed (this is exactly the testability Issue #31 asks for)
- A `WorkbookDiff` with one value-changed cell formats to the correct ` ```diff ` fence (`@@ A1 @@` / `- 1` / `+ 2`)
- A `WorkbookDiff` with zero sheets formats to `_No differences detected._`
- `max_rows_per_sheet` correctly caps the number of cell hunks and reports the remainder as `_...and N more change(s) in this sheet._`
- All three merge-change patterns (added/deleted/resized) format to the correct `@@ ... (merge) @@` hunk shape (`+ merged` / `- merged` / `- merged A:B` followed by `+ merged A:C`), and merge changes correctly count toward the summary line's "modified" total
- A sheet visibility change renders a `_Visibility: \`old\` → \`new\`_` line
- Every `FileStatus` variant (`Added`, `AddedParseError`, `Deleted`, `Modified`, `ModifiedMissingContent`, `ModifiedParseError`, `Unrecognized`) formats the expected heading badge and body text; `ModifiedParseError` is checked with both `RevisionSide::Base` and `Head`, confirming the error message names the correct failing side ("the previous version" / "the new version")
- A newline embedded in a text value is escaped so it can't break the diff line's one-line-per-value shape, while `|` is left entirely unescaped now that this isn't Markdown table syntax anymore (a deliberate behavior change from the old table format)

## Open questions

1. **Final naming of the public functions/types**: `format_file_section`/`FileStatus` are this implementation's chosen names. [Issue #31's own body](https://github.com/MinamiyamaKotaro/exceldiff/issues/31) left the public function name as "needs discussion" — this records the names that came out of review.
2. **`Write` support**: Issue #31's body said the output could be "a `String` (or a write to `Write`)"; this implementation only provides `String`. Whether to match [`json.rs`](json.en.md)'s "a `Write`-based version is primary, the `String` version is a thin wrapper around it" pattern is worth revisiting if output size/memory ever becomes a real concern in an actual workflow run.
3. **Relationship to a grid-paper Excel-style grid view**: as noted under "Design decision" above, GitHub sanitizes decorative HTML/CSS out of a PR comment, so showing a grid-paper `.xlsx` with an actual Excel-like grid appearance needs a separate path — a screenshot image plus a link to a page statically hosted on GitHub Pages. The concrete implementation of that path (headless-browser rendering in CI, adopting `actions/upload-pages-artifact`/`actions/deploy-pages`) is carved out as a separate, out-of-scope issue from this module.
