# `grid.rs` Design Document

*[日本語](grid.md)*

Design document for `src/grid.rs`. A second diff output format that grew out of exploring [`markdown.rs`](markdown.en.md) ([Issue #22](https://github.com/MinamiyamaKotaro/exceldiff/issues/22), [Issue #31](https://github.com/MinamiyamaKotaro/exceldiff/issues/31)). It formats one sheet's diff (a `SheetDiff` from `diff::WorkbookDiff`) as HTML that faithfully reproduces an actual Excel grid — column widths, merged cells, borders, fills, text wrapping — with Before and After laid out side by side.

## Background / motivation

As [markdown.md's "Design decision" section](markdown.en.md) explains, GitHub sanitizes a `style=` attribute out of any HTML pasted into a PR comment, so while `markdown.rs`'s ` ```diff ` fence can carry color, there's no way to show an actual Excel grid's real appearance (column widths, merged cells, fill colors, borders) that same way. This module targets a different, unconstrained output path instead (a screenshot image, a page statically hosted on GitHub Pages, a downloadable CI artifact — all out of this module's own scope), generating HTML that's faithful to what the grid actually looks like in Excel.

Its primary target is "grid-paper Excel" (a fine, uniform cell grid with merged cells standing in for headings/borders — a style common in Japanese business documents and [`lib.rs`](lib.en.md)'s own stated focus for the whole crate). Given that a sheet like this can run to thousands of rows/columns, unchanged rows/columns that sit far apart from any real change collapse the same way `git diff`'s context lines do.

## Responsibilities / Scope

- Takes a [`diff::SheetDiff`](diff/model.en.md) (plus the `base`/`head` [`Workbook`](model/workbook.en.md) the diff came from, and the target [`Sheet`](model/sheet.en.md) within each) and returns an HTML `<section class="sheet">` fragment with Before (base) and After (head) laid out side by side (`render_sheet_split`). It does not include a `<style>` block or full page HTML (`<html>`/`<head>`, etc.) — a caller like `examples/xlsx_diff_grid.rs` supplies its own stylesheet matching the returned fragment's class names
- Renders each cell with that side's (Before/After) *actual* resolved style — fill color resolved to real RGB via `resolve_color`, bold, `wrapText`, borders — rather than flattening a changed cell's background to a solid green/red/yellow. The real appearance stays intact; the change type is instead layered on as a 1px border CSS class (`border-added`/`border-deleted`/`border-value`/`border-style`)
- Renders each side's actual merged-cell structure as real HTML `rowspan`/`colspan`, via [`Sheet::merged_region_at`](model/sheet.en.md). A merge that was added, removed, or resized is correctly reflected in that structural information, but carries no dedicated visual marker of its own (removed per review feedback — a merge change is only counted toward the "modified" total `SheetDiff.merges` already carries)
- A cell with `wrapText` on wraps; one without reproduces Excel's actual default behavior instead (clip at the cell boundary, or spill naturally into an empty neighbor — `next_cell_is_empty`)
- Converts column width from Excel's character-based unit to pixels using Excel's own real formula (`excel_width_to_px`), and pins the `<table>`'s own total width explicitly so the browser can't widen columns to fit content (`table-layout: fixed` alone isn't enough — see `render_table`'s doc comment for why)
- Keeps only 2 rows/columns of context on either side of an actual change, collapsing any longer run of unchanged rows/columns into a single `⋯ N row(s)/column(s) omitted ⋯` line (`build_line_plan`) — the same logic applied independently to both the row and column axis
- **Explicitly out of scope**: publishing or distributing the generated HTML (taking a screenshot, deploying to GitHub Pages, uploading as a CI artifact — all separate, still-open design questions), and full page HTML/CSS/JS (the caller's responsibility — `examples/xlsx_diff_grid.rs` is one example of that)

## Key types / functions (draft)

```rust
use crate::diff::{DiffStatus, SheetDiff};
use crate::model::{Sheet, Workbook};

pub fn render_sheet_split(
    sheet_diff: &SheetDiff,
    base: &Workbook,
    head: &Workbook,
    base_sheet: Option<&Sheet>,
    head_sheet: Option<&Sheet>,
) -> String;
```

This is the only public function. The internal `LineSlot`/`CellChange`/`Side` enums and helpers (`render_table`, `render_cell`, `resolve_visual_style`, `border_sides_css`, `excel_width_to_px`, `column_pixel_width`, …) are all private. See [`src/grid.rs`](../../src/grid.rs) for the actual implementation.

## Dependencies

- Depends on: [`diff/model.rs`](diff/model.en.md) (`DiffStatus`, `SheetDiff`), [`model/`](model/) (`Borders`, `Cell`, `CellRef`, `CellValue`, `ColorRef`, `ResolvedStyle`, `Rgb`, `Sheet`, `ThemePalette`, `Workbook`), [`resolve/color.rs`](resolve/color.en.md) (`resolve_color` — resolves a `ColorRef` to a real RGB value, including theme and indexed colors), [`json.rs`](json.en.md) (`format_date_time` — renders a `DateTime` cell's value the same ISO-8601-without-timezone way `json.rs` does, instead of `DateTimeValue`'s derived `Debug` form)
- Depended on by: [`lib.rs`](lib.en.md) (re-exports `render_sheet_split` as public API), `examples/xlsx_diff_grid.rs` (calls `parse_workbook`/`diff_workbooks` and assembles the returned HTML fragment into a full page)

## Design decision: why a separate module instead of folding into `markdown.rs`

Both `markdown.rs` and this module take `diff::WorkbookDiff`/`SheetDiff` as input, but their output has fundamentally different requirements — `markdown.rs` targets output that survives GitHub's PR-comment sanitization (no decorative HTML/CSS at all), while this module's entire point is decorative CSS (color, borders, column width) as the primary payload. Merging the two into one function would mean always paying the cost of computing both styles, and would force a caller that only wants one of them to also supply the other's irrelevant arguments — this module needs the full `base`/`head` `Workbook` to read a cell's actual style/column-width/merge structure, which `markdown.rs` has no use for at all since `WorkbookDiff` alone is sufficient for it. In practice, `examples/xlsx_diff_cli.rs` (uses `markdown.rs`) and `examples/xlsx_diff_grid.rs` (uses this module) format the exact same `diff_workbooks` result into two entirely different shapes, sharing almost no implementation.

## Test plan

Builds synthetic `Sheet`/`Workbook` instances directly via `Sheet::new`/`insert_cell`/`insert_merge`/`set_col_widths` (all `pub(crate)`) — no real file parsing or process spawn needed for any test.

- An unchanged cell renders its actual value with no change-indicator CSS class attached
- An added cell renders as `not-present` (hatched) on the Before side and `border-added` on the After side; a deleted cell shows the symmetric opposite
- A cell whose value changed gets `border-value`; a cell whose value stayed the same but whose style changed gets `border-style` instead — and the two are mutually exclusive (verifies the `CellChange::ValueChanged`/`StyleOnly` decision logic)
- A merged region's extent change (`old_end`/`new_end`) is correctly reflected as a real `colspan` attribute on each respective side, with no dedicated merge-change visual marker emitted at all (a regression check for a feature removed during review), and that it correctly counts toward the summary line's "modified" total
- Unchanged rows between two far-apart changes collapse into a `gap-row`; conversely, a small sheet needing no collapsing never emits one
- On a multi-sheet workbook, the sheet heading's position number (`sheet1：`/`sheet2：`) correctly reflects head's own sheet order
- `excel_width_to_px` returns the correct value for a known conversion (2.14 characters — a common grid-paper width — → 15px), directly testing the implementation of Excel's own official formula
- `column_letters` correctly converts a multi-letter column (e.g. the column right after Z, AA)
- `html_escape` correctly escapes `&`/`<`/`>`
- A styled cell's bold flag (`font.bold`) is reflected as `font-weight:700;` in the inline CSS

## Open questions

1. **Delivery path for the generated HTML**: how to make this module's HTML viewable in a PR's context (screenshotting it and embedding the image in the PR comment, static-hosting it on GitHub Pages, a downloadable CI artifact, …) is left for a follow-up issue on [Issue #22](https://github.com/MinamiyamaKotaro/exceldiff/issues/22). Candidate implementations discussed so far: headless-browser rendering (e.g. Playwright) plus a screenshot step in CI, and deploying to GitHub Pages via `actions/upload-pages-artifact`/`actions/deploy-pages`.
2. **Whether this module should be treated as settled public API the way `markdown.rs` is**: `render_sheet_split` is currently re-exported as public from `lib.rs`, but no production workflow actually consumes it yet (the delivery path in question 1 doesn't exist). Once real usage exists, the signature (whether requiring the full `base`/`head` `Workbook` is the right ergonomic shape) may be worth revisiting.
3. **Interaction between column collapsing and merged cells**: as noted in `render_table`'s doc comment, there's a known limitation when a merge's origin falls inside a collapsed row/column range — `covered` tracking doesn't account for that case correctly. This seems unlikely to matter in grid-paper Excel's typical usage (a merged cell is usually near a heading/label where changes cluster, which tends to already fall within the kept context), but this hasn't been confirmed.
