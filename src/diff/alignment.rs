// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! Opt-in column-insertion/deletion-aware diffing (Issue #5).
//!
//! `diff::engine::diff_workbooks` compares cells strictly by `(row, col)`,
//! so a single inserted/deleted column cascades into spurious `Added`/
//! `Deleted` diffs for every cell to its right (see that module's doc
//! comment for the full rationale). `diff_workbooks_aligned_columns` is the
//! capped, explicitly opt-in alternative this crate deferred to Issue #5:
//! it first matches base/target columns by content (not position), then
//! diffs cells within each matched column pair by row — so a column that
//! merely shifted produces no diff at all, and only genuinely changed
//! cells are reported.
//!
//! # Why this is safe to ship where the Issue #3 PoC's 2D LCS aligner
//! wasn't
//!
//! The original 2D LCS aligner prototyped in `poc/issue3-poc` aligned both
//! rows *and* columns and had no cost cap at all — `engine.rs`'s doc
//! comment documents why that was rejected (O(distinct_rows² +
//! distinct_cols²), ~13s/~128MB at a single 4,000-row alignment). This
//! function aligns *columns only* (rows stay coordinate-pinned — see
//! "Rows are not realigned" below), and, unlike the PoC, enforces an
//! explicit cost budget before doing any O(cols²) work: see
//! `ColumnAlignmentLimits`.
//!
//! # Matching heuristic: threshold + a low-cardinality safety gate
//!
//! Five rounds of PoC investigation (`poc/issue5-poc` through
//! `poc/issue5-poc-v4`, summarized across
//! <https://github.com/MinamiyamaKotaro/exceldiff/issues/5>'s comment
//! thread) measured that a naive "any single matching cell counts as a
//! candidate" heuristic has ~15-38% precision. Requiring `match_count ≥
//! max(2, 20% of the shorter column's populated-cell count)` fixes this to
//! 100% precision for columns with ≥10 distinct values — but **columns
//! with only 2-4 distinct values (booleans, status flags) and no header
//! cannot be matched safely by any content-based score**: even an
//! unrestricted, unbounded content comparison (no thresholds, no indexing
//! shortcuts) still produced up to a 122% false-match rate there, because
//! unrelated low-cardinality columns clear the same similarity bar as the
//! true match purely by chance. A threshold on the match *score* cannot
//! fix that. What does: `MIN_DISTINCT_FOR_CONTENT_MATCH` — a column below
//! that many distinct values is excluded from content-based matching
//! entirely unless it has an exact header match, and is otherwise left
//! unmatched (falling through to ordinary per-column coordinate diffing,
//! i.e. the safe default rather than a wrong guess).
//!
//! # Rows are not realigned (Issue #4 is separate and unimplemented)
//!
//! Row insertion/deletion detection is Issue #4, tracked separately and
//! completely unimplemented (no code, no design decisions made) as of this
//! writing. Because rows are never realigned here, columns are matched by
//! plain per-row-coordinate content comparison rather than the
//! row-shift-invariant schemes (Bag-of-Values, Sequence-LCS) the Issue #5
//! PoC rounds explored — those exist specifically to keep column matching
//! working when rows *also* shift simultaneously, which cannot happen
//! while this function holds rows fixed. When #4 lands, the natural
//! integration point is a row mapping fed into `diff_matched_columns`'s
//! merge-join (comparing `base_row` against whatever target row it's been
//! aligned to, instead of assuming identity) — no rework of column
//! matching itself required.
//!
//! # Merged regions are not column-aware
//!
//! `SheetDiff::merges` is computed via `diff::engine::diff_merges`
//! unchanged — merged-region matching across a column shift (e.g. a merge
//! whose origin cell moved because a column was inserted before it) is out
//! of scope for this change. Merges remain subject to the same
//! coordinate-based cascade `diff_workbooks` has today, even when calling
//! this function.

use crate::diff::engine::{diff_merges, diff_sheet, visibility_diff};
use crate::diff::model::{CellDiff, DiffStatus, SheetDiff, WorkbookDiff};
use crate::error::{Error, Result};
use crate::json::{cell_value_to_json, style_to_json};
use crate::model::{Cell, CellValue, Sheet, Workbook};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

