// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! `diff_workbooks_best_effort` (Issue #25): a per-sheet strategy picker
//! that never produces more diff noise than [`crate::diff::diff_workbooks`]
//! (coordinate-based) would, and often produces much less, without asking
//! the caller to choose a mode up front.
//!
//! [`crate::diff::diff_workbooks_aligned_rows`]/
//! [`crate::diff::diff_workbooks_aligned_columns`] each fix cascading
//! false-positive `Modified` diffs (see `row_alignment`/`col_alignment`'s
//! own doc comments) — but only for whichever single axis a caller
//! chooses to align, applied uniformly to the whole workbook. A workbook
//! with, say, a row inserted in one sheet and a column inserted in
//! another has no single whole-workbook choice that fixes both; picking
//! one axis for the whole workbook necessarily leaves the other sheet's
//! cascade in place (verified in
//! <https://github.com/MinamiyamaKotaro/exceldiff/issues/25#issuecomment-5419091215>).
//!
//! This function instead evaluates, independently *per sheet*: the
//! coordinate-based diff, a row-aligned diff, and a column-aligned diff —
//! keeping whichever reports the fewest total changes for that one sheet.
//! Two optimizations, both verified safe in the Issue #25 PoC thread
//! before landing here:
//!
//! - **Short-circuit at `<=1`**: if the coordinate-based diff already
//!   reports at most one change for a sheet, neither aligned variant can
//!   possibly do better (a real row/column shift affecting any populated
//!   cell always produces at least one `Added` entry — the shifted
//!   content's new coordinate — plus, in the coordinate-based comparison,
//!   typically far more from the resulting cascade; see
//!   [`crate::diff::engine::diff_sheet`]'s cells vs. this reasoning), so
//!   both alignment attempts are skipped entirely for that sheet.
//! - **Early exit at exactly `0`**: if row alignment finds nothing left to
//!   report at all (`Ok(None)` — not just "fewer changes", but the
//!   absolute floor), column alignment is skipped too. This is reachable
//!   with realistic edits, not just contrived ones: inserting a blank
//!   (no-cell) row is a pure, monotonic, contiguous shift with no new
//!   content to report, which collapses to `Ok(None)` under row alignment
//!   (see this module's tests).
//!
//! A sheet whose alignment cost would exceed `row_limits`/`col_limits`
//! (`Err(Error::RowAlignmentCostTooHigh)`/`Err(Error::
//! ColumnAlignmentCostTooHigh)`) simply falls back to that sheet's
//! coordinate-based result for that one axis — never propagated as an
//! error, matching [`crate::diff_file_section_from_paths`]'s existing
//! "never fail the whole comment over one file" policy (Issue #32).

use crate::diff::col_alignment::align_sheet_columns;
use crate::diff::engine::diff_sheet;
use crate::diff::model::{SheetDiff, WorkbookDiff};
use crate::diff::row_alignment::align_sheet_rows;
use crate::diff::{ColumnAlignmentLimits, RowAlignmentLimits};
use crate::error::Error;
use crate::model::Workbook;
use std::collections::BTreeSet;

