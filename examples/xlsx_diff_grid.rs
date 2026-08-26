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
//! surface. Demonstrates `exceldiff::render_sheet_split` for a two-file
//! CLI invocation; the actual PR-comment delivery path (screenshot +
//! orphan-branch publishing) lives in `cli/`'s `--grid-html-dir` flag
//! instead, which uses `exceldiff::grid_sections_from_paths` — the
//! path-based, per-status (A/M/D) counterpart of this file's manual
//! two-workbook diff (see `docs/design/grid.md`).
//!
//! ```text
//! xlsx_diff_grid <base.xlsx> <head.xlsx> <output.html>
//! ```

use exceldiff::{diff_workbooks, parse_workbook, wrap_grid_page};
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

    let html = wrap_grid_page(&sections);
    if let Err(e) = fs::write(out_path, html) {
        eprintln!("could not write {out_path}: {e}");
        return ExitCode::FAILURE;
    }
    println!("wrote {out_path}");
    ExitCode::SUCCESS
}