/// A column is only eligible for content-based matching (i.e. matching
/// that doesn't require an exact header match) once it holds at least this
/// many *distinct* populated values. Below this floor, unrelated columns
/// clear any content-similarity bar too easily by chance — see this
/// module's doc comment ("Matching heuristic") for the measured evidence.
/// Verified directly (PoC round 4's `min_distinct` sweep): 8 cleanly
/// separates the safe case (≥10 distinct values: 100% match accuracy) from
/// the dangerous one (2-4 distinct values: up to 122% false-match rate),
/// with zero observed regression on normal-cardinality columns.
const MIN_DISTINCT_FOR_CONTENT_MATCH: usize = 8;

/// Score bonus for an exact header match, large enough to outrank any
/// purely content-based match regardless of column length — bigger than
/// Excel's absolute maximum row count (1,048,576), which upper-bounds any
/// possible `match_count`.
const HEADER_MATCH_BONUS: u64 = 2_000_000;

/// Cap on `distinct_cols_base * distinct_cols_target * sample_rows`, the
/// measured cost driver of `align_columns` below (O(distinct_cols_base ×
/// distinct_cols_target × max_row) — see this module's doc comment and
/// Issue #5's PoC thread). A flat distinct-column-count cap (the
/// `resolve::merge::MAX_MERGE_REGIONS` pattern) is not sufficient on its
/// own here, unlike for merged regions: merge overlap validation costs
/// O(N²) in region count *alone*, but column alignment's cost also scales
/// with row count, so a column-count-only cap that's safe at 500 rows
/// becomes unsafe by an order of magnitude at 50,000 rows. The three
/// factors are therefore budgeted together.
///
/// Measured directly against this exact function (release build, Apple
/// Silicon, `diff_workbooks_aligned_columns` end-to-end, not a
/// microbenchmark of an inner loop):
///
/// | distinct cols (base=target) | rows | cost | measured time |
/// |---:|---:|---:|---:|
/// | 50 | 500 | 1,250,000 | 8.7ms |
/// | 200 | 500 | 20,000,000 | 108ms |
/// | 100 | 5,000 | 50,000,000 | 408ms |
/// | 20 | 50,000 | 20,000,000 | 364ms |
/// | 100 | 50,000 | 500,000,000 | 11.9s |
///
/// Cost-normalized time (`ms / cost`) isn't perfectly constant across
/// shapes — it ranges from ~5.4e-6 to ~2.4e-5 ms/unit, worse at low
/// column counts with very high row counts, where the O(cols × rows)
/// per-column setup work (`column_contents`/`has_enough_distinct_values`)
/// stops being negligible next to the O(cols² × rows) matching work. This
/// cap uses the worst observed rate (~2.4e-5 ms/unit) with headroom,
/// keeping the worst case comfortably inside the same "few hundred ms"
/// budget class `MAX_MERGE_REGIONS` targets (10,000,000 × 2.4e-5 ≈ 240ms).
pub(crate) const MAX_COLUMN_ALIGNMENT_COST: usize = 10_000_000;

/// Configuration for [`diff_workbooks_aligned_columns`]. A plain struct
/// parameter (not a builder, not `Option<T>`), matching this crate's
/// existing `SizeLimits`/`parse_workbook_with_limits` convention.
#[derive(Debug, Clone, Copy)]
pub struct ColumnAlignmentLimits {
    /// Defaults to `MAX_COLUMN_ALIGNMENT_COST` — see that constant's doc
    /// comment for how it was measured.
    pub max_cost: usize,
}

impl Default for ColumnAlignmentLimits {
    fn default() -> Self {
        ColumnAlignmentLimits {
            max_cost: MAX_COLUMN_ALIGNMENT_COST,
        }
    }
}

