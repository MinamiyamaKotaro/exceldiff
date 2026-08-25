// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! Opt-in row-insertion/deletion-aware diffing (Issue #4).
//!
//! `diff::engine::diff_workbooks` compares cells strictly by `(row, col)`,
//! so a single inserted/deleted row cascades into spurious `Added`/
//! `Deleted` diffs for every cell below it (see that module's doc comment
//! for the full rationale). `diff_workbooks_aligned_rows` is the capped,
//! explicitly opt-in alternative this crate deferred from Issue #3: it
//! first matches base/target rows by content (not position), then diffs
//! cells within each matched row pair by column — so a row that merely
//! shifted produces no diff at all, and only genuinely changed cells are
//! reported.
//!
//! # Why this is a different algorithm from `diff::col_alignment`'s columns
//!
//! `diff::col_alignment::diff_workbooks_aligned_columns` (Issue #5) matches
//! columns with an O(distinct_cols²) DP, budgeted by
//! `ColumnAlignmentLimits`. That approach does not transfer to rows: Excel
//! permits up to 1,048,576 rows per sheet but only 16,384 columns (see
//! `CellRef`'s doc comment), so a quadratic algorithm that is safely
//! boundable for columns becomes unusable at realistic row counts. The
//! Issue #3 PoC's original row aligner used exactly this O(distinct_rows²)
//! DP and was rejected for that reason (~13s/128MB at a single 4,000-row
//! alignment, extrapolating to hours/tens of GB at 100,000+ rows — see that
//! module's doc comment and Issue #3's PoC comment thread).
//!
//! This module instead uses a hash-anchored patience alignment (the same
//! core idea as `git diff --patience` / Bram Cohen's patience diff),
//! refined across several rounds of PoC investigation
//! (`poc/issue4-poc-v2` through `poc/issue4-poc-v8`, summarized across
//! <https://github.com/MinamiyamaKotaro/exceldiff/issues/4>'s comment
//! thread):
//!
//! 1. **Common prefix/suffix trim** (`O(1)` per matched row): rows whose
//!    content-hash matches at the same position from either end are
//!    matched immediately, without any O(n²) work at all. Real edits are
//!    usually localized, so this alone typically shrinks the "active"
//!    region needing alignment down to just the edited area.
//! 2. **Hash-anchored patience matching over the active region** (`O(n)`
//!    to build, `O(k log k)` to align `k` anchors): a row whose content
//!    hash occurs exactly once on both sides is a safe anchor — the
//!    longest order-preserving subsequence of anchors (via patience-sort
//!    LIS) becomes the confirmed row matches.
//! 3. **Myers diff within each unresolved gap between anchors** (`O((n+m)
//!    × D)`, `D` = that gap's edit distance, capped by
//!    `RowAlignmentLimits::max_gap_myers_d`): resolves rows that share a
//!    hash but aren't globally unique (e.g. a duplicated template row) by
//!    finding the true minimal edit script for that gap alone, not the
//!    whole sheet.
//! 4. **Bounded content-similarity pairing for the leftover
//!    Deleted/Inserted rows** (`O(span²)`, `span` capped by
//!    `CONTENT_SIMILARITY_SPAN_CAP`): a row that both shifted *and* had
//!    some of its cells changed has no exact-hash match at all, so Myers
//!    reports it as a plain delete-then-insert. Within a small,
//!    Myers-resolved leftover span, this pairs a deleted row with the
//!    insert it's most similar to (≥ `CONTENT_SIMILARITY_THRESHOLD`),
//!    recovering a single `Modified` row-diff instead of a wasteful
//!    whole-row replace.
//!
//! # Two lessons from the PoC rounds that shape every design choice below
//!
//! **Never let a step fall back to blind positional pairing.** An early
//! PoC round (`poc/issue4-poc-v2`) recorded Myers' snake (exact-match)
//! steps correctly but then bridged the gaps *between* them by pairing
//! whatever fell at the same relative array index — discarding the actual
//! delete/insert decisions Myers had already computed. This reproduced the
//! exact cascade this whole feature exists to prevent, *even when Myers
//! stayed within its budget* (`found == true`): a duplicate-heavy sheet
//! with edits scattered across many points, not one contiguous burst,
//! produced tens of thousands of false `Modified` cells in a 5,000-row
//! test case. `myers_diff_gap` below decodes every single step of Myers'
//! backtrace (diagonal, vertical, *and* horizontal) directly into
//! `Match`/`Inserted`/`Deleted` — nothing is ever reconstructed by index
//! position.
//!
//! **Never let a per-gap operation scale with the whole gap when it can
//! scale with the edit itself.** A later round (`poc/issue4-poc-v6`)
//! applied content-similarity pairing across an *entire* unresolved span
//! rather than a bounded one, which multiplies Myers' own `O((n+m)×D)`
//! cost by another `O(D)` factor. Measured directly against a single
//! sheet region where an entire block was replaced with unrelated content
//! (so `D` = the whole block, exactly the case a large `max_gap_myers_d`
//! is meant to permit): 29–36× slower than the version without this step,
//! reaching over 9 seconds for one 4,000-row block. `CONTENT_SIMILARITY_SPAN_CAP`
//! below exists specifically to keep this bounded — content-similarity
//! pairing is only cheap and useful for genuinely small, localized edits,
//! not a whole-sheet replacement.
//!
//! # Known limitation: content-similarity pairing is not always unique
//!
//! When two candidate rows are *equally* similar to a deleted row (e.g.
//! several rows share the same descriptive columns and differ only in one
//! value column), which one gets paired depends on the order Myers'
//! backtrace produces them in — an accident of hash/array order, not a
//! genuine identity signal. This was verified directly (PoC-v7/v8): the
//! aggregate `added`/`modified`/`deleted` counts stay correct either way,
//! but *which* row's old value is attributed to *which* row's new value
//! can be ambiguous in this specific tie case. No tie-break rule (index
//! order, position proximity, or otherwise) resolves this in general —
//! it is an inherent limitation of similarity-based matching, not a bug to
//! fix, and is accepted here the same way `diff::col_alignment`'s
//! `MIN_DISTINCT_FOR_CONTENT_MATCH` gate accepts that some genuine
//! low-cardinality changes can't be told apart from coincidence.
//!
//! # Columns are not realigned (Issue #5 is separate, already implemented)
//!
//! This module holds columns coordinate-pinned — a matched row pair is
//! diffed cell-by-cell at the same column index on both sides, the same
//! way `diff::engine::diff_workbooks` compares cells within a row. Running
//! row alignment and column alignment together (e.g. a sheet where both a
//! row and a column were inserted) is out of scope here: see
//! `diff::col_alignment`'s "Rows are not realigned" doc section for the
//! integration points (`diff_matched_columns`'s merge-join,
//! `count_matching_rows`) that would need a row mapping fed through before
//! that combination is safe.
//!
//! # Hashing: randomized, not fixed-seed
//!
//! Row content signatures are hashed with a fresh
//! `std::collections::hash_map::RandomState` per call, not a fixed-seed
//! hasher — the same hash-flooding-resistance property `std::HashMap`
//! already has by default. A fixed seed would let a maliciously crafted
//! workbook be built to produce colliding row signatures on purpose; the
//! failure mode of a collision here is a misaligned row pair (surfaced as
//! ordinary `Added`/`Modified`/`Deleted` cells, not a crash or memory
//! issue), but there is no reason to accept even that risk when avoiding
//! it costs nothing.

use crate::diff::engine::{diff_merges, diff_sheet, visibility_diff};
use crate::diff::model::{CellDiff, DiffStatus, SheetDiff, WorkbookDiff};
use crate::error::{Error, Result};
use crate::json::{cell_value_to_json, style_to_json};
use crate::model::{Cell, CellValue, Sheet, Workbook};
use std::cmp::Ordering;
use std::collections::hash_map::RandomState;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::{BuildHasher, Hash, Hasher};

/// Similarity threshold (fraction of populated columns that must agree)
/// for two rows in a small, Myers-resolved leftover span to be paired as
/// a single `Modified` row rather than left as a separate delete+insert.
/// Verified across the PoC rounds (`poc/issue4-poc-v6` onward) to recover
/// genuine single/multi-row modifications (e.g. a price column changing
/// while every other column stays the same) without this module's doc
/// comment's "Known limitation" section applying any more often than
/// necessary — lower values pair rows on weaker evidence, raising exactly
/// that ambiguity risk; this crate ships the same value the PoC rounds
/// converged on rather than re-deriving it, since no PR round changed it.
const CONTENT_SIMILARITY_THRESHOLD: f64 = 0.40;

/// Cap on `del_count + ins_count` for one contiguous unmatched span
/// (between two Myers snake matches) before content-similarity pairing is
/// skipped in favor of plain `Deleted`/`Inserted` — see this module's doc
/// comment ("Never let a per-gap operation scale with the whole gap") for
/// why an unbounded span is not safe: content-similarity pairing costs
/// O(span²), measured directly (`poc/issue4-poc-v7`) at 29–36× slower than
/// without it once a span reaches a few hundred rows, reaching 9+ seconds
/// for a single 8,000-row span. 64 is generous for any realistic "a
/// handful of rows changed together" edit while keeping the worst case
/// (64² = 4,096 similarity computations) trivial regardless of sheet size.
const CONTENT_SIMILARITY_SPAN_CAP: usize = 64;

/// Default per-gap Myers edit-distance budget (`RowAlignmentLimits::max_gap_myers_d`).
/// See `MAX_ROW_ALIGNMENT_COST`'s doc comment for how this and `max_cost`
/// interact to bound worst-case time.
pub(crate) const DEFAULT_MAX_GAP_MYERS_D: usize = 200;

