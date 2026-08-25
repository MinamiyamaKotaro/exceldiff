// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! Renders one sheet's diff as two Excel-like spreadsheet grids side by
//! side — "Before" (base) on the left, "After" (head) on the right — the
//! same split-view convention `git diff --color-moved`/GitHub's "Split"
//! PR view use for text. A cell that only exists on one side (an added
//! or deleted cell) renders as a hatched "not present" placeholder on
//! the other side, exactly like a split diff leaves the opposite column
//! blank for an inserted/deleted line.
//!
//! Complements [`crate::markdown`] rather than replacing it: GitHub
//! sanitizes a `style=` attribute out of HTML pasted into a PR comment
//! (see [`crate::markdown`]'s own doc comment), so the colored
//! fills/borders/merged cells this module renders can never be embedded
//! directly into a PR comment body — a consumer that wants this grid
//! visible from a PR still needs to get the resulting HTML in front of a
//! reader some other way (a screenshot image, a page published to GitHub
//! Pages, a downloadable CI artifact, …). This module only renders the
//! HTML; publishing it anywhere is the caller's responsibility.
//!
//! Follow-up for "grid-paper Excel" (a Japanese business document style
//! built from a fine, uniform grid of narrow cells with merged regions
//! standing in for headings/labels/borders — [`lib.rs`](crate)'s own doc
//! comment names this as the library's core target shape): each side
//! renders its *own* actual merge structure
//! ([`Sheet::merged_region_at`]) as real HTML `rowspan`/`colspan`, its
//! own real fill color/bold/border (rather than a flat added/deleted/
//! modified color overriding the cell's actual appearance), and collapses
//! any long run of rows/columns with nothing changed in them into a
//! single `⋯ N row(s)/column(s) omitted ⋯` indicator — the same
//! `git diff` context-line convention, applied to a 2-D grid instead of
//! a 1-D line list, since a grid-paper sheet routinely runs to thousands
//! of rows/columns and a dense unwindowed render of one would be both
//! enormous and useless.

use crate::diff::{DiffStatus, SheetDiff};
use crate::model::{
    Borders, Cell, CellRef, CellValue, ColorRef, ResolvedStyle, Rgb, Sheet, ThemePalette, Workbook,
};
use crate::resolve::resolve_color;
use std::collections::{BTreeSet, HashMap, HashSet};

/// The row-number column's fixed width in pixels — must match the
/// `th.row-head` CSS rule a caller's stylesheet defines, since
/// [`render_table`] needs it (as a plain number) to compute the
/// `<table>`'s own total pinned width (see that function's doc comment
/// for why the table needs one at all).
const ROW_HEAD_WIDTH_PX: u32 = 36;

/// Fixed width of a collapsed-column gap indicator — deliberately much
/// narrower than a real grid-paper column (which can be as little as
/// ~15px itself, see [`column_pixel_width`]), since the gap only ever
/// holds a single "⋯" glyph, not real content.
const GAP_COL_WIDTH_PX: u32 = 22;

/// How many unchanged rows/columns to keep on either side of a line that
/// actually changed — `git diff`'s context-line count, applied to a grid
/// instead of text (row-wise and column-wise both, via
/// [`build_line_plan`] — the same collapsing logic works on either axis,
/// since both are just "a 1-D sequence of coordinates, some of which are
/// interesting").
const CONTEXT_LINES: u32 = 2;

/// Two changed lines (or their context windows) closer together than this
/// get merged into one visible block instead of leaving a gap indicator
/// that would collapse fewer lines than the indicator itself takes up
/// space to show.
const MIN_GAP_TO_COLLAPSE: u32 = 3;

/// One row or column of the rendered table: either a real sheet line, or
/// a collapsed run of lines with nothing interesting in them.
enum LineSlot {
    Line(u32),
    Gap { start: u32, end: u32 },
}

/// Builds the row (or column) plan shared by both the Before and After
/// tables (so they stay aligned on that axis): every line within
/// [`CONTEXT_LINES`] of a line that actually changed, plus a single
/// collapsed [`LineSlot::Gap`] for each stretch of lines with nothing of
/// interest in it.
fn build_line_plan(max: u32, changed: &BTreeSet<u32>) -> Vec<LineSlot> {
    if changed.is_empty() {
        return (1..=max).map(LineSlot::Line).collect();
    }

    let mut windows: Vec<(u32, u32)> = changed
        .iter()
        .map(|&r| {
            (
                r.saturating_sub(CONTEXT_LINES).max(1),
                (r + CONTEXT_LINES).min(max),
            )
        })
        .collect();
    windows.sort_unstable();

    let mut merged: Vec<(u32, u32)> = Vec::new();
    for (start, end) in windows {
        match merged.last_mut() {
            Some((_, last_end)) if start <= *last_end + MIN_GAP_TO_COLLAPSE => {
                *last_end = (*last_end).max(end);
            }
            _ => merged.push((start, end)),
        }
    }

    let mut plan = Vec::new();
    let mut cursor = 1;
    for (start, end) in merged {
        if cursor < start {
            plan.push(LineSlot::Gap {
                start: cursor,
                end: start - 1,
            });
        }
        plan.extend((start..=end).map(LineSlot::Line));
        cursor = end + 1;
    }
    if cursor <= max {
        plan.push(LineSlot::Gap {
            start: cursor,
            end: max,
        });
    }
    plan
}

/// What changed about a cell, beyond the plain `DiffStatus` — distinguishes
/// a style-only change (value identical, only paint differs) from a value
/// change, since `DiffStatus::Modified` alone doesn't (see
/// [`crate::diff`]'s own doc comment on why `old_style`/`new_style` are
/// sparse while `old_value`/`new_value` are always both present).
#[derive(Clone, Copy, PartialEq, Eq)]
enum CellChange {
    Unchanged,
    Added,
    Deleted,
    ValueChanged,
    StyleOnly,
}

