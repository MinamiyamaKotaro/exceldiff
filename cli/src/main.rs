// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! CI helper for `.github/workflows/xlsx-diff.yml`: prints a
//! Markdown-formatted summary of one changed `.xlsx` fixture's diff, for
//! the workflow to concatenate into a single PR comment body.
//!
//! Moved here from `examples/xlsx_diff_cli.rs` (Issue #32) into its own
//! workspace member so the CLI's binary surface is fully decoupled from
//! the `exceldiff` library crate — `cargo build`/`cargo package` run
//! against `exceldiff` alone never sees this binary at all, regardless of
//! Cargo's `examples/` vs `src/bin/` autodiscovery rules.
//!
//! ```text
//! xlsxdiff [--max-rows-per-sheet <N>] [--diff-mode <auto|coordinate>] <display_path> <A|M|D> [base_file] [head_file]
//! ```
//!
//! `display_path` is the path shown in the Markdown heading (the file's
//! path in the repo); `base_file`/`head_file` are the actual filesystem
//! paths the workflow extracted the base/head git revisions to (via `git
//! show`) — empty/omitted when not applicable (`A` has no `base_file`, `D`
//! has no `head_file`).
//!
//! The two `--` options thread through to `MarkdownOptions`
//! (`max_rows_per_sheet`/`diff_mode`) so `action.yml` can expose them as
//! its own inputs (Issue #24) without this crate depending on an argument
//! parsing crate — both are simple `--flag value` pairs, and this binary
//! never has more than these two, so manual parsing (`parse_options`
//! below) stays simpler than adding a dependency like `clap` would be.
//!
//! All parsing, diffing, and Markdown formatting lives in
//! `exceldiff::diff_file_section_from_paths` — this file only turns argv
//! into that function's arguments and writes the result to stdout.

use exceldiff::{diff_file_section_from_paths, DiffMode, MarkdownOptions};
use std::env;
use std::io::{self, Write};
use std::process::ExitCode;

const USAGE: &str = "usage: xlsxdiff [--max-rows-per-sheet <N>] [--diff-mode <auto|coordinate>] <display_path> <A|M|D> [base_file] [head_file]";

/// Consumes leading `--flag value` pairs off `args` into a
/// [`MarkdownOptions`], stopping at the first argument that isn't a
/// recognized flag (the start of the positional arguments). Returns the
/// options plus whatever's left unconsumed, or `None` on an unknown flag
/// value (an unparseable `--max-rows-per-sheet`, an unrecognized
/// `--diff-mode`) or a flag with no value following it.
fn parse_options(mut args: &[String]) -> Option<(MarkdownOptions, &[String])> {
    let mut options = MarkdownOptions::default();
    loop {
        match args {
            [flag, value, rest @ ..] if flag == "--max-rows-per-sheet" => {
                options.max_rows_per_sheet = value.parse().ok()?;
                args = rest;
            }
            [flag, value, rest @ ..] if flag == "--diff-mode" => {
                options.diff_mode = match value.as_str() {
                    "auto" => DiffMode::Auto,
                    "coordinate" => DiffMode::Coordinate,
                    _ => return None,
                };
                args = rest;
            }
            [flag, ..] if flag == "--max-rows-per-sheet" || flag == "--diff-mode" => {
                return None; // flag present with no value following it
            }
            _ => return Some((options, args)),
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some((options, positional)) = parse_options(&args) else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };
    let [display_path, status, rest @ ..] = positional else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };

    let base_path = rest.first().map(String::as_str).filter(|s| !s.is_empty());
    let head_path = rest.get(1).map(String::as_str).filter(|s| !s.is_empty());

    let md = diff_file_section_from_paths(display_path, status, base_path, head_path, &options);

    let stdout = io::stdout();
    let mut handle = stdout.lock();
    let _ = writeln!(handle, "{md}");

    ExitCode::SUCCESS
}
