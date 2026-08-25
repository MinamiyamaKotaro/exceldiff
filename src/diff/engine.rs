// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! Computes a `WorkbookDiff` between two already-parsed `Workbook`s (Issue
//! #3; style and merged-region diffs added by Issue #8).
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
//! future work (Issue #3 comment, tracked as Issue #4/#5).
//!
//! # Style diffs: sparser than value diffs, on purpose
//!
//! `CellDiff::old_style`/`new_style` (Issue #8) are populated only when
//! the style actually differs between the two sides being compared —
//! unlike `old_value`/`new_value`, which are always both present on a
//! `Modified` cell even if that particular cell's value didn't change
//! (e.g. a style-only change still reports `old_value == new_value`; see
//! `style_only_change_is_reported_as_modified`'s test). This asymmetry is
//! deliberate rather than an oversight: a value change is always *the*
//! reason a `CellDiff` exists in the first place, so showing both sides
//! costs nothing extra to interpret, whereas style is a secondary
//! dimension most `Modified` cells never touch at all — always attaching
//! a full `JsonStyle` pair (mirroring `old_value`/`new_value`'s
//! unconditional-pair convention) would bloat the common case for no
//! benefit. Changing `old_value`/`new_value` to match this sparser
//! convention retroactively was considered and rejected here to avoid
//! silently changing already-shipped, tested behavior (Issue #8 PR
//! review discussion).
//!
//! # Merged-region diffs: sheet-level, not cell-level
//!
//! `diff_merges` reports merge changes on `SheetDiff::merges`, not folded
//! into the origin cell's own `CellDiff` — unlike the full-snapshot JSON
//! (`json.rs`), which embeds a merge as the origin cell's `rowSpan`/
//! `colSpan`. This is a deliberate difference between the two output
//! shapes, not an inconsistency that slipped through: a diff's job is to
//! report *discrete changes*, and an `Added`/`Deleted` merge has no
//! natural originating `CellDiff` to attach to when neither that cell's
//! value nor style changed at all (synthesizing an otherwise-empty
//! `CellDiff` just to carry a span change would be its own kind of
//! awkwardness). A sheet-level list, mirroring how `json.rs` already
//! treats `images`/`columns` as sheet-level for the same "doesn't
//! naturally belong to one cell" reason, was chosen instead (Issue #8 PR
//! review discussion).

use crate::diff::model::{CellDiff, DiffStatus, MergeDiff, SheetDiff, WorkbookDiff};
use crate::error::Result;
use crate::json::{cell_value_to_json, style_to_json, visibility_tag};
use crate::model::{Cell, CellRef, Sheet, SheetVisibility, Workbook};
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
///
/// Every sheet is diffed regardless of `SheetVisibility` — `Hidden`/
/// `VeryHidden` sheets get their cells and merges compared exactly like
/// `Visible` ones (see `hidden_sheet_cell_changes_are_diffed_just_like_visible_ones`
/// below). This is a deliberate default kept in the absence of any actual
/// request to exclude hidden sheets, not an oversight — whether to add an
/// opt-in filter is an open question, tracked as Issue #16 (see
/// `engine.md`'s Open Questions for the full writeup).
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
/// identical visibility and zero cell/merge diffs — the "nothing to
/// report" case documented on `SheetDiff`.
///
/// `pub(crate)` (rather than private) so `diff::col_alignment` (Issue #5) can
/// reuse the `(None, Some)`/`(Some, None)` whole-sheet-added/deleted
/// branches as-is — a sheet that exists on only one side needs the exact
/// same treatment regardless of which cell-diffing strategy the `(Some,
/// Some)` case uses, so `diff::col_alignment` calls this function directly for
/// that case instead of duplicating it.
pub(crate) fn diff_sheet(
    name: &str,
    base: Option<&Sheet>,
    target: Option<&Sheet>,
) -> Option<SheetDiff> {
    match (base, target) {
        (None, Some(t)) => Some(SheetDiff {
            name: name.to_string(),
            status: DiffStatus::Added,
            old_visibility: None,
            new_visibility: Some(visibility_tag(t.visibility)),
            cells: t.iter_cells().map(|(r, c)| cell_diff_added(r, c)).collect(),
            merges: all_merges_added(t),
        }),
        (Some(b), None) => Some(SheetDiff {
            name: name.to_string(),
            status: DiffStatus::Deleted,
            old_visibility: Some(visibility_tag(b.visibility)),
            new_visibility: None,
            cells: b
                .iter_cells()
                .map(|(r, c)| cell_diff_deleted(r, c))
                .collect(),
            merges: all_merges_deleted(b),
        }),
        (Some(b), Some(t)) => {
            let cells = diff_cells(b, t);
            let merges = diff_merges(b, t);
            let (old_visibility, new_visibility) = visibility_diff(b.visibility, t.visibility);
            if cells.is_empty() && merges.is_empty() && old_visibility.is_none() {
                return None;
            }
            Some(SheetDiff {
                name: name.to_string(),
                status: DiffStatus::Modified,
                old_visibility,
                new_visibility,
                cells,
                merges,
            })
        }
        (None, None) => None,
    }
}