/// Cap on `2 × max(distinct_rows_base, distinct_rows_target) × max_gap_myers_d`,
/// the worst-case time budget: a single unresolved gap can span nearly the
/// entire active region on both sides (`n_gap + m_gap ≈ 2 × distinct
/// rows`), and Myers costs `O((n_gap + m_gap) × D)` with `D` capped at
/// `max_gap_myers_d` — so this product bounds the worst case regardless of
/// how few (or zero) real anchors a sheet's content offers, exactly the
/// scenario `diff::col_alignment::MAX_COLUMN_ALIGNMENT_COST` budgets for on
/// the column side.
///
/// Measured directly (`poc/issue4-poc-v7`, release build, Apple Silicon):
/// a single fully-disjoint replace block (zero shared row signatures, so
/// Myers must spend its entire budget as pure delete+insert — the
/// worst case this cap exists for) at increasing block size `B`
/// (`n_gap = m_gap = B`, `D = 2B`, cost = `(n_gap + m_gap) × D = 4B²`):
///
/// | B | cost (`4B²`) | measured time | ms/unit |
/// |---:|---:|---:|---:|
/// | 200 | 160,000 | 0.88ms | 5.5e-6 |
/// | 500 | 1,000,000 | 4.62ms | 4.6e-6 |
/// | 1,000 | 4,000,000 | 18.04ms | 4.5e-6 |
/// | 2,000 | 16,000,000 | 74.09ms | 4.6e-6 |
/// | 4,000 | 64,000,000 | 282.44ms | 4.4e-6 |
///
/// Cost-normalized time is close to constant (~4.4e-6 to 5.5e-6 ms/unit).
/// This cap uses the worst observed rate with headroom (6e-6 ms/unit),
/// keeping the worst case inside the same "few hundred ms" budget class
/// `MAX_COLUMN_ALIGNMENT_COST` targets (50,000,000 × 6e-6 ≈ 300ms).
///
/// At the default `max_gap_myers_d` (200), this permits up to ~125,000
/// distinct rows per side in the worst case (zero anchors, zero
/// prefix/suffix trim — the true adversarial floor). Realistic sheets are
/// far cheaper: rows with *any* content structure produce prefix/suffix
/// trim and/or patience anchors that shrink the active region well below
/// this floor (the scattered-edit benchmarks in Issue #4's PoC thread
/// stayed under 100ms at 1,000,000 rows). A caller diffing a larger
/// duplicate-heavy sheet, or one needing a bigger `max_gap_myers_d` to
/// resolve edits scattered across more than ~100 points, can raise
/// `max_cost` explicitly and accept the correspondingly higher worst-case
/// latency — the same self-service trade-off
/// `ColumnAlignmentLimits::max_cost` already offers.
pub(crate) const MAX_ROW_ALIGNMENT_COST: usize = 50_000_000;

/// Hard ceiling on `RowAlignmentLimits::max_gap_myers_d` alone, independent
/// of `max_cost` — bounds `myers_diff_gap`'s `flat_trace` buffer *memory*,
/// which is O(max_gap_myers_d²) regardless of row count. This is the row
/// counterpart of `diff::col_alignment::MAX_COLUMN_PAIR_COUNT`: `max_cost`
/// alone isn't sufficient here, the same way `diff::col_alignment`'s
/// `max_cost` alone wasn't sufficient for columns — a caller could raise
/// `max_gap_myers_d` *and* `max_cost` together (e.g. a tiny sheet with a
/// huge `max_gap_myers_d`) so the row-count-weighted time budget stays
/// satisfied while `flat_trace` alone still allocates gigabytes, since its
/// size never depends on row count at all (caught in review on PR #21).
///
/// Sized from the real allocated type: `flat_trace: Vec<usize>` of length
/// `(max_steps + 1) * (2 × max_steps + 2)`, 8 bytes per `usize`, where
/// `max_steps = min(sub_n + sub_m, max_gap_myers_d)` — so `max_gap_myers_d`
/// alone is the size driver at its own ceiling. At `MAX_GAP_MYERS_D_CEILING`
/// (2,000): `2,001 × 4,002 × 8 ≈ 64MB` — comfortably inside the same
/// "tens of MB" class `MAX_COLUMN_PAIR_COUNT` targets, while remaining an
/// order of magnitude above `DEFAULT_MAX_GAP_MYERS_D` (200) so it never
/// constrains a caller who hasn't deliberately overridden the default.
pub(crate) const MAX_GAP_MYERS_D_CEILING: usize = 2_000;

/// Configuration for [`diff_workbooks_aligned_rows`]. A plain struct
/// parameter (not a builder, not `Option<T>`), matching this crate's
/// existing `SizeLimits`/`ColumnAlignmentLimits` convention.
#[derive(Debug, Clone, Copy)]
pub struct RowAlignmentLimits {
    /// Per-gap Myers edit-distance budget — see `myers_diff_gap`'s doc
    /// comment. Defaults to `DEFAULT_MAX_GAP_MYERS_D`. Checked against
    /// `MAX_GAP_MYERS_D_CEILING` independently of `max_cost` — see that
    /// constant's doc comment for why a time-only budget isn't enough on
    /// its own.
    pub max_gap_myers_d: usize,
    /// Cap on `2 × max(distinct_rows_base, distinct_rows_target) ×
    /// max_gap_myers_d` — bounds worst-case matching *time* regardless of
    /// how few real anchors a sheet's content offers. Defaults to
    /// `MAX_ROW_ALIGNMENT_COST` — see that constant's doc comment for how
    /// it was measured.
    pub max_cost: usize,
}

impl Default for RowAlignmentLimits {
    fn default() -> Self {
        RowAlignmentLimits {
            max_gap_myers_d: DEFAULT_MAX_GAP_MYERS_D,
            max_cost: MAX_ROW_ALIGNMENT_COST,
        }
    }
}

/// Diffs two already-parsed `Workbook`s the same way `diff::engine::
/// diff_workbooks` does, except that within each sheet present on both
/// sides, rows are first matched by content (see this module's doc
/// comment) so an inserted/deleted row doesn't cascade into spurious
/// diffs for every row after it. Sheets present on only one side are
/// handled identically to `diff_workbooks` (reusing
/// `diff::engine::diff_sheet` directly) — there is nothing to align when
/// an entire sheet is new or gone.
///
/// Returns `Err(Error::RowAlignmentCostTooHigh)`, fail-fast and before any
/// O(gap²) matching work, when a sheet's alignment cost would exceed
/// `limits.max_cost` — deliberately not a silent fallback to
/// `diff_workbooks`'s coordinate-based result, since the caller opted into
/// alignment explicitly (see `RowAlignmentLimits`'s doc comment). A caller
/// that wants automatic fallback can catch this error and call
/// `diff_workbooks` itself.
pub fn diff_workbooks_aligned_rows(
    base: &Workbook,
    target: &Workbook,
    limits: RowAlignmentLimits,
) -> Result<WorkbookDiff> {
    let mut sheet_names: BTreeSet<&str> = BTreeSet::new();
    sheet_names.extend(base.sheets().iter().map(|s| s.name.as_str()));
    sheet_names.extend(target.sheets().iter().map(|s| s.name.as_str()));

    let mut sheets = Vec::new();
    for name in sheet_names {
        let base_sheet = base.sheet(name);
        let target_sheet = target.sheet(name);
        let sheet_diff = match (base_sheet, target_sheet) {
            (Some(b), Some(t)) => align_sheet_rows(name, b, t, limits)?,
            _ => diff_sheet(name, base_sheet, target_sheet),
        };
        if let Some(sheet_diff) = sheet_diff {
            sheets.push(sheet_diff);
        }
    }

    Ok(WorkbookDiff { sheets })
}

/// One row's content, extracted once per sheet-pair comparison. `cells`
/// mirrors `diff::col_alignment::ColumnContent::cells`'s row-sorted-for-free
/// invariant (transposed to columns here), so `diff_matched_rows` below
/// can merge-join two of these the same way `diff::engine::diff_cells`
/// merge-joins a whole sheet.
struct RowContent<'a> {
    row: u32,
    /// `(col, cell)` pairs, only for columns this row actually has a cell
    /// at, in ascending column order.
    cells: Vec<(u32, &'a Cell)>,
    /// Content-hash signature over `cells` (column index and value, in
    /// order) — see this module's doc comment ("Hashing") for why this
    /// uses a randomized, not fixed-seed, hasher.
    signature: u64,
    /// Number of entries in `cells` that carry a value (`value.is_some()`),
    /// as opposed to `cells.len()`, which also counts formatting-only
    /// blanks — mirrors `diff::col_alignment::ColumnContent::populated_count`.
    populated_count: usize,
}

fn hash_cell_value(v: &CellValue, h: &mut impl Hasher) {
    match v {
        CellValue::Number(n) => {
            0u8.hash(h);
            n.to_bits().hash(h);
        }
        CellValue::DateTime(dt) => {
            1u8.hash(h);
            (dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second).hash(h);
        }
        CellValue::Text(s) => {
            2u8.hash(h);
            s.as_ref().hash(h);
        }
        CellValue::Boolean(b) => {
            3u8.hash(h);
            b.hash(h);
        }
        CellValue::Error(e) => {
            4u8.hash(h);
            e.hash(h);
        }
    }
}

/// The number of distinct rows `sheet` has cells in, in true O(cells) —
/// cheap enough to call before the budget check in `align_sheet_rows`,
/// unlike `row_contents` below (which builds every row's full content).
/// Counts row-number transitions in a single pass rather than collecting
/// into a `BTreeSet`: `Sheet::iter_cells()` already yields `CellRef`s in
/// row-major order (row ascending, `Sheet`'s own `BTreeMap` invariant), so
/// distinct rows are always contiguous runs — a `BTreeSet` would pay an
/// extra O(log distinct_rows) insertion per row (a Copilot review on PR
/// #21 caught this; an earlier version paid that cost needlessly here).
fn distinct_row_count(sheet: &Sheet) -> usize {
    let mut count = 0usize;
    let mut last_row: Option<u32> = None;
    for (r, _) in sheet.iter_cells() {
        if last_row != Some(r.row) {
            count += 1;
            last_row = Some(r.row);
        }
    }
    count
}