/// `sheet1：報告書` — the sheet's 1-based tab position plus its name, so
/// two sheets that both happen to be called e.g. "Sheet1" internally (or
/// just visually similar report sheets, as in a multi-sheet diff) stay
/// distinguishable at a glance. Position comes from `head`'s own sheet
/// order (the "current" state) — a sheet that only existed in `base`
/// (`DiffStatus::Deleted`) falls back to `base`'s order instead, since
/// `head` doesn't have it at all to report a position from.
fn sheet_heading(sheet_diff: &SheetDiff, base: &Workbook, head: &Workbook) -> String {
    let position = head
        .sheets()
        .iter()
        .position(|s| s.name == sheet_diff.name)
        .or_else(|| base.sheets().iter().position(|s| s.name == sheet_diff.name))
        .map(|i| i + 1);
    match position {
        Some(n) => format!("sheet{n}：{}", html_escape(&sheet_diff.name)),
        None => html_escape(&sheet_diff.name),
    }
}

/// Renders one sheet's diff as two side-by-side HTML grids (Before |
/// After) wrapped in a `<section class="sheet">`. `base_sheet`/
/// `head_sheet` are the sheet named `sheet_diff.name` looked up from
/// `base`/`head` respectively (`None` when the sheet doesn't exist on
/// that side — `DiffStatus::Added`/`Deleted`) — the caller already has
/// `base`/`head` on hand from computing `sheet_diff` in the first place
/// (e.g. via [`crate::diff_workbooks`]), so this doesn't re-derive them.
///
/// Returns a self-contained HTML fragment with semantic class names
/// (`.sheet`, `.split`, `.pane-before`/`.pane-after`, `table.grid`,
/// `td.cell.border-added`/`.border-deleted`/`.border-value`/
/// `.border-style`, `.not-present`, `.gap-row`/`.gap-col`, …) — it does
/// not include a `<style>` block or page chrome. A caller assembling a
/// full HTML page (e.g. `examples/xlsx_diff_grid.rs`) supplies its own
/// stylesheet for these classes.
pub fn render_sheet_split(
    sheet_diff: &SheetDiff,
    base: &Workbook,
    head: &Workbook,
    base_sheet: Option<&Sheet>,
    head_sheet: Option<&Sheet>,
) -> String {
    let changed: HashMap<(u32, u32), DiffStatus> = sheet_diff
        .cells
        .iter()
        .map(|c| ((c.row, c.col), c.status))
        .collect();

    let mut max_row = 1;
    let mut max_col = 1;
    for sheet in [base_sheet, head_sheet].into_iter().flatten() {
        for (coord, _) in sheet.iter_cells() {
            max_row = max_row.max(coord.row);
            max_col = max_col.max(coord.col);
        }
    }
    for &(row, col) in changed.keys() {
        max_row = max_row.max(row);
        max_col = max_col.max(col);
    }
    for m in &sheet_diff.merges {
        for end in [m.old_end, m.new_end].into_iter().flatten() {
            max_row = max_row.max(end.row);
            max_col = max_col.max(end.col);
        }
    }

    let mut changed_rows: BTreeSet<u32> = changed.keys().map(|&(row, _)| row).collect();
    let mut changed_cols: BTreeSet<u32> = changed.keys().map(|&(_, col)| col).collect();
    for m in &sheet_diff.merges {
        changed_rows.insert(m.start.row);
        changed_cols.insert(m.start.col);
        for end in [m.old_end, m.new_end].into_iter().flatten() {
            changed_rows.insert(end.row);
            changed_cols.insert(end.col);
        }
    }
    let row_plan = build_line_plan(max_row, &changed_rows);
    let col_plan = build_line_plan(max_col, &changed_cols);

    let added = sheet_diff
        .cells
        .iter()
        .filter(|c| c.status == DiffStatus::Added)
        .count();
    // A merged region appearing, disappearing, or resizing is a
    // structural edit to the sheet, not a cell value edit, but there's no
    // separate "merges changed" slot in this summary line — lumping it
    // into "modified" (rather than adding it to "added"/"deleted"
    // according to the merge's own status) reads better: a merge that
    // vanished didn't remove any *cell*, it changed how existing cells
    // are grouped, which is a modification either way you look at it.
    // Mirrors `markdown::format_sheet_diff`'s identical treatment.
    let modified = sheet_diff
        .cells
        .iter()
        .filter(|c| c.status == DiffStatus::Modified)
        .count()
        + sheet_diff.merges.len();
    let deleted = sheet_diff
        .cells
        .iter()
        .filter(|c| c.status == DiffStatus::Deleted)
        .count();

    let mut out = format!(
        "<section class=\"sheet\">\n<h2>{} <span class=\"counts\">— {added} added, {modified} modified, {deleted} deleted</span></h2>\n<div class=\"split\">\n",
        sheet_heading(sheet_diff, base, head)
    );

    out.push_str("<div class=\"pane pane-before\">\n<div class=\"pane-label\">Before</div>\n");
    out.push_str(&render_table(
        &row_plan,
        &col_plan,
        &changed,
        Side::Before,
        base,
        head,
        base_sheet,
        head_sheet,
    ));
    out.push_str("</div>\n");

    out.push_str("<div class=\"pane pane-after\">\n<div class=\"pane-label\">After</div>\n");
    out.push_str(&render_table(
        &row_plan,
        &col_plan,
        &changed,
        Side::After,
        base,
        head,
        base_sheet,
        head_sheet,
    ));
    out.push_str("</div>\n");

    out.push_str("</div>\n</section>\n");
    out
}

#[derive(Clone, Copy)]
enum Side {
    Before,
    After,
}

