// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! End-to-end tests for the `xlsxdiff` binary itself (argv parsing, exit
//! codes, stdout/stderr wiring) — the underlying parse/diff/Markdown
//! logic these delegate to is already covered by
//! `exceldiff::markdown`'s own unit tests
//! (`diff_file_section_from_paths_*` in `src/markdown.rs`), so these
//! don't re-check every `FileStatus` rendering in detail. What's specific
//! to this crate and untested elsewhere: the `argv` slice pattern (too
//! few args), and the empty-string-means-absent convention the workflow
//! relies on (`git show` writing to `""` when a revision has no file).

use std::path::PathBuf;
use std::process::{Command, Output};

fn fixture(relative: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tests/fixtures")
        .join(relative)
        .to_str()
        .unwrap()
        .to_string()
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xlsxdiff"))
        .args(args)
        .output()
        .expect("failed to run xlsxdiff")
}

#[test]
fn usage_error_when_too_few_args() {
    let out = run(&["path/a.xlsx"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).starts_with("usage: xlsxdiff "));
}

#[test]
fn added_file_prints_new_file_summary() {
    let head = fixture("normal/basic_types.xlsx");
    let out = run(&["path/a.xlsx", "A", "", &head]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("### 🆕 Added · `path/a.xlsx`\n"));
    assert!(stdout.contains("sheet(s)"));
}

#[test]
fn added_without_head_and_added_with_empty_string_head_are_identical() {
    // The workflow passes `""` (not an omitted arg) for a revision with no
    // file — the CLI's `.filter(|s| !s.is_empty())` must treat that the
    // same as the arg being absent entirely.
    let omitted = run(&["path/a.xlsx", "A"]);
    let empty_string = run(&["path/a.xlsx", "A", ""]);
    assert!(omitted.status.success());
    assert!(empty_string.status.success());
    assert_eq!(omitted.stdout, empty_string.stdout);
    assert!(String::from_utf8_lossy(&omitted.stdout).contains("**New file.**"));
}

#[test]
fn added_file_with_corrupted_head_reports_a_parse_error() {
    let head = fixture("error/corrupted_xml.xlsx");
    let out = run(&["path/a.xlsx", "A", "", &head]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("⚠️ Could not parse:"));
}

#[test]
fn deleted_file_prints_removed() {
    let out = run(&["path/d.xlsx", "D"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("### 🗑️ Deleted · `path/d.xlsx`\n"));
    assert!(stdout.contains("**File removed.**"));
}

#[test]
fn modified_file_with_a_real_diff_prints_a_hunk() {
    let base = fixture("normal/basic_types.xlsx");
    let head = fixture("other/date.xlsx");
    let out = run(&["path/m.xlsx", "M", &base, &head]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("### ✏️ Modified · `path/m.xlsx`\n"));
    assert!(stdout.contains("@@"));
}

#[test]
fn modified_missing_content_when_both_paths_are_absent() {
    let out = run(&["path/m.xlsx", "M"]);
    assert!(out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("_Missing before/after content, skipped._")
    );
}

#[test]
fn unrecognized_status_is_reported() {
    let out = run(&["path/x.xlsx", "R"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with("### ❓ Unrecognized · `path/x.xlsx`\n"));
    assert!(stdout.contains("_Unrecognized git status `R`, skipped._"));
}
