# `diff/best_effort.rs` Design Doc

*[日本語](best_effort.md)*

Design doc for `src/diff/best_effort.rs`. Picks, per sheet and without the caller choosing a mode up front, whichever of the three existing diff algorithms — [`diff::engine::diff_workbooks`](engine.en.md) (coordinate-based), [`diff::row_alignment::diff_workbooks_aligned_rows`](row_alignment.en.md) (Issue #4), [`diff::col_alignment::diff_workbooks_aligned_columns`](col_alignment.en.md) (Issue #5) — reports the least noise ([Issue #25](https://github.com/MinamiyamaKotaro/exceldiff/issues/25)). The diff preview comment `.github/workflows/xlsx-diff.yml` posts has no way to know ahead of time what kind of edit a changed file received (a value change, a row insertion, a column insertion), which is exactly why this automatic selection was needed.

## Background: how the design converged

Issue #25 went through four rounds of PoC ([comment #5418963848](https://github.com/MinamiyamaKotaro/exceldiff/issues/25#issuecomment-5418963848) → [#5419091215](https://github.com/MinamiyamaKotaro/exceldiff/issues/25#issuecomment-5419091215) → [#5419182619](https://github.com/MinamiyamaKotaro/exceldiff/issues/25#issuecomment-5419182619) → [#5419237433](https://github.com/MinamiyamaKotaro/exceldiff/issues/25#issuecomment-5419237433)) before converging on this design:

1. **Per-sheet, not per-workbook**: an earlier design that picked one strategy for the whole workbook was demonstrated to leave one sheet's cascade completely unresolved whenever different sheets received different kinds of edits (e.g. one sheet had a row inserted, another had a column inserted) — see [comment #5419091215](https://github.com/MinamiyamaKotaro/exceldiff/issues/25#issuecomment-5419091215).
2. **Short-circuit past alignment when the coordinate diff already reports <=1 change**: a real row/column insertion/deletion affecting any populated cell always produces at least one coordinate-diff entry (the shifted content's new coordinate) — it can't produce fewer than that — so once the coordinate-based count is already at most 1, neither aligned variant can possibly improve on it. Measured at roughly a 30x speedup on a 5,000-row × 20-column sheet.
3. **Skip column alignment once row alignment reaches `Ok(None)` (exactly 0)**: 0 is a hard floor no non-negative count can beat. This was initially assessed as "rarely reachable with realistic data," but a genuinely common case does reach it: inserting a *blank* row (no cells at all) is a pure, monotonic, contiguous shift with nothing new to report at all — confirmed directly in this file's own tests.

## Responsibilities / Scope

- Takes two [`Workbook`](../model/workbook.en.md)s and, for every sheet name present on both sides, evaluates [`diff::engine::diff_sheet`](engine.en.md) (coordinate-based), [`diff::row_alignment::align_sheet_rows`](row_alignment.en.md), and [`diff::col_alignment::align_sheet_columns`](col_alignment.en.md), keeping whichever `SheetDiff` has the fewest total changes (`sheet_total_changes` — cell changes + merge changes + a visibility flip), and composes the results into one `WorkbookDiff` (`diff_workbooks_best_effort`)
- Skips both alignment calls entirely for a sheet whose coordinate-based change count is already `<= 1`
- Skips the column-alignment call once row alignment returns `Ok(None)` (0 changes)
- Falls back to whatever other candidate is available (coordinate-based, or whichever alignment succeeded) when either alignment cost-caps out (`Error::RowAlignmentCostTooHigh`/`Error::ColumnAlignmentCostTooHigh`) for a sheet — never returns a `Result`, always a `WorkbookDiff`
- A sheet present on only one side (added/deleted) is delegated to [`diff_sheet`](engine.en.md) — nothing to align there, same treatment every other `diff_workbooks_*` function gives it
- **Explicitly out of scope**: the individual algorithms' implementations themselves (their own files), a new combined algorithm that aligns rows and columns simultaneously (see Open Questions — this module only picks among the three existing strategies, it doesn't produce a solution none of them can already reach), wiring into the caller (CLI/`markdown.rs`, see [markdown.en.md](../markdown.en.md))

## Key Types / Functions

```rust
pub fn diff_workbooks_best_effort(
    base: &Workbook,
    target: &Workbook,
    row_limits: RowAlignmentLimits,
    col_limits: ColumnAlignmentLimits,
) -> WorkbookDiff;

fn sheet_total_changes(s: &SheetDiff) -> usize; // private helper
```

See [`src/diff/best_effort.rs`](../../../src/diff/best_effort.rs) for the actual implementation.

## Dependencies

- Depends on: [`diff/engine.rs`](engine.en.md) (`diff_sheet`, `pub(crate)`), [`diff/row_alignment.rs`](row_alignment.en.md) (`align_sheet_rows`, widened to `pub(crate)` for this issue), [`diff/col_alignment.rs`](col_alignment.en.md) (`align_sheet_columns`, likewise widened to `pub(crate)`), [`diff/model.rs`](model.en.md) (`SheetDiff`/`WorkbookDiff`), [`diff/mod.rs`](mod.en.md) (`RowAlignmentLimits`/`ColumnAlignmentLimits`, via its re-export), [`model/workbook.rs`](../model/workbook.en.md) (`Workbook`)
- Depended on by: [`diff/mod.rs`](mod.en.md) (re-exports `diff_workbooks_best_effort`), [`markdown.rs`](../markdown.en.md)'s `diff_file_section_from_paths` (calls this in the `"M"` branch, [Issue #25](https://github.com/MinamiyamaKotaro/exceldiff/issues/25))

## Error Handling Policy

`diff_workbooks_best_effort` itself never returns a `Result`. An `Err` from either alignment attempt (a cost cap exceeded) is swallowed on the spot, falling back to another candidate for that sheet — the same "carry a failure as data" design [`markdown.rs`'s error handling policy](../markdown.en.md) and [Issue #32's "never stop the whole comment for one file" policy](../markdown.en.md) already establish.

## Test Plan

- Row insertion and column insertion each individually no longer cascade (checked against the coordinate-based result)
- A mixed-edit workbook (a row-insertion sheet and a column-insertion sheet present at once) has both sheets optimized independently — the exact problem a whole-workbook mode choice couldn't solve
- An unchanged sheet is fully short-circuited and omitted from the result
- A single-cell change (count of 1) matches the coordinate-based result byte-for-byte, confirming alignment was never attempted (short-circuit correctness)
- **A blank row insertion reaches the `Ok(None)` floor and the sheet is omitted from the result entirely** — concrete proof this branch is reachable with realistic data, not just a contrived one
- Both row and column alignment given deliberately tiny cost limits fall back safely to the coordinate-based result, without panicking or propagating an error
- Sheet addition/deletion (present on only one side) matches every other `diff_workbooks_*` function's behavior

## Open Questions

1. **A combined algorithm that aligns rows and columns simultaneously**: inherited, still-unresolved from [`diff/mod.rs` Open Question 1](mod.en.md)/[col_alignment.en.md Open Question 1](col_alignment.en.md)/[row_alignment.en.md Open Question 1](row_alignment.en.md). This module only picks among the three existing strategies, so it can't fully resolve an edit that shifts both rows and columns on the same sheet at once (the Issue #25 verification measured only a ~32% improvement for that compound case).
2. **Whether to note which strategy was chosen in the rendered Markdown**: useful for debugging/trust, but could also be noise in the comment body. Currently not surfaced at all — [`markdown.rs`](../markdown.en.md) passes only the resulting `WorkbookDiff` to `FileStatus::Modified`, with no channel for which strategy `diff_workbooks_best_effort` picked.
3. **Whether `RowAlignmentLimits`/`ColumnAlignmentLimits` should be configurable as Action inputs**: currently always `::default()`. Revisit alongside the Action inputs/outputs design ([Issue #24](https://github.com/MinamiyamaKotaro/exceldiff/issues/24)).
