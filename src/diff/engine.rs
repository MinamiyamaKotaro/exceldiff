// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! Computes a `WorkbookDiff` between two already-parsed `Workbook`s (Issue
//! #3).
//!
//! # Algorithm choice: coordinate-based, not row/column-alignment-based
//!
//! `diff_workbooks` compares cells strictly by `(row, col)` — a cell that
//! shifted because a row or column was inserted/deleted elsewhere on the
//! sheet is reported as a `Deleted` at its old coordinate plus an `Added`
//! at its new one, rather than being recognized as "the same cell, moved".
//! This is a deliberate scope decision, not an oversight: an alignment
//! algorithm that *does* detect such shifts (e.g. a 2D LCS over rows and
//! columns, prototyped in `poc/issue3-poc`) costs O(distinct_rows² +
//! distinct_cols²) time and memory in the worst case — measured there at
//! ~13s and ~128MB for a single 4,000-row alignment — which is
//! incompatible with this crate's purpose (parsing "grid-paper Excel"
//! files with an extreme number of rows/columns; see `lib.rs`'s module
//! doc). This function instead walks each sheet's cells once each,
//! merge-join style, costing O(base_cells + target_cells) time and O(1)
//! extra memory beyond the output — safe on any sheet size this crate
//! otherwise supports, at the cost of over-reporting a diff across a
//! row/column insertion. A capped, opt-in alignment mode is left as
//! future work (Issue #3 comment).

use crate::diff::model::{CellDiff, DiffStatus, SheetDiff, WorkbookDiff};
use crate::error::Result;
use crate::json::{cell_value_to_json, visibility_tag};
use crate::model::{Cell, CellRef, Sheet, Workbook};
use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::path::Path;

/// Parses `base_path` and `target_path` (via `parse_workbook`, the same
/// pipeline every other entry point in this crate uses) and diffs the
/// results. The convenience entry point for callers that have file paths
/// rather than already-parsed `Workbook`s in hand.
pub fn diff_paths(
    base_path: impl AsRef<Path>,
    target_path: impl AsRef<Path>,
) -> Result<WorkbookDiff> {
    let base = crate::parse_workbook(base_path)?;
    let target = crate::parse_workbook(target_path)?;
    Ok(diff_workbooks(&base, &target))
}

/// Diffs two already-parsed `Workbook`s sheet by sheet (matched by name).
/// See this module's doc comment for the coordinate-based algorithm and its
/// tradeoffs.
pub fn diff_workbooks(base: &Workbook, target: &Workbook) -> WorkbookDiff {
    let mut sheet_names: BTreeSet<&str> = BTreeSet::new();
    sheet_names.extend(base.sheets().iter().map(|s| s.name.as_str()));
    sheet_names.extend(target.sheets().iter().map(|s| s.name.as_str()));

    let sheets = sheet_names
        .into_iter()
        .filter_map(|name| diff_sheet(name, base.sheet(name), target.sheet(name)))
        .collect();

    WorkbookDiff { sheets }
}

/// Diffs one sheet, identified by `name`, given its (possibly absent) form
/// on each side. Returns `None` when the sheet exists on both sides with
/// identical visibility and zero cell diffs — the "nothing to report" case
/// documented on `SheetDiff`.
fn diff_sheet(name: &str, base: Option<&Sheet>, target: Option<&Sheet>) -> Option<SheetDiff> {
    match (base, target) {
        (None, Some(t)) => Some(SheetDiff {
            name: name.to_string(),
            status: DiffStatus::Added,
            old_visibility: None,
            new_visibility: Some(visibility_tag(t.visibility)),
            cells: t
                .iter_cells()
                .map(|(r, c)| cell_diff(r, DiffStatus::Added, None, Some(c)))
                .collect(),
        }),
        (Some(b), None) => Some(SheetDiff {
            name: name.to_string(),
            status: DiffStatus::Deleted,
            old_visibility: Some(visibility_tag(b.visibility)),
            new_visibility: None,
            cells: b
                .iter_cells()
                .map(|(r, c)| cell_diff(r, DiffStatus::Deleted, Some(c), None))
                .collect(),
        }),
        (Some(b), Some(t)) => {
            let cells = diff_cells(b, t);
            let (old_visibility, new_visibility) = if b.visibility != t.visibility {
                (
                    Some(visibility_tag(b.visibility)),
                    Some(visibility_tag(t.visibility)),
                )
            } else {
                (None, None)
            };
            if cells.is_empty() && old_visibility.is_none() {
                return None;
            }
            Some(SheetDiff {
                name: name.to_string(),
                status: DiffStatus::Modified,
                old_visibility,
                new_visibility,
                cells,
            })
        }
        (None, None) => None,
    }
}