/// Diffs two already-parsed `Workbook`s the same way
/// `diff::engine::diff_workbooks` does, except that within each sheet
/// present on both sides, columns are first matched by content (see this
/// module's doc comment) so an inserted/deleted column doesn't cascade
/// into spurious diffs for every column after it. Sheets present on only
/// one side are handled identically to `diff_workbooks` (reusing
/// `diff::engine::diff_sheet` directly) — there is nothing to align when
/// an entire sheet is new or gone.
///
/// Returns `Err(Error::TooManyDistinctColumnsForAlignment)`, fail-fast and
/// before any O(cols²) matching work, when a sheet's alignment cost would
/// exceed `limits.max_cost` — deliberately not a silent fallback to
/// `diff_workbooks`'s coordinate-based result, since the caller opted into
/// alignment explicitly (see `ColumnAlignmentLimits`'s doc comment and
/// `MAX_COLUMN_ALIGNMENT_COST`). A caller that wants automatic fallback can
/// catch this error and call `diff_workbooks` itself.
pub fn diff_workbooks_aligned_columns(
    base: &Workbook,
    target: &Workbook,
    limits: ColumnAlignmentLimits,
) -> Result<WorkbookDiff> {
    let mut sheet_names: BTreeSet<&str> = BTreeSet::new();
    sheet_names.extend(base.sheets().iter().map(|s| s.name.as_str()));
    sheet_names.extend(target.sheets().iter().map(|s| s.name.as_str()));

    let mut sheets = Vec::new();
    for name in sheet_names {
        let base_sheet = base.sheet(name);
        let target_sheet = target.sheet(name);
        let sheet_diff = match (base_sheet, target_sheet) {
            (Some(b), Some(t)) => align_sheet_columns(name, b, t, limits)?,
            _ => diff_sheet(name, base_sheet, target_sheet),
        };
        if let Some(sheet_diff) = sheet_diff {
            sheets.push(sheet_diff);
        }
    }

    Ok(WorkbookDiff { sheets })
}

/// One column's content, extracted once per sheet-pair comparison so the
/// O(cols²) matching pass below never re-scans the sheet per candidate
/// pair.
struct ColumnContent<'a> {
    col: u32,
    /// `(row, cell)` pairs, only for rows this column actually has a cell
    /// at, in ascending row order. A plain `Vec`, not a `BTreeMap`:
    /// `Sheet::iter_cells()` already yields cells in row-then-column
    /// order (`Sheet`'s own `BTreeMap` invariant), so the subsequence
    /// belonging to any one column is already sorted by construction —
    /// paying for a second `BTreeMap`'s O(log rows) insertion/lookup cost
    /// on top of that would be pure waste. `count_matching_rows`/
    /// `diff_matched_columns` below merge-join two of these the same way
    /// `diff::engine::diff_cells` merge-joins a whole sheet, which is why
    /// this needs to stay row-sorted rather than being looked up by row on
    /// demand.
    cells: Vec<(u32, &'a Cell)>,
    /// Row 1's value, if any — the strongest available matching signal.
    /// Restricted to `Text` deliberately: a real header is virtually
    /// always a string label, and a sheet with no header row at all still
    /// very plausibly has *numeric* data starting at row 1 — treating a
    /// coincidental `Number`/`Number` match at row 1 as a "header match"
    /// would defeat the low-cardinality safety gate by accident (verified
    /// by this module's own
    /// `low_cardinality_headerless_columns_fall_back_to_coordinate_diff_safely`
    /// test, which originally used numeric row-1 values and failed until
    /// this restriction was added).
    header: Option<&'a CellValue>,
    /// Whether this column holds at least `MIN_DISTINCT_FOR_CONTENT_MATCH`
    /// distinct values — see that constant's doc comment.
    eligible_for_content_match: bool,
}

