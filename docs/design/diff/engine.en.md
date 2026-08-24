# `diff/engine.rs` Design Doc

*[日本語](engine.md)*

Design doc for `src/diff/engine.rs`. Computes the `WorkbookDiff` [`diff/model.rs`](model.en.md) defines from two `model::Workbook`s ([Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3); style and merged-cell diffs added by [Issue #8](https://github.com/MinamiyamaKotaro/xlsxparser/issues/8)).

## How the algorithm was chosen

[Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3)'s PoC (`poc/issue3-poc`) implemented a "2D LCS alignment" (column LCS, then row LCS) that detects cell-coordinate shifts caused by row/column insertion or deletion. Building and running it confirmed it works correctly on small samples ([Issue #3 comment](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3#issuecomment-5382524419), `poc/issue3-poc/output/verification_report.md`). Benchmarking `align_columns`/`align_rows_2d` directly, however, measured a clear **O(distinct_rows² + distinct_cols²)** time/memory behavior:

| Target | Input size | Elapsed |
|---|---|---|
| align_rows_2d | 500 rows × 50 matched cols | 200ms |
| align_rows_2d | 1,000 rows × 50 matched cols | 787ms (×3.9) |
| align_rows_2d | 2,000 rows × 50 matched cols | 3.16s (×4.0) |
| align_rows_2d | 4,000 rows × 50 matched cols | 12.98s (×4.1) |

Elapsed time increases roughly 4x (=2²) every time the size doubles; the DP table itself is a `(R+1)×(R+1)` `usize` matrix, so its memory also scales with the square of the row count (~128MB at 4,000 rows, an estimated ~80GB at 100,000 rows). This directly contradicts this crate's own stated design goal ("optimized for grid-paper Excel files with an extreme number of rows/columns," per `lib.rs`), and is inconsistent with the defensive pattern `resolve::merge::MAX_MERGE_REGIONS`/`resolve::column_width::MAX_COLUMN_WIDTH_RANGES` ([resolve/merge.md](../resolve/merge.en.md)/[resolve/column_width.md](../resolve/column_width.en.md)) already establishes: "cap any structure that can become O(N²) and fail fast."

For this reason, this file does **not** port the PoC's 2D LCS alignment as-is, and instead adopts a **coordinate-based lightweight diff as the default**. Row/column-insertion detection (alignment-based diffing) is tracked as a capped, opt-in feature under separate issues ([#4](https://github.com/MinamiyamaKotaro/xlsxparser/issues/4) rows, [#5](https://github.com/MinamiyamaKotaro/xlsxparser/issues/5) columns) and is out of scope for this file.

## Style diffs are sparser, on purpose (Issue #8)

`CellDiff::old_style`/`new_style` are populated only when the style actually differs. This is asymmetric with `old_value`/`new_value` (always both present on `Modified` even when the value itself is unchanged — a style-only change still shows `old_value == new_value`, an already-shipped, tested behavior). A value change is the very reason a `CellDiff` exists at all, so showing both sides costs nothing; style is a secondary dimension most `Modified` cells never touch, so always attaching a pair would bloat the common case. Retroactively making `old_value`/`new_value` match this sparser convention was considered and rejected, to avoid silently changing already-shipped, tested behavior (Issue #8 PR review discussion).

## Merged-region diffs are sheet-level (Issue #8)

`diff_merges` reports merge changes on `SheetDiff::merges`, not folded into the origin cell's `CellDiff` — a deliberate difference from the full-snapshot JSON (`json.rs`), which embeds a merge as the origin cell's `rowSpan`/`colSpan`. A diff's job is to report discrete changes, and an `Added`/`Deleted` merge with no value/style change has no natural `CellDiff` to attach to (synthesizing an empty one just for this would be its own kind of awkwardness). A sheet-level list was chosen instead, mirroring how `json.rs` already treats `images`/`columns` as sheet-level for the same "doesn't naturally belong to one cell" reason (Issue #8 PR review discussion).

## Responsibility / Scope

- `diff_paths`, which diffs two files directly by path (calls [`parse_workbook`](../lib.en.md) twice internally, then delegates to `diff_workbooks`)
- `diff_workbooks`, which compares two already-parsed `Workbook`s (the core public API)
- Walks the union of both sides' sheet names, handling a sheet present on only one side (`Added`/`Deleted`) and a sheet present on both (`Modified` — judging whether visibility changed and/or any cell/merge diffs exist)
- Computes one sheet's cell diffs via a "merge-join" that advances two iterators over `Sheet::iter_cells`'s `CellRef`-ascending (row-then-col) order in lockstep (`diff_cells`): compares coordinates, and where they match, compares value/style to decide whether `Modified` applies; a coordinate present on only one side is `Added`/`Deleted` directly
- Computes one sheet's merge diffs (`diff_merges`) by looking up `Sheet::merged_regions()` (a new `pub(crate)` accessor, Issue #8) — a `HashMap<CellRef, MergedRegion>` — keyed by origin coordinate; see below for granularity and complexity
- **Not responsible for**: the diff result type definitions ([`diff/model.rs`](model.en.md)), SQLite persistence ([`diff/storage.rs`](storage.en.md) — style/merge diffs are also persisted now, as of [Issue #9](https://github.com/MinamiyamaKotaro/xlsxparser/issues/9)), an alignment-based diff that detects row/column insertion (Open Question 1, Issue #4/#5)

## Key Types / Functions (draft)

```rust
use crate::diff::model::{CellDiff, DiffStatus, SheetDiff, WorkbookDiff};
use crate::error::Result;
use crate::json::{cell_value_to_json, visibility_tag};
use crate::model::{Cell, CellRef, Sheet, Workbook};
use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::path::Path;

/// Convenience function: parses `base_path`/`target_path` and diffs them.
pub fn diff_paths(
    base_path: impl AsRef<Path>,
    target_path: impl AsRef<Path>,
) -> Result<WorkbookDiff> {
    let base = crate::parse_workbook(base_path)?;
    let target = crate::parse_workbook(target_path)?;
    Ok(diff_workbooks(&base, &target))
}

/// The core function: diffs two `Workbook`s, matching sheets by name.
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

/// Diffs one sheet. `None` when it exists on both sides with identical
/// visibility and zero cell diffs (nothing to report).
fn diff_sheet(name: &str, base: Option<&Sheet>, target: Option<&Sheet>) -> Option<SheetDiff> {
    // Handles the 4 cases: (None, Some), (Some, None), (Some, Some),
    // (None, None). See src/diff/engine.rs for the full match.
    todo!()
}

/// Merge-joins base/target's cells in one linear pass, relying on
/// `Sheet::iter_cells` already being CellRef-ascending (row-major).
/// O(base_cells + target_cells), O(1) extra memory beyond the output.
fn diff_cells(base: &Sheet, target: &Sheet) -> Vec<CellDiff> {
    let mut out = Vec::new();
    let mut b = base.iter_cells().peekable();
    let mut t = target.iter_cells().peekable();

    loop {
        match (b.peek(), t.peek()) {
            (Some(&(br, bc)), Some(&(tr, tc))) => match br.cmp(&tr) {
                Ordering::Less => { /* base-only -> Deleted */ }
                Ordering::Greater => { /* target-only -> Added */ }
                Ordering::Equal => { /* compare value/style -> Modified or unchanged */ }
            },
            (Some(&(br, bc)), None) => { /* Deleted */ }
            (None, Some(&(tr, tc))) => { /* Added */ }
            (None, None) => break,
        }
    }
    out
}
```

The merge diff itself (skeleton):

```rust
fn diff_merges(base: &Sheet, target: &Sheet) -> Vec<MergeDiff> {
    let base_merges = base.merged_regions();   // &HashMap<CellRef, MergedRegion>
    let target_merges = target.merged_regions();

    let mut out = Vec::new();
    for (&origin, base_region) in base_merges {
        match target_merges.get(&origin) {
            Some(target_region) if target_region.end != base_region.end => {
                out.push(MergeDiff { status: Modified, start: origin.into(), .. });
            }
            Some(_) => {} // unchanged
            None => out.push(MergeDiff { status: Deleted, .. }),
        }
    }
    for (&origin, target_region) in target_merges {
        if !base_merges.contains_key(&origin) {
            out.push(MergeDiff { status: Added, .. });
        }
    }

    // merged_regions is a HashMap with no order, so only the (typically
    // much smaller) set of actual differences is sorted at the end,
    // cheaper than sorting every merge up front.
    out.sort_by_key(|m| (m.start.row, m.start.col));
    out
}
```

(See `src/diff/engine.rs` for the complete implementation of `diff_sheet`/`diff_cells`/`diff_merges`/`cell_diff_added`/`cell_diff_deleted`/`cell_diff_modified` — only the skeleton is shown here.)

## Dependencies

- Depends on: [`diff/model.rs`](model.en.md) (`CellDiff`, `DiffStatus`, `MergeDiff`, `SheetDiff`, `WorkbookDiff`), [`json.rs`](../json.en.md) (`cell_value_to_json`, `style_to_json`, `visibility_tag` — all widened to `pub(crate)` for reuse from this file), [`model/sheet.rs`](../model/sheet.en.md) (`Sheet::iter_cells`, `Sheet::merged_regions` — new in Issue #8), [`model/workbook.rs`](../model/workbook.en.md) (`Workbook::sheets`, `Workbook::sheet`), [`lib.rs`](../lib.en.md) (`parse_workbook` — from `diff_paths`), [`error.rs`](../error.en.md) (`Result`)
- Depended on by: [`diff/mod.rs`](mod.en.md) (re-exports `diff_paths`/`diff_workbooks`), [`diff/storage.rs`](storage.en.md)'s own unit tests (calls `diff_workbooks` to get a `WorkbookDiff` before handing it to `save_diff`)

Reusing `json.rs`'s `cell_value_to_json`/`style_to_json`/`visibility_tag` is for the same reason [model.md Dependencies](model.en.md) reuses `JsonCellValue`/`JsonStyle`: guaranteeing, at both the type and implementation level, that the same cell value, style, and visibility always render identically whether they reach output via a full snapshot (`to_json_string`) or a diff (`diff_workbooks`).

`Sheet::merged_regions()` (new) replaces `poc/issue8-poc`'s `iter_cells`-based reconstruction (O(cells), the only option available to a PoC external to the crate) with direct O(merges) access in the real implementation — measured at ~4.3ms for 300,000 cells with only 10 merges (see Performance below), which is wasted work on top of `diff_cells` already walking every cell once for value comparison.

## Error Handling Policy

- `diff_workbooks` returns no error (not `Result`) — both `Workbook`s are already parsed, and there is no external failure mode (I/O, malformed XML) left in the comparison step itself.
- `diff_paths` propagates errors from its two internal `parse_workbook` calls via `?` unchanged. Whichever of `base_path`/`target_path` fails to parse, the corresponding `Error` variant [pipeline.md's Error Handling Policy](../pipeline.en.md) defines reaches the caller as-is.

## Testing Strategy

Unit tests inside `src/diff/engine.rs` (build `Sheet`/`Workbook` directly via the public model API — never touch ZIP/XML):

- Two byte-for-byte identical workbooks produce an empty `WorkbookDiff` (`sheets: []`)
- A cell value change is detected as `Modified` (carrying both `old_value`/`new_value`)
- Cell addition/deletion are detected as `Added`/`Deleted` respectively
- Adding/removing a whole sheet reports every one of its cells as `Added`/`Deleted`
- A `SheetDiff` is still reported for a visibility-only change with zero cell diffs (`cells: []` is not itself a reason to omit)
- A style-only change (value unchanged) is detected as `Modified`, with `new_style` correctly carrying the new style (`old_style` staying `None` when the base side had no style at all) (`style_only_change_is_reported_as_modified_with_new_style_populated`)
- A value-only change (style unchanged) leaves both `old_style`/`new_style` as `None` (`value_only_change_carries_no_style_diff`) — the direct regression test for the asymmetry above
- Adding/deleting a styled cell reports only `new_style`/`old_style` respectively (the other stays `None`) (`added_cell_with_a_style_reports_new_style_only`)
- Merge addition/deletion/extent-change/no-change are each detected as `Added`/`Deleted`/`Modified`/(no diff) (`merge_added_is_detected_even_with_no_cell_changes`, `merge_deleted_is_detected`, `merge_extent_change_is_reported_as_modified`, `unchanged_merge_produces_no_diff_at_all`) — including that a merge-only change (no value/style change at all) still produces a `SheetDiff`
- Adding/removing a whole sheet reports its merges as `Added`/`Deleted` too (`sheet_added_reports_its_merges_as_added_too`, `sheet_deleted_reports_its_merges_as_deleted_too`)
- **Regression test for the algorithm's intended tradeoff**: a case where inserting one row shifts every subsequent row explicitly confirms the shifted-but-otherwise-unchanged rows cascade into the diff too (`row_insertion_cascades_into_shift_diffs_by_design`) — documented, via the test name, as the deliberate tradeoff described under "How the algorithm was chosen" above, not a bug

[`tests/diff.rs`](../../../tests/diff.rs) (integration tests via [tests/fixtures/diff.rs](../../../tests/fixtures/diff.rs), exercising the real ZIP/XML pipeline):

- Re-verifies the same scenarios above (cell change/add/delete, sheet add/delete, visibility change, style-only change, merge-only addition, identical) by actually assembling `.xlsx`-shaped bytes and parsing them via `parse_workbook_reader` (`style_only_change_is_reported_as_modified_end_to_end` checks the concrete old/new font size and bold values)
- Confirms `diff_paths` works correctly via temporary files on disk

Performance (measured, release build — from an ad hoc benchmark used for verification, not committed test code):

| Cell count | Change rate | Elapsed |
|---:|---:|---:|
| 100,000 | 0.1% | 1.8ms |
| 800,000 | 0.1% | 25.8ms |
| 4,000,000 | 0.1% | 105ms |

Confirmed to scale roughly linearly (O(n)) with cell count. Even the worst case (an entire sheet cascading from a single row insertion, 500,000 cells) completed in ~19ms — confirming the tradeoff is "more diff entries reported," never "the computation itself blows up," in contrast with the PoC's O(n²) implementation.

`diff_merges` (Issue #8) was measured in `poc/issue8-poc`:

| Merges | Elapsed (`diff_merges`-equivalent) |
|---:|---:|
| 1,000 | 128–143µs |
| 5,000 | 420–476µs |
| 10,000 | 702–766µs |
| 20,000 (`MAX_MERGE_REGIONS` cap) | 1.46–1.51ms |

Clearly linear (O(merges)), ~1.5ms even at the cap. A further benchmark, for the "many cells, few merges" profile this crate targets, directly measured the PoC's O(cells) baseline (the only option available externally) to confirm the `Sheet::merged_regions()` optimization is genuinely worthwhile:

| Cells | Merges | O(cells) baseline cost |
|---:|---:|---:|
| 10,000 | 10 | 145µs |
| 100,000 | 10 | 1.30ms |
| 300,000 | 10 | 4.32ms |

## Open Questions

1. **Where/how a row/column-insertion alignment mode is implemented**: whether the capped, opt-in alignment-based diff [Issue #4](https://github.com/MinamiyamaKotaro/xlsxparser/issues/4) (rows) / [Issue #5](https://github.com/MinamiyamaKotaro/xlsxparser/issues/5) (columns) ask for becomes a function added to this file (e.g. `diff_workbooks_aligned(base, target, limits) -> Result<WorkbookDiff, Error>`) or a separate submodule (`diff::alignment`) is undecided. Connects to [diff/mod.md Open Question 1](mod.en.md) if the latter is chosen.
2. **Robustness of the `similarity_score` heuristic**: the PoC's column alignment used a loose match rule ("any single matching cell value is a candidate match"), which risks false matches on sparse/duplicate-heavy sheets (see [Issue #5's discussion](https://github.com/MinamiyamaKotaro/xlsxparser/issues/5)). Whether to replace it with a more robust column/row signature (e.g. a hash over multiple cells) when the alignment mode is implemented is to be decided at that time.
3. ~~Finer-grained style diffs~~ → **Resolved** ([Issue #8](https://github.com/MinamiyamaKotaro/xlsxparser/issues/8)): added `CellDiff::old_style`/`new_style`, carrying fill color, font, borders, alignment, and number format (see this file's "Style diffs are sparser" section).
4. **Handling sheet reordering**: `diff_workbooks` matches sheets by name (via `BTreeSet<&str>`'s sort order), so it never reports "the sheet order changed" even when `workbook.xml`'s `<sheets>` definition order is shuffled (as long as same-named sheets' contents are identical, the reordering is silently ignored). Whether to add a dedicated field to `WorkbookDiff` for workbook-level sheet-order tracking, should that need ever arise, is undecided.
5. **A cap on `MergeDiff` count**: `diff_merges` itself has no dedicated cap like `resolve::merge::MAX_MERGE_REGIONS` — but since merge registration is already capped at that same limit (20,000) by `resolve::merge::resolve`, no additional cap was judged necessary at diff time (measured at ~1.5ms even at 20,000). Would need revisiting if that upstream cap is ever relaxed.