/// Diffs two [`Workbook`]s, choosing per sheet whichever of
/// coordinate-based, row-aligned, or column-aligned diffing reports the
/// fewest total changes (cells + merges + a visibility change, see
/// [`sheet_total_changes`]). See this module's doc comment for the full
/// strategy and why it's safe.
pub fn diff_workbooks_best_effort(
    base: &Workbook,
    target: &Workbook,
    row_limits: RowAlignmentLimits,
    col_limits: ColumnAlignmentLimits,
) -> WorkbookDiff {
    let mut sheet_names: BTreeSet<&str> = BTreeSet::new();
    sheet_names.extend(base.sheets().iter().map(|s| s.name.as_str()));
    sheet_names.extend(target.sheets().iter().map(|s| s.name.as_str()));

    let mut sheets = Vec::new();
    for name in sheet_names {
        let base_sheet = base.sheet(name);
        let target_sheet = target.sheet(name);

        let (Some(b), Some(t)) = (base_sheet, target_sheet) else {
            // A sheet added or removed wholesale has nothing to align —
            // identical treatment to every other diff function here.
            if let Some(s) = diff_sheet(name, base_sheet, target_sheet) {
                sheets.push(s);
            }
            continue;
        };

        let coord_sheet = diff_sheet(name, Some(b), Some(t));
        let coord_count = coord_sheet.as_ref().map(sheet_total_changes).unwrap_or(0);

        if coord_count <= 1 {
            if let Some(s) = coord_sheet {
                sheets.push(s);
            }
            continue;
        }

        let mut best_sheet = coord_sheet;
        let mut min_changes = coord_count;

        match align_sheet_rows(name, b, t, row_limits) {
            Ok(None) => {
                // The absolute floor — nothing beats "nothing to report",
                // so column alignment isn't even worth attempting.
                best_sheet = None;
                min_changes = 0;
            }
            Ok(Some(row_sheet)) => {
                let count = sheet_total_changes(&row_sheet);
                if count < min_changes {
                    min_changes = count;
                    best_sheet = Some(row_sheet);
                }
            }
            Err(Error::RowAlignmentCostTooHigh { .. }) => {} // fall back to what we have
            Err(other) => {
                // align_sheet_rows's own doc comment promises this is the
                // only error it returns — surface a violation loudly in
                // debug/test builds rather than silently falling back to
                // coordinate-based diffing the same way a real cost-cap
                // does, which could otherwise mask a real bug.
                debug_assert!(
                    false,
                    "align_sheet_rows returned an unexpected error: {other:?}"
                );
            }
        }

        if min_changes > 0 {
            match align_sheet_columns(name, b, t, col_limits) {
                Ok(None) => best_sheet = None,
                Ok(Some(col_sheet)) => {
                    if sheet_total_changes(&col_sheet) < min_changes {
                        best_sheet = Some(col_sheet);
                    }
                }
                Err(Error::ColumnAlignmentCostTooHigh { .. }) => {}
                Err(other) => {
                    debug_assert!(
                        false,
                        "align_sheet_columns returned an unexpected error: {other:?}"
                    );
                }
            }
        }

        if let Some(s) = best_sheet {
            sheets.push(s);
        }
    }

    WorkbookDiff { sheets }
}