/// Merge-joins `base`/`target`'s cells in one linear pass, relying on
/// `Sheet::iter_cells` already yielding `CellRef`-ascending order (row
/// before col — `Sheet`'s own `BTreeMap` invariant). O(base_cells +
/// target_cells) time, no intermediate collection beyond the output.
fn diff_cells(base: &Sheet, target: &Sheet) -> Vec<CellDiff> {
    let mut out = Vec::new();
    let mut b = base.iter_cells().peekable();
    let mut t = target.iter_cells().peekable();

    loop {
        match (b.peek(), t.peek()) {
            (Some(&(br, bc)), Some(&(tr, tc))) => match br.cmp(&tr) {
                Ordering::Less => {
                    out.push(cell_diff(br, DiffStatus::Deleted, Some(bc), None));
                    b.next();
                }
                Ordering::Greater => {
                    out.push(cell_diff(tr, DiffStatus::Added, None, Some(tc)));
                    t.next();
                }
                Ordering::Equal => {
                    if bc.value != tc.value || bc.style != tc.style {
                        out.push(cell_diff(br, DiffStatus::Modified, Some(bc), Some(tc)));
                    }
                    b.next();
                    t.next();
                }
            },
            (Some(&(br, bc)), None) => {
                out.push(cell_diff(br, DiffStatus::Deleted, Some(bc), None));
                b.next();
            }
            (None, Some(&(tr, tc))) => {
                out.push(cell_diff(tr, DiffStatus::Added, None, Some(tc)));
                t.next();
            }
            (None, None) => break,
        }
    }

    out
}

