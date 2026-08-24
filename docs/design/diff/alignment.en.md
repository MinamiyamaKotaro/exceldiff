# `diff/alignment.rs` Design Doc

*[日本語](alignment.md)*

Design doc for `src/diff/alignment.rs`. Complements [`diff/engine.rs`](engine.en.md)'s coordinate-based diff (`diff_workbooks`) with a capped, opt-in column-alignment-based diff (`diff_workbooks_aligned_columns`) ([Issue #5](https://github.com/MinamiyamaKotaro/exceldiff/issues/5)). Its purpose is to stop a column insertion/deletion from cascading into spurious diffs for every cell to its right.

## Background

The [Issue #3](https://github.com/MinamiyamaKotaro/exceldiff/issues/3) PoC (`poc/issue3-poc`) implemented a combined row+column 2D LCS aligner with no cost cap at all, costing O(distinct_rows² + distinct_cols²) — ~13s/128MB for a single 4,000-row alignment — which directly contradicted this crate's design goal of handling "grid-paper Excel" sheets with extreme row/column counts. `engine.rs` therefore shipped the coordinate-based engine as the default instead (see [engine.md](engine.en.md)), deferring alignment-based diffing to separate, capped, opt-in issues: rows ([Issue #4](https://github.com/MinamiyamaKotaro/exceldiff/issues/4), unimplemented) and columns (this Issue #5).

For Issue #5, five rounds of PoC investigation (`poc/issue5-poc` through `poc/issue5-poc-v4`, all throwaway and never committed — full detail in the GitHub issue's comment history) produced concrete answers to its two open questions:

1. **How to pick the cap**: `align_columns`'s cost is proportional to `distinct_cols_base × distinct_cols_target × max_row`, not distinct-column-count squared alone. A column-count cap safe at 500 rows becomes ~10x too permissive at 50,000 rows, so a single flat column-count cap (the `MAX_MERGE_REGIONS` pattern) isn't enough — the budget has to be the product of distinct column counts and row count.
2. **Matching heuristic robustness**: the naive "any single matching cell is a candidate" rule has 15-38% precision. Replacing it with a `match_count ≥ max(2, 20% of column length)` threshold gets columns with ≥10 distinct values to 100% precision, but extremely low-cardinality columns (2-4 distinct values — booleans, status flags) with no header **cannot be judged safely by any content-based similarity score at all** — even an unbounded, all-pairs Sequence-LCS comparison still produced a 122% false-match rate there. What's needed isn't a better threshold, it's excluding such columns from content-based matching entirely up front (`MIN_DISTINCT_FOR_CONTENT_MATCH`).

This implementation reflects both conclusions, plus a scoping decision to align columns only (see "Rows are not realigned" below).

## Responsibility / Scope

- Provides `diff_workbooks_aligned_columns`, which matches columns by content rather than position before diffing cells
- Checks the alignment cost budget (`ColumnAlignmentLimits`) before doing any O(cols²) matching work, returning `Err(Error::ColumnAlignmentCostTooHigh)` on overrun rather than silently falling back to `diff_workbooks`'s result — the caller opted into alignment explicitly, so whether it actually ran shouldn't be hidden from them
- Reuses `diff::engine::diff_sheet` as-is for a sheet present on only one side (nothing to align when a whole sheet is new or gone)
- Reuses `diff::engine::diff_merges` as-is for `SheetDiff::merges`, not made column-alignment-aware (see "Merged regions are not column-aware" below)
- **Not responsible for**: the coordinate-based diff computation itself ([`engine.rs`](engine.en.md)), row alignment ([Issue #4](https://github.com/MinamiyamaKotaro/exceldiff/issues/4), unimplemented — see "Rows are not realigned" below), persisting the alignment result (specifically `CellDiff::old_col`) to `diff::storage` (see [storage.en.md Open Question 6](storage.en.md))

## Rows are not realigned (Issue #4 is separate and unimplemented)

Issue #4 (row insertion/deletion detection) has zero comments and zero implementation as of this design. Consequently this implementation aligns columns only; rows stay coordinate-pinned throughout. This matters because the row-shift-invariant column-matching schemes the PoC rounds explored (Bag-of-Values, Sequence-LCS) exist specifically to keep working when rows *also* shift at the same time — with rows held fixed, plain same-row-number value comparison is sufficient and no such machinery is needed. The design leaves the natural integration point open for when #4 lands: feeding a row mapping (`base_row -> target_row`) into `diff_matched_columns`'s merge-join, with no rework needed to column matching itself (steps 1-7 below).

## Merged regions are not column-aware

`SheetDiff::merges` is computed by calling `diff::engine::diff_merges` (coordinate-based) unchanged. Matching a merged region across a column shift (e.g. a merge whose origin cell moved because a column was inserted before it) is out of scope — Issue #5's focus is cell-diff cascade avoidance, and merge alignment was never separately requested.

## Key Types / Functions

```rust
pub struct ColumnAlignmentLimits {
    pub max_cost: usize,         // cap on distinct_cols_base * distinct_cols_target * sample_rows
    pub max_column_pairs: usize, // cap on distinct_cols_base * distinct_cols_target alone (row-count-independent memory bound)
}

pub fn diff_workbooks_aligned_columns(
    base: &Workbook,
    target: &Workbook,
    limits: ColumnAlignmentLimits,
) -> Result<WorkbookDiff> { ... }
```

Algorithm (per sheet present on both sides):

1. One pass over `iter_cells()` collects the set of distinct column indices on each side.
2. The budget check is two-part (a PR #20 review finding — see "Issues found during PR review" below): first `cols_base.len() * cols_target.len() > limits.max_column_pairs` (a row-count-independent bound on the `scores`/`dp` matrices' memory), then `cols_base.len() * cols_target.len() * sample_rows > limits.max_cost` (a bound on matching time). Either overrun fails immediately, before any O(cols²) matching work.
3. Extract each column's content (`ColumnContent`: row → cell as a `Vec`, cheaper than a `BTreeMap` since `iter_cells()` already yields rows in ascending order; a header — row 1's value, but only if it's `Text`, so a coincidental numeric match at row 1 in a headerless numeric sheet is never mistaken for a header match; and whether the column is eligible for content matching at all, i.e. holds at least `MIN_DISTINCT_FOR_CONTENT_MATCH` distinct values).
4. Score each candidate pair (`column_match_score`): an exact header match is always a candidate (with a bonus, `HEADER_MATCH_BONUS`, that outranks any purely content-based score). If both columns are content-match-eligible (≥`MIN_DISTINCT_FOR_CONTENT_MATCH` distinct values), a pair is a candidate once its matching-row count clears the threshold (20% of column length, minimum 2). Otherwise (low cardinality on either side), a pair is still a candidate if it's an **exact** match (equal row counts, every populated row agreeing) — see "Issues found during PR review" below.
5. A weighted, order-preserving LCS-style DP over the score matrix picks the best column alignment (`align_columns`) — each cell takes the max of three options (diagonal/match, skip-up, skip-left), the standard weighted-LCS recurrence (also a PR review fix, see below).
6. Matched column pairs are diffed cell-by-cell via a merge-join (`diff_matched_columns`), setting `CellDiff::old_col` whenever the column actually shifted. Unmatched columns are reported entirely as Added/Deleted.

## Issues found during PR review (PR #20)

GitHub Copilot's automated PR review flagged four critical issues in the first implementation, all confirmed and fixed:

1. **The DP transition wasn't the correct weighted-LCS recurrence**: it took the diagonal unconditionally whenever `score > 0`, without comparing against the "skip up"/"skip left" options. The reviewer gave a concrete counterexample: one base column with two candidate target columns scoring 10 and 2 — if the weaker match (2) is evaluated where a stronger one (10) was reachable by skipping past it first, the weaker match wins and the true match gets reported as a spurious insert instead. Fixed to take the max of all three options at every cell.
2. **Formatting-only cells (value `None`) were counted as matching**: `Option<CellValue>`'s derived `PartialEq` makes `None == None` true, so `count_matching_rows` was counting two blank, formatting-only cells at the same row as a "match" with nothing actually compared. Fixed to require both sides to hold `Some` before counting a match.
3. **The budget check didn't account for memory independent of row count**: `max_cost` is weighted by row count, so a sheet with very few rows but very many columns (e.g. 1 row × ~3,162 columns per side) could stay under budget while the `scores`/`dp` matrices themselves still need O(cols²) memory regardless of row count (measured ~160MB at that shape). Added a second, row-count-independent budget, `max_column_pairs`.
4. **The low-cardinality gate excluded even genuinely unchanged columns**: a headerless column with fewer than 8 distinct values was excluded from content matching entirely in the original implementation — including one that hadn't changed at all and had merely shifted position, which got reported as a wholesale delete-plus-re-add instead of no diff, exactly the cascade this feature exists to avoid. Fixed by adding an "exact match" rescue: a low-cardinality pair is still accepted when every populated row agrees on both sides and the row counts match.

## Dependencies

- Depends on: [`diff/engine.rs`](engine.en.md) (`diff_sheet`/`diff_merges`/`visibility_diff`, widened to `pub(crate)` for reuse — so a one-sided sheet, merges, and visibility are handled *identically* to the coordinate-based engine rather than reimplemented), [`diff/model.rs`](model.en.md) (`CellDiff`/`SheetDiff`/`WorkbookDiff`/`DiffStatus` — reuses the same `CellDiff` type, populating only `old_col`), [`error.rs`](../error.en.md) (`Error::ColumnAlignmentCostTooHigh`), [`json.rs`](../json.en.md) (`cell_value_to_json`/`style_to_json`), [`model/sheet.rs`](../model/sheet.en.md) (`Sheet::iter_cells`, `max_row`/`max_col`)
- Depended on by: [`diff/mod.rs`](mod.en.md) (re-exports `diff_workbooks_aligned_columns`/`ColumnAlignmentLimits`)

## Error Handling Policy

- `diff_workbooks_aligned_columns` returns `Result<WorkbookDiff>` (fallible, unlike `diff_workbooks`) specifically so a budget overrun surfaces as `Error::ColumnAlignmentCostTooHigh` rather than being silently absorbed — a deliberate decision, confirmed with the user, that an explicitly opted-into operation shouldn't hide whether it actually ran. A caller wanting automatic fallback can match on this error and call `diff_workbooks` itself.

## Testing Strategy

Unit tests in `src/diff/alignment.rs` (build `Sheet`/`Workbook` directly via the public model API):

- Column insertion/deletion produces no cascade (`column_insertion_does_not_cascade_when_aligned`, `column_deletion_does_not_cascade_when_aligned`) — the counterpart to `engine.rs`'s `column_insertion_cascades_into_shift_diffs_by_design` (added alongside this change to document the default engine's column-cascade behavior explicitly, mirroring the existing row test)
- A genuine value change inside a shifted column still surfaces, with the correct `old_col` (`genuine_modification_survives_column_alignment`)
- `old_col` stays `None` when the matched column pair didn't actually shift (`old_col_is_absent_when_the_matched_column_did_not_shift`)
- Low-cardinality, headerless columns too short to clear `MIN_DISTINCT_FOR_CONTENT_MATCH` safely fall back to coordinate-based diffing (`low_cardinality_headerless_columns_fall_back_to_coordinate_diff_safely`); the same columns align correctly once a header is present (`header_match_rescues_low_cardinality_column_alignment`)
- Exact-match rescue (item 4 under "Issues found during PR review" above): an unchanged, unshifted low-cardinality column produces zero diff (`identical_low_cardinality_headerless_column_produces_no_diff`); a shifted-but-byte-identical low-cardinality column is correctly aligned (`shifted_but_unchanged_low_cardinality_headerless_column_is_recognized_via_exact_match`)
- Both budget kinds return `Error::ColumnAlignmentCostTooHigh` on overrun: the row-weighted cost (`distinct_column_cost_over_the_limit_is_column_alignment_cost_too_high`) and the row-count-independent pair count (`column_pair_count_over_the_limit_is_column_alignment_cost_too_high_even_with_one_row`)
- `diff_workbooks` (the default engine) is unaffected by this feature existing (`diff_workbooks_default_behavior_is_unaffected_by_alignment_existing`)

[`tests/diff.rs`](../../../tests/diff.rs): `column_insertion_does_not_cascade_when_aligned_end_to_end` re-verifies cascade avoidance through the real parse pipeline (`parse_workbook_reader` on constructed `.xlsx`-shaped bytes), not just the public-model-API fixtures the unit tests build directly.

Performance (measured, release build, Apple Silicon; `diff_workbooks_aligned_columns` end to end, not a microbenchmark of an inner loop):

| distinct cols (base=target) | rows | cost | measured time |
|---:|---:|---:|---:|
| 50 | 500 | 1,250,000 | 8.7ms |
| 200 | 500 | 20,000,000 | 108ms |
| 100 | 5,000 | 50,000,000 | 408ms |
| 20 | 50,000 | 20,000,000 | 364ms |
| 100 | 50,000 | 500,000,000 | 11.9s |

Cost-normalized time (ms / cost) isn't perfectly constant across shapes (ranging ~5.4e-6 to ~2.4e-5 ms/unit) — it's worse for shapes with very high row counts and few columns, where `ColumnContent` construction and `has_enough_distinct_values`'s O(cols × rows) setup work stops being negligible next to the O(cols² × rows) matching work. `MAX_COLUMN_ALIGNMENT_COST` (default 10,000,000) uses the worst observed rate with headroom, landing the worst case inside the same "few hundred ms" budget class `MAX_MERGE_REGIONS` targets (~240ms).

Note: `count_matching_rows` (the per-column-pair matching-row count) was originally implemented with `BTreeMap::get` lookups, and measurement showed this added a non-negligible O(log rows) factor at high row counts. Switching `ColumnContent::cells` from a `BTreeMap` to a row-sorted `Vec` (relying on `Sheet::iter_cells()` already yielding rows in ascending order) and rewriting the comparison as the same merge-join `diff::engine::diff_cells` uses eliminated that log factor, measured up to ~5x faster on the same inputs — the numbers above are from the code after that optimization.

## Open Questions

1. **Integration with row alignment** ([Issue #4](https://github.com/MinamiyamaKotaro/exceldiff/issues/4)): the integration point itself is designed (see "Rows are not realigned" above), but not implemented, since Issue #4 itself is unimplemented.
2. **Persisting `old_col` to SQLite** ([Issue #5](https://github.com/MinamiyamaKotaro/exceldiff/issues/5)): see [storage.en.md Open Question 6](storage.en.md).
3. **Signaling extreme-low-cardinality, headerless columns explicitly**: the current behavior falls back safely to coordinate-based diffing for these, but there's no way for a caller to learn "alignment was attempted here and abandoned due to low cardinality" (e.g. a dedicated `SheetDiff` flag) — left for if that need arises.
4. **Column-aware merged-region alignment**: out of scope per "Merged regions are not column-aware" above; to be considered separately if requested.