#[allow(clippy::too_many_arguments)]
fn render_table(
    row_plan: &[LineSlot],
    col_plan: &[LineSlot],
    changed: &HashMap<(u32, u32), DiffStatus>,
    side: Side,
    base: &Workbook,
    head: &Workbook,
    base_sheet: Option<&Sheet>,
    head_sheet: Option<&Sheet>,
) -> String {
    let this_sheet = match side {
        Side::Before => base_sheet,
        Side::After => head_sheet,
    };

    // Real width per visible column slot — a collapsed `LineSlot::Gap`
    // gets the small fixed `GAP_COL_WIDTH_PX` instead of a real
    // `column_pixel_width` lookup, since it holds only a "⋯" glyph, not
    // real column content.
    let col_widths: Vec<u32> = col_plan
        .iter()
        .map(|slot| match *slot {
            LineSlot::Line(col) => column_pixel_width(this_sheet, col),
            LineSlot::Gap { .. } => GAP_COL_WIDTH_PX,
        })
        .collect();
    // `table-layout: fixed` only fixes how a table's *own* width gets
    // distributed across columns — it does NOT stop an auto-width table
    // from growing past the sum of those column widths to fit unbreakable
    // (`white-space: nowrap`) content, which silently defeats any
    // `overflow: hidden` clip a caller's stylesheet applies (confirmed by
    // an isolated repro: the same markup clips correctly only once the
    // `<table>` itself gets an explicit `width`). So the table's width is
    // pinned here to the exact sum of its real column widths, forcing the
    // browser to actually clip instead of quietly widening columns around
    // long unwrapped text.
    let table_width: u32 = ROW_HEAD_WIDTH_PX + col_widths.iter().sum::<u32>();
    let mut out = format!(
        "<div class=\"grid-scroll\">\n<table class=\"grid\" style=\"width:{table_width}px;\">\n"
    );

    out.push_str("<tr><th class=\"corner\"></th>");
    for (slot, &px) in col_plan.iter().zip(&col_widths) {
        match *slot {
            LineSlot::Line(col) => out.push_str(&format!(
                "<th class=\"col-head\" style=\"width:{px}px;\">{}</th>",
                column_letters(col)
            )),
            LineSlot::Gap { start, end } => {
                let count = end - start + 1;
                out.push_str(&format!(
                    "<th class=\"col-head gap-head\" style=\"width:{px}px;\" title=\"{count} column(s) omitted ({}\u{2013}{})\">⋯</th>",
                    column_letters(start), column_letters(end)
                ));
            }
        }
    }
    out.push_str("</tr>\n");

    // Cells consumed by a rowspan/colspan from an earlier (row, col) in
    // this same table — HTML requires the `<td>` for a covered position
    // to be omitted entirely, not just left empty, or the row's columns
    // desync from the header. Merges only ever extend down/right from
    // their origin, so a single forward row-major pass is enough: every
    // origin is visited before any cell it covers. (A merge whose origin
    // falls inside a collapsed `LineSlot::Gap` on either axis — never
    // visited — would break this; not a shape any known caller produces.)
    let mut covered: HashSet<(u32, u32)> = HashSet::new();

    let total_cols = col_plan.len();
    for slot in row_plan {
        let row = match *slot {
            LineSlot::Line(row) => row,
            LineSlot::Gap { start, end } => {
                let count = end - start + 1;
                out.push_str(&format!(
                    "<tr class=\"gap-row\"><th class=\"row-head gap-head\">⋯</th><td class=\"gap\" colspan=\"{total_cols}\">⋯ {count} row(s) omitted ({start}\u{2013}{end}) ⋯</td></tr>\n"
                ));
                continue;
            }
        };
        out.push_str(&format!("<tr><th class=\"row-head\">{row}</th>"));
        for col_slot in col_plan {
            let col = match *col_slot {
                LineSlot::Line(col) => col,
                LineSlot::Gap { start, end } => {
                    let count = end - start + 1;
                    out.push_str(&format!(
                        "<td class=\"cell gap-col\" title=\"{count} column(s) omitted ({}\u{2013}{})\">⋯</td>",
                        column_letters(start), column_letters(end)
                    ));
                    continue;
                }
            };
            if covered.contains(&(row, col)) {
                continue;
            }
            let coord = CellRef { row, col };
            let span = this_sheet.and_then(|s| s.merged_region_at(coord));
            if let Some(region) = span {
                for r in region.start.row..=region.end.row {
                    for c in region.start.col..=region.end.col {
                        if (r, c) != (row, col) {
                            covered.insert((r, c));
                        }
                    }
                }
            }
            let status = changed.get(&(row, col)).copied();
            out.push_str(&render_cell(
                coord,
                status,
                span.map(|r| (r.row_span(), r.col_span())),
                side,
                base,
                head,
                base_sheet,
                head_sheet,
            ));
        }
        out.push_str("</tr>\n");
    }

    out.push_str("</table>\n</div>\n");
    out
}