/// Reports `base`/`target`'s `SheetVisibility` as an `old_visibility`/
/// `new_visibility` pair, `(None, None)` when unchanged — shared by
/// `diff_sheet`'s `(Some, Some)` branch and `diff::col_alignment`'s aligned
/// equivalent (Issue #5), which reports sheet-level visibility exactly the
/// same way regardless of how cells within the sheet are matched.
pub(crate) fn visibility_diff(
    base: SheetVisibility,
    target: SheetVisibility,
) -> (Option<&'static str>, Option<&'static str>) {
    if base != target {
        (Some(visibility_tag(base)), Some(visibility_tag(target)))
    } else {
        (None, None)
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
                    out.push(cell_diff_deleted(br, bc));
                    b.next();
                }
                Ordering::Greater => {
                    out.push(cell_diff_added(tr, tc));
                    t.next();
                }
                Ordering::Equal => {
                    if bc.value != tc.value || bc.style != tc.style {
                        out.push(cell_diff_modified(br, bc, tc));
                    }
                    b.next();
                    t.next();
                }
            },
            (Some(&(br, bc)), None) => {
                out.push(cell_diff_deleted(br, bc));
                b.next();
            }
            (None, Some(&(tr, tc))) => {
                out.push(cell_diff_added(tr, tc));
                t.next();
            }
            (None, None) => break,
        }
    }

    out
}

fn cell_diff_added(r: CellRef, new: &Cell) -> CellDiff {
    CellDiff {
        row: r.row,
        col: r.col,
        status: DiffStatus::Added,
        old_col: None,
        old_row: None,
        old_value: None,
        new_value: Some(cell_value_to_json(new.value.as_ref())),
        old_style: None,
        new_style: new.style.as_deref().map(style_to_json),
    }
}

fn cell_diff_deleted(r: CellRef, old: &Cell) -> CellDiff {
    CellDiff {
        row: r.row,
        col: r.col,
        status: DiffStatus::Deleted,
        old_col: None,
        old_row: None,
        old_value: Some(cell_value_to_json(old.value.as_ref())),
        new_value: None,
        old_style: old.style.as_deref().map(style_to_json),
        new_style: None,
    }
}

/// See this module's doc comment ("Style diffs: sparser than value diffs")
/// for why `old_style`/`new_style` are populated only when `old.style !=
/// new.style`, unlike `old_value`/`new_value` (always both present here).
fn cell_diff_modified(r: CellRef, old: &Cell, new: &Cell) -> CellDiff {
    let style_changed = old.style != new.style;
    CellDiff {
        row: r.row,
        col: r.col,
        status: DiffStatus::Modified,
        old_col: None,
        old_row: None,
        old_value: Some(cell_value_to_json(old.value.as_ref())),
        new_value: Some(cell_value_to_json(new.value.as_ref())),
        old_style: style_changed
            .then(|| old.style.as_deref().map(style_to_json))
            .flatten(),
        new_style: style_changed
            .then(|| new.style.as_deref().map(style_to_json))
            .flatten(),
    }
}

