//! Integration tests for `diff::engine`/`diff::storage` (Issue #3),
//! exercising them through the real parse pipeline end to end — see
//! `tests/fixtures/diff.rs` for what each fixture pair looks like.
//! Complements `src/diff/engine.rs`'s/`src/diff/storage.rs`'s own unit
//! tests, which build `Sheet`s/`Workbook`s directly via the public model
//! API and never touch ZIP/XML at all.

#[path = "fixtures/mod.rs"]
mod fixtures;

use exceldiff::{diff_workbooks, parse_workbook_reader, DiffStatus, JsonCellValue, WorkbookDiff};
use fixtures::diff;
use std::io::Cursor;

fn diff_pair(pair: (Vec<u8>, Vec<u8>)) -> WorkbookDiff {
    let (base_bytes, target_bytes) = pair;
    let base = parse_workbook_reader(Cursor::new(base_bytes)).unwrap();
    let target = parse_workbook_reader(Cursor::new(target_bytes)).unwrap();
    diff_workbooks(&base, &target)
}

#[test]
fn identical_workbooks_produce_no_diff_through_the_real_pipeline() {
    let result = diff_pair(diff::identical_workbooks());
    assert!(result.sheets.is_empty());
}

#[test]
fn cell_value_modification_is_detected_end_to_end() {
    let result = diff_pair(diff::cell_modified());

    assert_eq!(result.sheets.len(), 1);
    assert_eq!(result.sheets[0].name, "Inventory");
    let cells = &result.sheets[0].cells;
    assert_eq!(cells.len(), 1);

    let cell = &cells[0];
    assert_eq!(cell.row, 2);
    assert_eq!(cell.col, 2);
    assert_eq!(cell.status, DiffStatus::Modified);
    assert_eq!(cell.old_value, Some(JsonCellValue::Number(100.0)));
    assert_eq!(cell.new_value, Some(JsonCellValue::Number(120.0)));
}

#[test]
fn cell_addition_is_detected_end_to_end() {
    let result = diff_pair(diff::cell_added());

    let cells = &result.sheets[0].cells;
    assert_eq!(cells.len(), 1);
    let cell = &cells[0];
    assert_eq!(cell.row, 2);
    assert_eq!(cell.col, 3);
    assert_eq!(cell.status, DiffStatus::Added);
    assert_eq!(cell.old_value, None);
    assert_eq!(cell.new_value, Some(JsonCellValue::Text("Fruit".into())));
}

#[test]
fn cell_deletion_is_detected_end_to_end() {
    let result = diff_pair(diff::cell_deleted());

    let cells = &result.sheets[0].cells;
    assert_eq!(cells.len(), 1);
    let cell = &cells[0];
    assert_eq!(cell.row, 2);
    assert_eq!(cell.col, 3);
    assert_eq!(cell.status, DiffStatus::Deleted);
    assert_eq!(cell.old_value, Some(JsonCellValue::Text("Fruit".into())));
    assert_eq!(cell.new_value, None);
}

#[test]
fn sheet_addition_is_detected_end_to_end() {
    let result = diff_pair(diff::sheet_added());

    assert_eq!(result.sheets.len(), 1);
    let sheet_diff = &result.sheets[0];
    assert_eq!(sheet_diff.name, "New");
    assert_eq!(sheet_diff.status, DiffStatus::Added);
    assert_eq!(sheet_diff.old_visibility, None);
    assert_eq!(sheet_diff.new_visibility, Some("visible"));
    assert_eq!(sheet_diff.cells.len(), 1);
    assert_eq!(sheet_diff.cells[0].status, DiffStatus::Added);
}

#[test]
fn sheet_deletion_is_detected_end_to_end() {
    let result = diff_pair(diff::sheet_deleted());

    assert_eq!(result.sheets.len(), 1);
    let sheet_diff = &result.sheets[0];
    assert_eq!(sheet_diff.name, "New");
    assert_eq!(sheet_diff.status, DiffStatus::Deleted);
    assert_eq!(sheet_diff.old_visibility, Some("visible"));
    assert_eq!(sheet_diff.new_visibility, None);
    assert_eq!(sheet_diff.cells.len(), 1);
    assert_eq!(sheet_diff.cells[0].status, DiffStatus::Deleted);
}

#[test]
fn sheet_visibility_change_is_detected_even_with_no_cell_changes() {
    let result = diff_pair(diff::sheet_visibility_changed());

    assert_eq!(result.sheets.len(), 1);
    let sheet_diff = &result.sheets[0];
    assert_eq!(sheet_diff.status, DiffStatus::Modified);
    assert_eq!(sheet_diff.old_visibility, Some("visible"));
    assert_eq!(sheet_diff.new_visibility, Some("hidden"));
    assert!(sheet_diff.cells.is_empty());
}

#[test]
fn style_only_change_is_reported_as_modified_end_to_end() {
    let result = diff_pair(diff::style_only_change());

    let cells = &result.sheets[0].cells;
    assert_eq!(cells.len(), 1);
    assert_eq!(cells[0].status, DiffStatus::Modified);
    // The value itself never changed (1 on both sides) — only the style
    // did, and that alone must still be enough to flag Modified.
    assert_eq!(cells[0].old_value, cells[0].new_value);
}

#[test]
fn diff_paths_parses_both_files_from_disk_and_diffs_them() {
    let (base_bytes, target_bytes) = diff::cell_modified();
    let dir = std::env::temp_dir();
    let base_path = dir.join(format!(
        "exceldiff-test-{}-diff_paths_base.xlsx",
        std::process::id()
    ));
    let target_path = dir.join(format!(
        "exceldiff-test-{}-diff_paths_target.xlsx",
        std::process::id()
    ));
    std::fs::write(&base_path, base_bytes).unwrap();
    std::fs::write(&target_path, target_bytes).unwrap();

    let result = exceldiff::diff_paths(&base_path, &target_path);

    std::fs::remove_file(&base_path).ok();
    std::fs::remove_file(&target_path).ok();

    let diff = result.unwrap();
    assert_eq!(diff.sheets.len(), 1);
    assert_eq!(diff.sheets[0].cells.len(), 1);
    assert_eq!(diff.sheets[0].cells[0].status, DiffStatus::Modified);
}

#[cfg(feature = "diff-storage")]
#[test]
fn diff_store_round_trips_a_real_parsed_workbook_as_head_json() {
    use exceldiff::DiffStore;

    let (base_bytes, target_bytes) = diff::cell_modified();
    let base = parse_workbook_reader(Cursor::new(base_bytes)).unwrap();
    let target = parse_workbook_reader(Cursor::new(target_bytes)).unwrap();
    let diff = diff_workbooks(&base, &target);

    let mut store = DiffStore::open(":memory:").unwrap();
    let base_id = store.save_revision("base", false, &base).unwrap();
    let target_id = store.save_revision("target", true, &target).unwrap();
    store.save_diff(base_id, target_id, &diff).unwrap();

    // HEAD must come back as exactly `to_json_string`'s own output for the
    // target workbook — Issue #3's "HEAD 指定時は完全な JSON 出力をその
    // まま行う" requirement, verified against a real parsed workbook
    // rather than a hand-built one.
    let expected = exceldiff::to_json_string(&target).unwrap();
    assert_eq!(store.head_json().unwrap(), Some(expected));
}