#[allow(clippy::too_many_arguments)]
fn render_cell(
    coord: CellRef,
    status: Option<DiffStatus>,
    span: Option<(u32, u32)>,
    side: Side,
    base: &Workbook,
    head: &Workbook,
    base_sheet: Option<&Sheet>,
    head_sheet: Option<&Sheet>,
) -> String {
    let base_cell = base_sheet.and_then(|s| s.get(coord));
    let head_cell = head_sheet.and_then(|s| s.get(coord));

    let change = match status {
        None => CellChange::Unchanged,
        Some(DiffStatus::Added) => CellChange::Added,
        Some(DiffStatus::Deleted) => CellChange::Deleted,
        Some(DiffStatus::Modified) => {
            let old_value = base_cell.and_then(|c| c.value.as_ref());
            let new_value = head_cell.and_then(|c| c.value.as_ref());
            if old_value == new_value {
                CellChange::StyleOnly
            } else {
                CellChange::ValueChanged
            }
        }
    };

    // A cell that doesn't exist on this side at all (added → nothing to
    // show in "Before", deleted → nothing to show in "After") — the split
    // view's equivalent of a blank line on the opposite column.
    let not_present = matches!(
        (side, change),
        (Side::Before, CellChange::Added) | (Side::After, CellChange::Deleted)
    );
    if not_present {
        return "<td class=\"cell not-present\"></td>".to_string();
    }

    let (this_cell, this_workbook) = match side {
        Side::Before => (base_cell, base),
        Side::After => (head_cell, head),
    };
    let (bg, bold, wrap) = resolve_visual_style(this_cell, this_workbook.theme());
    let mut style_attr = String::new();
    if let Some(bg) = bg {
        style_attr.push_str(&format!("background-color:{bg};"));
    }
    if bold {
        style_attr.push_str("font-weight:700;");
    }
    // Excel only wraps a cell's text when its own `wrapText` alignment
    // flag is set (`ResolvedStyle::wrap_text`, Issue #37). Otherwise it
    // spills the unwrapped text visually into the next cell over — but
    // *only* while that neighbor is itself empty; a neighbor with its own
    // content blocks the spill and the text clips at the column boundary
    // instead. `column_pixel_width` still bounds the *column*'s real
    // width either way (this table's own width stays pinned — see
    // `render_table`'s doc comment) — an overflow here is genuinely
    // Excel's own rendering, not the column silently growing.
    let this_sheet = match side {
        Side::Before => base_sheet,
        Side::After => head_sheet,
    };
    let overflow_allowed = span.is_none() && next_cell_is_empty(this_sheet, coord);
    style_attr.push_str(match (wrap, overflow_allowed) {
        (true, _) => "white-space:normal;overflow-wrap:anywhere;",
        (false, true) => "white-space:nowrap;overflow:visible;",
        (false, false) => "white-space:nowrap;overflow:hidden;text-overflow:clip;",
    });
    // `model::Borders` only ever records *whether* a side has a border
    // (Issue #97) — it carries no color, because ECMA-376 lets a
    // `<border>` side omit `<color>` entirely, which Excel's own UI shows
    // and treats as "Automatic" (black), not "no color". So a real border
    // renders in black here rather than in the neutral grey a caller's
    // base gridline rule would otherwise use everywhere — the two need to
    // read as visually different things (a real border in the source file
    // vs. a caller's own gridline), and only the explicit sides get an
    // override; the rest keep inheriting whatever the caller's stylesheet
    // sets by default.
    if let Some(borders) = this_cell
        .and_then(|c| c.style.as_deref())
        .map(|s| &s.borders)
    {
        style_attr.push_str(&border_sides_css(borders));
    }
    let style_attr = format!(" style=\"{style_attr}\"");

    let border_class = match (side, change) {
        (_, CellChange::Unchanged) => "",
        (Side::After, CellChange::Added) => " border-added",
        (Side::Before, CellChange::Deleted) => " border-deleted",
        (_, CellChange::ValueChanged) => " border-value",
        (_, CellChange::StyleOnly) => " border-style",
        // Unreachable: the `not_present` check above already returned for
        // (Before, Added) and (After, Deleted).
        _ => "",
    };

    let span_attrs = match span {
        Some((row_span, col_span)) if row_span > 1 || col_span > 1 => {
            format!(" rowspan=\"{row_span}\" colspan=\"{col_span}\"")
        }
        _ => String::new(),
    };

    let value = cell_value_html(this_cell.and_then(|c| c.value.as_ref()));
    if value.is_empty() && border_class.is_empty() {
        return format!("<td class=\"cell empty\"{style_attr}{span_attrs}></td>");
    }
    format!("<td class=\"cell{border_class}\"{style_attr}{span_attrs}>{value}</td>")
}

/// Resolves a cell's fill color (foreground first, then background — same
/// precedence [`crate::json`]'s `style_to_json` documents for a solid
/// fill), bold flag, and `wrapText` flag. `(None, false, false)` when the
/// cell carries no style at all — Excel's own "no fill, not bold, doesn't
/// wrap" look.
fn resolve_visual_style(
    cell: Option<&Cell>,
    theme: Option<&ThemePalette>,
) -> (Option<String>, bool, bool) {
    let Some(style) = cell.and_then(|c| c.style.as_deref()) else {
        return (None, false, false);
    };
    let bg = fill_color(style, theme).map(rgb_to_css);
    (bg, style.font.bold, style.wrap_text)
}

/// Inline `border-{side}` overrides for whichever sides `borders` marks
/// present, in black (Excel's "Automatic" default — see the call site's
/// doc comment for why the model has no other color to draw from). A side
/// left `false` emits nothing, so it keeps inheriting whatever a caller's
/// stylesheet sets by default for that edge.
fn border_sides_css(borders: &Borders) -> String {
    let mut css = String::new();
    if borders.top {
        css.push_str("border-top:1px solid #000;");
    }
    if borders.right {
        css.push_str("border-right:1px solid #000;");
    }
    if borders.bottom {
        css.push_str("border-bottom:1px solid #000;");
    }
    if borders.left {
        css.push_str("border-left:1px solid #000;");
    }
    css
}

fn fill_color(style: &ResolvedStyle, theme: Option<&ThemePalette>) -> Option<Rgb> {
    let color: &ColorRef = style
        .fill_fg_color
        .as_ref()
        .or(style.fill_bg_color.as_ref())?;
    resolve_color(color, theme)
}

fn rgb_to_css(rgb: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb.r, rgb.g, rgb.b)
}

/// Excel's real column-width unit is "characters of the workbook's
/// default font" (`<col width="2.14"/>` etc.), not pixels — so a 15px
/// grid-paper column doesn't render as 15px unless this conversion runs.
/// This is Excel's own documented algorithm (`MDW` = Maximum Digit Width,
/// the widest digit glyph's pixel width in the default font — 7px for
/// Calibri 11 @ 96 DPI, [`crate::model`]'s `Font::default()` exact font,
/// so hardcoding 7 here matches the model's own default rather than
/// guessing): `floor(((256*width + floor(128/MDW)) / 256) * MDW)`.
fn excel_width_to_px(width: f64) -> u32 {
    const MDW: f64 = 7.0;
    (((256.0 * width + (128.0 / MDW).trunc()) / 256.0) * MDW).trunc() as u32
}

