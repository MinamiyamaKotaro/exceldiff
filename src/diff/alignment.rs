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
//! that many distinct values is excluded from *partial*-overlap
//! content-based matching entirely unless it has an exact header match.
//! It can still be matched via one narrow exception: an **exact** match
//! (every populated row present on both sides with an equal value, same
//! length, *and* at least 2 distinct values among them) is accepted even
//! here, since the odds of two unrelated low-cardinality columns agreeing
//! on *every* row by chance are negligible — this is what lets a
//! low-cardinality column that merely shifted, with no actual content
//! change, still produce no diff rather than a wholesale delete-and-re-add
//! (see `column_match_score`'s doc comment). The "≥2 distinct values" part
//! is load-bearing, not incidental: a *constant* column (every populated
//! cell the same single value) matches any other column holding that same
//! constant with 100% certainty, not negligible odds, so without this it
//! could silently swallow a genuine change (e.g. a column that actually
//! changed from all `0` to all `1` matched against an unrelated new column
//! that happens to still be all `0`) — a Copilot review caught this
//! concretely. Anything short of the full requirement — a genuinely
//! changed, too-short-to-trust, or constant low-cardinality column — is
//! left unmatched, falling through to ordinary per-column coordinate
//! diffing (the safe default rather than a wrong guess).
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

/// A column is only eligible for *partial*-overlap content-based matching
/// (i.e. matching that doesn't require an exact header match, and doesn't
/// require every single row to agree) once it holds at least this many
/// *distinct* populated values. Below this floor, unrelated columns clear
/// any content-similarity bar too easily by chance — see this module's doc
/// comment ("Matching heuristic") for the measured evidence. Verified
/// directly (PoC round 4's `min_distinct` sweep): 8 cleanly separates the
/// safe case (≥10 distinct values: 100% match accuracy) from the dangerous
/// one (2-4 distinct values: up to 122% false-match rate), with zero
/// observed regression on normal-cardinality columns.
///
/// `column_match_score` also reuses this same value as a minimum sample
/// size for its *exact*-match rescue below the eligibility floor — see
/// that function's doc comment.
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
/// per-column setup work (`column_contents`/`has_at_least_n_distinct_values`)
/// stops being negligible next to the O(cols² × rows) matching work. This
/// cap uses the worst observed rate (~2.4e-5 ms/unit) with headroom,
/// keeping the worst case comfortably inside the same "few hundred ms"
/// budget class `MAX_MERGE_REGIONS` targets (10,000,000 × 2.4e-5 ≈ 240ms).
pub(crate) const MAX_COLUMN_ALIGNMENT_COST: usize = 10_000_000;

/// Cap on `distinct_cols_base * distinct_cols_target` alone (no row
/// factor), bounding the *memory* `align_columns`'s `scores`/`dp` matrices
/// need — independent of `MAX_COLUMN_ALIGNMENT_COST`, which only bounds
/// matching *time*. These are genuinely different constraints: a one-row
/// sheet with ~3,162 distinct columns per side has a rows-weighted cost of
/// only ~10,000,000 (well within budget) yet would still allocate two
/// `Vec<Vec<u64>>` matrices of roughly `cols² * 8 bytes` each — about
/// 160MB combined — before any matching even runs, regardless of how few
/// rows exist to compare. Both `scores` (`cols_base * cols_target * 8`
/// bytes) and `dp` (`(cols_base+1) * (cols_target+1) * 8` bytes, roughly
/// the same order) are allocated up front, so capping their combined size
/// at `MAX_COLUMN_PAIR_COUNT * 16` bytes keeps the worst case around 16MB
/// — a bound picked independently of the time budget specifically because
/// a sheet can clear one without clearing the other.
pub(crate) const MAX_COLUMN_PAIR_COUNT: usize = 1_000_000;

/// Configuration for [`diff_workbooks_aligned_columns`]. A plain struct
/// parameter (not a builder, not `Option<T>`), matching this crate's
/// existing `SizeLimits`/`parse_workbook_with_limits` convention.
#[derive(Debug, Clone, Copy)]
pub struct ColumnAlignmentLimits {
    /// Cap on `distinct_cols_base * distinct_cols_target * sample_rows`
    /// (bounds matching *time*). Defaults to `MAX_COLUMN_ALIGNMENT_COST` —
    /// see that constant's doc comment for how it was measured.
    pub max_cost: usize,
    /// Cap on `distinct_cols_base * distinct_cols_target` alone (bounds
    /// score-matrix *memory*, independent of row count). Defaults to
    /// `MAX_COLUMN_PAIR_COUNT` — see that constant's doc comment for why
    /// this can't simply be folded into `max_cost`.
    pub max_column_pairs: usize,
}

