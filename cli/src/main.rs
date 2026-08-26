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
//! xlsxdiff [--max-rows-per-sheet <N>] [--diff-mode <auto|coordinate>] [--grid-html-dir <dir>] <display_path> <A|M|D> [base_file] [head_file]
//! ```
//!
//! `display_path` is the path shown in the Markdown heading (the file's
//! path in the repo); `base_file`/`head_file` are the actual filesystem
//! paths the workflow extracted the base/head git revisions to (via `git
//! show`) — empty/omitted when not applicable (`A` has no `base_file`, `D`
//! has no `head_file`).
//!
//! The `--` options thread through to `MarkdownOptions`
//! (`max_rows_per_sheet`/`diff_mode`) and to `grid_sections_from_paths`
//! (`--grid-html-dir`) so `action.yml` can expose them as its own inputs
//! (Issue #24, "visual" grid rendering) without this crate depending on
//! an argument parsing crate — all are simple `--flag value` pairs, and
//! this binary never has more than a handful, so manual parsing
//! (`parse_options` below) stays simpler than adding a dependency like
//! `clap` would be.
//!
//! All parsing, diffing, and Markdown formatting lives in
//! `exceldiff::diff_file_section_from_paths` — this file only turns argv
//! into that function's arguments and writes the result to stdout.
//! `--grid-html-dir`, when given, additionally calls
//! `exceldiff::grid_sections_from_paths` and writes one standalone HTML
//! page per changed sheet into that directory (for `action.yml`'s
//! `visual` mode to screenshot), plus a `manifest.tsv` (`sheet_name\t
//! html_path` per line, the same TSV convention `git diff --name-status`
//! already uses in `action.yml`) so the caller knows what was written
//! without having to list the directory itself.

use exceldiff::{
    diff_file_section_from_paths, grid_sections_from_paths, wrap_grid_page, DiffMode,
    MarkdownOptions,
};
use std::env;
use std::io::{self, Write};
use std::process::ExitCode;

const USAGE: &str = "usage: xlsxdiff [--max-rows-per-sheet <N>] [--diff-mode <auto|coordinate>] [--grid-html-dir <dir>] <display_path> <A|M|D> [base_file] [head_file]";

struct Options {
    markdown: MarkdownOptions,
    grid_html_dir: Option<String>,
}

/// Consumes leading `--flag value` pairs off `args` into [`Options`],
/// stopping at the first argument that isn't a recognized flag (the
/// start of the positional arguments). Returns the options plus
/// whatever's left unconsumed, or `None` on an unknown flag value (an
/// unparseable `--max-rows-per-sheet`, an unrecognized `--diff-mode`) or
/// a flag with no value following it.
fn parse_options(mut args: &[String]) -> Option<(Options, &[String])> {
    let mut options = Options {
        markdown: MarkdownOptions::default(),
        grid_html_dir: None,
    };
    loop {
        match args {
            [flag, value, rest @ ..] if flag == "--max-rows-per-sheet" => {
                options.markdown.max_rows_per_sheet = value.parse().ok()?;
                args = rest;
            }
            [flag, value, rest @ ..] if flag == "--diff-mode" => {
                options.markdown.diff_mode = match value.as_str() {
                    "auto" => DiffMode::Auto,
                    "coordinate" => DiffMode::Coordinate,
                    _ => return None,
                };
                args = rest;
            }
            [flag, value, rest @ ..] if flag == "--grid-html-dir" => {
                options.grid_html_dir = Some(value.clone());
                args = rest;
            }
            [flag, ..]
                if flag == "--max-rows-per-sheet"
                    || flag == "--diff-mode"
                    || flag == "--grid-html-dir" =>
            {
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

    let md = diff_file_section_from_paths(
        display_path,
        status,
        base_path,
        head_path,
        &options.markdown,
    );

    let stdout = io::stdout();
    let mut handle = stdout.lock();
    let _ = writeln!(handle, "{md}");

    if let Some(dir) = &options.grid_html_dir {
        if write_grid_sections(
            dir,
            status,
            base_path,
            head_path,
            options.markdown.diff_mode,
        )
        .is_err()
        {
            eprintln!("warning: could not write grid HTML to {dir}");
        }
    }

    ExitCode::SUCCESS
}

/// Writes one HTML page per changed sheet (see [`grid_sections_from_paths`])
/// into `dir`, plus `dir/manifest.tsv` listing them. A grid-rendering
/// failure is reported to stderr by the caller but never turns into a
/// non-zero exit — the Markdown output above is already written and is
/// the primary artifact; `action.yml`'s `visual` mode is a best-effort
/// addition on top of it (same "one file's failure shouldn't stop the
/// rest of the comment" policy `diff_file_section_from_paths` already
/// follows for parse errors).
fn write_grid_sections(
    dir: &str,
    status: &str,
    base_path: Option<&str>,
    head_path: Option<&str>,
    diff_mode: DiffMode,
) -> io::Result<()> {
    let sections = grid_sections_from_paths(status, base_path, head_path, diff_mode);
    std::fs::create_dir_all(dir)?;

    let mut manifest = String::new();
    for (i, section) in sections.iter().enumerate() {
        let html_path = format!("{dir}/sheet-{i}.html");
        std::fs::write(&html_path, wrap_grid_page(&section.html))?;
        manifest.push_str(&format!("{}\t{html_path}\n", section.sheet_name));
    }
    std::fs::write(format!("{dir}/manifest.tsv"), manifest)
}
