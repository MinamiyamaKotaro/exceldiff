# `grid.rs` Design Document

*[日本語](grid.md)*

Design document for `src/grid.rs`. A second diff output format that grew out of exploring [`markdown.rs`](markdown.en.md) ([Issue #22](https://github.com/MinamiyamaKotaro/exceldiff/issues/22), [Issue #31](https://github.com/MinamiyamaKotaro/exceldiff/issues/31)). It formats one sheet's diff (a `SheetDiff` from `diff::WorkbookDiff`) as HTML that faithfully reproduces an actual Excel grid — column widths, merged cells, borders, fills, text wrapping — with Before and After laid out side by side.

## Background / motivation

As [markdown.md's "Design decision" section](markdown.en.md) explains, GitHub sanitizes a `style=` attribute out of any HTML pasted into a PR comment, so while `markdown.rs`'s ` ```diff ` fence can carry color, there's no way to show an actual Excel grid's real appearance (column widths, merged cells, fill colors, borders) that same way. This module targets a different, unconstrained output path instead (a standalone HTML page downloadable as a workflow artifact — actual delivery is out of this module's own scope, see [action.en.md](action.en.md)), generating HTML that's faithful to what the grid actually looks like in Excel.

Its primary target is "grid-paper Excel" (a fine, uniform cell grid with merged cells standing in for headings/borders — a style common in Japanese business documents and [`lib.rs`](lib.en.md)'s own stated focus for the whole crate). Given that a sheet like this can run to thousands of rows/columns, unchanged rows/columns that sit far apart from any real change collapse the same way `git diff`'s context lines do.

## Responsibilities / Scope

- Takes a [`diff::SheetDiff`](diff/model.en.md) (plus the `base`/`head` [`Workbook`](model/workbook.en.md) the diff came from, and the target [`Sheet`](model/sheet.en.md) within each) and returns an HTML `<section class="sheet">` fragment with Before (base) and After (head) laid out side by side (`render_sheet_split`). It does not include a `<style>` block or full page HTML (`<html>`/`<head>`, etc.) — a caller like `examples/xlsx_diff_grid.rs` supplies its own stylesheet matching the returned fragment's class names
- Renders each cell with that side's (Before/After) *actual* resolved style — fill color resolved to real RGB via `resolve_color`, bold, `wrapText`, borders — rather than flattening a changed cell's background to a solid green/red/yellow. The real appearance stays intact; the change type is instead layered on as a 1px border CSS class (`border-added`/`border-deleted`/`border-value`/`border-style`)
- Renders each side's actual merged-cell structure as real HTML `rowspan`/`colspan`, via [`Sheet::merged_region_at`](model/sheet.en.md). A merge that was added, removed, or resized is correctly reflected in that structural information, but carries no dedicated visual marker of its own (removed per review feedback — a merge change is only counted toward the "modified" total `SheetDiff.merges` already carries)
- A cell with `wrapText` on wraps; one without reproduces Excel's actual default behavior instead (clip at the cell boundary, or spill naturally into an empty neighbor — `next_cell_is_empty`)
- Converts column width from Excel's character-based unit to pixels using Excel's own real formula (`excel_width_to_px`), and pins the `<table>`'s own total width explicitly so the browser can't widen columns to fit content (`table-layout: fixed` alone isn't enough — see `render_table`'s doc comment for why)
- Converts row height from Excel's points unit to pixels (`excel_height_pt_to_px` — unlike column width, needs no font metrics at all; see [model/sheet.en.md](model/sheet.en.md)'s "Feature: row height" section), and sets it explicitly on each `<tr>` as `style="height:...px;"` (Issue #51)
- Keeps only 2 rows/columns of context on either side of an actual change, collapsing any longer run of unchanged rows/columns into a single `⋯ N row(s)/column(s) omitted ⋯` line (`build_line_plan`) — the same logic applied independently to both the row and column axis
- `grid_sections_from_paths` ([Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23) follow-up, [`action.yml`'s `visual` input](action.en.md)) provides its own path-based, "parse → diff → render" high-level entry point mirroring `markdown.rs::diff_file_section_from_paths`'s shape — the `A`/`D` statuses are expressed with no separate diffing logic at all, just a `diff_workbooks` call against an empty stand-in `Workbook` on the missing side. It returns `Vec<GridSection>`, one HTML fragment per sheet — empty on a parse error or when nothing changed
- `wrap_grid_page` wraps `render_sheet_split`/`GridSection::html`'s fragment(s) into a standalone HTML page with a stylesheet and legend — shared by both `examples/xlsx_diff_grid.rs` and `cli/`'s `--grid-html-dir` flag (for attaching to a workflow artifact)
- **Explicitly out of scope**: actually publishing/distributing the generated HTML (attaching it to a workflow artifact is [`cli/`](cli.en.md)'s and [`action.yml`](action.en.md)'s responsibility — see "Open questions" below)

## Key types / functions (draft)

```rust
use crate::diff::{DiffStatus, SheetDiff};
use crate::markdown::DiffMode;
use crate::model::{Sheet, Workbook};

pub fn render_sheet_split(
    sheet_diff: &SheetDiff,
    base: &Workbook,
    head: &Workbook,
    base_sheet: Option<&Sheet>,
    head_sheet: Option<&Sheet>,
) -> String;

pub fn wrap_grid_page(sections: &str) -> String;

pub struct GridSection {
    pub sheet_name: String,
    pub html: String,
}

pub fn grid_sections_from_paths(
    git_status: &str,
    base_path: Option<&str>,
    head_path: Option<&str>,
    diff_mode: DiffMode,
) -> Vec<GridSection>;
```

The internal `LineSlot`/`CellChange`/`Side` enums and helpers (`render_table`, `render_cell`, `resolve_visual_style`, `border_sides_css`, `excel_width_to_px`, `column_pixel_width`, `excel_height_pt_to_px`, `row_pixel_height`, …) are all private. See [`src/grid.rs`](../../src/grid.rs) for the actual implementation.

## Dependencies

- Depends on: [`diff/model.rs`](diff/model.en.md) (`DiffStatus`, `SheetDiff`), [`model/`](model/) (`Borders`, `Cell`, `CellRef`, `CellValue`, `ColorRef`, `ResolvedStyle`, `Rgb`, `Sheet`, `ThemePalette`, `Workbook`), [`resolve/color.rs`](resolve/color.en.md) (`resolve_color` — resolves a `ColorRef` to a real RGB value, including theme and indexed colors), [`json.rs`](json.en.md) (`format_date_time` — renders a `DateTime` cell's value the same ISO-8601-without-timezone way `json.rs` does, instead of `DateTimeValue`'s derived `Debug` form)
- Depends on (`grid_sections_from_paths` only): [`markdown.rs`](markdown.en.md) (`DiffMode` — referenced only to pick `M`'s diffing algorithm; no other `markdown.rs` type or function is ever called, keeping this module's independence intact), [`lib.rs`](lib.en.md) (`parse_workbook`)
- Depended on by: [`lib.rs`](lib.en.md) (re-exports `render_sheet_split`/`wrap_grid_page`/`GridSection`/`grid_sections_from_paths` as public API), `examples/xlsx_diff_grid.rs` (calls `parse_workbook`/`diff_workbooks` and assembles the page via `wrap_grid_page`), [`cli/`](cli.en.md)'s `--grid-html-dir` flag (concatenates every sheet's fragment from `grid_sections_from_paths` and calls `wrap_grid_page` once to write a single combined HTML file)

## Design decision: why a separate module instead of folding into `markdown.rs`

Both `markdown.rs` and this module take `diff::WorkbookDiff`/`SheetDiff` as input, but their output has fundamentally different requirements — `markdown.rs` targets output that survives GitHub's PR-comment sanitization (no decorative HTML/CSS at all), while this module's entire point is decorative CSS (color, borders, column width) as the primary payload. Merging the two into one function would mean always paying the cost of computing both styles, and would force a caller that only wants one of them to also supply the other's irrelevant arguments — this module needs the full `base`/`head` `Workbook` to read a cell's actual style/column-width/merge structure, which `markdown.rs` has no use for at all since `WorkbookDiff` alone is sufficient for it. In practice, `cli/` (uses `markdown.rs::diff_file_section_from_paths`) and `examples/xlsx_diff_grid.rs` (uses this module) format the exact same `diff_workbooks` result into two entirely different shapes, sharing almost no implementation.

## Test plan

Builds synthetic `Sheet`/`Workbook` instances directly via `Sheet::new`/`insert_cell`/`insert_merge`/`set_col_widths` (all `pub(crate)`) — no real file parsing or process spawn needed for any test.

- An unchanged cell renders its actual value with no change-indicator CSS class attached
- An added cell renders as `not-present` (hatched) on the Before side and `border-added` on the After side; a deleted cell shows the symmetric opposite
- A cell whose value changed gets `border-value`; a cell whose value stayed the same but whose style changed gets `border-style` instead — and the two are mutually exclusive (verifies the `CellChange::ValueChanged`/`StyleOnly` decision logic)
- A merged region's extent change (`old_end`/`new_end`) is correctly reflected as a real `colspan` attribute on each respective side, with no dedicated merge-change visual marker emitted at all (a regression check for a feature removed during review), and that it correctly counts toward the summary line's "modified" total
- Unchanged rows between two far-apart changes collapse into a `gap-row`; conversely, a small sheet needing no collapsing never emits one
- A `SheetDiff` with nothing changed on an axis at all (e.g. a visibility-only diff) collapses that whole axis into a single gap when it's large enough that a gap would actually save space, and renders it in full otherwise — it doesn't unconditionally render every line just because nothing on that axis is "changed"
- On a multi-sheet workbook, the sheet heading's position number (`sheet1：`/`sheet2：`) correctly reflects head's own sheet order
- `excel_width_to_px` returns the correct value for a known conversion (2.14 characters — a common grid-paper width — → 15px), directly testing the implementation of Excel's own official formula
- `excel_height_pt_to_px` returns the correct value for known conversions independently confirmed against a real file (15pt → 20px, 166.5pt → 222px), `row_pixel_height` falls back explicit height → `defaultRowHeight` → Excel's own 15pt default in that order, and `render_sheet_split` actually emits `<tr style="height:...px;">` (Issue #51)
- `column_letters` correctly converts a multi-letter column (e.g. the column right after Z, AA)
- `html_escape` correctly escapes `&`/`<`/`>`
- A styled cell's bold flag (`font.bold`) is reflected as `font-weight:700;` in the inline CSS
- `grid_sections_from_paths` (path-based, built from in-memory `.xlsx` bytes the same way `markdown.rs`'s `diff_file_section_from_paths` tests are): each of A/M/D returns the expected number of `GridSection`s; a parse error, no changes, an unrecognized status, or a missing path all return an empty `Vec`

## Open questions

1. ~~**Delivery path for the generated HTML**~~ **Resolved**: implemented as a follow-up to [Issue #24](https://github.com/MinamiyamaKotaro/exceldiff/issues/24). CI originally rendered a screenshot PNG via Playwright (headless Chromium), committed and pushed it to a dedicated orphan branch (`xlsx-diff-images`), and embedded it in the PR comment as a Markdown image via its `raw.githubusercontent.com` URL. That left screenshots unviewable on private repositories, so it was replaced with uploading the screenshot as a workflow artifact (`actions/upload-artifact@v4`) and posting a download link in the comment instead — and then, after feedback that a large sheet's screenshot shrinks to illegible, the PNG rendering step was dropped entirely in favor of attaching `wrap_grid_page`'s standalone HTML directly to that artifact, which also removed the Playwright/Node.js dependency altogether ([Issue #47](https://github.com/MinamiyamaKotaro/exceldiff/issues/47); `action.yml`'s `visual` input, see [action.md](action.en.md) for the full design).
2. **Whether this module should be treated as settled public API the way `markdown.rs` is**: now that `grid_sections_from_paths`/`wrap_grid_page` are actually consumed by a production workflow (`action.yml`'s `visual` mode), `render_sheet_split`'s signature (requiring the full `base`/`head` `Workbook`) has been validated through real use and isn't expected to change significantly going forward.
3. **Interaction between column collapsing and merged cells**: as noted in `render_table`'s doc comment, there's a known limitation when a merge's origin falls inside a collapsed row/column range — `covered` tracking doesn't account for that case correctly. This seems unlikely to matter in grid-paper Excel's typical usage (a merged cell is usually near a heading/label where changes cluster, which tends to already fall within the kept context), but this hasn't been confirmed.
4. ~~**A merged cell's right/bottom border can go missing**~~ **Resolved (Issue #50)**: contrary to this module's own opening claim of faithfully reproducing the real Excel grid, a merged region's outer edge (specifically its right/bottom side) could go missing against a real `.xlsx` file. The cause wasn't this module at all — it was `Sheet::finalize_merges` in [`model/sheet.en.md`](model/sheet.en.md): a non-origin cell's own border was unconditionally discarded once parsing finished (see model/sheet.en.md's "Correction (Issue #50)" section for the full story). `grid.rs` itself needed no changes — fixing the model layer alone corrected this module's rendered output too.