/// Diffs one sheet known to exist on both sides, aligning columns by
/// content before diffing cells. Returns `Ok(None)` when nothing changed
/// (same "nothing to report" convention `diff::engine::diff_sheet` uses).
fn align_sheet_columns(
    name: &str,
    base: &Sheet,
    target: &Sheet,
    limits: ColumnAlignmentLimits,
) -> Result<Option<SheetDiff>> {
    let base_cols = column_contents(base);
    let target_cols = column_contents(target);

    let sample_rows = base.max_row.max(target.max_row) as usize;
    let cost = base_cols
        .len()
        .saturating_mul(target_cols.len())
        .saturating_mul(sample_rows);
    if cost > limits.max_cost {
        return Err(Error::TooManyDistinctColumnsForAlignment {
            count: cost,
            limit: limits.max_cost,
        });
    }

    let alignments = align_columns(&base_cols, &target_cols);

    let mut cells = Vec::new();
    for alignment in &alignments {
        match *alignment {
            ColumnAlignment::Match {
                base_idx,
                target_idx,
            } => diff_matched_columns(&base_cols[base_idx], &target_cols[target_idx], &mut cells),
            ColumnAlignment::Inserted { target_idx } => {
                let t = &target_cols[target_idx];
                for &(row, cell) in &t.cells {
                    cells.push(cell_diff_added_aligned(row, t.col, cell));
                }
            }
            ColumnAlignment::Deleted { base_idx } => {
                let b = &base_cols[base_idx];
                for &(row, cell) in &b.cells {
                    cells.push(cell_diff_deleted_aligned(row, b.col, cell));
                }
            }
        }
    }
    cells.sort_by_key(|c| (c.row, c.col));

    let merges = diff_merges(base, target);
    let (old_visibility, new_visibility) = visibility_diff(base.visibility, target.visibility);

    if cells.is_empty() && merges.is_empty() && old_visibility.is_none() {
        return Ok(None);
    }
    Ok(Some(SheetDiff {
        name: name.to_string(),
        status: DiffStatus::Modified,
        old_visibility,
        new_visibility,
        cells,
        merges,
    }))
}

/// Buckets `sheet.iter_cells()` by column in one O(cells) pass, then builds
/// each column's `ColumnContent` (also O(cells) total, not per-column).
/// Each bucket comes out already row-sorted for free — see
/// `ColumnContent::cells`'s doc comment — so this only ever pushes, never
/// sorts or inserts into a second ordered structure.
fn column_contents(sheet: &Sheet) -> Vec<ColumnContent<'_>> {
    let mut by_col: BTreeMap<u32, Vec<(u32, &Cell)>> = BTreeMap::new();
    for (r, cell) in sheet.iter_cells() {
        by_col.entry(r.col).or_default().push((r.row, cell));
    }

    by_col
        .into_iter()
        .map(|(col, cells)| {
            let header = cells
                .first()
                .filter(|&&(row, _)| row == 1)
                .and_then(|&(_, c)| c.value.as_ref())
                .filter(|v| matches!(v, CellValue::Text(_)));
            let eligible_for_content_match = has_enough_distinct_values(&cells);
            ColumnContent {
                col,
                cells,
                header,
                eligible_for_content_match,
            }
        })
        .collect()
}

/// Whether `cells` holds at least `MIN_DISTINCT_FOR_CONTENT_MATCH` distinct
/// values. Stops scanning as soon as the floor is reached — the caller
/// only needs a yes/no answer, not an exact count — and caps its own
/// working set at that same floor, so this stays cheap (effectively O(rows
/// × MIN_DISTINCT_FOR_CONTENT_MATCH), not O(rows²)) even on a column with
/// many populated rows. `CellValue` cannot derive `Hash` (its `Number(f64)`
/// variant), so distinctness is checked by linear scan against the capped
/// working set rather than a `HashSet`.
fn has_enough_distinct_values(cells: &[(u32, &Cell)]) -> bool {
    let mut seen: Vec<&CellValue> = Vec::with_capacity(MIN_DISTINCT_FOR_CONTENT_MATCH);
    for &(_, cell) in cells {
        if let Some(value) = cell.value.as_ref() {
            if !seen.contains(&value) {
                seen.push(value);
                if seen.len() >= MIN_DISTINCT_FOR_CONTENT_MATCH {
                    return true;
                }
            }
        }
    }
    false
}

#[derive(Debug, Clone, Copy)]
enum ColumnAlignment {
    Match { base_idx: usize, target_idx: usize },
    Inserted { target_idx: usize },
    Deleted { base_idx: usize },
}