/// Groups `sheet.iter_cells()` into one `RowContent` per row, in a single
/// true-O(cells) linear pass — no `BTreeMap` bucketing step. Relies on the
/// same row-major-order invariant `distinct_row_count` does
/// (`Sheet::iter_cells()` already yields `CellRef`s row-ascending, `Sheet`'s
/// own `BTreeMap` invariant), so distinct rows are always contiguous runs:
/// a row transition just closes out the previous row's accumulator and
/// starts a fresh one, computing each row's hash incrementally as its
/// cells are visited rather than in a second pass over a collected `Vec`
/// (a Copilot review on PR #21 caught the earlier `BTreeMap`-bucketing
/// version paying an avoidable O(cells·log distinct_rows), the same class
/// of issue as `distinct_row_count`'s).
fn row_contents<'a>(sheet: &'a Sheet, build_hasher: &RandomState) -> Vec<RowContent<'a>> {
    let mut result = Vec::new();
    let mut current_row: Option<u32> = None;
    let mut cells: Vec<(u32, &'a Cell)> = Vec::new();
    let mut hasher = build_hasher.build_hasher();
    let mut populated_count = 0usize;

    for (r, cell) in sheet.iter_cells() {
        if current_row != Some(r.row) {
            if let Some(row) = current_row {
                result.push(RowContent {
                    row,
                    cells: std::mem::take(&mut cells),
                    signature: hasher.finish(),
                    populated_count,
                });
            }
            current_row = Some(r.row);
            hasher = build_hasher.build_hasher();
            populated_count = 0;
        }

        cells.push((r.col, cell));
        r.col.hash(&mut hasher);
        match &cell.value {
            Some(v) => {
                populated_count += 1;
                hash_cell_value(v, &mut hasher);
            }
            None => 0xFFu8.hash(&mut hasher),
        }
    }
    if let Some(row) = current_row {
        result.push(RowContent {
            row,
            cells,
            signature: hasher.finish(),
            populated_count,
        });
    }

    result
}

/// Fraction of `base`'s and `target`'s combined populated columns that
/// agree in value — a Jaccard-like similarity used only by the bounded
/// content-similarity pairing step (see `CONTENT_SIMILARITY_SPAN_CAP`'s
/// doc comment).
fn row_similarity(base: &RowContent, target: &RowContent) -> f64 {
    let mut base_values: HashMap<u32, &CellValue> = HashMap::new();
    for &(col, cell) in &base.cells {
        if let Some(v) = &cell.value {
            base_values.insert(col, v);
        }
    }
    let mut matching = 0usize;
    let mut total = base_values.len();
    for &(col, cell) in &target.cells {
        if let Some(tv) = &cell.value {
            match base_values.get(&col) {
                Some(&bv) if bv == tv => matching += 1,
                Some(_) => {}
                None => total += 1,
            }
        }
    }
    if total == 0 {
        0.0
    } else {
        matching as f64 / total as f64
    }
}

#[derive(Debug, Clone, Copy)]
enum RowAlignment {
    Match { base_idx: usize, target_idx: usize },
    Inserted { target_idx: usize },
    Deleted { base_idx: usize },
}

/// Standard patience-sort LIS over `seq`. Returns the indices *into* `seq`
/// (not the values) forming the longest strictly increasing subsequence,
/// in order.
fn lis_indices(seq: &[usize]) -> Vec<usize> {
    let mut tails: Vec<usize> = Vec::new();
    let mut prev: Vec<Option<usize>> = vec![None; seq.len()];

    for i in 0..seq.len() {
        let x = seq[i];
        let pos = tails.partition_point(|&idx| seq[idx] < x);
        prev[i] = if pos > 0 { Some(tails[pos - 1]) } else { None };
        if pos == tails.len() {
            tails.push(i);
        } else {
            tails[pos] = i;
        }
    }

    let mut result = Vec::new();
    let mut cur = tails.last().copied();
    while let Some(i) = cur {
        result.push(i);
        cur = prev[i];
    }
    result.reverse();
    result
}

/// Reports `[b_start, b_end)`/`[t_start, t_end)` as a plain
/// delete-everything + insert-everything, with no `Match` pairing at all.
/// Used both when Myers' own budget is exceeded and, within a resolved
/// gap, for any leftover span too large for content-similarity pairing to
/// examine — see this module's doc comment ("Never let a per-gap
/// operation scale with the whole gap") and `CONTENT_SIMILARITY_SPAN_CAP`.
/// Strictly more conservative than positionally pairing rows that were
/// never verified to correspond: it can never fabricate a false
/// `Modified` row, at the cost of not being the minimal edit.
fn fill_gap_no_match(
    b_start: usize,
    b_end: usize,
    t_start: usize,
    t_end: usize,
    out: &mut Vec<RowAlignment>,
) {
    for bi in b_start..b_end {
        out.push(RowAlignment::Deleted { base_idx: bi });
    }
    for ti in t_start..t_end {
        out.push(RowAlignment::Inserted { target_idx: ti });
    }
}

/// Resolves one gap between two confirmed anchors (or before the first /
/// after the last) via Myers diff over row signatures, decoding the
/// *entire* backtrace (every diagonal/vertical/horizontal step) directly
/// into `Match`/`Inserted`/`Deleted` — see this module's doc comment
/// ("Never let a step fall back to blind positional pairing") for why a
/// partial decode (recording only the snake positions and bridging the
/// rest by array index) is unsafe. Any leftover contiguous
/// Deleted/Inserted span left by the decode (rows with no exact-signature
/// match within this gap) is then examined for content-similarity pairing,
/// bounded by `CONTENT_SIMILARITY_SPAN_CAP`.
#[allow(clippy::too_many_arguments)]
fn myers_diff_gap(
    base: &[RowContent],
    b_start: usize,
    b_end: usize,
    target: &[RowContent],
    t_start: usize,
    t_end: usize,
    max_d: usize,
    out: &mut Vec<RowAlignment>,
) {
    let n = b_end - b_start;
    let m = t_end - t_start;

    if n == 0 {
        for j in t_start..t_end {
            out.push(RowAlignment::Inserted { target_idx: j });
        }
        return;
    }
    if m == 0 {
        for i in b_start..b_end {
            out.push(RowAlignment::Deleted { base_idx: i });
        }
        return;
    }

    // Common prefix/suffix within this gap: cheap, and lets a gap that's
    // mostly a clean insert/delete run skip the Myers search entirely for
    // the parts that don't need it.
    let mut p = 0;
    while p < n && p < m && base[b_start + p].signature == target[t_start + p].signature {
        out.push(RowAlignment::Match {
            base_idx: b_start + p,
            target_idx: t_start + p,
        });
        p += 1;
    }
    if p == n && p == m {
        return;
    }
    let mut s = 0;
    while s < (n - p)
        && s < (m - p)
        && base[b_end - 1 - s].signature == target[t_end - 1 - s].signature
    {
        s += 1;
    }

    let sub_b_start = b_start + p;
    let sub_b_end = b_end - s;
    let sub_t_start = t_start + p;
    let sub_t_end = t_end - s;
    let sub_n = sub_b_end - sub_b_start;
    let sub_m = sub_t_end - sub_t_start;

    if sub_n == 0 {
        for j in sub_t_start..sub_t_end {
            out.push(RowAlignment::Inserted { target_idx: j });
        }
    } else if sub_m == 0 {
        for i in sub_b_start..sub_b_end {
            out.push(RowAlignment::Deleted { base_idx: i });
        }
    } else {
        let max_steps = (sub_n + sub_m).min(max_d);
        let offset = max_steps as isize;
        let stride = 2 * max_steps + 2;
        // Flat buffer, not Vec<Vec<usize>> — avoids one heap allocation
        // per Myers step (measured to matter at scale in the PoC rounds).
        let mut flat_trace = vec![0usize; (max_steps + 1) * stride];
        let mut v = vec![0usize; stride];

        let mut found = false;
        let mut final_d = 0;

        for d in 0..=max_steps {
            flat_trace[d * stride..(d + 1) * stride].copy_from_slice(&v);
            let d_isize = d as isize;
            for k in (-d_isize..=d_isize).step_by(2) {
                let k_idx = (k + offset) as usize;
                let mut x = if k == -d_isize
                    || (k != d_isize
                        && flat_trace[d * stride + k_idx - 1] < flat_trace[d * stride + k_idx + 1])
                {
                    flat_trace[d * stride + k_idx + 1]
                } else {
                    flat_trace[d * stride + k_idx - 1] + 1
                };
                let mut y = (x as isize - k) as usize;

                while x < sub_n
                    && y < sub_m
                    && base[sub_b_start + x].signature == target[sub_t_start + y].signature
                {
                    x += 1;
                    y += 1;
                }
                v[k_idx] = x;

                if x >= sub_n && y >= sub_m {
                    found = true;
                    final_d = d;
                    break;
                }
            }
            if found {
                break;
            }
        }

        if !found {
            fill_gap_no_match(sub_b_start, sub_b_end, sub_t_start, sub_t_end, out);
        } else {
            // Decode the FULL backtrace: every diagonal step is a Match,
            // every vertical step an Inserted, every horizontal step a
            // Deleted. This is the fix identified in
            // `poc/issue4-poc-v2`/`v4`: recording only the diagonal
            // (snake) steps and bridging the rest positionally silently
            // reintroduces the exact cascade this feature exists to
            // prevent.
            let mut decoded: Vec<RowAlignment> = Vec::new();
            let mut x = sub_n;
            let mut y = sub_m;

            for d in (1..=final_d).rev() {
                let prev_offset = d * stride;
                let k = x as isize - y as isize;
                let k_idx = (k + offset) as usize;
                let d_isize = d as isize;

                let prev_k = if k == -d_isize
                    || (k != d_isize
                        && flat_trace[prev_offset + k_idx - 1]
                            < flat_trace[prev_offset + k_idx + 1])
                {
                    k + 1
                } else {
                    k - 1
                };
                let prev_k_idx = (prev_k + offset) as usize;
                let prev_x = flat_trace[prev_offset + prev_k_idx];
                let prev_y = (prev_x as isize - prev_k) as usize;

                while x > prev_x && y > prev_y {
                    decoded.push(RowAlignment::Match {
                        base_idx: sub_b_start + x - 1,
                        target_idx: sub_t_start + y - 1,
                    });
                    x -= 1;
                    y -= 1;
                }

                if x == prev_x {
                    decoded.push(RowAlignment::Inserted {
                        target_idx: sub_t_start + y - 1,
                    });
                    y -= 1;
                } else {
                    decoded.push(RowAlignment::Deleted {
                        base_idx: sub_b_start + x - 1,
                    });
                    x -= 1;
                }
            }

            // No trailing d=0 snake walk here (the textbook Myers
            // backtrace has one, to consume a leading match at the very
            // start of the two sequences): this function's own prefix
            // trim above already strips that case before the D-step
            // search ever runs, and only *k=0* is ever evaluated at d=0
            // (by construction — `-d..=d step 2` yields just `[0]` when
            // `d == 0`), which reduces to re-checking
            // `base[sub_b_start] == target[sub_t_start]` — already
            // known false, or this whole `else` branch wouldn't have been
            // reached. So backtracking through d=1 always lands exactly
            // on `(x, y) == (0, 0)`, making a trailing snake walk here
            // provably a no-op; verified directly rather than assumed.
            decoded.reverse();

            merge_leftover_spans_by_content_similarity(base, target, &decoded, out);
        }
    }

    for k in 0..s {
        out.push(RowAlignment::Match {
            base_idx: sub_b_end + k,
            target_idx: sub_t_end + k,
        });
    }
}

