// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! CI helper for `.github/workflows/xlsx-diff.yml`: prints a
//! Markdown-formatted summary of one changed `.xlsx` fixture's diff, for
//! the workflow to concatenate into a single PR comment body. Not part of
//! the library's public API — kept under `examples/`, not `[[bin]]`, so it
//! never ships as part of the published crate's binary surface.
//!
//! ```text
//! xlsx_diff_cli <display_path> <A|M|D> [base_file] [head_file]
//! ```
//!
//! `display_path` is the path shown in the Markdown heading (the file's
//! path in the repo); `base_file`/`head_file` are the actual filesystem
//! paths the workflow extracted the base/head git revisions to (via `git
//! show`) — empty/omitted when not applicable (`A` has no `base_file`, `D`
//! has no `head_file`).

use exceldiff::{parse_workbook, CellRef, DiffStatus, JsonCellValue, WorkbookDiff};
use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let Some((display_path, status, base_path, head_path)) = parse_args(&args) else {
        eprintln!("usage: xlsx_diff_cli <display_path> <A|M|D> [base_file] [head_file]");
        return ExitCode::FAILURE;
    };

    println!("### `{display_path}`");
    println!();

    match status {
        "A" => print_added(head_path),
        "D" => print_deleted(),
        "M" => print_modified(base_path, head_path),
        other => println!("_Unrecognized git status `{other}`, skipped._"),
    }
    println!();

    ExitCode::SUCCESS
}

fn parse_args(args: &[String]) -> Option<(&str, &str, Option<&str>, Option<&str>)> {
    if args.len() < 3 {
        return None;
    }
    let non_empty = |s: &String| -> bool { !s.is_empty() };
    let display_path = args[1].as_str();
    let status = args[2].as_str();
    let base_path = args.get(3).filter(|s| non_empty(s)).map(String::as_str);
    let head_path = args.get(4).filter(|s| non_empty(s)).map(String::as_str);
    Some((display_path, status, base_path, head_path))
}

fn print_added(head_path: Option<&str>) {
    println!("**New file.**");
    let Some(head_path) = head_path else {
        return;
    };
    match parse_workbook(head_path) {
        Ok(wb) => println!(
            "{} sheet(s), {} total cell(s).",
            wb.sheets().len(),
            wb.sheets()
                .iter()
                .map(|s| s.iter_cells().count())
                .sum::<usize>()
        ),
        Err(e) => println!("⚠️ Could not parse: {e}"),
    }
}

fn print_deleted() {
    println!("**File removed.**");
}

fn print_modified(base_path: Option<&str>, head_path: Option<&str>) {
    let (Some(base_path), Some(head_path)) = (base_path, head_path) else {
        println!("_Missing before/after content, skipped._");
        return;
    };

    let base = match parse_workbook(base_path) {
        Ok(wb) => wb,
        Err(e) => {
            println!("⚠️ Could not parse the previous version: {e}");
            return;
        }
    };
    let head = match parse_workbook(head_path) {
        Ok(wb) => wb,
        Err(e) => {
            println!("⚠️ Could not parse the new version: {e}");
            return;
        }
    };

    print_diff(&exceldiff::diff_workbooks(&base, &head));
}

/// Caps the per-sheet table so one huge fixture rewrite can't blow up the
/// PR comment size.
const MAX_ROWS_PER_SHEET: usize = 30;

fn print_diff(diff: &WorkbookDiff) {
    if diff.sheets.is_empty() {
        println!("_No differences detected._");
        return;
    }

    for sheet in &diff.sheets {
        let added = count(sheet, DiffStatus::Added);
        let modified = count(sheet, DiffStatus::Modified);
        let deleted = count(sheet, DiffStatus::Deleted);

        let sheet_note = match sheet.status {
            DiffStatus::Added => " (sheet added)",
            DiffStatus::Deleted => " (sheet removed)",
            DiffStatus::Modified => "",
        };
        println!(
            "**Sheet `{}`{sheet_note}** — {added} added, {modified} modified, {deleted} deleted",
            sheet.name
        );
        if let (Some(old_v), Some(new_v)) = (sheet.old_visibility, sheet.new_visibility) {
            println!("_Visibility: `{old_v}` → `{new_v}`_");
        }
        println!();

        if sheet.cells.is_empty() {
            continue;
        }

        println!("| | Cell | Before | After |");
        println!("|---|---|---|---|");
        for cell in sheet.cells.iter().take(MAX_ROWS_PER_SHEET) {
            let marker = match cell.status {
                DiffStatus::Added => "➕",
                DiffStatus::Modified => "✏️",
                DiffStatus::Deleted => "➖",
            };
            let coord = CellRef {
                row: cell.row,
                col: cell.col,
            }
            .to_a1();
            println!(
                "| {marker} | {coord} | {} | {} |",
                format_value(cell.old_value.as_ref()),
                format_value(cell.new_value.as_ref())
            );
        }
        if sheet.cells.len() > MAX_ROWS_PER_SHEET {
            println!(
                "_...and {} more change(s) in this sheet._",
                sheet.cells.len() - MAX_ROWS_PER_SHEET
            );
        }
        println!();
    }
}

fn count(sheet: &exceldiff::SheetDiff, status: DiffStatus) -> usize {
    sheet.cells.iter().filter(|c| c.status == status).count()
}

/// Renders one cell value for a Markdown table cell — escaping `|` and
/// newlines, either of which would otherwise break the table's row
/// structure.
fn format_value(v: Option<&JsonCellValue>) -> String {
    match v {
        None => "—".to_string(),
        Some(JsonCellValue::Number(n)) => n.to_string(),
        Some(JsonCellValue::Boolean(b)) => b.to_string(),
        Some(JsonCellValue::DateTime(s)) => s.clone(),
        Some(JsonCellValue::Error(e)) => format!("`{e}`"),
        Some(JsonCellValue::Empty) => "_(empty)_".to_string(),
        Some(JsonCellValue::Text(s)) => format!("\"{}\"", escape_table_cell(s)),
    }
}

fn escape_table_cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', "\\n")
}