impl Default for ColumnAlignmentLimits {
    fn default() -> Self {
        ColumnAlignmentLimits {
            max_cost: MAX_COLUMN_ALIGNMENT_COST,
            max_column_pairs: MAX_COLUMN_PAIR_COUNT,
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
/// Returns `Err(Error::ColumnAlignmentCostTooHigh)`, fail-fast and before
/// any O(cols²) matching work, when a sheet's alignment cost would exceed
/// either `limits.max_cost` (matching time) or `limits.max_column_pairs`
/// (score-matrix memory, checked independently since it doesn't scale with
/// row count — see `MAX_COLUMN_PAIR_COUNT`'s doc comment) — deliberately
/// not a silent fallback to `diff_workbooks`'s coordinate-based result,
/// since the caller opted into alignment explicitly (see
/// `ColumnAlignmentLimits`'s doc comment). A caller that wants automatic
/// fallback can catch this error and call `diff_workbooks` itself.
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
    /// Number of entries in `cells` that actually carry a value
    /// (`value.is_some()`), as opposed to `cells.len()`, which also counts
    /// formatting-only blanks. `column_match_score` uses this — not
    /// `cells.len()` — anywhere it needs "how much real evidence does this
    /// column offer," since a column can carry an arbitrarily large
    /// formatted-but-blank range (e.g. preset borders/backgrounds across
    /// an entire imported table's unused rows) without that inflating the
    /// sample it's safe to draw conclusions from.
    populated_count: usize,
    /// Whether this column holds at least 2 distinct values — i.e. is
    /// *not* constant. `column_match_score`'s exact-match rescue requires
    /// this from `b` before ever attempting `columns_are_exact_match`: a
    /// constant column (every populated cell the same single value) isn't
    /// a probabilistic near-miss the way a genuine low-cardinality column
    /// is — it matches *any other* column holding that same constant with
    /// 100% certainty, not the "negligible by chance" odds
    /// `MIN_DISTINCT_FOR_CONTENT_MATCH`'s doc comment relies on. Without
    /// this guard, a column whose values genuinely changed (e.g. shifted
    /// from all `0` to all `1`) could be silently matched against an
    /// unrelated new column that happens to still be all `0`, hiding the
    /// real change entirely (a Copilot review caught this concretely).
    has_at_least_two_distinct_values: bool,
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

    // Memory bound first: independent of row count, so it must be checked
    // even when the rows-weighted time budget below would otherwise pass
    // (see MAX_COLUMN_PAIR_COUNT's doc comment for why a low-row, many-column
    // sheet needs this separately).
    let pair_count = base_cols.len().saturating_mul(target_cols.len());
    if pair_count > limits.max_column_pairs {
        return Err(Error::ColumnAlignmentCostTooHigh {
            cost: pair_count,
            limit: limits.max_column_pairs,
        });
    }

    let sample_rows = base.max_row.max(target.max_row) as usize;
    let cost = pair_count.saturating_mul(sample_rows);
    if cost > limits.max_cost {
        return Err(Error::ColumnAlignmentCostTooHigh {
            cost,
            limit: limits.max_cost,
        });
    }

    let alignments = align_columns(&base_cols, &target_cols);

    // Reconcile columns the content-based DP couldn't explain (left as
    // Deleted on one side, Inserted on the other) but that share the
    // exact same column index: a purely positional, coordinate-based
    // fallback for whatever content matching couldn't align — e.g. a
    // low-cardinality column that never shifted at all (nothing else
    // moved around it) but has one real cell change no content score can
    // distinguish from an unrelated column. Without this, such a column
    // was reported as a full delete-plus-re-add instead of the single
    // Modified cell `diff_workbooks` would report for the identical
    // input (a Copilot review caught this concretely) — worse than not
    // aligning at all, and a direct contradiction of this module's own
    // documented "falls through to ordinary per-column coordinate
    // diffing" contract for unmatched columns. Only genuinely unmatched
    // columns reach this; anything the DP already matched by content
    // keeps that result.
    let mut deleted_by_col: BTreeMap<u32, usize> = BTreeMap::new();
    let mut inserted_by_col: BTreeMap<u32, usize> = BTreeMap::new();
    for &alignment in &alignments {
        match alignment {
            ColumnAlignment::Deleted { base_idx } => {
                deleted_by_col.insert(base_cols[base_idx].col, base_idx);
            }
            ColumnAlignment::Inserted { target_idx } => {
                inserted_by_col.insert(target_cols[target_idx].col, target_idx);
            }
            ColumnAlignment::Match { .. } => {}
        }
    }
    let coordinate_pairs: BTreeMap<u32, usize> = deleted_by_col
        .iter()
        .filter(|(col, _)| inserted_by_col.contains_key(col))
        .map(|(&col, &base_idx)| (col, base_idx))
        .collect();

    let mut cells = Vec::new();
    for alignment in &alignments {
        match *alignment {
            ColumnAlignment::Match {
                base_idx,
                target_idx,
            } => diff_matched_columns(&base_cols[base_idx], &target_cols[target_idx], &mut cells),
            ColumnAlignment::Inserted { target_idx } => {
                let t = &target_cols[target_idx];
                if let Some(&base_idx) = coordinate_pairs.get(&t.col) {
                    diff_matched_columns(&base_cols[base_idx], t, &mut cells);
                } else {
                    for &(row, cell) in &t.cells {
                        cells.push(cell_diff_added_aligned(row, t.col, cell));
                    }
                }
            }
            ColumnAlignment::Deleted { base_idx } => {
                let b = &base_cols[base_idx];
                if !coordinate_pairs.contains_key(&b.col) {
                    for &(row, cell) in &b.cells {
                        cells.push(cell_diff_deleted_aligned(row, b.col, cell));
                    }
                }
                // Else: already emitted once via the Inserted branch above.
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
            let eligible_for_content_match =
                has_at_least_n_distinct_values(&cells, MIN_DISTINCT_FOR_CONTENT_MATCH);
            let has_at_least_two_distinct_values = has_at_least_n_distinct_values(&cells, 2);
            let populated_count = cells.iter().filter(|(_, c)| c.value.is_some()).count();
            ColumnContent {
                col,
                cells,
                header,
                eligible_for_content_match,
                populated_count,
                has_at_least_two_distinct_values,
            }
        })
        .collect()
}

/// Whether `cells` holds at least `n` distinct values. Stops scanning as
/// soon as the floor is reached — the caller only needs a yes/no answer,
/// not an exact count — and caps its own working set at that same floor,
/// so this stays cheap (effectively O(rows × n), not O(rows²)) even on a
/// column with many populated rows. `CellValue` cannot derive `Hash` (its
/// `Number(f64)` variant), so distinctness is checked by linear scan
/// against the capped working set rather than a `HashSet`.
fn has_at_least_n_distinct_values(cells: &[(u32, &Cell)], n: usize) -> bool {
    let mut seen: Vec<&CellValue> = Vec::with_capacity(n);
    for &(_, cell) in cells {
        if let Some(value) = cell.value.as_ref() {
            if !seen.contains(&value) {
                seen.push(value);
                if seen.len() >= n {
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

    // Standard weighted-LCS recurrence: dp[i+1][j+1] is the best of taking
    // the (i,j) match (if it's a candidate at all) or skipping either
    // side — *not* an unconditional diagonal step whenever a match
    // exists. Taking the diagonal unconditionally would let a weak match
    // (e.g. score 2) block a much stronger one available by skipping past
    // it first (e.g. score 10 one column further along), silently
    // reporting the correct match as a spurious Insert/Delete pair.
    let mut dp = vec![vec![0u64; m + 1]; n + 1];
    for i in 0..n {
        for j in 0..m {
            let score = scores[i][j];
            let diagonal = if score > 0 { dp[i][j] + score } else { 0 };
            dp[i + 1][j + 1] = diagonal.max(dp[i + 1][j]).max(dp[i][j + 1]);
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
    let match_count = count_matching_rows(b, t);

    if header_match {
        return Some(match_count + HEADER_MATCH_BONUS);
    }

    if b.eligible_for_content_match && t.eligible_for_content_match {
        // Populated-value count, not cells.len(): a column can carry a
        // large formatted-but-blank range that inflates cells.len() far
        // beyond how much real data it holds (see populated_count's doc
        // comment), which would otherwise raise this threshold well
        // beyond what the *actual* matching evidence could ever clear.
        let min_len = b.populated_count.min(t.populated_count) as u64;
        let required = ((min_len as f64 * 0.2).ceil() as u64).max(2);
        return (match_count >= required).then_some(match_count);
    }

    // Below MIN_DISTINCT_FOR_CONTENT_MATCH, no *partial* overlap can be
    // trusted (see this module's doc comment) — but flatly refusing to
    // match here at all means a low-cardinality column that merely
    // shifted, with *zero* actual content change, gets reported as a
    // wholesale delete-and-re-add instead of no diff at all, which is
    // exactly the cascade this feature exists to avoid. An EXACT match —
    // every populated row present on both sides with an equal value, and
    // no extra rows on either side — is safe to accept even at low
    // cardinality: the odds of two truly unrelated low-cardinality
    // columns agreeing on *every single row* by chance are negligible for
    // any column long enough to matter (e.g. even a 2-valued/boolean
    // column needs only `MIN_DISTINCT_FOR_CONTENT_MATCH` populated rows —
    // reused here as a minimum sample size, not just a distinct-value
    // floor — to bring that chance below 1/256). A column too short to
    // clear that floor is left unmatched, the same safe default as
    // before.
    // Populated-value count again, for the same reason as the threshold
    // path above: MIN_DISTINCT_FOR_CONTENT_MATCH's "brings the chance of
    // an accidental full match below 1/256" claim is about real values to
    // compare, not the raw number of cell-map entries (which formatting
    // alone can inflate without adding any actual comparable content).
    let long_enough = b.populated_count >= MIN_DISTINCT_FOR_CONTENT_MATCH;
    // A constant column (every populated cell the same single value)
    // isn't a probabilistic near-miss the "negligible by chance" argument
    // above covers — it matches *any other* column holding that same
    // constant with 100% certainty, not 1-in-256 odds. Requiring real
    // variation (≥2 distinct values) before ever attempting the exact
    // match rules that out: see `ColumnContent::has_at_least_two_distinct_values`'s
    // doc comment for the concrete failure this guards against (a
    // genuinely changed column silently matched to an unrelated new one
    // that happens to still hold the old constant value).
    let varied_enough = b.has_at_least_two_distinct_values;
    if !(long_enough && varied_enough && columns_are_exact_match(b, t)) {
        return None;
    }
    // Scored by populated_count, not cells.len(): the DP picks whichever
    // candidate scores highest, so scoring this on raw cell-map size
    // (inflatable by an arbitrarily large formatted-but-blank range)
    // could let a padded exact match outrank a smaller but genuine
    // threshold-path match for a competing column pair.
    Some(b.populated_count as u64)
}

/// Whether `b` and `t` are byte-for-byte identical: same row keys in the
/// same order, same value at every row. Deliberately a *different* check
/// from `count_matching_rows == len` — this counts two formatting-only
/// (`value: None`) cells at the same row as consistent (a real part of
/// being identical), where `count_matching_rows` deliberately does not
/// (see that function's doc comment): the two checks serve different
/// purposes. `count_matching_rows`'s exclusion exists to stop a
/// *coincidental* shared blank range from inflating a *partial*-overlap
/// score; that risk doesn't apply here, since this function already
/// requires every single row to agree before returning `true` at all.
fn columns_are_exact_match(b: &ColumnContent, t: &ColumnContent) -> bool {
    if b.cells.len() != t.cells.len() {
        return false;
    }
    b.cells
        .iter()
        .zip(t.cells.iter())
        .all(|(&(b_row, b_cell), &(t_row, t_cell))| b_row == t_row && b_cell.value == t_cell.value)
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
                // A cell can exist with `value: None` (formatting-only —
                // see `Cell::value`'s doc comment). `Option<CellValue>`'s
                // derived equality means `None == None`, so without the
                // `is_some()` guard two formatting-only blank cells at the
                // same row would count as a "matching" value despite
                // neither side actually holding any value to compare —
                // letting a shared blank range alone push unrelated
                // columns over the matching threshold.
                if b_cell.value.is_some() && b_cell.value == t_cell.value {
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
    fn unmatched_low_cardinality_column_at_the_same_coordinate_falls_back_to_coordinate_diffing() {
        // Regression test for a Copilot review finding: a headerless
        // low-cardinality column that never shifted at all (stays at the
        // same column index on both sides, nothing else moved) but has
        // one real cell change has no content score at all (too
        // low-cardinality for the threshold path, not an exact match).
        // Before align_sheet_columns's coordinate-based fallback for
        // same-index unmatched columns, this was reported as the whole
        // column deleted-and-re-added (16 cells) instead of the single
        // Modified cell diff_workbooks would report for identical input —
        // worse than not aligning at all, and a direct contradiction of
        // this module's own documented "falls through to ordinary
        // per-column coordinate diffing" contract.
        let base = workbook(vec![sheet_with_cells("Sheet1", &boolean_column(1, 0))]);
        let mut target_cells = boolean_column(1, 0);
        target_cells[2].2 = 1.0 - target_cells[2].2; // flip row 3's value only
        let target = workbook(vec![sheet_with_cells("Sheet1", &target_cells)]);

        let diff = diff_workbooks_aligned_columns(&base, &target, ColumnAlignmentLimits::default())
            .unwrap();
        let cells = &diff.sheets[0].cells;
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].status, DiffStatus::Modified);
        assert_eq!(cells[0].row, 3);
        assert_eq!(cells[0].col, 1);
        assert_eq!(cells[0].old_col, None); // same coordinate, not a shift
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
        // header, too short (3 rows) to clear either the partial-overlap
        // eligibility floor or the exact-match rescue's minimum sample
        // size (both MIN_DISTINCT_FOR_CONTENT_MATCH = 8), are never
        // *content*-aligned. Base column 1 stays unmatched by content, but
        // shares its column index with target column 1, so
        // align_sheet_columns's coordinate-based fallback (see that
        // function's doc comment) diffs them directly by position —
        // exactly what diff_workbooks would do, since neither column
        // moved. Target column 2 (genuinely new at that position, no base
        // counterpart to reconcile with) is reported as a fresh Added
        // column. See
        // shifted_but_unchanged_low_cardinality_headerless_column_is_recognized_via_exact_match
        // below for the longer-column case where a genuine *shift* is
        // recognized via the exact-match rescue instead.
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
        // Column 1 (same position both sides) is diffed coordinate-wise:
        // all 3 rows differ -> 3 Modified. Column 2 is a fresh Added
        // column: 3 more cells. old_col stays None throughout, since
        // nothing was recognized as *shifted* here — only genuinely
        // unmatched, same-position columns and a brand new column.
        assert_eq!(cells.len(), 6);
        assert_eq!(
            cells
                .iter()
                .filter(|c| c.status == DiffStatus::Modified)
                .count(),
            3
        );
        assert_eq!(
            cells
                .iter()
                .filter(|c| c.status == DiffStatus::Added)
                .count(),
            3
        );
        assert!(cells.iter().all(|c| c.old_col.is_none()));
    }

    /// Boolean column, long enough (10 rows) to clear the exact-match
    /// rescue's minimum sample size. `seed` selects between two
    /// complementary (i.e. guaranteed to differ at every row) patterns, so
    /// two columns built with different seeds are never an exact match.
    fn boolean_column(col: u32, seed: u32) -> Vec<(u32, u32, f64)> {
        (1..=10)
            .map(|row| {
                let value = if (row + seed).is_multiple_of(2) {
                    1.0
                } else {
                    0.0
                };
                (row, col, value)
            })
            .collect()
    }

    #[test]
    fn identical_low_cardinality_headerless_column_produces_no_diff() {
        // Regression test for a bug a Copilot review caught (PR #20): the
        // low-cardinality gate originally refused *any* content match
        // below MIN_DISTINCT_FOR_CONTENT_MATCH, including a column that
        // didn't change or shift at all — reporting a wholesale delete +
        // re-add for a genuinely unchanged boolean/low-cardinality column
        // instead of no diff, exactly the cascade this feature exists to
        // avoid. An unshifted, byte-identical low-cardinality column must
        // still produce zero diff.
        let cells = boolean_column(1, 0);
        let base = workbook(vec![sheet_with_cells("Sheet1", &cells)]);
        let target = workbook(vec![sheet_with_cells("Sheet1", &cells)]);

        let diff = diff_workbooks_aligned_columns(&base, &target, ColumnAlignmentLimits::default())
            .unwrap();
        assert!(diff.sheets.is_empty());
    }

    #[test]
    fn different_length_low_cardinality_columns_are_never_an_exact_match() {
        // columns_are_exact_match's length check must reject a pair before
        // ever comparing content — two low-cardinality, headerless columns
        // long enough to reach the exact-match rescue (>= 8 rows) but with
        // *different* row counts can never be identical, so they must fall
        // back to plain coordinate diffing rather than being aligned. A
        // genuine shift (different column indices), so
        // align_sheet_columns's same-position coordinate fallback can't
        // also explain the outcome — this test is specifically about
        // columns_are_exact_match's length check.
        let base = workbook(vec![sheet_with_cells("Sheet1", &boolean_column(1, 0))]);

        let mut target_cells = boolean_column(5, 1); // brand new, unrelated column at a different index
        let mut shifted: Vec<(u32, u32, f64)> = boolean_column(1, 0)
            .into_iter()
            .map(|(row, _, v)| (row, 2, v))
            .collect();
        shifted.push((11, 2, 1.0)); // same 10-row prefix, one row longer
        target_cells.extend(shifted);
        let target = workbook(vec![sheet_with_cells("Sheet1", &target_cells)]);

        let diff = diff_workbooks_aligned_columns(&base, &target, ColumnAlignmentLimits::default())
            .unwrap();
        let cells = &diff.sheets[0].cells;
        // Not an exact match (different lengths): base's 10 cells are
        // wholly Deleted, both target columns (10 and 11 cells) are
        // wholly Added.
        assert_eq!(cells.len(), 31);
        assert!(cells.iter().all(|c| c.old_col.is_none()));
    }

    #[test]
    fn shifted_but_unchanged_low_cardinality_headerless_column_is_recognized_via_exact_match() {
        // Same underlying bug as identical_low_cardinality_headerless_column_produces_no_diff,
        // but with a genuine column shift: the low-cardinality column is
        // byte-identical to its base counterpart, just moved from column 1
        // to column 2 by a brand new (genuinely different-content) column
        // 1 being inserted. The exact full-row match should still let the
        // shifted column be recognized as "shifted, unchanged" rather than
        // falling back to coordinate diffing.
        let shifted = boolean_column(1, 0);
        let base = workbook(vec![sheet_with_cells("Sheet1", &shifted)]);

        let mut target_cells = boolean_column(1, 1); // different pattern (seed 1)
        let shifted_to_col2: Vec<(u32, u32, f64)> =
            shifted.iter().map(|&(row, _, v)| (row, 2, v)).collect();
        target_cells.extend(shifted_to_col2);
        let target = workbook(vec![sheet_with_cells("Sheet1", &target_cells)]);

        let diff = diff_workbooks_aligned_columns(&base, &target, ColumnAlignmentLimits::default())
            .unwrap();
        let cells = &diff.sheets[0].cells;
        // Only the 10 cells of the brand new column 1 should be reported
        // — the shifted-but-unchanged column produces no diff at all.
        assert_eq!(cells.len(), 10);
        assert!(cells
            .iter()
            .all(|c| c.col == 1 && c.status == DiffStatus::Added));
    }

    fn sheet_with_optional_cells(name: &str, cells: &[(u32, u32, Option<CellValue>)]) -> Sheet {
        let mut sheet = Sheet::new(name.to_string(), SheetVisibility::Visible);
        for &(row, col, ref value) in cells {
            sheet.insert_cell(
                CellRef { row, col },
                Cell {
                    value: value.clone(),
                    style: None,
                },
            );
        }
        sheet
    }

    #[test]
    fn formatting_only_blank_cells_do_not_inflate_the_partial_match_threshold() {
        // Regression test for a bug a Copilot review caught (PR #20):
        // Option<CellValue>'s derived equality makes None == None true, so
        // two formatting-only (value: None) cells at the same row were
        // counted as a "matching" value in count_matching_rows, letting a
        // shared blank range alone push unrelated columns over the 20%
        // partial-match threshold. Two genuinely unrelated columns, each
        // with 10 distinct real values (so both clear
        // MIN_DISTINCT_FOR_CONTENT_MATCH and use the threshold path, not
        // the exact-match rescue) plus 3 shared blank rows, must NOT be
        // *content*-aligned: 3 blank-row "matches" alone would have
        // cleared the threshold (20% of 13 total cell entries, rounded
        // up, is 3) before this fix. Uses different column indices on
        // each side so align_sheet_columns's same-position coordinate
        // fallback (see that function's doc comment) can't also explain
        // the outcome — this test is specifically about the content-based
        // threshold, not the coordinate fallback.
        fn column(col: u32, value_offset: f64) -> Vec<(u32, u32, Option<CellValue>)> {
            let mut cells: Vec<(u32, u32, Option<CellValue>)> = (1..=10)
                .map(|row| (row, col, Some(CellValue::Number(row as f64 + value_offset))))
                .collect();
            cells.extend((11..=13).map(|row| (row, col, None)));
            cells
        }

        let base = workbook(vec![sheet_with_optional_cells("Sheet1", &column(1, 0.0))]);
        let target = workbook(vec![sheet_with_optional_cells("Sheet1", &column(2, 500.0))]);

        let diff = diff_workbooks_aligned_columns(&base, &target, ColumnAlignmentLimits::default())
            .unwrap();
        let cells = &diff.sheets[0].cells;
        // Not aligned: every materialized cell on each side (10 real
        // values + 3 formatting-only blanks = 13 per side) is reported
        // via plain coordinate-based Added/Deleted — a formatting-only
        // cell still exists in the sparse cell map, so it still gets a
        // diff entry of its own (an "empty" old/new value) when its whole
        // column is treated as wholesale added/deleted.
        assert_eq!(cells.len(), 26);
        assert!(cells.iter().all(|c| c.old_col.is_none()));
    }

    #[test]
    fn a_large_formatted_blank_range_does_not_raise_the_match_threshold_out_of_reach() {
        // Regression test for a second Copilot review finding on the same
        // PR: `min_len` (the threshold path) and `long_enough` (the
        // exact-match gate) originally used `cells.len()`, which also
        // counts formatting-only blanks — a column can carry an
        // arbitrarily large formatted-but-empty range (e.g. preset
        // borders/backgrounds across an entire imported table's unused
        // rows) without that reflecting any real data. An unchanged,
        // shifted column with exactly 8 real matching values plus 40
        // blank cells must still be recognized as unchanged: before this
        // fix, cells.len() = 48 would have raised the 20%
        // threshold to `ceil(48 * 0.2) = 10`, out of reach for the 8 real
        // matches, wrongly rejecting an otherwise-perfect match.
        fn column_with_blank_range(col: u32) -> Vec<(u32, u32, Option<CellValue>)> {
            let mut cells: Vec<(u32, u32, Option<CellValue>)> = (1..=8u32)
                .map(|row| (row, col, Some(CellValue::Number(row as f64))))
                .collect();
            cells.extend((9..=48).map(|row| (row, col, None)));
            cells
        }

        let base = workbook(vec![sheet_with_optional_cells(
            "Sheet1",
            &column_with_blank_range(1),
        )]);

        // Column 1 shifts to column 2, byte-identical (including the
        // blank range); column 1 in the target is a brand new, unrelated
        // column with no blanks at all.
        let mut target_cells: Vec<(u32, u32, Option<CellValue>)> = (1..=8u32)
            .map(|row| (row, 1u32, Some(CellValue::Number(2000.0 + row as f64))))
            .collect();
        target_cells.extend(
            column_with_blank_range(1)
                .into_iter()
                .map(|(row, _, v)| (row, 2u32, v)),
        );
        let target = workbook(vec![sheet_with_optional_cells("Sheet1", &target_cells)]);

        let diff = diff_workbooks_aligned_columns(&base, &target, ColumnAlignmentLimits::default())
            .unwrap();
        let cells = &diff.sheets[0].cells;
        // Only the 8 cells of the brand new column 1 are reported — the
        // shifted column (with its blank range) produces no diff at all.
        assert_eq!(cells.len(), 8);
        assert!(cells
            .iter()
            .all(|c| c.col == 1 && c.status == DiffStatus::Added));
    }

    #[test]
    fn identical_low_cardinality_column_with_a_shared_blank_cell_still_produces_no_diff() {
        // Complements formatting_only_blank_cells_do_not_inflate_the_partial_match_threshold:
        // the fix there must not overcorrect into treating two genuinely
        // identical columns, that happen to share a blank row, as
        // non-identical. columns_are_exact_match (unlike
        // count_matching_rows) counts a shared blank row as consistent —
        // it already requires every row to agree before returning `true`
        // at all, so the coincidental-match risk that motivated excluding
        // blanks from count_matching_rows doesn't apply here. Genuinely
        // low-cardinality (2 distinct values, boolean-style) so this
        // actually exercises the exact-match rescue branch, not the
        // partial-overlap threshold path (a Copilot review caught an
        // earlier version of this test using 8 *distinct* real values,
        // which made it eligible for — and pass through — the ordinary
        // threshold path instead, without ever reaching
        // has_at_least_two_distinct_values/columns_are_exact_match at
        // all). 8 populated values (not 7) plus the blank, so
        // `populated_count` alone — not the blank cell padding it out —
        // is what clears MIN_DISTINCT_FOR_CONTENT_MATCH; see
        // seven_populated_values_plus_a_blank_does_not_clear_the_exact_match_floor
        // below for the negative case.
        let mut cells: Vec<(u32, u32, Option<CellValue>)> = (1..=8u32)
            .map(|row| {
                let value = if row.is_multiple_of(2) { 1.0 } else { 0.0 };
                (row, 1, Some(CellValue::Number(value)))
            })
            .collect();
        cells.push((9, 1, None));
        let base = workbook(vec![sheet_with_optional_cells("Sheet1", &cells)]);
        let target = workbook(vec![sheet_with_optional_cells("Sheet1", &cells)]);

        let diff = diff_workbooks_aligned_columns(&base, &target, ColumnAlignmentLimits::default())
            .unwrap();
        assert!(diff.sheets.is_empty());
    }

    #[test]
    fn seven_populated_values_plus_a_blank_does_not_clear_the_exact_match_floor() {
        // Negative counterpart to
        // identical_low_cardinality_column_with_a_shared_blank_cell_still_produces_no_diff:
        // only 7 real (boolean) values plus a blank — cells.len() is 8,
        // but populated_count is 7, one short of
        // MIN_DISTINCT_FOR_CONTENT_MATCH. The rescue must stay gated by
        // populated_count, not cells.len(). Uses a genuine column shift
        // (not the same coordinate on both sides) so the *content-based*
        // exact-match rescue is the only mechanism that could explain
        // this pair — the coordinate-based fallback for same-index
        // columns (see align_sheet_columns) doesn't apply here and can't
        // mask a populated_count gate bug the way it would if base and
        // target both used column 1.
        fn low_card_column_with_blank(col: u32) -> Vec<(u32, u32, Option<CellValue>)> {
            let mut cells: Vec<(u32, u32, Option<CellValue>)> = (1..=7u32)
                .map(|row| {
                    let value = if row.is_multiple_of(2) { 1.0 } else { 0.0 };
                    (row, col, Some(CellValue::Number(value)))
                })
                .collect();
            cells.push((8, col, None));
            cells
        }

        let base = workbook(vec![sheet_with_optional_cells(
            "Sheet1",
            &low_card_column_with_blank(1),
        )]);
        let mut target_cells: Vec<(u32, u32, Option<CellValue>)> = (1..=8u32)
            .map(|row| (row, 5u32, Some(CellValue::Number(2000.0 + row as f64))))
            .collect(); // brand new column at a different index, no relation to base
        target_cells.extend(
            low_card_column_with_blank(1)
                .into_iter()
                .map(|(row, _, v)| (row, 2u32, v)),
        ); // base's column shifted to column 2, byte-identical
        let target = workbook(vec![sheet_with_optional_cells("Sheet1", &target_cells)]);

        let diff = diff_workbooks_aligned_columns(&base, &target, ColumnAlignmentLimits::default())
            .unwrap();
        let cells = &diff.sheets[0].cells;
        // Not rescued: base's column (8 cells) is wholly Deleted, and
        // both target columns (8 cells each) are wholly Added — the
        // shifted column isn't recognized as unchanged because 7 real
        // values falls one short of the exact-match rescue's floor.
        assert_eq!(cells.len(), 24);
        assert!(cells.iter().all(|c| c.old_col.is_none()));
    }

    #[test]
    fn constant_column_exact_match_rescue_requires_at_least_two_distinct_values() {
        // Regression test for a Copilot review finding: without a
        // diversity requirement, the exact-match rescue would accept a
        // *constant* low-cardinality column (every populated cell the
        // same single value) against any other constant column holding
        // that same value — not a "negligible by chance" coincidence the
        // way a genuine boolean column's full-row agreement is, but a
        // 100%-certain false positive. Concretely: base column A (index
        // 1) is all `0`; in the target, a brand new *unrelated* column
        // (index 5, deliberately not index 1, so
        // align_sheet_columns's same-position coordinate fallback can't
        // also explain the outcome — this test is specifically about the
        // content-based exact-match rescue) is also all `0`, while the
        // true continuation of base A (shifted to column 2) actually
        // changed to all `1`. Without the fix, base A would wrongly
        // content-match the new all-`0` column, silently hiding that
        // column 2's values changed at all.
        fn constant_column(col: u32, value: f64) -> Vec<(u32, u32, f64)> {
            (1..=8).map(|row| (row, col, value)).collect()
        }

        let base = workbook(vec![sheet_with_cells("Sheet1", &constant_column(1, 0.0))]);

        let mut target_cells = constant_column(5, 0.0); // brand new, unrelated column
        target_cells.extend(constant_column(2, 1.0)); // true continuation of A, values changed
        let target = workbook(vec![sheet_with_cells("Sheet1", &target_cells)]);

        let diff = diff_workbooks_aligned_columns(&base, &target, ColumnAlignmentLimits::default())
            .unwrap();
        let cells = &diff.sheets[0].cells;
        // Not aligned (a constant column offers no genuine identity
        // signal): base's column is wholly Deleted, and both target
        // columns are wholly Added — critically, base A is NOT matched to
        // the new column, which would have hidden the real 0 -> 1 change
        // on what was actually the same column.
        assert_eq!(cells.len(), 24);
        assert!(cells.iter().all(|c| c.old_col.is_none()));
        assert_eq!(
            cells
                .iter()
                .filter(|c| c.status == DiffStatus::Modified)
                .count(),
            0
        );
    }

    #[test]
    fn sheet_present_on_only_one_side_reuses_the_coordinate_engine_through_alignment() {
        // Exercises diff_workbooks_aligned_columns's one-sided-sheet
        // branch, which delegates directly to diff::engine::diff_sheet
        // (see this module's doc comment) — a whole new/gone sheet has
        // nothing to align, so it behaves identically to diff_workbooks.
        let base = workbook(vec![]);
        let target = workbook(vec![sheet_with_cells("New", &[(1, 1, 1.0), (1, 2, 2.0)])]);

        let diff = diff_workbooks_aligned_columns(&base, &target, ColumnAlignmentLimits::default())
            .unwrap();
        assert_eq!(diff.sheets.len(), 1);
        let sheet_diff = &diff.sheets[0];
        assert_eq!(sheet_diff.status, DiffStatus::Added);
        assert_eq!(sheet_diff.cells.len(), 2);
        assert!(sheet_diff
            .cells
            .iter()
            .all(|c| c.status == DiffStatus::Added));
    }

    #[test]
    fn matched_columns_with_sparse_non_overlapping_rows_exercise_every_merge_join_branch() {
        // count_matching_rows and diff_matched_columns each merge-join two
        // row-sorted slices; a header match forces alignment regardless of
        // content, so row presence can be deliberately non-overlapping
        // here to exercise every branch of both merge-joins in one test:
        // rows present on both sides (Equal), rows present on only one
        // side while the other iterator is still non-empty (the Less/
        // Greater arms), and rows left over after one side's iterator is
        // fully exhausted (the trailing (Some, None) and (None, Some)
        // arms) — column 1 ends with base exhausting first, column 2 ends
        // with target exhausting first, so both trailing arms fire.
        let base = workbook(vec![sheet_with_values(
            "Sheet1",
            &[
                (1, 1, CellValue::Text("Header1".into())),
                (3, 1, CellValue::Number(30.0)),
                (5, 1, CellValue::Number(50.0)),
                (6, 1, CellValue::Number(60.0)),
                (8, 1, CellValue::Number(80.0)),
                (1, 2, CellValue::Text("Header2".into())),
                (3, 2, CellValue::Number(1.0)),
            ],
        )]);
        let target = workbook(vec![sheet_with_values(
            "Sheet1",
            &[
                (1, 1, CellValue::Text("Header1".into())),
                (2, 1, CellValue::Number(20.0)),
                (5, 1, CellValue::Number(50.0)),
                (7, 1, CellValue::Number(70.0)),
                (1, 2, CellValue::Text("Header2".into())),
                (3, 2, CellValue::Number(1.0)),
                (9, 2, CellValue::Number(9.0)),
            ],
        )]);

        let diff = diff_workbooks_aligned_columns(&base, &target, ColumnAlignmentLimits::default())
            .unwrap();
        let cells = &diff.sheets[0].cells;
        // Column 1 (base exhausts first): row1 & row5 match (no diff);
        // row2 & row7 are target-only (Added); row3, row6, row8 are
        // base-only (Deleted, row8 via the trailing arm) = 5 cells.
        // Column 2 (target exhausts first): row1 & row3 match (no diff);
        // row9 is target-only (Added, via the trailing arm) = 1 cell.
        assert_eq!(cells.len(), 6);
        let added = cells
            .iter()
            .filter(|c| c.status == DiffStatus::Added)
            .count();
        let deleted = cells
            .iter()
            .filter(|c| c.status == DiffStatus::Deleted)
            .count();
        assert_eq!(added, 3); // col1 rows 2,7 + col2 row9
        assert_eq!(deleted, 3); // col1 rows 3,6,8
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
    fn distinct_column_cost_over_the_limit_is_column_alignment_cost_too_high() {
        // 10 distinct base cols * 10 distinct target cols * 100 rows =
        // 10,000 > limit of 9,999.
        let base_cells: Vec<(u32, u32, f64)> = (1..=10)
            .flat_map(|col| (1..=100).map(move |row| (row, col, row as f64)))
            .collect();
        let base = workbook(vec![sheet_with_cells("Sheet1", &base_cells)]);
        let target = workbook(vec![sheet_with_cells("Sheet1", &base_cells)]);

        let limits = ColumnAlignmentLimits {
            max_cost: 9_999,
            max_column_pairs: MAX_COLUMN_PAIR_COUNT,
        };
        let err = diff_workbooks_aligned_columns(&base, &target, limits).unwrap_err();
        assert!(matches!(
            err,
            Error::ColumnAlignmentCostTooHigh {
                cost: 10_000,
                limit: 9_999
            }
        ));
    }

    #[test]
    fn column_pair_count_over_the_limit_is_column_alignment_cost_too_high_even_with_one_row() {
        // A single row keeps the rows-weighted `max_cost` budget trivially
        // satisfied (cols * cols * 1 row is tiny), but the score-matrix
        // memory bound (cols * cols alone) must still reject an
        // unreasonably wide sheet — this is exactly the gap
        // MAX_COLUMN_PAIR_COUNT closes independently of max_cost.
        let base_cells: Vec<(u32, u32, f64)> =
            (1..=200u32).map(|col| (1, col, col as f64)).collect();
        let base = workbook(vec![sheet_with_cells("Sheet1", &base_cells)]);
        let target = workbook(vec![sheet_with_cells("Sheet1", &base_cells)]);

        let limits = ColumnAlignmentLimits {
            max_cost: usize::MAX,
            max_column_pairs: 39_999, // 200 * 200 = 40,000 > 39,999
        };
        let err = diff_workbooks_aligned_columns(&base, &target, limits).unwrap_err();
        assert!(matches!(
            err,
            Error::ColumnAlignmentCostTooHigh {
                cost: 40_000,
                limit: 39_999
            }
        ));
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