/// Scans `decoded` for contiguous leftover spans of `Deleted`/`Inserted`
/// (rows Myers found no exact-signature match for) and, when a span is
/// small enough (`CONTENT_SIMILARITY_SPAN_CAP`), pairs each deleted row
/// with its most similar not-yet-claimed insert (≥
/// `CONTENT_SIMILARITY_THRESHOLD`) as a `Match` — recovering genuine
/// single/multi-row modifications. A span over the cap, or a row within a
/// small span with no similar-enough partner, is left as plain
/// `Deleted`/`Inserted`. See this module's doc comment ("Never let a
/// per-gap operation scale with the whole gap" and "Known limitation") for
/// the cost bound and the residual tie-break ambiguity this accepts.
fn merge_leftover_spans_by_content_similarity(
    base: &[RowContent],
    target: &[RowContent],
    decoded: &[RowAlignment],
    out: &mut Vec<RowAlignment>,
) {
    let mut i = 0;
    while i < decoded.len() {
        if !matches!(
            decoded[i],
            RowAlignment::Deleted { .. } | RowAlignment::Inserted { .. }
        ) {
            out.push(decoded[i]);
            i += 1;
            continue;
        }

        let mut del_indices = Vec::new();
        let mut ins_indices = Vec::new();
        let mut j = i;
        while j < decoded.len() {
            match decoded[j] {
                RowAlignment::Deleted { base_idx } => del_indices.push(base_idx),
                RowAlignment::Inserted { target_idx } => ins_indices.push(target_idx),
                RowAlignment::Match { .. } => break,
            }
            j += 1;
        }

        if del_indices.len() + ins_indices.len() > CONTENT_SIMILARITY_SPAN_CAP {
            for &bi in &del_indices {
                out.push(RowAlignment::Deleted { base_idx: bi });
            }
            for &ti in &ins_indices {
                out.push(RowAlignment::Inserted { target_idx: ti });
            }
            i = j;
            continue;
        }

        let mut claimed: HashSet<usize> = HashSet::new();
        for &bi in &del_indices {
            let b_row = &base[bi];
            let mut best_similarity = 0.0;
            let mut best_ti = None;
            for &ti in &ins_indices {
                if claimed.contains(&ti) {
                    continue;
                }
                let similarity = row_similarity(b_row, &target[ti]);
                if similarity >= CONTENT_SIMILARITY_THRESHOLD && similarity > best_similarity {
                    best_similarity = similarity;
                    best_ti = Some(ti);
                }
            }
            if let Some(ti) = best_ti {
                claimed.insert(ti);
                out.push(RowAlignment::Match {
                    base_idx: bi,
                    target_idx: ti,
                });
            } else {
                out.push(RowAlignment::Deleted { base_idx: bi });
            }
        }
        for &ti in &ins_indices {
            if !claimed.contains(&ti) {
                out.push(RowAlignment::Inserted { target_idx: ti });
            }
        }

        i = j;
    }
}

/// Matches `base_rows` against `target_rows` (see this module's doc
/// comment for the full algorithm).
fn align_rows(
    base_rows: &[RowContent],
    target_rows: &[RowContent],
    limits: &RowAlignmentLimits,
) -> Vec<RowAlignment> {
    let mut prefix_len = 0;
    while prefix_len < base_rows.len()
        && prefix_len < target_rows.len()
        && base_rows[prefix_len].signature == target_rows[prefix_len].signature
    {
        prefix_len += 1;
    }

    let mut suffix_len = 0;
    while suffix_len < (base_rows.len() - prefix_len)
        && suffix_len < (target_rows.len() - prefix_len)
        && base_rows[base_rows.len() - 1 - suffix_len].signature
            == target_rows[target_rows.len() - 1 - suffix_len].signature
    {
        suffix_len += 1;
    }

    let active_base = &base_rows[prefix_len..(base_rows.len() - suffix_len)];
    let active_target = &target_rows[prefix_len..(target_rows.len() - suffix_len)];

    let mut result = Vec::with_capacity(base_rows.len().max(target_rows.len()) + 16);
    for k in 0..prefix_len {
        result.push(RowAlignment::Match {
            base_idx: k,
            target_idx: k,
        });
    }

    if !active_base.is_empty() || !active_target.is_empty() {
        let mut base_sig_count: HashMap<u64, u32> = HashMap::new();
        for r in active_base {
            *base_sig_count.entry(r.signature).or_insert(0) += 1;
        }
        let mut target_sig_count: HashMap<u64, u32> = HashMap::new();
        let mut target_sig_to_idx: HashMap<u64, usize> = HashMap::new();
        for (idx, r) in active_target.iter().enumerate() {
            *target_sig_count.entry(r.signature).or_insert(0) += 1;
            target_sig_to_idx.insert(r.signature, idx);
        }

        // A signature must be unique on *both* sides, and the row must
        // carry at least one real value — an all-blank row's signature is
        // a fixed constant shared by every other all-blank row, so it
        // would never be unique anyway, but guard explicitly rather than
        // relying on that coincidence.
        let mut anchor_base_idx: Vec<usize> = Vec::new();
        let mut anchor_target_idx: Vec<usize> = Vec::new();
        for (i, r) in active_base.iter().enumerate() {
            if r.populated_count == 0 {
                continue;
            }
            if base_sig_count.get(&r.signature) == Some(&1)
                && target_sig_count.get(&r.signature) == Some(&1)
            {
                if let Some(&j) = target_sig_to_idx.get(&r.signature) {
                    anchor_base_idx.push(i);
                    anchor_target_idx.push(j);
                }
            }
        }

        let lis = lis_indices(&anchor_target_idx);
        let confirmed: Vec<(usize, usize)> = lis
            .into_iter()
            .map(|k| (anchor_base_idx[k], anchor_target_idx[k]))
            .collect();

        let mut b_cursor = 0usize;
        let mut t_cursor = 0usize;
        for &(bi, ti) in &confirmed {
            myers_diff_gap(
                active_base,
                b_cursor,
                bi,
                active_target,
                t_cursor,
                ti,
                limits.max_gap_myers_d,
                &mut result,
            );
            result.push(RowAlignment::Match {
                base_idx: bi,
                target_idx: ti,
            });
            b_cursor = bi + 1;
            t_cursor = ti + 1;
        }
        myers_diff_gap(
            active_base,
            b_cursor,
            active_base.len(),
            active_target,
            t_cursor,
            active_target.len(),
            limits.max_gap_myers_d,
            &mut result,
        );

        for item in &mut result[prefix_len..] {
            match item {
                RowAlignment::Match {
                    base_idx,
                    target_idx,
                } => {
                    *base_idx += prefix_len;
                    *target_idx += prefix_len;
                }
                RowAlignment::Inserted { target_idx } => *target_idx += prefix_len,
                RowAlignment::Deleted { base_idx } => *base_idx += prefix_len,
            }
        }
    }

    for k in 0..suffix_len {
        result.push(RowAlignment::Match {
            base_idx: base_rows.len() - suffix_len + k,
            target_idx: target_rows.len() - suffix_len + k,
        });
    }

    result
}