/// Matches `base_cols` against `target_cols` by content, preserving
/// relative order (a weighted longest-common-subsequence alignment over
/// the O(cols²) score matrix `column_match_score` builds) — the same
/// structure validated across every round of Issue #5's PoC investigation,
/// adapted to plain `Vec<Vec<u64>>` DP with no external dependencies.
fn align_columns(
    base_cols: &[ColumnContent],
    target_cols: &[ColumnContent],
) -> Vec<ColumnAlignment> {
    let n = base_cols.len();
    let m = target_cols.len();

    let mut scores = vec![vec![0u64; m]; n];
    for (i, b) in base_cols.iter().enumerate() {
        for (j, t) in target_cols.iter().enumerate() {
            if let Some(score) = column_match_score(b, t) {
                scores[i][j] = score;
            }
        }
    }

    let mut dp = vec![vec![0u64; m + 1]; n + 1];
    for i in 0..n {
        for j in 0..m {
            let score = scores[i][j];
            dp[i + 1][j + 1] = if score > 0 {
                dp[i][j] + score
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }

    let mut aligned = Vec::new();
    let mut i = n;
    let mut j = m;
    while i > 0 || j > 0 {
        if i > 0 && j > 0 {
            let score = scores[i - 1][j - 1];
            if score > 0 && dp[i][j] == dp[i - 1][j - 1] + score {
                aligned.push(ColumnAlignment::Match {
                    base_idx: i - 1,
                    target_idx: j - 1,
                });
                i -= 1;
                j -= 1;
                continue;
            }
        }
        if j > 0 && (i == 0 || dp[i][j - 1] >= dp[i - 1][j]) {
            aligned.push(ColumnAlignment::Inserted { target_idx: j - 1 });
            j -= 1;
        } else {
            aligned.push(ColumnAlignment::Deleted { base_idx: i - 1 });
            i -= 1;
        }
    }
    aligned.reverse();
    aligned
}

/// Scores a candidate `(base, target)` column pair, or `None` if they
/// aren't a candidate at all. See this module's doc comment ("Matching
/// heuristic") for the reasoning behind the header-match escape hatch and
/// the `MIN_DISTINCT_FOR_CONTENT_MATCH` gate.
fn column_match_score(b: &ColumnContent, t: &ColumnContent) -> Option<u64> {
    let header_match = matches!((b.header, t.header), (Some(bh), Some(th)) if bh == th);
    if !header_match && !(b.eligible_for_content_match && t.eligible_for_content_match) {
        return None;
    }

    let match_count = count_matching_rows(b, t);
    if header_match {
        return Some(match_count + HEADER_MATCH_BONUS);
    }

    let min_len = b.cells.len().min(t.cells.len()) as u64;
    let required = ((min_len as f64 * 0.2).ceil() as u64).max(2);
    (match_count >= required).then_some(match_count)
}

/// Counts rows where both columns have a cell and its value matches
/// (style is deliberately ignored here — it's a much weaker identity
/// signal than the value itself, and comparing it would only add cost).
/// Merge-joins the two already row-sorted `cells` slices (see
/// `ColumnContent::cells`'s doc comment) in O(b.len() + t.len()), rather
/// than looking up each of `b`'s rows in `t` one at a time — this runs
/// once per *candidate pair*, so an accidental extra O(log rows) factor
/// here directly multiplies the whole O(cols²) matching pass's cost
/// (measured impact: switching this from `BTreeMap::get` lookups to a
/// merge-join was the single biggest factor in deriving
/// `MAX_COLUMN_ALIGNMENT_COST`'s real value, since it removed the
/// log(rows) term entirely).
fn count_matching_rows(b: &ColumnContent, t: &ColumnContent) -> u64 {
    let mut count = 0u64;
    let mut bi = b.cells.iter().copied().peekable();
    let mut ti = t.cells.iter().copied().peekable();
    while let (Some(&(b_row, b_cell)), Some(&(t_row, t_cell))) = (bi.peek(), ti.peek()) {
        match b_row.cmp(&t_row) {
            Ordering::Less => {
                bi.next();
            }
            Ordering::Greater => {
                ti.next();
            }
            Ordering::Equal => {
                if b_cell.value == t_cell.value {
                    count += 1;
                }
                bi.next();
                ti.next();
            }
        }
    }
    count
}

/// Diffs one matched `(base, target)` column pair by row, merge-join style
/// (the same approach `diff::engine::diff_cells` uses across a whole
/// sheet, applied here within a single column pair).
fn diff_matched_columns(b: &ColumnContent, t: &ColumnContent, out: &mut Vec<CellDiff>) {
    let mut bi = b.cells.iter().copied().peekable();
    let mut ti = t.cells.iter().copied().peekable();

    loop {
        match (bi.peek(), ti.peek()) {
            (Some(&(b_row, b_cell)), Some(&(t_row, t_cell))) => match b_row.cmp(&t_row) {
                Ordering::Less => {
                    out.push(cell_diff_deleted_aligned(b_row, b.col, b_cell));
                    bi.next();
                }
                Ordering::Greater => {
                    out.push(cell_diff_added_aligned(t_row, t.col, t_cell));
                    ti.next();
                }
                Ordering::Equal => {
                    if b_cell.value != t_cell.value || b_cell.style != t_cell.style {
                        out.push(cell_diff_modified_aligned(
                            b_row, b.col, t.col, b_cell, t_cell,
                        ));
                    }
                    bi.next();
                    ti.next();
                }
            },
            (Some(&(b_row, b_cell)), None) => {
                out.push(cell_diff_deleted_aligned(b_row, b.col, b_cell));
                bi.next();
            }
            (None, Some(&(t_row, t_cell))) => {
                out.push(cell_diff_added_aligned(t_row, t.col, t_cell));
                ti.next();
            }
            (None, None) => break,
        }
    }
}

fn cell_diff_added_aligned(row: u32, col: u32, new: &Cell) -> CellDiff {
    CellDiff {
        row,
        col,
        status: DiffStatus::Added,
        old_col: None,
        old_value: None,
        new_value: Some(cell_value_to_json(new.value.as_ref())),
        old_style: None,
        new_style: new.style.as_deref().map(style_to_json),
    }
}

fn cell_diff_deleted_aligned(row: u32, col: u32, old: &Cell) -> CellDiff {
    CellDiff {
        row,
        col,
        status: DiffStatus::Deleted,
        old_col: None,
        old_value: Some(cell_value_to_json(old.value.as_ref())),
        new_value: None,
        old_style: old.style.as_deref().map(style_to_json),
        new_style: None,
    }
}

/// See `CellDiff::old_col`'s doc comment for why it's only populated when
/// `old_col != new_col`.
fn cell_diff_modified_aligned(
    row: u32,
    old_col: u32,
    new_col: u32,
    old: &Cell,
    new: &Cell,
) -> CellDiff {
    let style_changed = old.style != new.style;
    CellDiff {
        row,
        col: new_col,
        status: DiffStatus::Modified,
        old_col: (old_col != new_col).then_some(old_col),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CellRef, SheetVisibility};

    fn sheet_with_cells(name: &str, cells: &[(u32, u32, f64)]) -> Sheet {
        let mut sheet = Sheet::new(name.to_string(), SheetVisibility::Visible);
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

    /// A rich (≥8 distinct values), unambiguously-identifiable column so
    /// tests can focus on the alignment logic itself without also having
    /// to reason about the low-cardinality safety gate.
    fn rich_column(col: u32, offset: f64) -> Vec<(u32, u32, f64)> {
        (1..=10)
            .map(|row| (row, col, row as f64 + offset))
            .collect()
    }

    #[test]
    fn column_insertion_does_not_cascade_when_aligned() {
        // Direct counterpart to engine.rs's
        // column_insertion_cascades_into_shift_diffs_by_design, proving
        // the opposite outcome under alignment: base col 1 shifts to
        // target col 2 unchanged, and only the genuinely new column 1 (and
        // any real value change) is reported.
        let mut base_cells = rich_column(1, 0.0);
        base_cells.extend(rich_column(2, 100.0));
        let base = workbook(vec![sheet_with_cells("Sheet1", &base_cells)]);

        // Target: a brand new column 1 inserted, old columns 1/2 shifted
        // to 2/3 unchanged.
        let mut target_cells = rich_column(1, 500.0);
        target_cells.extend(rich_column(2, 0.0));
        target_cells.extend(rich_column(3, 100.0));
        let target = workbook(vec![sheet_with_cells("Sheet1", &target_cells)]);

        let diff = diff_workbooks_aligned_columns(&base, &target, ColumnAlignmentLimits::default())
            .unwrap();
        assert_eq!(diff.sheets.len(), 1);
        let cells = &diff.sheets[0].cells;
        // Only the 10 cells of the newly inserted column should appear —
        // the two shifted-but-unchanged columns produce no diff at all.
        assert_eq!(cells.len(), 10);
        assert!(cells
            .iter()
            .all(|c| c.col == 1 && c.status == DiffStatus::Added));
    }

    #[test]
    fn column_deletion_does_not_cascade_when_aligned() {
        let mut base_cells = rich_column(1, 500.0);
        base_cells.extend(rich_column(2, 0.0));
        base_cells.extend(rich_column(3, 100.0));
        let base = workbook(vec![sheet_with_cells("Sheet1", &base_cells)]);

        let mut target_cells = rich_column(1, 0.0);
        target_cells.extend(rich_column(2, 100.0));
        let target = workbook(vec![sheet_with_cells("Sheet1", &target_cells)]);

        let diff = diff_workbooks_aligned_columns(&base, &target, ColumnAlignmentLimits::default())
            .unwrap();
        let cells = &diff.sheets[0].cells;
        assert_eq!(cells.len(), 10);
        assert!(cells
            .iter()
            .all(|c| c.col == 1 && c.status == DiffStatus::Deleted));
    }

    #[test]
    fn genuine_modification_survives_column_alignment() {
        let mut base_cells = rich_column(1, 500.0); // column that gets deleted
        base_cells.extend(rich_column(2, 0.0));
        let base = workbook(vec![sheet_with_cells("Sheet1", &base_cells)]);

        let mut target_cells = rich_column(1, 0.0);
        // Change one cell's value within the shifted column.
        target_cells.retain(|&(r, _, _)| r != 5);
        target_cells.push((5, 1, 999.0));
        let target = workbook(vec![sheet_with_cells("Sheet1", &target_cells)]);

        let diff = diff_workbooks_aligned_columns(&base, &target, ColumnAlignmentLimits::default())
            .unwrap();
        let cells = &diff.sheets[0].cells;
        let modified: Vec<_> = cells
            .iter()
            .filter(|c| c.status == DiffStatus::Modified)
            .collect();
        assert_eq!(modified.len(), 1);
        assert_eq!(modified[0].row, 5);
        assert_eq!(modified[0].col, 1);
        // The column shifted from base col 2 to target col 1.
        assert_eq!(modified[0].old_col, Some(2));
    }

    #[test]
    fn old_col_is_absent_when_the_matched_column_did_not_shift() {
        let mut base_cells = rich_column(1, 0.0);
        base_cells.retain(|&(r, _, _)| r != 5);
        base_cells.push((5, 1, 5.0));
        let base = workbook(vec![sheet_with_cells("Sheet1", &base_cells)]);

        let mut target_cells = rich_column(1, 0.0);
        target_cells.retain(|&(r, _, _)| r != 5);
        target_cells.push((5, 1, 999.0));
        let target = workbook(vec![sheet_with_cells("Sheet1", &target_cells)]);

        let diff = diff_workbooks_aligned_columns(&base, &target, ColumnAlignmentLimits::default())
            .unwrap();
        let cells = &diff.sheets[0].cells;
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].status, DiffStatus::Modified);
        assert_eq!(cells[0].old_col, None);
    }

    #[test]
    fn low_cardinality_headerless_columns_fall_back_to_coordinate_diff_safely() {
        // Documents the deliberate tradeoff from this module's doc comment
        // ("Matching heuristic"): two boolean-valued columns with no
        // header are never aligned, regardless of any coincidental
        // content overlap — they're diffed as plain coordinate-based
        // Added/Deleted, the same safe default diff_workbooks would give.
        let base = workbook(vec![sheet_with_cells(
            "Sheet1",
            &[(1, 1, 1.0), (2, 1, 0.0), (3, 1, 1.0)],
        )]);
        let target = workbook(vec![sheet_with_cells(
            "Sheet1",
            // A new boolean column 1 inserted; the old boolean column
            // shifted to 2 unchanged.
            &[
                (1, 1, 0.0),
                (2, 1, 1.0),
                (3, 1, 0.0),
                (1, 2, 1.0),
                (2, 2, 0.0),
                (3, 2, 1.0),
            ],
        )]);

        let diff = diff_workbooks_aligned_columns(&base, &target, ColumnAlignmentLimits::default())
            .unwrap();
        let cells = &diff.sheets[0].cells;
        // No alignment attempted: every target cell is reported relative
        // to the coordinate-based default (3 unmatched base cells deleted,
        // 6 target cells added) rather than any column being recognized as
        // "shifted, unchanged".
        assert_eq!(cells.len(), 9);
        assert!(cells.iter().all(|c| c.old_col.is_none()));
    }

    #[test]
    fn header_match_rescues_low_cardinality_column_alignment() {
        // Positive counterpart: the same low-cardinality shift as above,
        // but with a header row present, aligns correctly.
        let base = workbook(vec![sheet_with_values(
            "Sheet1",
            &[
                (1, 1, CellValue::Text("Active".into())),
                (2, 1, CellValue::Number(1.0)),
                (3, 1, CellValue::Number(0.0)),
                (4, 1, CellValue::Number(1.0)),
            ],
        )]);

        let target = workbook(vec![sheet_with_values(
            "Sheet1",
            &[
                (1, 1, CellValue::Text("New".into())),
                (2, 1, CellValue::Number(9.0)),
                (3, 1, CellValue::Number(9.0)),
                (1, 2, CellValue::Text("Active".into())),
                (2, 2, CellValue::Number(1.0)),
                (3, 2, CellValue::Number(0.0)),
                (4, 2, CellValue::Number(1.0)),
            ],
        )]);

        let diff = diff_workbooks_aligned_columns(&base, &target, ColumnAlignmentLimits::default())
            .unwrap();
        let cells = &diff.sheets[0].cells;
        // The "Active" column shifted from 1 to 2 with no data changes;
        // only the brand new "New" column's 3 cells should be reported.
        assert_eq!(cells.len(), 3);
        assert!(cells
            .iter()
            .all(|c| c.col == 1 && c.status == DiffStatus::Added));
    }

    #[test]
    fn distinct_column_cost_over_the_limit_is_too_many_distinct_columns_for_alignment() {
        // 10 distinct base cols * 10 distinct target cols * 100 rows =
        // 10,000 > limit of 9,999.
        let base_cells: Vec<(u32, u32, f64)> = (1..=10)
            .flat_map(|col| (1..=100).map(move |row| (row, col, row as f64)))
            .collect();
        let base = workbook(vec![sheet_with_cells("Sheet1", &base_cells)]);
        let target = workbook(vec![sheet_with_cells("Sheet1", &base_cells)]);

        let limits = ColumnAlignmentLimits { max_cost: 9_999 };
        let err = diff_workbooks_aligned_columns(&base, &target, limits).unwrap_err();
        match err {
            Error::TooManyDistinctColumnsForAlignment { count, limit } => {
                assert_eq!(count, 10_000);
                assert_eq!(limit, 9_999);
            }
            other => panic!("expected TooManyDistinctColumnsForAlignment, got {other:?}"),
        }
    }

    #[test]
    fn diff_workbooks_default_behavior_is_unaffected_by_alignment_existing() {
        // Guards against accidental shared-state coupling: the plain
        // diff_workbooks entry point still cascades exactly as documented,
        // unaffected by diff_workbooks_aligned_columns existing.
        let base = workbook(vec![sheet_with_cells(
            "Sheet1",
            &[(1, 1, 10.0), (1, 2, 20.0)],
        )]);
        let target = workbook(vec![sheet_with_cells(
            "Sheet1",
            &[(1, 1, 99.0), (1, 2, 10.0), (1, 3, 20.0)],
        )]);

        let diff = crate::diff::engine::diff_workbooks(&base, &target);
        assert_eq!(diff.sheets[0].cells.len(), 3);
    }
}