/// The "how noisy is this result" metric `diff_workbooks_best_effort`
/// minimizes per sheet: every kind of change `SheetDiff` can carry —
/// cell-level changes, merge-region changes, and a visibility flip — each
/// counted once. Cheap (no allocation) since it only sums existing
/// `Vec` lengths and compares two `Option<&'static str>`s.
fn sheet_total_changes(s: &SheetDiff) -> usize {
    s.cells.len() + s.merges.len() + (s.old_visibility != s.new_visibility) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Cell, CellRef, CellValue, Sheet, SheetVisibility};

    fn sheet_with_values(name: &str, cells: &[(u32, u32, CellValue)]) -> Sheet {
        let mut sheet = Sheet::new(name.to_string(), SheetVisibility::Visible);
        for (row, col, value) in cells {
            sheet.insert_cell(
                CellRef {
                    row: *row,
                    col: *col,
                },
                Cell {
                    value: Some(value.clone()),
                    style: None,
                },
            );
        }
        sheet
    }

    fn workbook(sheets: Vec<Sheet>) -> Workbook {
        Workbook::new(sheets, None)
    }

    fn num(n: f64) -> CellValue {
        CellValue::Number(n)
    }

    fn grid(rows: u32, cols: u32) -> Vec<(u32, u32, CellValue)> {
        (1..=rows)
            .flat_map(|r| (1..=cols).map(move |c| (r, c, num((r * 1000 + c) as f64))))
            .collect()
    }

    #[test]
    fn row_insertion_no_longer_cascades() {
        let base = workbook(vec![sheet_with_values("S", &grid(30, 5))]);
        let mut target_cells: Vec<(u32, u32, CellValue)> = grid(15, 5);
        target_cells.extend((1..=5).map(|c| (16, c, num(9000.0 + c as f64))));
        target_cells.extend(
            (16..=30u32).flat_map(|r| (1..=5).map(move |c| (r + 1, c, num((r * 1000 + c) as f64)))),
        );
        let target = workbook(vec![sheet_with_values("S", &target_cells)]);

        let coord = diff_sheet("S", Some(&base.sheets()[0]), Some(&target.sheets()[0])).unwrap();
        let best = diff_workbooks_best_effort(
            &base,
            &target,
            RowAlignmentLimits::default(),
            ColumnAlignmentLimits::default(),
        );

        assert_eq!(best.sheets.len(), 1);
        let best_total = sheet_total_changes(&best.sheets[0]);
        let coord_total = sheet_total_changes(&coord);
        assert!(best_total < coord_total);
        assert_eq!(best_total, 5); // just the new row's own 5 cells
    }

    #[test]
    fn mixed_edit_workbook_optimizes_each_sheet_independently() {
        // The exact scenario that defeats a whole-workbook mode choice
        // (https://github.com/MinamiyamaKotaro/exceldiff/issues/25#issuecomment-5419091215):
        // one sheet needs row alignment, another needs column alignment.
        let mut row_shift_target: Vec<(u32, u32, CellValue)> = grid(10, 3);
        row_shift_target.extend((1..=3).map(|c| (11, c, num(9000.0 + c as f64))));
        // (kept small: this test only checks that BOTH sheets improve,
        // not exact cascade sizes — those are covered by the two
        // single-sheet tests above/below)
        let mut col_shift_target = Vec::new();
        for r in 1..=10u32 {
            col_shift_target.push((r, 1, num((r * 1000 + 1) as f64)));
            col_shift_target.push((r, 2, num(9000.0 + r as f64)));
            for c in 2..=3u32 {
                col_shift_target.push((r, c + 1, num((r * 1000 + c) as f64)));
            }
        }

        let base = workbook(vec![
            sheet_with_values("RowShift", &grid(10, 3)),
            sheet_with_values("ColShift", &grid(10, 3)),
        ]);
        let target = workbook(vec![
            sheet_with_values("RowShift", &row_shift_target),
            sheet_with_values("ColShift", &col_shift_target),
        ]);

        let best = diff_workbooks_best_effort(
            &base,
            &target,
            RowAlignmentLimits::default(),
            ColumnAlignmentLimits::default(),
        );
        let by_name = |name: &str| best.sheets.iter().find(|s| s.name == name).unwrap();

        // RowShift: only the new row's 3 cells. ColShift: only the new
        // column's 10 cells. Neither shows the other sheet's cascade.
        assert_eq!(sheet_total_changes(by_name("RowShift")), 3);
        assert_eq!(sheet_total_changes(by_name("ColShift")), 10);
    }

    #[test]
    fn unchanged_sheet_is_short_circuited_and_omitted() {
        let base = workbook(vec![sheet_with_values("S", &grid(50, 5))]);
        let target = workbook(vec![sheet_with_values("S", &grid(50, 5))]);

        let best = diff_workbooks_best_effort(
            &base,
            &target,
            RowAlignmentLimits::default(),
            ColumnAlignmentLimits::default(),
        );
        assert!(best.sheets.is_empty());
    }

    #[test]
    fn single_cell_change_is_short_circuited_to_the_coordinate_result() {
        let base = workbook(vec![sheet_with_values("S", &grid(50, 5))]);
        let mut target_cells = grid(50, 5);
        target_cells[0] = (1, 1, num(999999.0));
        let target = workbook(vec![sheet_with_values("S", &target_cells)]);

        let best = diff_workbooks_best_effort(
            &base,
            &target,
            RowAlignmentLimits::default(),
            ColumnAlignmentLimits::default(),
        );
        assert_eq!(best.sheets.len(), 1);
        assert_eq!(sheet_total_changes(&best.sheets[0]), 1);
    }

    #[test]
    fn blank_row_insertion_reaches_the_ok_none_floor() {
        // A pure, monotonic, contiguous shift with no new content at all
        // — row alignment should find truly nothing left to report
        // (`Ok(None)`), which best-effort must recognize as the floor
        // (rather than silently keeping a worse coordinate-based result).
        let base = workbook(vec![sheet_with_values("S", &grid(20, 1))]);
        let mut target_cells: Vec<(u32, u32, CellValue)> = grid(10, 1);
        target_cells.extend((11..=20u32).map(|r| (r + 1, 1, num((r * 1000 + 1) as f64))));
        let target = workbook(vec![sheet_with_values("S", &target_cells)]);

        let coord = diff_sheet("S", Some(&base.sheets()[0]), Some(&target.sheets()[0])).unwrap();
        assert!(
            sheet_total_changes(&coord) > 1,
            "sanity: coordinate diff should see the cascade"
        );

        let best = diff_workbooks_best_effort(
            &base,
            &target,
            RowAlignmentLimits::default(),
            ColumnAlignmentLimits::default(),
        );
        assert!(
            best.sheets.is_empty(),
            "expected the sheet to be fully explained away, got {:?}",
            best.sheets
        );
    }

    #[test]
    fn blank_column_insertion_reaches_the_ok_none_floor_via_columns() {
        // The column-alignment counterpart of
        // `blank_row_insertion_reaches_the_ok_none_floor`: a blank column
        // insertion shifts every subsequent column's index, which changes
        // every row's *row*-alignment hash signature (it hashes (col,
        // value) pairs) — so row alignment does NOT reach 0 here, and this
        // specifically exercises the `Ok(None)` branch on the *column*
        // side (`best_sheet = None` without also zeroing `min_changes`,
        // since nothing tries column alignment again afterward).
        let base = workbook(vec![sheet_with_values("S", &grid(20, 10))]);
        let mut target_cells: Vec<(u32, u32, CellValue)> = Vec::new();
        for r in 1..=20u32 {
            for c in 1..=5u32 {
                target_cells.push((r, c, num((r * 1000 + c) as f64)));
            }
            for c in 6..=10u32 {
                target_cells.push((r, c + 1, num((r * 1000 + c) as f64)));
            }
        }
        let target = workbook(vec![sheet_with_values("S", &target_cells)]);

        // Sanity: row alignment alone does not reach the floor here, so
        // diff_workbooks_best_effort really does exercise the column-side
        // Ok(None) branch rather than short-circuiting past it.
        let row_only = align_sheet_rows(
            "S",
            &base.sheets()[0],
            &target.sheets()[0],
            RowAlignmentLimits::default(),
        );
        assert!(!matches!(row_only, Ok(None)));

        let best = diff_workbooks_best_effort(
            &base,
            &target,
            RowAlignmentLimits::default(),
            ColumnAlignmentLimits::default(),
        );
        assert!(
            best.sheets.is_empty(),
            "expected the sheet to be fully explained away, got {:?}",
            best.sheets
        );
    }

    #[test]
    fn cost_capped_alignment_falls_back_to_coordinate_diff_without_erroring() {
        let base = workbook(vec![sheet_with_values("S", &grid(30, 5))]);
        let mut target_cells: Vec<(u32, u32, CellValue)> = grid(15, 5);
        target_cells.extend((1..=5).map(|c| (16, c, num(9000.0 + c as f64))));
        target_cells.extend(
            (16..=30u32).flat_map(|r| (1..=5).map(move |c| (r + 1, c, num((r * 1000 + c) as f64)))),
        );
        let target = workbook(vec![sheet_with_values("S", &target_cells)]);

        let tiny_row_limits = RowAlignmentLimits {
            max_gap_myers_d: 1,
            max_cost: 1,
        };
        let tiny_col_limits = ColumnAlignmentLimits {
            max_cost: 1,
            max_column_pairs: 1,
        };
        let coord = diff_sheet("S", Some(&base.sheets()[0]), Some(&target.sheets()[0])).unwrap();

        let best = diff_workbooks_best_effort(&base, &target, tiny_row_limits, tiny_col_limits);
        assert_eq!(best.sheets.len(), 1);
        assert_eq!(
            sheet_total_changes(&best.sheets[0]),
            sheet_total_changes(&coord)
        );
    }

    #[test]
    fn added_and_deleted_sheets_are_unaffected() {
        let base = workbook(vec![sheet_with_values("Gone", &grid(5, 5))]);
        let target = workbook(vec![sheet_with_values("New", &grid(5, 5))]);

        let best = diff_workbooks_best_effort(
            &base,
            &target,
            RowAlignmentLimits::default(),
            ColumnAlignmentLimits::default(),
        );
        let names: Vec<&str> = best.sheets.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["Gone", "New"]);
    }
}