/// Diffs one sheet known to exist on both sides, aligning rows by content
/// before diffing cells. Returns `Ok(None)` when nothing changed (same
/// "nothing to report" convention `diff::engine::diff_sheet` uses).
fn align_sheet_rows(
    name: &str,
    base: &Sheet,
    target: &Sheet,
    limits: RowAlignmentLimits,
) -> Result<Option<SheetDiff>> {
    // Memory bound first: independent of row count, so it must be checked
    // even when the rows-weighted time budget below would otherwise pass
    // (see MAX_GAP_MYERS_D_CEILING's doc comment for why a caller raising
    // max_cost and max_gap_myers_d together can't otherwise be trusted to
    // keep myers_diff_gap's flat_trace allocation bounded).
    if limits.max_gap_myers_d > MAX_GAP_MYERS_D_CEILING {
        return Err(Error::RowAlignmentCostTooHigh {
            cost: limits.max_gap_myers_d,
            limit: MAX_GAP_MYERS_D_CEILING,
        });
    }

    // Budget check next, using only a cheap O(cells) distinct-row *count*
    // — not the full `row_contents` build below.
    let base_row_count = distinct_row_count(base);
    let target_row_count = distinct_row_count(target);
    let distinct_rows = base_row_count.max(target_row_count);

    let cost = 2usize
        .saturating_mul(distinct_rows)
        .saturating_mul(limits.max_gap_myers_d);
    if cost > limits.max_cost {
        return Err(Error::RowAlignmentCostTooHigh {
            cost,
            limit: limits.max_cost,
        });
    }

    let build_hasher = RandomState::new();
    let base_rows = row_contents(base, &build_hasher);
    let target_rows = row_contents(target, &build_hasher);

    let alignments = align_rows(&base_rows, &target_rows, &limits);

    let mut cells = Vec::new();
    for alignment in &alignments {
        match *alignment {
            RowAlignment::Match {
                base_idx,
                target_idx,
            } => diff_matched_rows(&base_rows[base_idx], &target_rows[target_idx], &mut cells),
            RowAlignment::Inserted { target_idx } => {
                let t = &target_rows[target_idx];
                for &(col, cell) in &t.cells {
                    cells.push(cell_diff_added_aligned(t.row, col, cell));
                }
            }
            RowAlignment::Deleted { base_idx } => {
                let b = &base_rows[base_idx];
                for &(col, cell) in &b.cells {
                    cells.push(cell_diff_deleted_aligned(b.row, col, cell));
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

/// Diffs one matched `(base, target)` row pair by column, merge-join style
/// (the same approach `diff::engine::diff_cells` uses across a whole
/// sheet, applied here within a single row pair — columns stay
/// coordinate-pinned, see this module's doc comment).
fn diff_matched_rows(b: &RowContent, t: &RowContent, out: &mut Vec<CellDiff>) {
    let mut bi = b.cells.iter().copied().peekable();
    let mut ti = t.cells.iter().copied().peekable();

    loop {
        match (bi.peek(), ti.peek()) {
            (Some(&(b_col, b_cell)), Some(&(t_col, t_cell))) => match b_col.cmp(&t_col) {
                Ordering::Less => {
                    out.push(cell_diff_deleted_aligned(b.row, b_col, b_cell));
                    bi.next();
                }
                Ordering::Greater => {
                    out.push(cell_diff_added_aligned(t.row, t_col, t_cell));
                    ti.next();
                }
                Ordering::Equal => {
                    if b_cell.value != t_cell.value || b_cell.style != t_cell.style {
                        out.push(cell_diff_modified_aligned(
                            b.row, t.row, t_col, b_cell, t_cell,
                        ));
                    }
                    bi.next();
                    ti.next();
                }
            },
            (Some(&(b_col, b_cell)), None) => {
                out.push(cell_diff_deleted_aligned(b.row, b_col, b_cell));
                bi.next();
            }
            (None, Some(&(t_col, t_cell))) => {
                out.push(cell_diff_added_aligned(t.row, t_col, t_cell));
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
        old_row: None,
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
        old_row: None,
        old_col: None,
        old_value: Some(cell_value_to_json(old.value.as_ref())),
        new_value: None,
        old_style: old.style.as_deref().map(style_to_json),
        new_style: None,
    }
}

/// See `CellDiff::old_row`'s doc comment for why it's only populated when
/// `old_row != new_row`.
fn cell_diff_modified_aligned(
    old_row: u32,
    new_row: u32,
    col: u32,
    old: &Cell,
    new: &Cell,
) -> CellDiff {
    let style_changed = old.style != new.style;
    CellDiff {
        row: new_row,
        col,
        status: DiffStatus::Modified,
        old_col: None,
        old_row: (old_row != new_row).then_some(old_row),
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
    use crate::model::{CellRef, DateTimeValue, SheetVisibility};

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

    fn text(s: &str) -> CellValue {
        CellValue::Text(s.into())
    }
    fn num(n: f64) -> CellValue {
        CellValue::Number(n)
    }

    #[test]
    fn row_insertion_does_not_cascade_when_aligned() {
        let base = workbook(vec![sheet_with_values(
            "Inventory",
            &[
                (1, 1, text("Item")),
                (1, 2, text("Price")),
                (1, 3, text("Stock")),
                (2, 1, text("Apple")),
                (2, 2, num(100.0)),
                (2, 3, num(50.0)),
                (3, 1, text("Banana")),
                (3, 2, num(150.0)),
                (3, 3, num(30.0)),
                (4, 1, text("Melon")),
                (4, 2, num(500.0)),
                (4, 3, num(10.0)),
            ],
        )]);
        let target = workbook(vec![sheet_with_values(
            "Inventory",
            &[
                (1, 1, text("Item")),
                (1, 2, text("Price")),
                (1, 3, text("Stock")),
                (2, 1, text("Cherry")),
                (2, 2, num(80.0)),
                (2, 3, num(100.0)),
                (3, 1, text("Apple")),
                (3, 2, num(100.0)),
                (3, 3, num(50.0)),
                (4, 1, text("Banana")),
                (4, 2, num(180.0)),
                (4, 3, num(30.0)),
                (5, 1, text("Melon")),
                (5, 2, num(500.0)),
                (5, 3, num(10.0)),
            ],
        )]);

        let diff =
            diff_workbooks_aligned_rows(&base, &target, RowAlignmentLimits::default()).unwrap();
        let cells = &diff.sheets[0].cells;

        let added = cells
            .iter()
            .filter(|c| c.status == DiffStatus::Added)
            .count();
        let modified: Vec<_> = cells
            .iter()
            .filter(|c| c.status == DiffStatus::Modified)
            .collect();
        let deleted = cells
            .iter()
            .filter(|c| c.status == DiffStatus::Deleted)
            .count();

        // Only Cherry's 3 cells are Added and Banana's price is Modified —
        // Apple/Melon shifted but produce zero diff.
        assert_eq!(added, 3);
        assert_eq!(modified.len(), 1);
        assert_eq!(deleted, 0);
        assert_eq!(
            modified[0].old_value,
            Some(cell_value_to_json(Some(&num(150.0))))
        );
        assert_eq!(
            modified[0].new_value,
            Some(cell_value_to_json(Some(&num(180.0))))
        );
        // Banana shifted from base row 3 to target row 4.
        assert_eq!(modified[0].old_row, Some(3));
        assert_eq!(modified[0].row, 4);
    }

    #[test]
    fn row_deletion_does_not_cascade_when_aligned() {
        let base = workbook(vec![sheet_with_values(
            "Sheet1",
            &[
                (1, 1, text("A")),
                (2, 1, text("B")),
                (3, 1, text("C")),
                (4, 1, text("D")),
            ],
        )]);
        let target = workbook(vec![sheet_with_values(
            "Sheet1",
            &[(1, 1, text("A")), (2, 1, text("C")), (3, 1, text("D"))],
        )]);

        let diff =
            diff_workbooks_aligned_rows(&base, &target, RowAlignmentLimits::default()).unwrap();
        let cells = &diff.sheets[0].cells;
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].status, DiffStatus::Deleted);
        assert_eq!(cells[0].row, 2);
        assert_eq!(
            cells[0].old_value,
            Some(cell_value_to_json(Some(&text("B"))))
        );
    }

    #[test]
    fn old_row_is_absent_when_the_matched_row_did_not_shift() {
        let base = workbook(vec![sheet_with_values(
            "Sheet1",
            &[(1, 1, text("X")), (1, 2, num(1.0))],
        )]);
        let target = workbook(vec![sheet_with_values(
            "Sheet1",
            &[(1, 1, text("X")), (1, 2, num(2.0))],
        )]);

        let diff =
            diff_workbooks_aligned_rows(&base, &target, RowAlignmentLimits::default()).unwrap();
        let cells = &diff.sheets[0].cells;
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].status, DiffStatus::Modified);
        assert_eq!(cells[0].old_row, None);
    }

    #[test]
    fn low_cardinality_duplicated_rows_with_scattered_insertion_do_not_cascade() {
        // Regression test for the cascade found in poc/issue4-poc-v3: a
        // duplicate-heavy sheet (only 4 distinct row patterns repeating)
        // with edits scattered across many points, not one contiguous
        // burst, must not report false Modified cells even though no
        // single row is content-unique.
        let rows = 400u32;
        let mut base_cells = Vec::new();
        for r in 1..=rows {
            let pattern = (r % 4) as f64;
            base_cells.push((r, 1, num(pattern)));
            base_cells.push((r, 2, num(pattern * 10.0)));
        }
        let base = workbook(vec![sheet_with_values("Sheet1", &base_cells)]);

        // Insert one new row every 20 rows (20 scattered insertions total).
        let mut target_cells = Vec::new();
        let mut out_row = 1u32;
        let mut inserted = 0u32;
        for r in 1..=rows {
            if inserted < 20 && r % 20 == 0 {
                target_cells.push((out_row, 1, num(99.0)));
                target_cells.push((out_row, 2, num(999.0)));
                out_row += 1;
                inserted += 1;
            }
            let pattern = (r % 4) as f64;
            target_cells.push((out_row, 1, num(pattern)));
            target_cells.push((out_row, 2, num(pattern * 10.0)));
            out_row += 1;
        }
        let target = workbook(vec![sheet_with_values("Sheet1", &target_cells)]);

        let diff =
            diff_workbooks_aligned_rows(&base, &target, RowAlignmentLimits::default()).unwrap();
        let cells = &diff.sheets[0].cells;
        let modified = cells
            .iter()
            .filter(|c| c.status == DiffStatus::Modified)
            .count();
        let added = cells
            .iter()
            .filter(|c| c.status == DiffStatus::Added)
            .count();
        assert_eq!(modified, 0, "must not cascade false modifications");
        assert_eq!(added, 20 * 2);
    }

    #[test]
    fn consecutive_modified_rows_are_each_detected_as_modified() {
        // Two adjacent rows both changed one cell each — must be detected
        // as 2 Modified rows, not a whole-row delete+insert (the flaw
        // found in poc/issue4-poc-v6's evaluation of an earlier isolated-
        // pair-only merge rule).
        let base = workbook(vec![sheet_with_values(
            "Sheet1",
            &[
                (1, 1, text("Apple")),
                (1, 2, num(100.0)),
                (2, 1, text("Banana")),
                (2, 2, num(150.0)),
                (3, 1, text("Melon")),
                (3, 2, num(500.0)),
                (4, 1, text("Orange")),
                (4, 2, num(200.0)),
            ],
        )]);
        let target = workbook(vec![sheet_with_values(
            "Sheet1",
            &[
                (1, 1, text("Apple")),
                (1, 2, num(100.0)),
                (2, 1, text("Banana")),
                (2, 2, num(180.0)),
                (3, 1, text("Melon")),
                (3, 2, num(550.0)),
                (4, 1, text("Orange")),
                (4, 2, num(200.0)),
            ],
        )]);

        let diff =
            diff_workbooks_aligned_rows(&base, &target, RowAlignmentLimits::default()).unwrap();
        let cells = &diff.sheets[0].cells;
        let modified: Vec<_> = cells
            .iter()
            .filter(|c| c.status == DiffStatus::Modified)
            .collect();
        assert_eq!(modified.len(), 2);
        assert!(cells
            .iter()
            .all(|c| c.status != DiffStatus::Added && c.status != DiffStatus::Deleted));
    }

    #[test]
    fn sheet_present_on_only_one_side_reuses_the_coordinate_engine_through_alignment() {
        let base = workbook(vec![]);
        let target = workbook(vec![sheet_with_values(
            "New",
            &[(1, 1, num(1.0)), (1, 2, num(2.0))],
        )]);

        let diff =
            diff_workbooks_aligned_rows(&base, &target, RowAlignmentLimits::default()).unwrap();
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
    fn row_alignment_cost_too_high_is_reported_fail_fast() {
        // 500 distinct rows on each side, max_gap_myers_d high enough that
        // 2 * 500 * max_gap_myers_d exceeds a deliberately tiny max_cost.
        let base_cells: Vec<(u32, u32, CellValue)> =
            (1..=500u32).map(|r| (r, 1, num(r as f64))).collect();
        let base = workbook(vec![sheet_with_values("Sheet1", &base_cells)]);
        let target = workbook(vec![sheet_with_values("Sheet1", &base_cells)]);

        let limits = RowAlignmentLimits {
            max_gap_myers_d: 100,
            max_cost: 99_999, // 2 * 500 * 100 = 100,000 > 99,999
        };
        let err = diff_workbooks_aligned_rows(&base, &target, limits).unwrap_err();
        assert!(matches!(
            err,
            Error::RowAlignmentCostTooHigh {
                cost: 100_000,
                limit: 99_999
            }
        ));
    }

    #[test]
    fn max_gap_myers_d_over_the_ceiling_is_row_alignment_cost_too_high_even_with_one_row() {
        // A single row on each side keeps the rows-weighted max_cost
        // budget trivially satisfied (2 * 1 * max_gap_myers_d is tiny),
        // but the flat_trace memory bound (max_gap_myers_d alone) must
        // still reject an unreasonably large max_gap_myers_d — this is
        // exactly the gap MAX_GAP_MYERS_D_CEILING closes independently of
        // max_cost (PR #21 review).
        let base = workbook(vec![sheet_with_values("Sheet1", &[(1, 1, num(1.0))])]);
        let target = workbook(vec![sheet_with_values("Sheet1", &[(1, 1, num(2.0))])]);

        let limits = RowAlignmentLimits {
            max_gap_myers_d: MAX_GAP_MYERS_D_CEILING + 1,
            max_cost: usize::MAX,
        };
        let err = diff_workbooks_aligned_rows(&base, &target, limits).unwrap_err();
        assert!(matches!(
            err,
            Error::RowAlignmentCostTooHigh {
                cost,
                limit: MAX_GAP_MYERS_D_CEILING,
            } if cost == MAX_GAP_MYERS_D_CEILING + 1
        ));
    }

    #[test]
    fn diff_workbooks_default_behavior_is_unaffected_by_row_alignment_existing() {
        let base = workbook(vec![sheet_with_values(
            "Sheet1",
            &[(1, 1, num(10.0)), (2, 1, num(20.0))],
        )]);
        let target = workbook(vec![sheet_with_values(
            "Sheet1",
            &[(1, 1, num(99.0)), (2, 1, num(10.0)), (3, 1, num(20.0))],
        )]);

        let diff = crate::diff::engine::diff_workbooks(&base, &target);
        assert_eq!(diff.sheets[0].cells.len(), 3);
    }

    #[test]
    fn identical_sheets_produce_no_diff() {
        // The whole sheet is consumed by align_rows's own prefix trim (no
        // active region at all left for anchor/Myers work) — exercises the
        // "nothing to report" path both in align_rows itself and in
        // align_sheet_rows's Ok(None) return.
        let cells = &[
            (1, 1, text("Item")),
            (2, 1, text("Apple")),
            (2, 2, num(100.0)),
        ];
        let base = workbook(vec![sheet_with_values("Sheet1", cells)]);
        let target = workbook(vec![sheet_with_values("Sheet1", cells)]);

        let diff =
            diff_workbooks_aligned_rows(&base, &target, RowAlignmentLimits::default()).unwrap();
        assert!(diff.sheets.is_empty());
    }

    #[test]
    fn date_time_and_boolean_row_values_are_hashed_and_aligned_correctly() {
        // Exercises hash_cell_value's DateTime and Boolean arms (Number/
        // Text are already covered by every other test).
        let base = workbook(vec![sheet_with_values(
            "Sheet1",
            &[
                (1, 1, text("Header")),
                (2, 1, CellValue::Boolean(true)),
                (2, 2, num(1.0)),
                (
                    3,
                    1,
                    CellValue::DateTime(DateTimeValue {
                        year: 2026,
                        month: 1,
                        day: 1,
                        hour: 0,
                        minute: 0,
                        second: 0,
                    }),
                ),
                (3, 2, num(2.0)),
            ],
        )]);
        let target = workbook(vec![sheet_with_values(
            "Sheet1",
            &[
                (1, 1, text("Header")),
                (2, 1, CellValue::Boolean(false)), // changed
                (2, 2, num(1.0)),
                (
                    3,
                    1,
                    CellValue::DateTime(DateTimeValue {
                        year: 2026,
                        month: 1,
                        day: 1,
                        hour: 0,
                        minute: 0,
                        second: 0,
                    }),
                ),
                (3, 2, num(2.0)),
            ],
        )]);

        let diff =
            diff_workbooks_aligned_rows(&base, &target, RowAlignmentLimits::default()).unwrap();
        let cells = &diff.sheets[0].cells;
        // Only the Boolean cell changed; the DateTime row is untouched.
        let modified: Vec<_> = cells
            .iter()
            .filter(|c| c.status == DiffStatus::Modified)
            .collect();
        assert_eq!(modified.len(), 1);
        assert_eq!(modified[0].row, 2);
        assert_eq!(modified[0].col, 1);
    }

    #[test]
    fn formatting_only_cell_with_no_value_is_hashed_without_panicking() {
        // Exercises row_contents's `None => ...` arm (a cell that carries
        // formatting but no value).
        let mut base = Sheet::new("Sheet1".to_string(), SheetVisibility::Visible);
        base.insert_cell(
            CellRef { row: 1, col: 1 },
            Cell {
                value: None,
                style: None,
            },
        );
        base.insert_cell(
            CellRef { row: 1, col: 2 },
            Cell {
                value: Some(num(1.0)),
                style: None,
            },
        );
        let base = workbook(vec![base]);

        let mut target = Sheet::new("Sheet1".to_string(), SheetVisibility::Visible);
        target.insert_cell(
            CellRef { row: 1, col: 1 },
            Cell {
                value: None,
                style: None,
            },
        );
        target.insert_cell(
            CellRef { row: 1, col: 2 },
            Cell {
                value: Some(num(2.0)),
                style: None,
            },
        );
        let target = workbook(vec![target]);

        let diff =
            diff_workbooks_aligned_rows(&base, &target, RowAlignmentLimits::default()).unwrap();
        let cells = &diff.sheets[0].cells;
        // A single row on each side with no exact-signature match and no
        // similar-enough content-similarity partner (a lone Number cell
        // is the only real evidence) falls back to plain delete+insert —
        // 2 cells per side (the formatting-only cell included), which is
        // the point: row_contents hashed it without panicking and it
        // flowed through the rest of the pipeline like any other cell.
        assert_eq!(cells.len(), 4);
        assert_eq!(
            cells
                .iter()
                .filter(|c| c.status == DiffStatus::Deleted)
                .count(),
            2
        );
        assert_eq!(
            cells
                .iter()
                .filter(|c| c.status == DiffStatus::Added)
                .count(),
            2
        );
    }

    #[test]
    fn reordered_anchors_exercise_the_lis_replacement_branch() {
        // Base order [X, Y, Z] but target order [Y, Z, X]: X's anchor
        // target-index (2) is visited before Y's (0) and Z's (1) in base
        // order, forcing patience-sort LIS to replace an existing tail
        // entry rather than only ever appending one.
        let base = workbook(vec![sheet_with_values(
            "Sheet1",
            &[
                (1, 1, text("X")),
                (1, 2, num(1.0)),
                (2, 1, text("Y")),
                (2, 2, num(2.0)),
                (3, 1, text("Z")),
                (3, 2, num(3.0)),
            ],
        )]);
        let target = workbook(vec![sheet_with_values(
            "Sheet1",
            &[
                (1, 1, text("Y")),
                (1, 2, num(2.0)),
                (2, 1, text("Z")),
                (2, 2, num(3.0)),
                (3, 1, text("X")),
                (3, 2, num(1.0)),
            ],
        )]);

        let diff =
            diff_workbooks_aligned_rows(&base, &target, RowAlignmentLimits::default()).unwrap();
        let cells = &diff.sheets[0].cells;
        // Whichever 2 of the 3 rows end up as the confirmed (order-
        // preserving) LIS match, the row left over is reported as a plain
        // delete+insert pair -- no Modified cells, since every row's
        // content is byte-identical to some row on the other side.
        assert!(cells
            .iter()
            .all(|c| c.status == DiffStatus::Added || c.status == DiffStatus::Deleted));
        assert!(!cells.is_empty());
    }

    #[test]
    fn myers_budget_exceeded_falls_back_to_safe_delete_and_insert() {
        // Two completely unrelated rows on each side (no shared
        // signatures at all, so zero anchors) with max_gap_myers_d set too
        // small to resolve the D=4 edit distance needed -- the overall
        // row-count budget (2 * 2 * 1 = 4) trivially clears max_cost, but
        // the per-gap Myers search itself must give up and fall back to
        // fill_gap_no_match.
        let base = workbook(vec![sheet_with_values(
            "Sheet1",
            &[(1, 1, text("A")), (2, 1, text("B"))],
        )]);
        let target = workbook(vec![sheet_with_values(
            "Sheet1",
            &[(1, 1, text("C")), (2, 1, text("D"))],
        )]);

        let limits = RowAlignmentLimits {
            max_gap_myers_d: 1,
            max_cost: RowAlignmentLimits::default().max_cost,
        };
        let diff = diff_workbooks_aligned_rows(&base, &target, limits).unwrap();
        let cells = &diff.sheets[0].cells;
        // Safe fallback: every row reported as a plain delete/insert, no
        // fabricated Modified pairing.
        assert!(cells
            .iter()
            .all(|c| c.status == DiffStatus::Added || c.status == DiffStatus::Deleted));
        assert_eq!(
            cells
                .iter()
                .filter(|c| c.status == DiffStatus::Deleted)
                .count(),
            2
        );
        assert_eq!(
            cells
                .iter()
                .filter(|c| c.status == DiffStatus::Added)
                .count(),
            2
        );
    }

    #[test]
    fn content_similarity_pairing_handles_mismatched_and_partner_less_rows() {
        // A single unresolved span with two deleted rows: one has a
        // similar-enough insert to pair with (recovering a Modified row,
        // and exercising row_similarity's mismatched-value and target-
        // only-column branches along the way), the other has no
        // similar-enough partner at all and must stay a plain Deleted.
        let base = workbook(vec![sheet_with_values(
            "Sheet1",
            &[
                (1, 1, text("Anchor")),
                (2, 1, text("Region")),
                (2, 2, text("East")),
                (2, 3, text("Old")),
                (3, 1, text("Unrelated")),
                (3, 2, num(1.0)),
                (4, 1, text("Anchor2")),
            ],
        )]);
        let target = workbook(vec![sheet_with_values(
            "Sheet1",
            &[
                (1, 1, text("Anchor")),
                (2, 1, text("Region")),
                (2, 2, text("East")),  // matches base row 2
                (2, 3, text("New")),   // same column as base, different value
                (2, 4, text("Extra")), // a target-only column
                (3, 1, text("SomethingElse")),
                (3, 2, num(999.0)),
                (4, 1, text("Anchor2")),
            ],
        )]);

        let diff =
            diff_workbooks_aligned_rows(&base, &target, RowAlignmentLimits::default()).unwrap();
        let cells = &diff.sheets[0].cells;

        // Row 2 (Region/East) is similar enough (2 of 4 union columns agree,
        // one value differs in place (col3 "Old" -> "New"), one is
        // target-only (col4 "Extra")) to be paired as a single row --
        // col3's in-place value change surfaces as a genuine Modified
        // cell, exercising row_similarity's mismatched-value branch and
        // the target-only-column branch at the same time.
        assert!(cells.iter().any(|c| c.status == DiffStatus::Modified));
        // Row 3 ("Unrelated"/1.0) has nothing similar enough in the
        // target and stays a plain Deleted; its target counterpart
        // ("SomethingElse"/999.0) stays a plain Added.
        assert!(cells
            .iter()
            .any(|c| c.status == DiffStatus::Deleted && c.old_value.as_ref().is_some()));
        assert!(cells.iter().any(|c| c.status == DiffStatus::Added));
    }

    #[test]
    fn content_similarity_span_over_the_cap_falls_back_to_plain_delete_and_insert() {
        // CONTENT_SIMILARITY_SPAN_CAP is 64 -- a single unresolved span of
        // more than that many rows must skip content-similarity pairing
        // entirely (O(span^2) would otherwise apply), even when every
        // deleted row has an obviously similar insert right next to it.
        let n = 40u32; // 40 deletes + 40 inserts = 80 > CONTENT_SIMILARITY_SPAN_CAP
        let mut base_cells = vec![(1, 1, text("Anchor"))];
        let mut target_cells = vec![(1, 1, text("Anchor"))];
        for k in 1..=n {
            base_cells.push((k + 1, 1, text("Row")));
            base_cells.push((k + 1, 2, num(k as f64)));
            target_cells.push((k + 1, 1, text("Row")));
            target_cells.push((k + 1, 2, num(k as f64 + 1000.0)));
        }
        base_cells.push((n + 2, 1, text("Anchor2")));
        target_cells.push((n + 2, 1, text("Anchor2")));

        let base = workbook(vec![sheet_with_values("Sheet1", &base_cells)]);
        let target = workbook(vec![sheet_with_values("Sheet1", &target_cells)]);

        let diff =
            diff_workbooks_aligned_rows(&base, &target, RowAlignmentLimits::default()).unwrap();
        let cells = &diff.sheets[0].cells;
        // Over the span cap: no content-similarity pairing at all, so
        // every row is a plain delete/insert, never Modified.
        assert!(cells
            .iter()
            .all(|c| c.status == DiffStatus::Added || c.status == DiffStatus::Deleted));
    }

    #[test]
    fn myers_internal_trim_matches_rows_inside_a_gap_between_anchors() {
        // Z/W and Y/X block align_rows's own whole-sheet prefix/suffix
        // trim from ever reaching the Dup rows, so the only way the Dup
        // pair (duplicated -- never a patience anchor) can produce zero
        // diff is via myers_diff_gap's own internal prefix trim, run
        // fresh within the gap between anchors A and B.
        let base = workbook(vec![sheet_with_values(
            "Sheet1",
            &[
                (1, 1, text("Z")),
                (2, 1, text("A")),
                (3, 1, text("Dup")),
                (3, 2, num(1.0)),
                (4, 1, text("Dup")),
                (4, 2, num(1.0)),
                (5, 1, text("Old")),
                (6, 1, text("B")),
                (7, 1, text("Y")),
            ],
        )]);
        let target = workbook(vec![sheet_with_values(
            "Sheet1",
            &[
                (1, 1, text("W")),
                (2, 1, text("A")),
                (3, 1, text("Dup")),
                (3, 2, num(1.0)),
                (4, 1, text("Dup")),
                (4, 2, num(1.0)),
                (5, 1, text("New")),
                (6, 1, text("B")),
                (7, 1, text("X")),
            ],
        )]);

        let diff =
            diff_workbooks_aligned_rows(&base, &target, RowAlignmentLimits::default()).unwrap();
        let cells = &diff.sheets[0].cells;
        // The Dup pair produces zero diff; only Z->W, Old->New, and Y->X
        // remain (each a plain delete+insert, since none of the three
        // pairs share enough content to clear the similarity threshold).
        assert!(!cells.is_empty());
        assert!(cells
            .iter()
            .all(|c| c.status == DiffStatus::Added || c.status == DiffStatus::Deleted));
    }

    #[test]
    fn myers_internal_trim_can_fully_resolve_a_gap_with_no_leftover() {
        // The gap between anchors B and C is *entirely* explained by
        // myers_diff_gap's own internal prefix trim (the Dup2 pair is
        // byte-identical on both sides and nothing else is in the gap) --
        // exercises the `p == n && p == m` early-return.
        let base = workbook(vec![sheet_with_values(
            "Sheet1",
            &[
                (1, 1, text("Z")),
                (2, 1, text("B")),
                (3, 1, text("Dup2")),
                (3, 2, num(9.0)),
                (4, 1, text("Dup2")),
                (4, 2, num(9.0)),
                (5, 1, text("C")),
                (6, 1, text("Y")),
            ],
        )]);
        let target = workbook(vec![sheet_with_values(
            "Sheet1",
            &[
                (1, 1, text("W")),
                (2, 1, text("B")),
                (3, 1, text("Dup2")),
                (3, 2, num(9.0)),
                (4, 1, text("Dup2")),
                (4, 2, num(9.0)),
                (5, 1, text("C")),
                (6, 1, text("X")),
            ],
        )]);

        let diff =
            diff_workbooks_aligned_rows(&base, &target, RowAlignmentLimits::default()).unwrap();
        let cells = &diff.sheets[0].cells;
        // Only Z->W and Y->X remain; the Dup2 pair contributes no diff at
        // all.
        assert!(!cells.is_empty());
        assert!(cells
            .iter()
            .all(|c| c.status == DiffStatus::Added || c.status == DiffStatus::Deleted));
    }

    #[test]
    fn matched_row_with_interleaved_and_trailing_column_mismatches() {
        // A content-similarity-matched row pair (3 of 4 base columns
        // agree with target, clearing the 0.40 threshold) whose remaining
        // columns interleave and leave a base-only trailing column --
        // exercises every branch of diff_matched_rows's merge-join
        // (Less/Greater/Equal, plus the base-exhausted-last tail; the
        // target-exhausted-last tail is covered by an earlier test).
        let base = workbook(vec![sheet_with_values(
            "Sheet1",
            &[
                (1, 1, text("Anchor")),
                (2, 1, text("Row")),
                (2, 2, num(1.0)),
                (2, 3, num(3.0)),
                (2, 5, num(5.0)),
                (2, 7, num(7.0)),
                (3, 1, text("Anchor2")),
            ],
        )]);
        let target = workbook(vec![sheet_with_values(
            "Sheet1",
            &[
                (1, 1, text("Anchor")),
                (2, 1, text("Row")),
                (2, 2, num(1.0)),
                (2, 4, num(4.0)),
                (2, 5, num(5.0)),
                (3, 1, text("Anchor2")),
            ],
        )]);

        let diff =
            diff_workbooks_aligned_rows(&base, &target, RowAlignmentLimits::default()).unwrap();
        let cells = &diff.sheets[0].cells;
        assert!(cells.iter().any(|c| c.status == DiffStatus::Added));
        assert!(cells.iter().any(|c| c.status == DiffStatus::Deleted));
        // Recognized as a single matched row (not a whole-row replace):
        // col1/col2/col5 agree unchanged, so the only reported cells are
        // the non-shared columns (3 and 7 deleted, 4 added).
        assert_eq!(cells.len(), 3);
    }

    #[test]
    fn content_similarity_skips_a_row_with_zero_populated_cells() {
        // Both rows are formatting-only (no value at all), at different
        // columns so their signatures don't accidentally coincide (which
        // would make them an exact Myers snake match instead). Neither
        // can be a patience anchor (populated_count == 0), and once
        // they're a Deleted/Inserted candidate pair in a content-
        // similarity span, row_similarity has nothing at all to compare
        // on either side -- exercises the `total == 0` branch, which must
        // return 0.0 (never a false Modified match) rather than dividing
        // by zero.
        let mut base_sheet = Sheet::new("Sheet1".to_string(), SheetVisibility::Visible);
        base_sheet.insert_cell(
            CellRef { row: 1, col: 1 },
            Cell {
                value: Some(text("Anchor")),
                style: None,
            },
        );
        base_sheet.insert_cell(
            CellRef { row: 2, col: 1 },
            Cell {
                value: None,
                style: None,
            },
        );
        base_sheet.insert_cell(
            CellRef { row: 3, col: 1 },
            Cell {
                value: Some(text("Anchor2")),
                style: None,
            },
        );
        let base = workbook(vec![base_sheet]);

        let mut target_sheet = Sheet::new("Sheet1".to_string(), SheetVisibility::Visible);
        target_sheet.insert_cell(
            CellRef { row: 1, col: 1 },
            Cell {
                value: Some(text("Anchor")),
                style: None,
            },
        );
        target_sheet.insert_cell(
            CellRef { row: 2, col: 2 },
            Cell {
                value: None,
                style: None,
            },
        );
        target_sheet.insert_cell(
            CellRef { row: 3, col: 1 },
            Cell {
                value: Some(text("Anchor2")),
                style: None,
            },
        );
        let target = workbook(vec![target_sheet]);

        let diff =
            diff_workbooks_aligned_rows(&base, &target, RowAlignmentLimits::default()).unwrap();
        let cells = &diff.sheets[0].cells;
        // The blank row is never mistakenly paired with the unrelated
        // populated row -- both are reported as plain delete/insert.
        assert!(cells
            .iter()
            .all(|c| c.status == DiffStatus::Added || c.status == DiffStatus::Deleted));
        assert!(!cells.is_empty());
    }

    #[test]
    fn error_valued_row_is_hashed_and_diffed_correctly() {
        // Exercises hash_cell_value's Error arm.
        let base = workbook(vec![sheet_with_values(
            "Sheet1",
            &[
                (1, 1, text("Anchor")),
                (2, 1, CellValue::Error("#DIV/0!".to_string())),
                (3, 1, text("Anchor2")),
            ],
        )]);
        let target = workbook(vec![sheet_with_values(
            "Sheet1",
            &[
                (1, 1, text("Anchor")),
                (2, 1, CellValue::Error("#N/A".to_string())),
                (3, 1, text("Anchor2")),
            ],
        )]);

        let diff =
            diff_workbooks_aligned_rows(&base, &target, RowAlignmentLimits::default()).unwrap();
        let cells = &diff.sheets[0].cells;
        assert!(!cells.is_empty());
    }

    #[test]
    fn gap_prefix_trim_leaving_a_pure_insert_remainder() {
        // The gap between anchors A and B holds a duplicated (non-anchor)
        // "M" pair on both sides, plus one extra "Extra" row only on the
        // target side: myers_diff_gap's own prefix trim consumes both M's
        // (limited by the shorter, base, side), leaving sub_n == 0 and a
        // pure Inserted remainder.
        let base = workbook(vec![sheet_with_values(
            "Sheet1",
            &[
                (1, 1, text("Z")),
                (2, 1, text("A")),
                (3, 1, text("M")),
                (4, 1, text("M")),
                (5, 1, text("B")),
                (6, 1, text("Y")),
            ],
        )]);
        let target = workbook(vec![sheet_with_values(
            "Sheet1",
            &[
                (1, 1, text("W")),
                (2, 1, text("A")),
                (3, 1, text("M")),
                (4, 1, text("M")),
                (5, 1, text("Extra")),
                (6, 1, text("B")),
                (7, 1, text("X")),
            ],
        )]);

        let diff =
            diff_workbooks_aligned_rows(&base, &target, RowAlignmentLimits::default()).unwrap();
        let cells = &diff.sheets[0].cells;
        assert_eq!(
            cells
                .iter()
                .filter(|c| c.status == DiffStatus::Added && c.new_value.as_ref().is_some())
                .count(),
            // Z->W, Y->X (2 replaced rows, 1 Added cell each) + "Extra"
            // (1 Added cell) = 3 Added cells total.
            3
        );
        assert!(cells.iter().all(|c| c.status != DiffStatus::Modified));
    }

    #[test]
    fn gap_prefix_trim_leaving_a_pure_delete_remainder() {
        // Mirror of the previous test: the extra unmatched row ("Extra")
        // is on the base side instead, so myers_diff_gap's prefix trim
        // (limited by the shorter, target, side) leaves sub_m == 0 and a
        // pure Deleted remainder.
        let base = workbook(vec![sheet_with_values(
            "Sheet1",
            &[
                (1, 1, text("Z")),
                (2, 1, text("A")),
                (3, 1, text("M")),
                (4, 1, text("M")),
                (5, 1, text("Extra")),
                (6, 1, text("B")),
                (7, 1, text("Y")),
            ],
        )]);
        let target = workbook(vec![sheet_with_values(
            "Sheet1",
            &[
                (1, 1, text("W")),
                (2, 1, text("A")),
                (3, 1, text("M")),
                (4, 1, text("M")),
                (5, 1, text("B")),
                (6, 1, text("X")),
            ],
        )]);

        let diff =
            diff_workbooks_aligned_rows(&base, &target, RowAlignmentLimits::default()).unwrap();
        let cells = &diff.sheets[0].cells;
        assert_eq!(
            cells
                .iter()
                .filter(|c| c.status == DiffStatus::Deleted && c.old_value.as_ref().is_some())
                .count(),
            3
        );
        assert!(cells.iter().all(|c| c.status != DiffStatus::Modified));
    }

    #[test]
    fn gap_internal_suffix_trim_appends_matched_rows() {
        // The gap between anchors A and B is [X, M] on base and [Y, M] on
        // target: X/Y differ (blocking the gap's own prefix trim), but M
        // matches at the gap's tail. M is duplicated elsewhere in base
        // (a second, unrelated "M" row after Y) so it can never qualify
        // as a whole-sheet patience anchor and is only ever resolved via
        // myers_diff_gap's own internal *suffix* trim.
        let base = workbook(vec![sheet_with_values(
            "Sheet1",
            &[
                (1, 1, text("Z")),
                (2, 1, text("A")),
                (3, 1, text("X")),
                (4, 1, text("M")),
                (5, 1, text("B")),
                (6, 1, text("Y")),
                (7, 1, text("M")), // extra, blocks M's anchor eligibility
            ],
        )]);
        let target = workbook(vec![sheet_with_values(
            "Sheet1",
            &[
                (1, 1, text("W")),
                (2, 1, text("A")),
                (3, 1, text("Q")),
                (4, 1, text("M")),
                (5, 1, text("B")),
                (6, 1, text("X2")),
            ],
        )]);

        let diff =
            diff_workbooks_aligned_rows(&base, &target, RowAlignmentLimits::default()).unwrap();
        let cells = &diff.sheets[0].cells;
        // The shared M row inside the gap produces no diff at all; only
        // the genuinely differing rows (Z/W, X/Q, Y/X2, plus the extra
        // trailing M with no target counterpart) show up.
        assert!(cells.iter().all(|c| c.status != DiffStatus::Modified));
        assert!(!cells.is_empty());
    }
}