/// A column's real rendered width in pixels: its own `<cols>` entry, else
/// the sheet's `defaultColWidth`, else Excel's own hardcoded global
/// default (8.43 characters — the width Excel itself falls back to when a
/// workbook specifies neither).
fn column_pixel_width(sheet: Option<&Sheet>, col: u32) -> u32 {
    let width = sheet
        .and_then(|s| s.column_width(col))
        .or_else(|| sheet.and_then(|s| s.default_col_width()))
        .unwrap_or(8.43);
    excel_width_to_px(width).max(2)
}

fn cell_value_html(v: Option<&CellValue>) -> String {
    match v {
        None => String::new(),
        Some(CellValue::Number(n)) => format!("<span class=\"num\">{n}</span>"),
        Some(CellValue::Boolean(b)) => b.to_string(),
        Some(CellValue::DateTime(d)) => html_escape(&format!("{d:?}")),
        Some(CellValue::Error(e)) => format!("<span class=\"err\">{}</span>", html_escape(e)),
        Some(CellValue::Text(s)) => html_escape(s),
    }
}

/// Whether the cell immediately to the right, on this same side, is empty
/// enough for an unwrapped overflow to spill into — true both when no
/// `Cell` is stored there at all and when one exists but carries no
/// `value` (formatting-only cells don't block a spill in Excel either).
/// Doesn't account for that neighbor itself being covered by an earlier
/// merge's `colspan` — a cell directly against a merge boundary on its
/// right always clips rather than overflowing into the merge, which is
/// close enough to Excel's own behavior for this to not matter in
/// practice (a merge boundary is itself usually a visual break).
fn next_cell_is_empty(sheet: Option<&Sheet>, coord: CellRef) -> bool {
    let next = CellRef {
        row: coord.row,
        col: coord.col + 1,
    };
    match sheet.and_then(|s| s.get(next)) {
        Some(cell) => cell.value.is_none(),
        None => true,
    }
}

