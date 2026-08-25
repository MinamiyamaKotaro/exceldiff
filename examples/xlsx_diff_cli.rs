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
//!
//! All Markdown formatting now lives in `exceldiff::markdown`
//! (`format_file_section`, Issue #31) — this file only parses args, calls
//! `parse_workbook`/`diff_workbooks`, and maps the outcome onto
//! `exceldiff::FileStatus` for the library to render. Fully rewiring this
//! CLI into a thin wrapper (removing the parse/diff orchestration here
//! too) is Issue #32's scope, not this one's.

use exceldiff::{parse_workbook, AddedSummary, FileStatus, MarkdownOptions, RevisionSide};
use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let Some((display_path, status, base_path, head_path)) = parse_args(&args) else {
        eprintln!("usage: xlsx_diff_cli <display_path> <A|M|D> [base_file] [head_file]");
        return ExitCode::FAILURE;
    };

    let options = MarkdownOptions::default();
    let md = match status {
        "A" => render_added(display_path, head_path, &options),
        "D" => exceldiff::format_file_section(display_path, &FileStatus::Deleted, &options),
        "M" => render_modified(display_path, base_path, head_path, &options),
        other => {
            exceldiff::format_file_section(display_path, &FileStatus::Unrecognized(other), &options)
        }
    };
    print!("{md}");
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

fn render_added(display_path: &str, head_path: Option<&str>, options: &MarkdownOptions) -> String {
    let Some(head_path) = head_path else {
        return exceldiff::format_file_section(display_path, &FileStatus::Added(None), options);
    };
    let wb = match parse_workbook(head_path) {
        Ok(wb) => wb,
        Err(e) => {
            let message = e.to_string();
            let status = FileStatus::AddedParseError(&message);
            return exceldiff::format_file_section(display_path, &status, options);
        }
    };
    let summary = AddedSummary {
        sheet_count: wb.sheets().len(),
        cell_count: wb.sheets().iter().map(|s| s.iter_cells().count()).sum(),
    };
    exceldiff::format_file_section(display_path, &FileStatus::Added(Some(summary)), options)
}

fn render_modified(
    display_path: &str,
    base_path: Option<&str>,
    head_path: Option<&str>,
    options: &MarkdownOptions,
) -> String {
    let (Some(base_path), Some(head_path)) = (base_path, head_path) else {
        return exceldiff::format_file_section(
            display_path,
            &FileStatus::ModifiedMissingContent,
            options,
        );
    };

    let base = match parse_workbook(base_path) {
        Ok(wb) => wb,
        Err(e) => {
            let message = e.to_string();
            let status = FileStatus::ModifiedParseError(RevisionSide::Base, &message);
            return exceldiff::format_file_section(display_path, &status, options);
        }
    };
    let head = match parse_workbook(head_path) {
        Ok(wb) => wb,
        Err(e) => {
            let message = e.to_string();
            let status = FileStatus::ModifiedParseError(RevisionSide::Head, &message);
            return exceldiff::format_file_section(display_path, &status, options);
        }
    };

    let diff = exceldiff::diff_workbooks(&base, &head);
    exceldiff::format_file_section(display_path, &FileStatus::Modified(&diff), options)
}
