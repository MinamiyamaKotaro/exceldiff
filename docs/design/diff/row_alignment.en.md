# `diff/row_alignment.rs` Design Doc

*[日本語](row_alignment.md)*

Design doc for `src/diff/row_alignment.rs`. Complements [`diff/engine.rs`](engine.en.md)'s coordinate-based diff (`diff_workbooks`) with a capped, opt-in row-alignment-based diff (`diff_workbooks_aligned_rows`) ([Issue #4](https://github.com/MinamiyamaKotaro/exceldiff/issues/4)). Its purpose is to stop a row insertion/deletion from cascading into spurious diffs for every cell below it.

## Background

The [Issue #3](https://github.com/MinamiyamaKotaro/exceldiff/issues/3) PoC (`poc/issue3-poc`) implemented a combined row+column 2D LCS aligner with no cost cap at all, costing O(distinct_rows² + distinct_cols²) — ~13s/128MB for a single 4,000-row alignment — which directly contradicted this crate's design goal of handling "grid-paper Excel" sheets with extreme row/column counts. `engine.rs` therefore shipped the coordinate-based engine as the default instead (see [engine.en.md](engine.en.md)), deferring alignment-based diffing to separate, capped, opt-in issues: columns ([Issue #5](https://github.com/MinamiyamaKotaro/exceldiff/issues/5), implemented) and rows (this Issue #4).

The O(distinct_cols²) DP that worked for columns does not transfer to rows as-is. Excel permits up to 1,048,576 rows per sheet but only 16,384 columns, so an algorithm that's safely boundable for columns is unusable at row counts real workbooks actually have. As the Issue #3 PoC measured, O(distinct_rows²) costs minutes to hours and tens of gigabytes at tens-of-thousands-of-rows scale.

Issue #4 went through 8 rounds of PoC investigation (`poc/issue4-poc` through `poc/issue4-poc-v8`, all disposable and uncommitted per this repo's convention — full detail is in the GitHub issue's comment history) before converging on this design:

1. **Hash-anchored patience diff (the same idea as `git diff --patience`) to stay O(n log n)**: rows whose content is unique on both sides become anchors, and the longest order-preserving subsequence of anchors (an LIS) becomes the confirmed matches. O(n) to build, O(k log k) to align. Unlike the O(distinct_rows²) DP, this stays fast at real-world scale (sub-second at 1,000,000 rows in the favorable case).
2. **Myers diff resolves the unresolved gaps between anchors, but the backtrace must decode every step**: an early PoC round (`poc/issue4-poc-v2`) found that recording only the snake (diagonal) steps and bridging the rest by array index reproduced a large false-`Modified` cascade *even while Myers stayed within its budget* (`found == true`) — 18,240 false positives in a 5,000-row duplicate-heavy sheet test case (see the Issue #4 comment thread for detail). `poc/issue4-poc-v4` fixed this by decoding every step (diagonal, vertical, horizontal) directly.
3. **The budget-exceeded fallback is a safe "delete everything, insert everything"**: not a positional-pairing fallback (which carries the same risk as the v1/v2 bug) — any row Myers couldn't confirm a correspondence for is reported as a plain Added/Deleted. This never fabricates a false `Modified`, at the cost of not being the minimal edit — the same "safe over optimal" trade-off `MIN_DISTINCT_FOR_CONTENT_MATCH` already accepts.
4. **Content-similarity pairing is only safe once the span it operates on is capped**: `poc/issue4-poc-v6` proposed pairing Deleted/Inserted rows by content similarity (column-value agreement) instead of adjacency, but applying it to an entire unresolved span costs O(span²) — measured directly (`poc/issue4-poc-v7`) at 29–36× slower than without it once a span spans an entire replaced block (a 4,000-row block took over 9 seconds). `poc/issue4-poc-v8` capped the span size (`CONTENT_SIMILARITY_SPAN_CAP`), recovering v5-equivalent speed.
5. **When content similarity ties, the correspondence isn't uniquely determined in principle**: `poc/issue4-poc-v7`/`v8` demonstrated with a minimal example that which row pairs with which can flip purely due to row ordering — an accident of the algorithm, not a real identity signal. The aggregate diff counts stay correct, but this ambiguity is a structural limitation no heuristic resolves, accepted here as a "known limitation" (see "Content-similarity pairing is not always unique" below).

This implementation reflects the above design, plus a scoping decision to limit implementation scope to rows only (see "Columns are not realigned" below).

## Responsibility / Scope

- Provides `diff_workbooks_aligned_rows`, which matches rows by content (not coordinate) before computing cell diffs
- Performs the matching budget check (`RowAlignmentLimits`) before any O(gap²) matching work begins, returning `Err(Error::RowAlignmentCostTooHigh)` on overrun — the same design decision `diff::col_alignment` makes (never silently downgrade to `diff_workbooks`)
- A sheet present on only one side reuses `diff::engine::diff_sheet` as-is
- Merged-region diffs (`SheetDiff::merges`) reuse `diff::engine::diff_merges` as-is; merges are not subject to row alignment
- **Not responsible for**: the coordinate-based diff computation itself ([`engine.rs`](engine.en.md)), integration with column alignment ([`col_alignment.rs`](col_alignment.en.md), implemented for Issue #5 — see "Columns are not realigned" below), persisting the alignment result (specifically `CellDiff::old_row`) to `diff::storage` (same kind of open question as [storage.en.md Open Question 6](storage.en.md))

## Columns are not realigned (Issue #5 is separate and already implemented, but not integrated)

This implementation aligns rows only; columns stay coordinate-pinned throughout — the mirror image of `diff::col_alignment` aligning columns only and holding rows coordinate-pinned. Running row alignment and column alignment together on a sheet where both a row and a column were inserted is out of scope here: `diff::col_alignment`'s own "Rows are not realigned" doc section already identifies the integration points needed (feeding a row mapping into `diff_matched_columns`'s merge-join and `count_matching_rows`) — the same kind of integration is needed in the other direction (feeding a column mapping into this module), and the two-way combination is left as future work.

## Content-similarity pairing is not always unique (known limitation)

When multiple candidate rows are *equally* similar to a deleted row (e.g. several rows share the same descriptive columns and differ only in one value column), which one gets paired depends on the order Myers' backtrace produces them in — an accident of hash/array order, not a genuine identity signal. Verified directly with a minimal example (`poc/issue4-poc-v7`/`v8`): the aggregate `added`/`modified`/`deleted` counts stay correct, but which row's old value gets attributed to which row's new value can be ambiguous in this tie case. No tie-break rule (position proximity or otherwise) resolves this in general — an inherent limitation of similarity-based matching, accepted here the same way `diff::col_alignment`'s `MIN_DISTINCT_FOR_CONTENT_MATCH` gate accepts that a coincidental match and a genuine low-cardinality change can't always be told apart.

## Key Types / Functions

```rust
pub struct RowAlignmentLimits {
    pub max_gap_myers_d: usize, // per-gap Myers edit-distance budget (checked against MAX_GAP_MYERS_D_CEILING independently)
    pub max_cost: usize,        // cap on 2 * max(distinct_rows_base, distinct_rows_target) * max_gap_myers_d
}

pub fn diff_workbooks_aligned_rows(
    base: &Workbook,
    target: &Workbook,
    limits: RowAlignmentLimits,
) -> Result<WorkbookDiff> { ... }
```

Algorithm (per sheet present on both sides):

1. The budget check looks at the memory bound first: if `limits.max_gap_myers_d > MAX_GAP_MYERS_D_CEILING`, return an error immediately. This is a row-count-independent memory cap — `myers_diff_gap`'s `flat_trace` buffer is O(max_gap_myers_d²), so the row-count-weighted time budget alone can't stop that buffer from ballooning to gigabytes if a caller raises both `max_cost` and `max_gap_myers_d` together, the same reason `diff::col_alignment::MAX_COLUMN_PAIR_COUNT` was needed on top of column alignment's `max_cost` alone (caught in review on PR #21).
2. Walk `iter_cells()` once to find the distinct row count on each of base/target (`distinct_row_count`, true O(cells) — since `Sheet::iter_cells()` already yields rows in ascending order, this just counts row-number transitions rather than paying `BTreeSet`'s extra O(log distinct_rows) insertion cost per row; caught in review on PR #21).
3. Time budget check: if `2 * max(distinct_rows_base, distinct_rows_target) * limits.max_gap_myers_d > limits.max_cost`, return an error immediately, before any real matching work (see `MAX_ROW_ALIGNMENT_COST`'s doc comment for the measured basis).
4. Extract each row's content (`RowContent`: a `Vec` from column to cell, a content signature hashed with `RandomState` (a fresh, process-randomized seed), and the real (non-formatting-only) cell count).
5. Trim the common prefix/suffix in O(1) per row — as long as the same signature continues from either end, those rows are confirmed matches with zero O(n²) work.
6. Within the trimmed "active region," rows whose signature is unique on both sides become anchor candidates; a patience-sort LIS (`lis_indices`) finds the largest order-preserving set of confirmed matches (`align_rows`).
7. Each gap between confirmed matches is resolved by `myers_diff_gap` via Myers diff. The backtrace decodes every step directly — diagonal (Match), vertical (Inserted), horizontal (Deleted) — never taking the shortcut of recording only the snake and bridging the rest positionally. When the budget (`max_gap_myers_d`) is exceeded, `fill_gap_no_match` reports the whole span as a safe delete-everything + insert-everything.
8. Any leftover contiguous Deleted/Inserted span Myers resolved but couldn't explain via an exact signature match is handed to `merge_leftover_spans_by_content_similarity`, which attempts content-similarity pairing — skipped (left as plain Deleted/Inserted) whenever the span exceeds `CONTENT_SIMILARITY_SPAN_CAP`, to avoid its O(span²) cost.
9. Matched row pairs are diffed cell-by-cell via a merge-join (`diff_matched_rows`), attaching `CellDiff::old_row` when the row shifted. Every populated cell of an unmatched row becomes a plain Added/Deleted.

## Dependencies

- Depends on: [`diff/engine.rs`](engine.en.md) (reuses `diff_sheet`/`diff_merges`/`visibility_diff`, made `pub(crate)`), [`diff/model.rs`](model.en.md) (`CellDiff`/`SheetDiff`/`WorkbookDiff`/`DiffStatus` — reuses the same `CellDiff` type `diff::col_alignment` does, populating only `old_row`), [`error.rs`](../error.en.md) (`Error::RowAlignmentCostTooHigh`), [`json.rs`](../json.en.md) (`cell_value_to_json`/`style_to_json`), [`model/sheet.rs`](../model/sheet.en.md) (`Sheet::iter_cells`)
- Depended on by: [`diff/mod.rs`](mod.en.md) (re-exports `diff_workbooks_aligned_rows`/`RowAlignmentLimits`)

## Error Handling Policy

- `diff_workbooks_aligned_rows` returns `Result<WorkbookDiff>` (fallible, unlike `diff_workbooks`) — it returns `Error::RowAlignmentCostTooHigh` on budget overrun. The same design decision `diff::col_alignment` makes: never silently hide from the caller whether the alignment they explicitly opted into actually ran. A caller that wants automatic fallback can `match` this error and call `diff_workbooks` itself.

## Testing Strategy

Unit tests in `src/diff/row_alignment.rs` (build `Sheet`/`Workbook` directly via the public model API):

- Row insertion/deletion doesn't cascade (`row_insertion_does_not_cascade_when_aligned`, `row_deletion_does_not_cascade_when_aligned`). A genuine value change within a shifted row is correctly detected with `old_row` set; an unshifted matched row leaves `old_row` as `None` (`old_row_is_absent_when_the_matched_row_did_not_shift`)
- Edits scattered across a low-cardinality, duplicate-heavy sheet don't cascade (`low_cardinality_duplicated_rows_with_scattered_insertion_do_not_cascade`) — a direct regression test for the bug found in `poc/issue4-poc-v2`
- Two or more consecutive modified rows are each detected independently as `Modified` (`consecutive_modified_rows_are_each_detected_as_modified`) — a direct regression test for the limitation found in `poc/issue4-poc-v6` (an adjacency-only merge rule failing here)
- Budget overrun correctly returns `Error::RowAlignmentCostTooHigh` (`row_alignment_cost_too_high_is_reported_fail_fast`)
- A sheet present on only one side is delegated to the coordinate engine even through alignment (`sheet_present_on_only_one_side_reuses_the_coordinate_engine_through_alignment`)
- `diff_workbooks` (the default engine) is unaffected by this feature existing (`diff_workbooks_default_behavior_is_unaffected_by_row_alignment_existing`)

[`tests/diff.rs`](../../../tests/diff.rs): `row_insertion_does_not_cascade_when_aligned_end_to_end` re-verifies cascade avoidance after parsing real `.xlsx`-equivalent byte streams through `parse_workbook_reader`.

Performance (measured, release build, Apple Silicon; measured against the PoC implementation — re-measuring after porting into the main implementation is future work):

- Myers diff's own cost (`poc/issue4-poc-v7`, a fully-disjoint replace block with no shared signatures at all, block size B, cost = 4B²): B = 4,000 (cost 64,000,000) took 282.44ms; the cost-normalized rate stayed roughly constant at ~4.4e-6 to 5.5e-6 ms/unit. `MAX_ROW_ALIGNMENT_COST` was derived from this measured rate with headroom (see the `MAX_ROW_ALIGNMENT_COST` doc comment in `src/diff/row_alignment.rs` for detail).
- Content-similarity pairing with no span cap was 29–36× slower than v5 (adjacent-pair-only merge) under the same conditions (`poc/issue4-poc-v7`). After `CONTENT_SIMILARITY_SPAN_CAP` (`poc/issue4-poc-v8`), this recovered to 0.8–1.4× v5.
- Realistic edit patterns (localized insertions/deletions/modifications scattered across a sheet with modest duplication) completed in under one second even at 1,000,000 rows (the `poc/issue4-poc` measurement series).

## Open Questions

1. **Integration with column alignment** ([Issue #5](https://github.com/MinamiyamaKotaro/exceldiff/issues/5)): as noted in "Columns are not realigned" above, the two-way integration is neither designed nor implemented.
2. **Persisting `old_row` to SQLite**: like `old_col` ([storage.en.md Open Question 6](storage.en.md)), `diff::storage` does not currently persist `old_row`. A caller wanting to persist `diff_workbooks_aligned_rows`'s output today has to save the `WorkbookDiff` as JSON separately.
3. **Content-similarity pairing's non-uniqueness**: as noted above, currently accepted as a known limitation. If stricter correspondence becomes necessary in practice (e.g. preferring a stable identifier column as the primary matching signal when one exists), this can be revisited.
4. **Whether `RowAlignmentLimits`'s defaults are well-calibrated**: `MAX_ROW_ALIGNMENT_COST`/`DEFAULT_MAX_GAP_MYERS_D` are based on PoC measurements; re-measuring end-to-end after porting into the main implementation (in particular, including `row_contents`'s memory cost) hasn't been done yet. May be adjusted based on real-world usage feedback.