fn cell_diff(r: CellRef, status: DiffStatus, old: Option<&Cell>, new: Option<&Cell>) -> CellDiff {
    CellDiff {
        row: r.row,
        col: r.col,
        status,
        old_value: old.map(|c| cell_value_to_json(c.value.as_ref())),
        new_value: new.map(|c| cell_value_to_json(c.value.as_ref())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CellValue, SheetVisibility};

    fn sheet_with_cells(name: &str, vis: SheetVisibility, cells: &[(u32, u32, f64)]) -> Sheet {
        let mut sheet = Sheet::new(name.to_string(), vis);
        for &(row, col, n) in cells {
            sheet.insert_cell(
                CellRef { row, col },
                Cell {
                    value: Some(CellValue::Number(n)),
                    style: None,
                },
            );
        }
        sheet
    }

    fn workbook(sheets: Vec<Sheet>) -> Workbook {
        Workbook::new(sheets, None)
    }

    #[test]
    fn identical_workbooks_produce_no_diff() {
        let base = workbook(vec![sheet_with_cells(
            "Sheet1",
            SheetVisibility::Visible,
            &[(1, 1, 42.0)],
        )]);
        let target = workbook(vec![sheet_with_cells(
            "Sheet1",
            SheetVisibility::Visible,
            &[(1, 1, 42.0)],
        )]);

        let diff = diff_workbooks(&base, &target);
        assert!(diff.sheets.is_empty());
    }

    #[test]
    fn modified_cell_carries_old_and_new_value() {
        let base = workbook(vec![sheet_with_cells(
            "Sheet1",
            SheetVisibility::Visible,
            &[(1, 1, 100.0)],
        )]);
        let target = workbook(vec![sheet_with_cells(
            "Sheet1",
            SheetVisibility::Visible,
            &[(1, 1, 120.0)],
        )]);

        let diff = diff_workbooks(&base, &target);
        assert_eq!(diff.sheets.len(), 1);
        assert_eq!(diff.sheets[0].cells.len(), 1);
        let cell = &diff.sheets[0].cells[0];
        assert_eq!(cell.row, 1);
        assert_eq!(cell.col, 1);
        assert_eq!(cell.status, DiffStatus::Modified);
        assert_eq!(
            cell.old_value,
            Some(cell_value_to_json(Some(&CellValue::Number(100.0))))
        );
        assert_eq!(
            cell.new_value,
            Some(cell_value_to_json(Some(&CellValue::Number(120.0))))
        );
    }

    #[test]
    fn added_and_deleted_cells_are_detected() {
        let base = workbook(vec![sheet_with_cells(
            "Sheet1",
            SheetVisibility::Visible,
            &[(1, 1, 1.0), (2, 1, 2.0)],
        )]);
        let target = workbook(vec![sheet_with_cells(
            "Sheet1",
            SheetVisibility::Visible,
            &[(1, 1, 1.0), (3, 1, 3.0)],
        )]);

        let diff = diff_workbooks(&base, &target);
        let cells = &diff.sheets[0].cells;
        assert_eq!(cells.len(), 2);
        assert!(cells
            .iter()
            .any(|c| c.row == 2 && c.status == DiffStatus::Deleted));
        assert!(cells
            .iter()
            .any(|c| c.row == 3 && c.status == DiffStatus::Added));
    }

    #[test]
    fn added_cell_ahead_of_remaining_base_cells_is_detected() {
        // added_and_deleted_cells_are_detected only exercises an Added
        // cell trailing after every base cell has been consumed
        // ((None, Some(..)) in diff_cells's merge). This covers the
        // distinct Ordering::Greater branch instead — target has an extra
        // cell that sorts *before* a base cell diff_cells hasn't reached
        // yet, with the base iterator still non-empty at that point.
        let base = workbook(vec![sheet_with_cells(
            "Sheet1",
            SheetVisibility::Visible,
            &[(2, 1, 2.0)],
        )]);
        let target = workbook(vec![sheet_with_cells(
            "Sheet1",
            SheetVisibility::Visible,
            &[(1, 1, 1.0), (2, 1, 2.0)],
        )]);

        let diff = diff_workbooks(&base, &target);
        let cells = &diff.sheets[0].cells;
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].row, 1);
        assert_eq!(cells[0].status, DiffStatus::Added);
    }

    #[test]
    fn sheet_added_reports_every_cell_as_added() {
        let base = workbook(vec![]);
        let target = workbook(vec![sheet_with_cells(
            "New",
            SheetVisibility::Visible,
            &[(1, 1, 1.0), (1, 2, 2.0)],
        )]);

        let diff = diff_workbooks(&base, &target);
        assert_eq!(diff.sheets.len(), 1);
        let sheet_diff = &diff.sheets[0];
        assert_eq!(sheet_diff.status, DiffStatus::Added);
        assert_eq!(sheet_diff.old_visibility, None);
        assert_eq!(sheet_diff.new_visibility, Some("visible"));
        assert_eq!(sheet_diff.cells.len(), 2);
        assert!(sheet_diff
            .cells
            .iter()
            .all(|c| c.status == DiffStatus::Added));
    }

    #[test]
    fn sheet_deleted_reports_every_cell_as_deleted() {
        let base = workbook(vec![sheet_with_cells(
            "Gone",
            SheetVisibility::Visible,
            &[(1, 1, 1.0)],
        )]);
        let target = workbook(vec![]);

        let diff = diff_workbooks(&base, &target);
        assert_eq!(diff.sheets.len(), 1);
        let sheet_diff = &diff.sheets[0];
        assert_eq!(sheet_diff.status, DiffStatus::Deleted);
        assert_eq!(sheet_diff.old_visibility, Some("visible"));
        assert_eq!(sheet_diff.new_visibility, None);
        assert_eq!(sheet_diff.cells.len(), 1);
        assert_eq!(sheet_diff.cells[0].status, DiffStatus::Deleted);
    }

    #[test]
    fn visibility_change_with_no_cell_changes_is_still_reported() {
        let base = workbook(vec![sheet_with_cells(
            "Sheet1",
            SheetVisibility::Visible,
            &[(1, 1, 1.0)],
        )]);
        let target = workbook(vec![sheet_with_cells(
            "Sheet1",
            SheetVisibility::Hidden,
            &[(1, 1, 1.0)],
        )]);

        let diff = diff_workbooks(&base, &target);
        assert_eq!(diff.sheets.len(), 1);
        let sheet_diff = &diff.sheets[0];
        assert_eq!(sheet_diff.status, DiffStatus::Modified);
        assert_eq!(sheet_diff.old_visibility, Some("visible"));
        assert_eq!(sheet_diff.new_visibility, Some("hidden"));
        assert!(sheet_diff.cells.is_empty());
    }

    #[test]
    fn row_insertion_cascades_into_shift_diffs_by_design() {
        // Documents this module's documented tradeoff: inserting a row at
        // the top shifts every subsequent row's coordinates, which the
        // coordinate-based engine reports as a delete-at-old-coordinate
        // plus an add-at-new-coordinate rather than "no change" — the
        // exact behavior an alignment-based engine would avoid, at the
        // O(rows^2) cost documented in this module's doc comment.
        let base = workbook(vec![sheet_with_cells(
            "Sheet1",
            SheetVisibility::Visible,
            &[(1, 1, 10.0), (2, 1, 20.0)],
        )]);
        let target = workbook(vec![sheet_with_cells(
            "Sheet1",
            SheetVisibility::Visible,
            &[(1, 1, 99.0), (2, 1, 10.0), (3, 1, 20.0)],
        )]);

        let diff = diff_workbooks(&base, &target);
        let cells = &diff.sheets[0].cells;
        // Every one of the 3 target rows differs from its same-coordinate
        // base counterpart (or has none), even though rows 2-3 are really
        // just rows 1-2 shifted down unchanged.
        assert_eq!(cells.len(), 3);
    }

    #[test]
    fn style_only_change_is_reported_as_modified() {
        use crate::model::ResolvedStyle;
        use std::sync::Arc;

        let mut base_sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        base_sheet.insert_cell(
            CellRef { row: 1, col: 1 },
            Cell {
                value: Some(CellValue::Number(1.0)),
                style: None,
            },
        );
        let mut target_sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        target_sheet.insert_cell(
            CellRef { row: 1, col: 1 },
            Cell {
                value: Some(CellValue::Number(1.0)),
                style: Some(Arc::new(ResolvedStyle::default())),
            },
        );

        let diff = diff_workbooks(&workbook(vec![base_sheet]), &workbook(vec![target_sheet]));
        assert_eq!(diff.sheets[0].cells.len(), 1);
        assert_eq!(diff.sheets[0].cells[0].status, DiffStatus::Modified);
    }
}
