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

/// Builds a worksheet with one row per entry of `values`, each holding a
/// single numeric cell in column A — `None` renders as a row with no `<c>`
/// element at all (a blank row). Used by the `--diff-mode` test below to
/// build a "blank row inserted" pair, matching
/// `src/markdown.rs`'s own `diff_mode_*` unit tests (same scenario, this
/// time verified through the real `xlsxdiff` process to confirm the flag
/// is actually wired through, not just that the underlying library
/// behavior exists).
fn xlsx_column_a(values: &[Option<i64>]) -> Vec<u8> {
    let mut rows = String::new();
    for (i, value) in values.iter().enumerate() {
        let row = i + 1;
        match value {
            Some(v) => rows.push_str(&format!(
                r#"<row r="{row}"><c r="A{row}"><v>{v}</v></c></row>"#
            )),
            None => rows.push_str(&format!(r#"<row r="{row}"/>"#)),
        }
    }
    let worksheet_xml = format!(r#"<worksheet><sheetData>{rows}</sheetData></worksheet>"#);

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

/// Builds a minimal two-sheet `.xlsx`, each sheet holding a single A1
/// cell from `sheets` (`(name, cell_value)` pairs) — for the
/// `--grid-html-dir` "combines every sheet into one page" test below.
/// Same in-memory-pair approach as `minimal_xlsx_zip` above, for the same
/// reason: an unrelated pair of real fixtures can't guarantee which
/// sheets actually differ, or in what order, across environments.
fn xlsx_zip_multi_sheet(sheets: &[(&str, &str)]) -> Vec<u8> {
    const ROOT_RELS_XML: &[u8] = br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#;

    let mut workbook_sheets = String::new();
    let mut workbook_rels = String::new();
    for (i, (name, _)) in sheets.iter().enumerate() {
        let sheet_id = i + 1;
        let r_id = format!("rId{sheet_id}");
        workbook_sheets.push_str(&format!(
            r#"<sheet name="{name}" sheetId="{sheet_id}" r:id="{r_id}"/>"#
        ));
        workbook_rels.push_str(&format!(
            r#"<Relationship Id="{r_id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{sheet_id}.xml"/>"#
        ));
    }
    let styles_r_id = format!("rId{}", sheets.len() + 1);
    workbook_rels.push_str(&format!(
        r#"<Relationship Id="{styles_r_id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>"#
    ));

    let rels_xml = format!(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{workbook_rels}</Relationships>"#
    );
    let workbook_xml = format!(
        r#"<?xml version="1.0"?>
<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets>{workbook_sheets}</sheets>
</workbook>"#
    );
    const STYLES_XML: &[u8] = br#"<styleSheet><cellXfs><xf numFmtId="0"/></cellXfs></styleSheet>"#;

    let mut buf = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("_rels/.rels", options).unwrap();
        writer.write_all(ROOT_RELS_XML).unwrap();
        writer
            .start_file("xl/_rels/workbook.xml.rels", options)
            .unwrap();
        writer.write_all(rels_xml.as_bytes()).unwrap();
        writer.start_file("xl/workbook.xml", options).unwrap();
        writer.write_all(workbook_xml.as_bytes()).unwrap();
        writer.start_file("xl/styles.xml", options).unwrap();
        writer.write_all(STYLES_XML).unwrap();
        for (i, (_, cell_value)) in sheets.iter().enumerate() {
            let sheet_id = i + 1;
            let worksheet_xml = format!(
                r#"<worksheet><sheetData><row r="1"><c r="A1"><v>{cell_value}</v></c></row></sheetData></worksheet>"#
            );
            writer
                .start_file(format!("xl/worksheets/sheet{sheet_id}.xml"), options)
                .unwrap();
            writer.write_all(worksheet_xml.as_bytes()).unwrap();
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

/// A uniquely-named, not-yet-created directory path for `--grid-html-dir`
/// tests (the CLI itself creates it via `create_dir_all`) — removed
/// recursively on drop so a failing assertion can't leak it behind.
struct TempDir(PathBuf);

impl TempDir {
    fn new(test_name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "xlsxdiff-cli-test-grid-{}-{test_name}",
            std::process::id()
        ));
        Self(path)
    }

    fn path_str(&self) -> &str {
        self.0.to_str().unwrap()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
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

// --- `--max-rows-per-sheet` / `--diff-mode` (Issue #24) ---

#[test]
fn max_rows_per_sheet_flag_caps_cell_hunks() {
    let base = TempFile::new("max_rows_base", &minimal_xlsx_zip("0"));
    // 3 distinct value changes on the same fixture shape wouldn't be
    // possible with the single-cell `minimal_xlsx_zip` helper, so this
    // reuses the same one-cell pair `modified_file_with_a_real_diff_prints_a_hunk`
    // already relies on and just caps a 1-hunk diff down to 0 — enough to
    // prove the flag reaches `MarkdownOptions`, without needing a
    // multi-cell fixture builder just for this.
    let head = TempFile::new("max_rows_head", &minimal_xlsx_zip("1"));
    let out = run(&[
        "--max-rows-per-sheet",
        "0",
        "path/m.xlsx",
        "M",
        base.path_str(),
        head.path_str(),
    ]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("@@ A1 @@"), "unexpected stdout: {stdout}");
    assert!(
        stdout.contains("_...and 1 more change(s) in this sheet._"),
        "unexpected stdout: {stdout}"
    );
}

#[test]
fn max_rows_per_sheet_flag_rejects_a_non_numeric_value() {
    let out = run(&["--max-rows-per-sheet", "not-a-number", "path/a.xlsx", "A"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).starts_with("usage: xlsxdiff "));
}

#[test]
fn diff_mode_flag_rejects_an_unknown_value() {
    let out = run(&["--diff-mode", "nonsense", "path/a.xlsx", "A"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).starts_with("usage: xlsxdiff "));
}

#[test]
fn a_recognized_flag_with_no_value_following_it_is_a_usage_error() {
    // "--diff-mode" is the only argument — nothing left for
    // parse_options to consume as its value.
    let out = run(&["--diff-mode"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).starts_with("usage: xlsxdiff "));
}

#[test]
fn diff_mode_coordinate_sees_a_row_shift_that_auto_mode_explains_away() {
    // Same "blank row inserted" scenario as
    // `src/markdown.rs`'s `diff_mode_*` unit tests — verified again here
    // through the actual `xlsxdiff` process, to confirm `--diff-mode`
    // itself is correctly threaded from argv through to `MarkdownOptions`
    // (not just that the underlying library behavior exists).
    let base = TempFile::new("diff_mode_cli_base", &xlsx_column_a(&[Some(1), Some(2)]));
    let head = TempFile::new(
        "diff_mode_cli_head",
        &xlsx_column_a(&[None, Some(1), Some(2)]),
    );

    let auto = run(&["path/m.xlsx", "M", base.path_str(), head.path_str()]);
    assert!(auto.status.success());
    assert!(
        String::from_utf8_lossy(&auto.stdout).contains("_No differences detected._"),
        "unexpected stdout: {}",
        String::from_utf8_lossy(&auto.stdout)
    );

    let coordinate = run(&[
        "--diff-mode",
        "coordinate",
        "path/m.xlsx",
        "M",
        base.path_str(),
        head.path_str(),
    ]);
    assert!(coordinate.status.success());
    let stdout = String::from_utf8_lossy(&coordinate.stdout);
    assert!(stdout.contains("@@ A1 @@"), "unexpected stdout: {stdout}");
    assert!(stdout.contains("@@ A2 @@"), "unexpected stdout: {stdout}");
    assert!(stdout.contains("@@ A3 @@"), "unexpected stdout: {stdout}");
}

// --- `--grid-html-dir` (Issue #24 "visual" follow-up) ---

#[test]
fn grid_html_dir_writes_a_page_and_manifest_for_an_added_file() {
    let head = TempFile::new("grid_added", &minimal_xlsx_zip("42"));
    let dir = TempDir::new("added");
    let out = run(&[
        "--grid-html-dir",
        dir.path_str(),
        "path/a.xlsx",
        "A",
        "",
        head.path_str(),
    ]);
    assert!(out.status.success());
    // Markdown output on stdout is unaffected by --grid-html-dir.
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("### 🆕 Added · `path/a.xlsx`\n"));

    let manifest = std::fs::read_to_string(format!("{}/manifest.tsv", dir.path_str())).unwrap();
    assert_eq!(
        manifest,
        format!("Sheet1\t{}/grid.html\n", dir.path_str())
    );
    let html = std::fs::read_to_string(format!("{}/grid.html", dir.path_str())).unwrap();
    assert!(html.contains("<!doctype html>"));
    assert!(html.contains("class=\"sheet\""));
}

#[test]
fn grid_html_dir_combines_every_changed_sheet_into_one_page() {
    // Two sheets, both changed, in a single `M` diff — this is the case
    // the combined-page design targets: previously each sheet got its
    // own separate HTML file, now both should land in one `grid.html`.
    let base = TempFile::new(
        "grid_multi_base",
        &xlsx_zip_multi_sheet(&[("Sheet1", "1"), ("Sheet2", "10")]),
    );
    let head = TempFile::new(
        "grid_multi_head",
        &xlsx_zip_multi_sheet(&[("Sheet1", "2"), ("Sheet2", "20")]),
    );
    let dir = TempDir::new("multi");
    let out = run(&[
        "--grid-html-dir",
        dir.path_str(),
        "path/multi.xlsx",
        "M",
        base.path_str(),
        head.path_str(),
    ]);
    assert!(out.status.success());

    let manifest = std::fs::read_to_string(format!("{}/manifest.tsv", dir.path_str())).unwrap();
    assert_eq!(
        manifest,
        format!(
            "Sheet1\t{0}/grid.html\nSheet2\t{0}/grid.html\n",
            dir.path_str()
        )
    );
    // Exactly one HTML file was written for both sheets combined.
    let html_files: Vec<_> = std::fs::read_dir(dir.path_str())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "html"))
        .collect();
    assert_eq!(html_files.len(), 1);

    let html = std::fs::read_to_string(format!("{}/grid.html", dir.path_str())).unwrap();
    assert!(html.contains("<!doctype html>"));
    // Both sheets' own <section class="sheet"> fragments are present on
    // the same page.
    assert_eq!(html.matches("class=\"sheet\"").count(), 2);
    assert!(html.contains(">Sheet1<") || html.contains("Sheet1</h2>") || html.contains("Sheet1 "));
    assert!(html.contains(">Sheet2<") || html.contains("Sheet2</h2>") || html.contains("Sheet2 "));
}

#[test]
fn grid_html_dir_writes_nothing_when_there_are_no_visual_changes() {
    let base = TempFile::new("grid_unchanged_base", &minimal_xlsx_zip("42"));
    let head = TempFile::new("grid_unchanged_head", &minimal_xlsx_zip("42"));
    let dir = TempDir::new("unchanged");
    let out = run(&[
        "--grid-html-dir",
        dir.path_str(),
        "path/m.xlsx",
        "M",
        base.path_str(),
        head.path_str(),
    ]);
    assert!(out.status.success());
    let manifest = std::fs::read_to_string(format!("{}/manifest.tsv", dir.path_str())).unwrap();
    assert_eq!(manifest, "");
}

#[test]
fn grid_html_dir_failure_only_warns_and_still_exits_successfully() {
    // A path nested *inside* a plain file can never be created as a
    // directory (`create_dir_all` fails partway through) — this is
    // enough to exercise write_grid_sections's error path without
    // needing to fabricate a permissions failure.
    let not_a_dir = TempFile::new("grid_html_dir_blocker", b"not a directory");
    let unreachable_dir = format!("{}/subdir", not_a_dir.path_str());

    let head = TempFile::new("grid_html_dir_failure_head", &minimal_xlsx_zip("42"));
    let out = run(&[
        "--grid-html-dir",
        &unreachable_dir,
        "path/a.xlsx",
        "A",
        "",
        head.path_str(),
    ]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("### 🆕 Added · `path/a.xlsx`\n"));
    assert!(String::from_utf8_lossy(&out.stderr).contains("warning: could not write grid HTML"));
}