/// Diffs `base`/`target`'s merged regions (Issue #8), matched by origin
/// coordinate (a merge's `start`) — the same coordinate-based matching
/// `diff_cells` uses for values. `Sheet::merged_regions` is a `HashMap`
/// with no guaranteed order (unlike `cells`'s `BTreeMap`, kept ordered for
/// Issue #87's determinism reasons), so this deliberately does *not* sort
/// every merge up front the way `diff_cells` can rely on `iter_cells`
/// already being sorted: it instead does an average-O(1) `HashMap` lookup
/// per `base` merge (catching `Modified`/`Deleted`) plus a `contains_key`
/// scan of `target` for merges absent from `base` (catching `Added`),
/// average O(base_merges + target_merges) overall, and sorts only the
/// (typically far smaller) set of actual differences at the end — paying
/// for a full sort of every unchanged merge would be wasted work on a
/// sheet with many merges but few actual changes.
///
/// `pub(crate)` so `diff::col_alignment` (Issue #5) can reuse it as-is for
/// merge diffing — column alignment only changes how *cell* diffs are
/// matched; merged-region alignment across a column shift is explicitly
/// out of scope for now (see `diff::col_alignment`'s module doc), so aligned
/// mode reports merges exactly the same coordinate-based way this default
/// engine does.
pub(crate) fn diff_merges(base: &Sheet, target: &Sheet) -> Vec<MergeDiff> {
    let base_merges = base.merged_regions();
    let target_merges = target.merged_regions();

    let mut out = Vec::new();
    for (&origin, base_region) in base_merges {
        match target_merges.get(&origin) {
            Some(target_region) if target_region.end != base_region.end => {
                out.push(MergeDiff {
                    status: DiffStatus::Modified,
                    start: origin.into(),
                    old_end: Some(base_region.end.into()),
                    new_end: Some(target_region.end.into()),
                });
            }
            Some(_) => {}
            None => out.push(MergeDiff {
                status: DiffStatus::Deleted,
                start: origin.into(),
                old_end: Some(base_region.end.into()),
                new_end: None,
            }),
        }
    }
    for (&origin, target_region) in target_merges {
        if !base_merges.contains_key(&origin) {
            out.push(MergeDiff {
                status: DiffStatus::Added,
                start: origin.into(),
                old_end: None,
                new_end: Some(target_region.end.into()),
            });
        }
    }

    out.sort_by_key(|m| (m.start.row, m.start.col));
    out
}

/// Every merge on `sheet`, reported as `Added` — used when the whole sheet
/// is new (`diff_sheet`'s `(None, Some(t))` case). Sorted by origin
/// coordinate for the same reason `diff_merges` sorts its own output:
/// `Sheet::merged_regions` is a `HashMap` with no guaranteed order (code
/// review on PR #10 — this and `all_merges_deleted` originally iterated it
/// directly, producing nondeterministic `SheetDiff::merges` ordering).
fn all_merges_added(sheet: &Sheet) -> Vec<MergeDiff> {
    let mut out: Vec<MergeDiff> = sheet
        .merged_regions()
        .iter()
        .map(|(&origin, region)| MergeDiff {
            status: DiffStatus::Added,
            start: origin.into(),
            old_end: None,
            new_end: Some(region.end.into()),
        })
        .collect();
    out.sort_by_key(|m| (m.start.row, m.start.col));
    out
}

