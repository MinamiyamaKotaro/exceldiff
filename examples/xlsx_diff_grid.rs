// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! Renders a diff as two Excel-like grids side by side (Before | After),
//! git split-diff style, and writes it out as a standalone HTML page. See
//! `src/grid.rs` for the rendering logic itself — this file only parses
//! args, calls `parse_workbook`/`diff_workbooks`, and supplies the page
//! chrome (`<style>` block, legend, `<title>`) `render_sheet_split`
//! deliberately leaves out, so a different caller can supply its own.
//!
//! Not part of the library's public API — kept under `examples/`, not
//! `[[bin]]`, so it never ships as part of the published crate's binary
//! surface. Demonstrates `exceldiff::render_sheet_split`; it is not
//! currently wired into `.github/workflows/xlsx-diff.yml` (see
//! `docs/design/grid.md`'s "Open questions" for why: GitHub sanitizes the
//! `style=` attributes this page's grid relies on out of a PR comment
//! body, so actually publishing this HTML from CI needs a separate
//! delivery path — a screenshot image, a page hosted on GitHub Pages, a
//! downloadable artifact — that hasn't been built yet).
//!
//! ```text
//! xlsx_diff_grid <base.xlsx> <head.xlsx> <output.html>
//! ```

use exceldiff::{diff_workbooks, parse_workbook};
use std::env;
use std::fs;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let [_, base_path, head_path, out_path] = args.as_slice() else {
        eprintln!("usage: xlsx_diff_grid <base.xlsx> <head.xlsx> <output.html>");
        return ExitCode::FAILURE;
    };

    let base = match parse_workbook(base_path) {
        Ok(wb) => wb,
        Err(e) => {
            eprintln!("could not parse {base_path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let head = match parse_workbook(head_path) {
        Ok(wb) => wb,
        Err(e) => {
            eprintln!("could not parse {head_path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let diff = diff_workbooks(&base, &head);

    let mut sections = String::new();
    for sheet_diff in &diff.sheets {
        let base_sheet = base.sheets().iter().find(|s| s.name == sheet_diff.name);
        let head_sheet = head.sheets().iter().find(|s| s.name == sheet_diff.name);
        sections.push_str(&exceldiff::render_sheet_split(
            sheet_diff, &base, &head, base_sheet, head_sheet,
        ));
    }
    if diff.sheets.is_empty() {
        sections.push_str("<p class=\"empty\">No differences detected.</p>\n");
    }

    let html = wrap_page(&sections);
    if let Err(e) = fs::write(out_path, html) {
        eprintln!("could not write {out_path}: {e}");
        return ExitCode::FAILURE;
    }
    println!("wrote {out_path}");
    ExitCode::SUCCESS
}

fn wrap_page(sections: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="ja">
<head>
<meta charset="utf-8">
<title>xlsx diff (split grid)</title>
<style>
  body {{
    font-family: Calibri, Aptos, "Segoe UI", "Yu Gothic UI", sans-serif;
    background: #f3f2f1;
    margin: 0;
    padding: 2rem;
    color: #1a1a1a;
  }}
  h2 {{ font-size: 1rem; font-weight: 600; margin: 0 0 0.8rem; }}
  h2 .counts {{ font-weight: 400; color: #555; font-size: 0.85rem; }}
  .sheet {{ margin-bottom: 2.5rem; }}
  .split {{ display: flex; gap: 1.25rem; align-items: flex-start; }}
  .pane {{ flex: 1; min-width: 0; }}
  .pane-label {{
    font-size: 0.72rem; font-weight: 700; letter-spacing: 0.06em;
    text-transform: uppercase; margin-bottom: 0.4rem; padding: 0.15rem 0.5rem;
    display: inline-block; border-radius: 3px;
  }}
  .pane-before .pane-label {{ color: #9C0006; background: #FFC7CE; }}
  .pane-after .pane-label {{ color: #006100; background: #C6EFCE; }}
  .grid-scroll {{ overflow-x: auto; }}
  table.grid {{
    border-collapse: collapse;
    background: #ffffff;
    box-shadow: 0 1px 3px rgba(0,0,0,0.15);
    /* `table-layout: fixed` makes the header row's explicit per-column
       `width:Npx` (from `column_pixel_width` — Excel's own real column
       width, converted from character units) the actual rendered column
       width, instead of the browser widening columns to fit content. */
    table-layout: fixed;
  }}
  table.grid th, table.grid td {{
    border: 1px solid #d4d4d4;
    padding: 0.15rem 0.3rem;
    font-size: 0.78rem;
    /* Matches Excel's own default (`wrapText` off): text clips at the
       cell's real boundary rather than wrapping or forcing the column
       wider — either of which would defeat `column_pixel_width`'s whole
       point of reproducing the real column width faithfully.
       `render_cell` overrides this per-cell to `white-space:normal` only
       when that cell's actual `wrapText` alignment flag is set. */
    white-space: nowrap;
    overflow: hidden;
    text-overflow: clip;
    box-sizing: border-box;
    /* `<td>`'s real default is `vertical-align: baseline`, not `top` —
       invisible for an ordinary 1-row cell, but a `rowspan` merge (e.g.
       a tall label like 担当者 spanning 2 rows) then aligns to the
       *row's* text baseline instead of the merged cell's own top edge,
       making its content appear to drift down across the row boundary
       below it instead of sitting flush in its own box. Excel itself
       anchors cell content top-left by default. */
    vertical-align: top;
  }}
  th.corner, th.col-head, th.row-head {{
    background: #f3f2f1;
    color: #444;
    font-weight: 600;
    text-align: center;
  }}
  /* 36px must match `ROW_HEAD_WIDTH_PX` in src/grid.rs. */
  th.row-head {{ text-align: right; padding-right: 0.5rem; width: 36px; }}
  td.cell {{ text-align: left; }}
  td.cell .num {{ display: block; text-align: right; font-variant-numeric: tabular-nums; }}
  td.cell.empty {{ background: #ffffff; }}
  td.cell.not-present {{
    background: repeating-linear-gradient(
      135deg, #ececec, #ececec 6px, #e2e2e2 6px, #e2e2e2 12px
    );
  }}
  /* The plain grid line (`table.grid th, table.grid td`'s `border: 1px
     solid #d4d4d4` above) sits at the exact same 1px edge these
     `box-shadow: inset` rings occupy, so the grey line was showing
     through/blending with the indicator color instead of the indicator
     reading as its own clean color. `border-color: transparent` on the
     changed cell removes the grey competitor for that cell's own edge —
     the box-shadow ring (unaffected by `border-collapse`, unlike the
     `border` property) still draws a complete ring on all four sides
     regardless. */
  td.cell.border-added, td.cell.border-deleted,
  td.cell.border-value, td.cell.border-style {{
    border-color: transparent;
  }}
  td.cell.border-added {{ box-shadow: inset 0 0 0 1px #1a7f37; }}
  td.cell.border-deleted {{ box-shadow: inset 0 0 0 1px #c0392b; }}
  td.cell.border-value {{ box-shadow: inset 0 0 0 1px #b7791f; }}
  td.cell.border-style {{ box-shadow: inset 0 0 0 1px #6639ba; }}
  /* A collapsed run of rows with nothing changed in them (build_line_plan)
     — deliberately looks like a gap, not a data row: no gridlines, a
     dashed top/bottom rule, muted centered label. */
  tr.gap-row th.gap-head {{ background: transparent; border: none; color: #aaa; }}
  tr.gap-row td.gap {{
    background: #f3f2f1;
    border-left: none; border-right: none;
    border-top: 1px dashed #c7c2b3; border-bottom: 1px dashed #c7c2b3;
    text-align: center; color: #8a8371;
    font-size: 0.72rem; letter-spacing: 0.02em;
    padding: 0.25rem 0;
  }}
  /* The column-axis counterpart of the row gap above: a single narrow
     "⋯" column standing in for a collapsed run of columns, both in the
     header row (`th.col-head.gap-head`) and every data row
     (`td.cell.gap-col`) — same dashed-rule, muted-label treatment,
     rotated onto the vertical axis (dashed left/right instead of
     top/bottom, since this indicator's own edges run vertically). */
  th.col-head.gap-head, td.cell.gap-col {{
    background: #f3f2f1;
    border-top: none; border-bottom: none;
    border-left: 1px dashed #c7c2b3; border-right: 1px dashed #c7c2b3;
    color: #8a8371;
    font-size: 0.72rem;
    text-align: center;
  }}
  .err {{ font-family: ui-monospace, monospace; }}
  .empty {{ color: #666; }}

  .legend {{
    display: flex; flex-wrap: wrap; gap: 1rem;
    margin: 0 0 1.5rem; font-size: 0.8rem; color: #444;
  }}
  .legend span {{ display: inline-flex; align-items: center; gap: 0.35rem; }}
  .legend i {{ width: 0.8rem; height: 0.8rem; display: inline-block; border-radius: 2px; }}
  .legend .added {{ box-shadow: inset 0 0 0 2px #1a7f37; }}
  .legend .deleted {{ box-shadow: inset 0 0 0 2px #c0392b; }}
  .legend .value {{ box-shadow: inset 0 0 0 2px #b7791f; }}
  .legend .style {{ box-shadow: inset 0 0 0 2px #6639ba; }}
  .legend .not-present {{
    background: repeating-linear-gradient(
      135deg, #ececec, #ececec 4px, #e2e2e2 4px, #e2e2e2 8px
    );
  }}
</style>
</head>
<body>
<div class="legend">
  <span><i class="added"></i>Added</span>
  <span><i class="deleted"></i>Deleted</span>
  <span><i class="value"></i>Value changed</span>
  <span><i class="style"></i>Style only</span>
  <span><i class="not-present"></i>None</span>
</div>
{sections}</body>
</html>
"#
    )
}
