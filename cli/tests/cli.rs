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
//! relies on (`.github/workflows/xlsx-diff.yml` initializes `base_file`/
//! `head_file` to `""` and never runs `git show` for whichever side
//! doesn't apply to the file's git status, so `""` reaches this binary
//! as a plain placeholder argument, not a `git show` fallback).

use std::io::Write as _;
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

/// Builds a minimal, single-cell `.xlsx` with A1 set to `cell_value`, for
/// the "real diff" test below. Two arbitrary files under
/// `tests/fixtures/other/` (`basic_types.xlsx` vs `date.xlsx`) were tried
/// first, but which cells actually differ between two unrelated
/// real-world fixtures isn't something this test controls or verifies —
/// it happened to produce a cell-value hunk locally, then produced none
/// in CI (a different, freshly-resolved dependency graph, since this
/// library-crate repo doesn't commit `Cargo.lock`), failing
/// `stdout.contains("@@")` with no hunk at all. Building the pair here
/// instead guarantees exactly one changed cell regardless of environment
/// — the same fix `tests/fixtures/diff.rs`'s own `cell_modified()` (and
/// `src/markdown.rs`'s `minimal_xlsx_zip` unit-test helper) already apply
/// for the same reason.
fn minimal_xlsx_zip(cell_value: &str) -> Vec<u8> {
    const ROOT_RELS_XML: &[u8] = br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#;
    const RELS_XML: &[u8] = br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
  <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
</Relationships>"#;
    const WORKBOOK_XML: &[u8] = br#"<?xml version="1.0"?>
<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets>
    <sheet name="Sheet1" sheetId="1" r:id="rId1"/>
  </sheets>
</workbook>"#;
    const STYLES_XML: &[u8] = br#"<styleSheet><cellXfs><xf numFmtId="0"/></cellXfs></styleSheet>"#;

    let worksheet_xml = format!(
        r#"<worksheet><sheetData><row r="1"><c r="A1"><v>{cell_value}</v></c></row></sheetData></worksheet>"#
    );

    let mut buf = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in [
            ("_rels/.rels", ROOT_RELS_XML),
            ("xl/_rels/workbook.xml.rels", RELS_XML),
            ("xl/workbook.xml", WORKBOOK_XML),
            ("xl/styles.xml", STYLES_XML),
            ("xl/worksheets/sheet1.xml", worksheet_xml.as_bytes()),
        ] {
            writer.start_file(name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
    }
    buf
}

/// Writes `bytes` to a uniquely-named file under `std::env::temp_dir()`
/// and deletes it on drop, so a failing assertion can't leak it behind.
struct TempFile(PathBuf);

impl TempFile {
    fn new(test_name: &str, bytes: &[u8]) -> Self {
        let path = std::env::temp_dir().join(format!(
            "xlsxdiff-cli-test-{}-{test_name}.xlsx",
            std::process::id()
        ));
        std::fs::write(&path, bytes).unwrap();
        Self(path)
    }

    fn path_str(&self) -> &str {
        self.0.to_str().unwrap()
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
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
    let base = TempFile::new("modified_base", &minimal_xlsx_zip("42"));
    let head = TempFile::new("modified_head", &minimal_xlsx_zip("100"));
    let out = run(&["path/m.xlsx", "M", base.path_str(), head.path_str()]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with("### ✏️ Modified · `path/m.xlsx`\n"),
        "unexpected stdout: {stdout}"
    );
    assert!(stdout.contains("@@ A1 @@"), "unexpected stdout: {stdout}");
    assert!(stdout.contains("- 42"), "unexpected stdout: {stdout}");
    assert!(stdout.contains("+ 100"), "unexpected stdout: {stdout}");
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