/// The `Deleted` counterpart of [`all_merges_added`], used when the whole
/// sheet was removed (`diff_sheet`'s `(Some(b), None)` case).
fn all_merges_deleted(sheet: &Sheet) -> Vec<MergeDiff> {
    let mut out: Vec<MergeDiff> = sheet
        .merged_regions()
        .iter()
        .map(|(&origin, region)| MergeDiff {
            status: DiffStatus::Deleted,
            start: origin.into(),
            old_end: Some(region.end.into()),
            new_end: None,
        })
        .collect();
    out.sort_by_key(|m| (m.start.row, m.start.col));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::model::CellPos;
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
    fn hidden_sheet_cell_changes_are_diffed_just_like_visible_ones() {
        // Issue #16 (open question, docs/design/diff/engine.md): whether
        // hidden/veryHidden sheets should ever be *excludable* from a diff
        // is unresolved, but the current default — every sheet is diffed
        // regardless of visibility — is a deliberate decision, not an
        // accidental gap. Locks that contract in for both `Hidden` and
        // `VeryHidden` so a future change can't silently start skipping
        // hidden-sheet content without a test failing here first.
        for hidden_vis in [SheetVisibility::Hidden, SheetVisibility::VeryHidden] {
            let base = workbook(vec![sheet_with_cells("Hidden", hidden_vis, &[(1, 1, 1.0)])]);
            let target = workbook(vec![sheet_with_cells("Hidden", hidden_vis, &[(1, 1, 2.0)])]);

            let diff = diff_workbooks(&base, &target);
            assert_eq!(diff.sheets.len(), 1, "visibility = {hidden_vis:?}");
            let sheet_diff = &diff.sheets[0];
            // Visibility itself didn't change (same on both sides) — only
            // the cell value did, and that alone must still surface.
            assert_eq!(sheet_diff.old_visibility, None);
            assert_eq!(sheet_diff.new_visibility, None);
            assert_eq!(sheet_diff.cells.len(), 1);
            assert_eq!(sheet_diff.cells[0].status, DiffStatus::Modified);
        }
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
    fn column_insertion_cascades_into_shift_diffs_by_design() {
        // Column counterpart of row_insertion_cascades_into_shift_diffs_by_design
        // above — locks in the same documented tradeoff for the other axis.
        // diff::col_alignment::diff_workbooks_aligned_columns (Issue #5) is the
        // opt-in escape hatch from this behavior; see its
        // column_insertion_does_not_cascade_when_aligned test for the
        // direct contrast on this exact shape of input.
        let base = workbook(vec![sheet_with_cells(
            "Sheet1",
            SheetVisibility::Visible,
            &[(1, 1, 10.0), (1, 2, 20.0)],
        )]);
        let target = workbook(vec![sheet_with_cells(
            "Sheet1",
            SheetVisibility::Visible,
            &[(1, 1, 99.0), (1, 2, 10.0), (1, 3, 20.0)],
        )]);

        let diff = diff_workbooks(&base, &target);
        let cells = &diff.sheets[0].cells;
        // Every one of the 3 target columns differs from its
        // same-coordinate base counterpart (or has none), even though
        // columns 2-3 are really just columns 1-2 shifted right unchanged.
        assert_eq!(cells.len(), 3);
    }

    #[test]
    fn style_only_change_is_reported_as_modified_with_new_style_populated() {
        use crate::json::style_to_json;
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
        let cell = &diff.sheets[0].cells[0];
        assert_eq!(cell.status, DiffStatus::Modified);
        // Base had no style at all -> old_style stays None (there is
        // nothing to report on that side), while new_style carries the
        // style that was actually added.
        assert_eq!(cell.old_style, None);
        assert_eq!(
            cell.new_style,
            Some(style_to_json(&ResolvedStyle::default()))
        );
        // Value never changed -> old_value == new_value, unchanged from
        // the pre-Issue-#8 behavior this test used to lock in.
        assert_eq!(cell.old_value, cell.new_value);
    }

    #[test]
    fn value_only_change_carries_no_style_diff() {
        // The asymmetric counterpart of the test above: when only the
        // value changes and the style is identical on both sides,
        // old_style/new_style must both stay None (this module's doc
        // comment "Style diffs: sparser than value diffs" documents why).
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
        let cell = &diff.sheets[0].cells[0];
        assert_eq!(cell.status, DiffStatus::Modified);
        assert_eq!(cell.old_style, None);
        assert_eq!(cell.new_style, None);
    }

    #[test]
    fn added_cell_with_a_style_reports_new_style_only() {
        use crate::json::style_to_json;
        use crate::model::ResolvedStyle;
        use std::sync::Arc;

        let base = workbook(vec![]);
        let mut target_sheet = Sheet::new("Sheet1".into(), SheetVisibility::Visible);
        target_sheet.insert_cell(
            CellRef { row: 1, col: 1 },
            Cell {
                value: Some(CellValue::Number(1.0)),
                style: Some(Arc::new(ResolvedStyle::default())),
            },
        );
        let target = workbook(vec![target_sheet]);

        let diff = diff_workbooks(&base, &target);
        let cell = &diff.sheets[0].cells[0];
        assert_eq!(cell.status, DiffStatus::Added);
        assert_eq!(cell.old_style, None);
        assert_eq!(
            cell.new_style,
            Some(style_to_json(&ResolvedStyle::default()))
        );
    }

    fn sheet_with_merge(
        name: &str,
        vis: SheetVisibility,
        cell: (u32, u32, f64),
        merge: (CellRef, CellRef),
    ) -> Sheet {
        let mut sheet = sheet_with_cells(name, vis, &[cell]);
        sheet.insert_merge(crate::model::MergedRegion {
            start: merge.0,
            end: merge.1,
        });
        sheet.finalize_merges();
        sheet
    }

    #[test]
    fn merge_added_is_detected_even_with_no_cell_changes() {
        let base = workbook(vec![sheet_with_cells(
            "Sheet1",
            SheetVisibility::Visible,
            &[(1, 1, 1.0)],
        )]);
        let target = workbook(vec![sheet_with_merge(
            "Sheet1",
            SheetVisibility::Visible,
            (1, 1, 1.0),
            (CellRef { row: 1, col: 1 }, CellRef { row: 1, col: 2 }),
        )]);

        let diff = diff_workbooks(&base, &target);
        assert_eq!(
            diff.sheets.len(),
            1,
            "a merge-only change must still surface a SheetDiff"
        );
        let sheet_diff = &diff.sheets[0];
        assert!(sheet_diff.cells.is_empty());
        assert_eq!(sheet_diff.merges.len(), 1);
        let m = &sheet_diff.merges[0];
        assert_eq!(m.status, DiffStatus::Added);
        assert_eq!(m.start, CellPos { row: 1, col: 1 });
        assert_eq!(m.old_end, None);
        assert_eq!(m.new_end, Some(CellPos { row: 1, col: 2 }));
    }

    #[test]
    fn merge_deleted_is_detected() {
        let base = workbook(vec![sheet_with_merge(
            "Sheet1",
            SheetVisibility::Visible,
            (1, 1, 1.0),
            (CellRef { row: 1, col: 1 }, CellRef { row: 1, col: 2 }),
        )]);
        let target = workbook(vec![sheet_with_cells(
            "Sheet1",
            SheetVisibility::Visible,
            &[(1, 1, 1.0)],
        )]);

        let diff = diff_workbooks(&base, &target);
        let merges = &diff.sheets[0].merges;
        assert_eq!(merges.len(), 1);
        assert_eq!(merges[0].status, DiffStatus::Deleted);
        assert_eq!(merges[0].old_end, Some(CellPos { row: 1, col: 2 }));
        assert_eq!(merges[0].new_end, None);
    }

    #[test]
    fn merge_extent_change_is_reported_as_modified() {
        let base = workbook(vec![sheet_with_merge(
            "Sheet1",
            SheetVisibility::Visible,
            (1, 1, 1.0),
            (CellRef { row: 1, col: 1 }, CellRef { row: 1, col: 2 }),
        )]);
        let target = workbook(vec![sheet_with_merge(
            "Sheet1",
            SheetVisibility::Visible,
            (1, 1, 1.0),
            (CellRef { row: 1, col: 1 }, CellRef { row: 1, col: 3 }),
        )]);

        let diff = diff_workbooks(&base, &target);
        let merges = &diff.sheets[0].merges;
        assert_eq!(merges.len(), 1);
        assert_eq!(merges[0].status, DiffStatus::Modified);
        assert_eq!(merges[0].old_end, Some(CellPos { row: 1, col: 2 }));
        assert_eq!(merges[0].new_end, Some(CellPos { row: 1, col: 3 }));
    }

    #[test]
    fn unchanged_merge_produces_no_diff_at_all() {
        let base = workbook(vec![sheet_with_merge(
            "Sheet1",
            SheetVisibility::Visible,
            (1, 1, 1.0),
            (CellRef { row: 1, col: 1 }, CellRef { row: 1, col: 2 }),
        )]);
        let target = workbook(vec![sheet_with_merge(
            "Sheet1",
            SheetVisibility::Visible,
            (1, 1, 1.0),
            (CellRef { row: 1, col: 1 }, CellRef { row: 1, col: 2 }),
        )]);

        let diff = diff_workbooks(&base, &target);
        assert!(
            diff.sheets.is_empty(),
            "an identical merge must not produce a SheetDiff at all"
        );
    }

    #[test]
    fn sheet_added_reports_its_merges_as_added_too() {
        let base = workbook(vec![]);
        let target = workbook(vec![sheet_with_merge(
            "New",
            SheetVisibility::Visible,
            (1, 1, 1.0),
            (CellRef { row: 1, col: 1 }, CellRef { row: 1, col: 2 }),
        )]);

        let diff = diff_workbooks(&base, &target);
        let sheet_diff = &diff.sheets[0];
        assert_eq!(sheet_diff.status, DiffStatus::Added);
        assert_eq!(sheet_diff.merges.len(), 1);
        assert_eq!(sheet_diff.merges[0].status, DiffStatus::Added);
    }

    #[test]
    fn sheet_deleted_reports_its_merges_as_deleted_too() {
        let base = workbook(vec![sheet_with_merge(
            "Gone",
            SheetVisibility::Visible,
            (1, 1, 1.0),
            (CellRef { row: 1, col: 1 }, CellRef { row: 1, col: 2 }),
        )]);
        let target = workbook(vec![]);

        let diff = diff_workbooks(&base, &target);
        let sheet_diff = &diff.sheets[0];
        assert_eq!(sheet_diff.status, DiffStatus::Deleted);
        assert_eq!(sheet_diff.merges.len(), 1);
        assert_eq!(sheet_diff.merges[0].status, DiffStatus::Deleted);
    }

    fn sheet_with_merges_at(name: &str, vis: SheetVisibility, origins: &[(u32, u32)]) -> Sheet {
        let mut sheet = Sheet::new(name.to_string(), vis);
        for &(row, col) in origins {
            sheet.insert_cell(
                CellRef { row, col },
                Cell {
                    value: Some(CellValue::Number(1.0)),
                    style: None,
                },
            );
            sheet.insert_merge(crate::model::MergedRegion {
                start: CellRef { row, col },
                end: CellRef { row, col: col + 1 },
            });
        }
        sheet.finalize_merges();
        sheet
    }

    #[test]
    fn sheet_added_reports_multiple_merges_in_deterministic_row_col_order() {
        // Regression test for code review on PR #10: an earlier
        // implementation built this list by iterating
        // `Sheet::merged_regions()` (a HashMap) directly, so with more
        // than one merge the output order could vary run to run.
        // Origins deliberately inserted out of row order.
        let base = workbook(vec![]);
        let target = workbook(vec![sheet_with_merges_at(
            "New",
            SheetVisibility::Visible,
            &[(5, 1), (1, 1), (3, 1)],
        )]);

        let diff = diff_workbooks(&base, &target);
        let starts: Vec<(u32, u32)> = diff.sheets[0]
            .merges
            .iter()
            .map(|m| (m.start.row, m.start.col))
            .collect();
        assert_eq!(starts, vec![(1, 1), (3, 1), (5, 1)]);
    }

    #[test]
    fn sheet_deleted_reports_multiple_merges_in_deterministic_row_col_order() {
        let base = workbook(vec![sheet_with_merges_at(
            "Gone",
            SheetVisibility::Visible,
            &[(5, 1), (1, 1), (3, 1)],
        )]);
        let target = workbook(vec![]);

        let diff = diff_workbooks(&base, &target);
        let starts: Vec<(u32, u32)> = diff.sheets[0]
            .merges
            .iter()
            .map(|m| (m.start.row, m.start.col))
            .collect();
        assert_eq!(starts, vec![(1, 1), (3, 1), (5, 1)]);
    }
}