fn column_letters(mut n: u32) -> String {
    let mut buf = Vec::new();
    while n > 0 {
        let rem = ((n - 1) % 26) as u8;
        buf.push(b'A' + rem);
        n = (n - 1) / 26;
    }
    buf.reverse();
    String::from_utf8(buf).unwrap_or_default()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{CellDiff, CellPos, MergeDiff};
    use crate::model::{DateTimeValue, Font, MergedRegion, SheetVisibility};

    fn cell(value: CellValue) -> Cell {
        Cell {
            value: Some(value),
            style: None,
        }
    }

    fn styled_cell(value: Option<CellValue>, style: ResolvedStyle) -> Cell {
        Cell {
            value,
            style: Some(std::sync::Arc::new(style)),
        }
    }

    fn workbook_with(sheet: Sheet) -> Workbook {
        Workbook::new(vec![sheet], None)
    }

    fn cell_diff(row: u32, col: u32, status: DiffStatus) -> CellDiff {
        CellDiff {
            row,
            col,
            status,
            old_col: None,
            old_row: None,
            old_value: None,
            new_value: None,
            old_style: None,
            new_style: None,
        }
    }

    #[test]
    fn unchanged_cell_renders_its_real_value_with_no_change_border() {
        let mut base = Sheet::new("Sheet1".to_string(), SheetVisibility::Visible);
        base.insert_cell(CellRef { row: 1, col: 1 }, cell(CellValue::Number(42.0)));
        let mut head = base.clone();
        // Force head to be a distinct clone so base/head are independently owned.
        head.insert_cell(CellRef { row: 1, col: 1 }, cell(CellValue::Number(42.0)));

        let base_wb = workbook_with(base.clone());
        let head_wb = workbook_with(head.clone());
        let sheet_diff = SheetDiff {
            name: "Sheet1".to_string(),
            status: DiffStatus::Modified,
            old_visibility: None,
            new_visibility: None,
            cells: Vec::new(),
            merges: Vec::new(),
        };

        let html = render_sheet_split(&sheet_diff, &base_wb, &head_wb, Some(&base), Some(&head));
        assert!(html.contains("<span class=\"num\">42</span>"));
        assert!(!html.contains("border-added"));
        assert!(!html.contains("border-value"));
    }

    #[test]
    fn added_cell_is_not_present_before_and_bordered_after() {
        let base = Sheet::new("Sheet1".to_string(), SheetVisibility::Visible);
        let mut head = Sheet::new("Sheet1".to_string(), SheetVisibility::Visible);
        head.insert_cell(
            CellRef { row: 1, col: 1 },
            cell(CellValue::Text("hi".into())),
        );

        let base_wb = workbook_with(base.clone());
        let head_wb = workbook_with(head.clone());
        let sheet_diff = SheetDiff {
            name: "Sheet1".to_string(),
            status: DiffStatus::Modified,
            old_visibility: None,
            new_visibility: None,
            cells: vec![cell_diff(1, 1, DiffStatus::Added)],
            merges: Vec::new(),
        };

        let html = render_sheet_split(&sheet_diff, &base_wb, &head_wb, Some(&base), Some(&head));
        assert!(html.contains("1 added, 0 modified, 0 deleted"));
        assert!(html.contains("cell not-present"));
        assert!(html.contains("border-added"));
    }

    #[test]
    fn deleted_cell_is_bordered_before_and_not_present_after() {
        let mut base = Sheet::new("Sheet1".to_string(), SheetVisibility::Visible);
        base.insert_cell(
            CellRef { row: 1, col: 1 },
            cell(CellValue::Text("bye".into())),
        );
        let head = Sheet::new("Sheet1".to_string(), SheetVisibility::Visible);

        let base_wb = workbook_with(base.clone());
        let head_wb = workbook_with(head.clone());
        let sheet_diff = SheetDiff {
            name: "Sheet1".to_string(),
            status: DiffStatus::Modified,
            old_visibility: None,
            new_visibility: None,
            cells: vec![cell_diff(1, 1, DiffStatus::Deleted)],
            merges: Vec::new(),
        };

        let html = render_sheet_split(&sheet_diff, &base_wb, &head_wb, Some(&base), Some(&head));
        assert!(html.contains("0 added, 0 modified, 1 deleted"));
        assert!(html.contains("border-deleted"));
        assert!(html.contains("cell not-present"));
    }

    #[test]
    fn style_only_change_gets_its_own_border_class_not_value_changed() {
        let yellow = ResolvedStyle {
            fill_fg_color: Some(ColorRef::Rgb(std::sync::Arc::from("FFFFFF00"))),
            ..Default::default()
        };
        let mut base = Sheet::new("Sheet1".to_string(), SheetVisibility::Visible);
        base.insert_cell(
            CellRef { row: 1, col: 1 },
            styled_cell(Some(CellValue::Number(5.0)), ResolvedStyle::default()),
        );
        let mut head = Sheet::new("Sheet1".to_string(), SheetVisibility::Visible);
        head.insert_cell(
            CellRef { row: 1, col: 1 },
            styled_cell(Some(CellValue::Number(5.0)), yellow),
        );

        let base_wb = workbook_with(base.clone());
        let head_wb = workbook_with(head.clone());
        let sheet_diff = SheetDiff {
            name: "Sheet1".to_string(),
            status: DiffStatus::Modified,
            old_visibility: None,
            new_visibility: None,
            cells: vec![cell_diff(1, 1, DiffStatus::Modified)],
            merges: Vec::new(),
        };

        let html = render_sheet_split(&sheet_diff, &base_wb, &head_wb, Some(&base), Some(&head));
        assert!(html.contains("border-style"));
        assert!(!html.contains("border-value"));
        assert!(html.contains("background-color:#ffff00"));
    }

    #[test]
    fn value_change_gets_border_value_not_border_style() {
        let mut base = Sheet::new("Sheet1".to_string(), SheetVisibility::Visible);
        base.insert_cell(CellRef { row: 1, col: 1 }, cell(CellValue::Number(1.0)));
        let mut head = Sheet::new("Sheet1".to_string(), SheetVisibility::Visible);
        head.insert_cell(CellRef { row: 1, col: 1 }, cell(CellValue::Number(2.0)));

        let base_wb = workbook_with(base.clone());
        let head_wb = workbook_with(head.clone());
        let sheet_diff = SheetDiff {
            name: "Sheet1".to_string(),
            status: DiffStatus::Modified,
            old_visibility: None,
            new_visibility: None,
            cells: vec![cell_diff(1, 1, DiffStatus::Modified)],
            merges: Vec::new(),
        };

        let html = render_sheet_split(&sheet_diff, &base_wb, &head_wb, Some(&base), Some(&head));
        assert!(html.contains("border-value"));
        assert!(!html.contains("border-style"));
    }

    #[test]
    fn merged_region_renders_as_real_rowspan_colspan_and_counts_as_modified() {
        let mut base = Sheet::new("Sheet1".to_string(), SheetVisibility::Visible);
        base.insert_merge(MergedRegion {
            start: CellRef { row: 1, col: 1 },
            end: CellRef { row: 1, col: 3 },
        });
        let mut head = Sheet::new("Sheet1".to_string(), SheetVisibility::Visible);
        head.insert_merge(MergedRegion {
            start: CellRef { row: 1, col: 1 },
            end: CellRef { row: 1, col: 4 },
        });

        let base_wb = workbook_with(base.clone());
        let head_wb = workbook_with(head.clone());
        let sheet_diff = SheetDiff {
            name: "Sheet1".to_string(),
            status: DiffStatus::Modified,
            old_visibility: None,
            new_visibility: None,
            cells: Vec::new(),
            merges: vec![MergeDiff {
                status: DiffStatus::Modified,
                start: CellPos { row: 1, col: 1 },
                old_end: Some(CellPos { row: 1, col: 3 }),
                new_end: Some(CellPos { row: 1, col: 4 }),
            }],
        };

        let html = render_sheet_split(&sheet_diff, &base_wb, &head_wb, Some(&base), Some(&head));
        // Merge changes count toward "modified" even with zero cell diffs.
        assert!(html.contains("0 added, 1 modified, 0 deleted"));
        assert!(html.contains("colspan=\"3\""));
        assert!(html.contains("colspan=\"4\""));
        // No dedicated visual marker for the merge change itself (removed
        // per review feedback — see `render_cell`'s doc comment).
        assert!(!html.contains("merge-changed"));
    }

    #[test]
    fn far_apart_changes_collapse_the_gap_between_them() {
        let mut base = Sheet::new("Sheet1".to_string(), SheetVisibility::Visible);
        base.insert_cell(CellRef { row: 1, col: 1 }, cell(CellValue::Number(1.0)));
        let mut head = base.clone();
        head.insert_cell(CellRef { row: 50, col: 1 }, cell(CellValue::Number(2.0)));

        let base_wb = workbook_with(base.clone());
        let head_wb = workbook_with(head.clone());
        let sheet_diff = SheetDiff {
            name: "Sheet1".to_string(),
            status: DiffStatus::Modified,
            old_visibility: None,
            new_visibility: None,
            cells: vec![cell_diff(50, 1, DiffStatus::Added)],
            merges: Vec::new(),
        };

        let html = render_sheet_split(&sheet_diff, &base_wb, &head_wb, Some(&base), Some(&head));
        assert!(html.contains("class=\"gap-row\""));
        assert!(html.contains("row(s) omitted"));
    }

    #[test]
    fn no_changes_at_all_produces_no_gap_when_sheet_is_small() {
        let mut base = Sheet::new("Sheet1".to_string(), SheetVisibility::Visible);
        base.insert_cell(CellRef { row: 1, col: 1 }, cell(CellValue::Number(1.0)));
        let head = base.clone();

        let base_wb = workbook_with(base.clone());
        let head_wb = workbook_with(head.clone());
        let sheet_diff = SheetDiff {
            name: "Sheet1".to_string(),
            status: DiffStatus::Modified,
            old_visibility: None,
            new_visibility: None,
            cells: Vec::new(),
            merges: Vec::new(),
        };

        let html = render_sheet_split(&sheet_diff, &base_wb, &head_wb, Some(&base), Some(&head));
        assert!(!html.contains("gap-row"));
    }

    #[test]
    fn sheet_position_prefers_head_order_over_base() {
        let mut sheet1 = Sheet::new("A".to_string(), SheetVisibility::Visible);
        sheet1.insert_cell(CellRef { row: 1, col: 1 }, cell(CellValue::Number(1.0)));
        let mut sheet2 = Sheet::new("B".to_string(), SheetVisibility::Visible);
        sheet2.insert_cell(CellRef { row: 1, col: 1 }, cell(CellValue::Number(1.0)));

        // "B" is second in head's own sheet order.
        let base_wb = Workbook::new(vec![sheet1.clone(), sheet2.clone()], None);
        let head_wb = Workbook::new(vec![sheet1.clone(), sheet2.clone()], None);
        let sheet_diff = SheetDiff {
            name: "B".to_string(),
            status: DiffStatus::Modified,
            old_visibility: None,
            new_visibility: None,
            cells: Vec::new(),
            merges: Vec::new(),
        };

        let html = render_sheet_split(
            &sheet_diff,
            &base_wb,
            &head_wb,
            Some(&sheet2),
            Some(&sheet2),
        );
        assert!(html.contains("sheet2：B"));
    }

    #[test]
    fn excel_width_to_px_matches_a_known_grid_paper_value() {
        // 2.14 characters is a commonly-used grid-paper column width that
        // yields exactly 15px under Excel's own conversion formula.
        assert_eq!(excel_width_to_px(2.14), 15);
    }

    #[test]
    fn column_letters_handles_multi_letter_columns() {
        assert_eq!(column_letters(1), "A");
        assert_eq!(column_letters(26), "Z");
        assert_eq!(column_letters(27), "AA");
        assert_eq!(column_letters(52), "AZ");
    }

    #[test]
    fn html_escape_escapes_reserved_characters() {
        assert_eq!(html_escape("<a & b>"), "&lt;a &amp; b&gt;");
    }

    #[test]
    fn font_bold_is_respected_when_style_present() {
        let bold_style = ResolvedStyle {
            font: Font {
                bold: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut base = Sheet::new("Sheet1".to_string(), SheetVisibility::Visible);
        base.insert_cell(
            CellRef { row: 1, col: 1 },
            styled_cell(Some(CellValue::Text("x".into())), bold_style),
        );
        let head = base.clone();

        let base_wb = workbook_with(base.clone());
        let head_wb = workbook_with(head.clone());
        let sheet_diff = SheetDiff {
            name: "Sheet1".to_string(),
            status: DiffStatus::Modified,
            old_visibility: None,
            new_visibility: None,
            cells: Vec::new(),
            merges: Vec::new(),
        };

        let html = render_sheet_split(&sheet_diff, &base_wb, &head_wb, Some(&base), Some(&head));
        assert!(html.contains("font-weight:700;"));
    }

    #[test]
    fn trailing_unchanged_rows_after_the_last_change_collapse_too() {
        let mut base = Sheet::new("Sheet1".to_string(), SheetVisibility::Visible);
        base.insert_cell(CellRef { row: 1, col: 1 }, cell(CellValue::Number(1.0)));
        base.insert_cell(CellRef { row: 50, col: 1 }, cell(CellValue::Number(1.0)));
        let mut head = base.clone();
        head.insert_cell(CellRef { row: 1, col: 1 }, cell(CellValue::Number(2.0)));

        let base_wb = workbook_with(base.clone());
        let head_wb = workbook_with(head.clone());
        let sheet_diff = SheetDiff {
            name: "Sheet1".to_string(),
            status: DiffStatus::Modified,
            old_visibility: None,
            new_visibility: None,
            cells: vec![cell_diff(1, 1, DiffStatus::Modified)],
            merges: Vec::new(),
        };

        // The only change is at row 1; rows 4..=50 are all unchanged and
        // trail off to the end of the sheet with nothing else of interest
        // after them, exercising the "gap after the last kept window"
        // branch of `build_line_plan` (as opposed to a gap *between* two
        // changes).
        let html = render_sheet_split(&sheet_diff, &base_wb, &head_wb, Some(&base), Some(&head));
        assert!(html.contains("gap-row"));
        assert!(html.contains("row(s) omitted (4"));
    }

    #[test]
    fn deleted_sheet_position_falls_back_to_base_order() {
        let mut sheet1 = Sheet::new("A".to_string(), SheetVisibility::Visible);
        sheet1.insert_cell(CellRef { row: 1, col: 1 }, cell(CellValue::Number(1.0)));
        let mut sheet2 = Sheet::new("B".to_string(), SheetVisibility::Visible);
        sheet2.insert_cell(CellRef { row: 1, col: 1 }, cell(CellValue::Number(1.0)));

        // "B" only exists in base (it was deleted) — head has just "A".
        let base_wb = Workbook::new(vec![sheet1.clone(), sheet2.clone()], None);
        let head_wb = Workbook::new(vec![sheet1.clone()], None);
        let sheet_diff = SheetDiff {
            name: "B".to_string(),
            status: DiffStatus::Deleted,
            old_visibility: None,
            new_visibility: None,
            cells: Vec::new(),
            merges: Vec::new(),
        };

        let html = render_sheet_split(&sheet_diff, &base_wb, &head_wb, Some(&sheet2), None);
        assert!(html.contains("sheet2：B"));
    }

    #[test]
    fn sheet_not_found_on_either_side_falls_back_to_the_bare_name() {
        // A defensive path: `sheet_diff.name` doesn't match any sheet in
        // either workbook's own `sheets()` list, so `sheet_heading` has no
        // position to report and falls back to the plain (HTML-escaped)
        // name instead of a `sheetN：` prefix.
        let base_wb = Workbook::new(Vec::new(), None);
        let head_wb = Workbook::new(Vec::new(), None);
        let sheet_diff = SheetDiff {
            name: "Ghost".to_string(),
            status: DiffStatus::Modified,
            old_visibility: None,
            new_visibility: None,
            cells: Vec::new(),
            merges: Vec::new(),
        };

        let html = render_sheet_split(&sheet_diff, &base_wb, &head_wb, None, None);
        assert!(html.contains("<h2>Ghost "));
        assert!(!html.contains("sheet1："));
    }

    #[test]
    fn far_apart_column_changes_collapse_the_column_gap() {
        let mut base = Sheet::new("Sheet1".to_string(), SheetVisibility::Visible);
        base.insert_cell(CellRef { row: 1, col: 1 }, cell(CellValue::Number(1.0)));
        let mut head = base.clone();
        head.insert_cell(CellRef { row: 1, col: 50 }, cell(CellValue::Number(2.0)));

        let base_wb = workbook_with(base.clone());
        let head_wb = workbook_with(head.clone());
        let sheet_diff = SheetDiff {
            name: "Sheet1".to_string(),
            status: DiffStatus::Modified,
            old_visibility: None,
            new_visibility: None,
            cells: vec![cell_diff(1, 50, DiffStatus::Added)],
            merges: Vec::new(),
        };

        let html = render_sheet_split(&sheet_diff, &base_wb, &head_wb, Some(&base), Some(&head));
        assert!(html.contains("class=\"cell gap-col\""));
        assert!(html.contains("class=\"col-head gap-head\""));
        assert!(html.contains("column(s) omitted"));
    }

    #[test]
    fn wrap_text_cell_wraps_instead_of_clipping_or_overflowing() {
        let wrapped = ResolvedStyle {
            wrap_text: true,
            ..Default::default()
        };
        let mut base = Sheet::new("Sheet1".to_string(), SheetVisibility::Visible);
        base.insert_cell(
            CellRef { row: 1, col: 1 },
            styled_cell(Some(CellValue::Text("wraps".into())), wrapped),
        );
        let head = base.clone();

        let base_wb = workbook_with(base.clone());
        let head_wb = workbook_with(head.clone());
        let sheet_diff = SheetDiff {
            name: "Sheet1".to_string(),
            status: DiffStatus::Modified,
            old_visibility: None,
            new_visibility: None,
            cells: Vec::new(),
            merges: Vec::new(),
        };

        let html = render_sheet_split(&sheet_diff, &base_wb, &head_wb, Some(&base), Some(&head));
        assert!(html.contains("white-space:normal;overflow-wrap:anywhere;"));
    }

    #[test]
    fn all_four_border_sides_render_as_black_inline_css() {
        let bordered = ResolvedStyle {
            borders: Borders {
                top: true,
                right: true,
                bottom: true,
                left: true,
            },
            ..Default::default()
        };
        let mut base = Sheet::new("Sheet1".to_string(), SheetVisibility::Visible);
        base.insert_cell(
            CellRef { row: 1, col: 1 },
            styled_cell(Some(CellValue::Number(1.0)), bordered),
        );
        let head = base.clone();

        let base_wb = workbook_with(base.clone());
        let head_wb = workbook_with(head.clone());
        let sheet_diff = SheetDiff {
            name: "Sheet1".to_string(),
            status: DiffStatus::Modified,
            old_visibility: None,
            new_visibility: None,
            cells: Vec::new(),
            merges: Vec::new(),
        };

        let html = render_sheet_split(&sheet_diff, &base_wb, &head_wb, Some(&base), Some(&head));
        assert!(html.contains("border-top:1px solid #000;"));
        assert!(html.contains("border-right:1px solid #000;"));
        assert!(html.contains("border-bottom:1px solid #000;"));
        assert!(html.contains("border-left:1px solid #000;"));
    }

    #[test]
    fn boolean_datetime_and_error_values_render_correctly() {
        assert_eq!(cell_value_html(Some(&CellValue::Boolean(true))), "true");
        assert_eq!(cell_value_html(Some(&CellValue::Boolean(false))), "false");
        assert_eq!(
            cell_value_html(Some(&CellValue::Error("#DIV/0!".to_string()))),
            "<span class=\"err\">#DIV/0!</span>"
        );
        let dt = DateTimeValue {
            year: 2024,
            month: 1,
            day: 5,
            hour: 3,
            minute: 5,
            second: 9,
        };
        assert_eq!(
            cell_value_html(Some(&CellValue::DateTime(dt))),
            html_escape(&format!("{dt:?}"))
        );
    }

    #[test]
    fn next_cell_is_empty_is_false_when_the_neighbor_has_a_real_value() {
        let mut sheet = Sheet::new("Sheet1".to_string(), SheetVisibility::Visible);
        sheet.insert_cell(
            CellRef { row: 1, col: 1 },
            cell(CellValue::Text("a".into())),
        );
        sheet.insert_cell(
            CellRef { row: 1, col: 2 },
            cell(CellValue::Text("b".into())),
        );

        assert!(!next_cell_is_empty(
            Some(&sheet),
            CellRef { row: 1, col: 1 }
        ));
    }
}
