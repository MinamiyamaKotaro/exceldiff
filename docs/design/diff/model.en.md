# `diff/model.rs` Design Doc

*[日本語](model.md)*

Design doc for `src/diff/model.rs`. Defines the output shape ([`diff/engine.rs`](engine.en.md) produces, [`diff/storage.rs`](storage.en.md) persists) of a diff result — `WorkbookDiff`/`SheetDiff`/`CellDiff`/`MergeDiff`/`CellPos`/`DiffStatus` ([Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3); style and merged-cell diffs added by [Issue #8](https://github.com/MinamiyamaKotaro/xlsxparser/issues/8)). Where [`json.rs`](../json.en.md) turns a full `model::Workbook` snapshot into JSON, this file makes its **diff** representable as JSON — effectively "json.rs's diff counterpart."

## Responsibility / Scope

- Defines the diff result types: `CellDiff` (one cell's change), `MergeDiff` (one merged region's change), `SheetDiff` (one sheet's changes — visibility, cells, and merges combined), `WorkbookDiff` (the whole workbook's diff), `DiffStatus` (`Added`/`Modified`/`Deleted`), and `CellPos` (a coordinate)
- Reuses [`json.rs`](../json.en.md)'s `JsonCellValue` for `CellDiff::old_value`/`new_value` and its `JsonStyle` for `old_style`/`new_style` (both widened to `pub` for this purpose — see Dependencies) rather than defining its own value/style representations
- Derives `serde::Serialize` on every type, guaranteeing `WorkbookDiff` is directly JSON-serializable
- Follows [json.rs](../json.en.md)'s existing sparse-output convention, but **deliberately uses a different granularity** for `old_value`/`new_value` versus `old_style`/`new_style` (see `CellDiff`'s doc comment and [engine.md](engine.en.md)'s "Style diffs are sparser" section)
- **Not responsible for**: the diff computation logic itself ([`diff/engine.rs`](engine.en.md)), persistence to SQLite ([`diff/storage.rs`](storage.en.md) — `old_style`/`new_style`/`merges` are not currently persisted, see [Issue #9](https://github.com/MinamiyamaKotaro/xlsxparser/issues/9))

## Key Types / Functions

```rust
use crate::json::JsonStyle;
use crate::model::CellRef;
use crate::JsonCellValue;
use serde::Serialize;

/// A cell-level change kind.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DiffStatus {
    Added,
    Modified,
    Deleted,
}

/// One changed cell. `row`/`col` are the coordinate shared by both revisions.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CellDiff {
    pub row: u32,
    pub col: u32,
    pub status: DiffStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_value: Option<JsonCellValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_value: Option<JsonCellValue>,
    /// Present for `Added` (when the added cell has a style) and for a
    /// `Modified` cell whose style actually changed — unlike `old_value`/
    /// `new_value`, NOT "always both present on Modified regardless of
    /// whether that field changed"; a deliberately sparser convention
    /// (Issue #8; see `diff::engine`'s doc comment for why). Never present
    /// for `Deleted`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_style: Option<JsonStyle>,
    /// Present for `Deleted` (when the removed cell had a style) and for a
    /// `Modified` cell whose style actually changed. Never present for
    /// `Added`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_style: Option<JsonStyle>,
}

/// A coordinate as `MergeDiff` reports it. Not `model::CellRef` directly,
/// to keep `model/` free of a `serde` dependency (same reason json.rs's
/// `alignment_tag` etc. convert instead of deriving `Serialize` on the
/// `model::` type).
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CellPos {
    pub row: u32,
    pub col: u32,
}

impl From<CellRef> for CellPos {
    fn from(r: CellRef) -> Self {
        CellPos { row: r.row, col: r.col }
    }
}

/// One changed merged region, matched across revisions by its origin
/// coordinate (`start`) — no separate `old_start`/`new_start` pair even
/// for `Modified` (always the same `start` by construction; see
/// `diff::engine::diff_merges`'s doc comment).
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MergeDiff {
    pub status: DiffStatus,
    pub start: CellPos,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_end: Option<CellPos>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_end: Option<CellPos>,
}

/// One sheet's changes. Only ever constructed for a sheet that actually
/// changed.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SheetDiff {
    pub name: String,
    pub status: DiffStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_visibility: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_visibility: Option<&'static str>,
    pub cells: Vec<CellDiff>,
    /// This sheet's merged-region changes (Issue #8) — reported
    /// sheet-level, not folded into a `CellDiff`, unlike the full-snapshot
    /// JSON (`json.rs`), which embeds a merge as the origin cell's
    /// `rowSpan`/`colSpan`. Deliberately different from the snapshot's
    /// representation (see `diff::engine`'s doc comment for why). Empty
    /// (and omitted from JSON) when nothing changed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub merges: Vec<MergeDiff>,
}

/// The full diff between two workbooks.
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct WorkbookDiff {
    pub sheets: Vec<SheetDiff>,
}
```

## Dependencies

- Depends on: [`json.rs`](../json.en.md) (`JsonCellValue`, `JsonStyle` — both widened to `pub` for reuse; `JsonStyle`'s own nested `JsonFont`/`JsonColorRef`/`JsonBorders` were likewise widened to `pub`, with every field of these structs made `pub` too — aligning the whole `JsonStyle` family with `CellDiff`/`SheetDiff`'s "fully public plain data" design), [`model/cell.rs`](../model/cell.en.md) (`CellRef` — the conversion source for `CellPos`). Depends on the external crate `serde`.
- Depended on by: [`diff/engine.rs`](engine.en.md) (constructs and returns each type), [`diff/storage.rs`](storage.en.md) (reads `CellDiff::old_value`/`new_value`/`DiffStatus` when converting to SQL rows — does not yet read `old_style`/`new_style`/`merges`, see [Issue #9](https://github.com/MinamiyamaKotaro/xlsxparser/issues/9)), [`lib.rs`](../lib.en.md) (re-exports `CellDiff`/`CellPos`/`DiffStatus`/`MergeDiff`/`SheetDiff`/`WorkbookDiff` onto the crate root via [`diff/mod.rs`](mod.en.md))

Reusing `JsonCellValue`/`JsonStyle` rather than duplicating them guarantees, at the type level, that the same cell value/style serializes identically whether it reaches JSON via `to_json_string` (a full snapshot) or via `diff_workbooks` (a diff) — extending to style the same departure from [Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3)'s PoC (which defined its own parallel `JsonValue` type) that `CellDiff::old_value`/`new_value` already made.

`CellDiff` carries no `old_row`/`old_col`, and `MergeDiff` carries no separate `old_start`/`new_start` pair, because the current default engine never detects coordinate shifts in the first place (see [engine.md](engine.en.md)).

## Error Handling Policy

- This file only defines data types; it has no logic that generates errors.

## Testing Strategy

- No direct unit tests of this file itself. Correct construction/serialization is verified indirectly via [`diff/engine.rs`](engine.en.md)'s unit tests (`style_only_change_is_reported_as_modified_with_new_style_populated`, `value_only_change_carries_no_style_diff`, `added_cell_with_a_style_reports_new_style_only`, `merge_added_is_detected_even_with_no_cell_changes`, `merge_deleted_is_detected`, `merge_extent_change_is_reported_as_modified`, `unchanged_merge_produces_no_diff_at_all`, `sheet_added_reports_its_merges_as_added_too`, `sheet_deleted_reports_its_merges_as_deleted_too`) and [`tests/diff.rs`](../../../tests/diff.rs)'s integration tests through the real parse pipeline (`style_only_change_is_reported_as_modified_end_to_end`, `merge_addition_is_detected_even_with_no_cell_changes_end_to_end`).

## Open Questions

1. **Type extension for a future row/column-alignment mode**: how `CellDiff`/`MergeDiff`'s coordinate fields would need to change is still undecided (unchanged by this update).
2. ~~Style/merged-cell diffs~~ → **Partially resolved** ([Issue #8](https://github.com/MinamiyamaKotaro/xlsxparser/issues/8)): added `CellDiff::old_style`/`new_style` (fill color, font, borders, alignment, number format) and `SheetDiff::merges`. Formula/column-width/image diffs remain unaddressed.
3. **Reflecting style/merge diffs in SQLite persistence**: tracked separately as [Issue #9](https://github.com/MinamiyamaKotaro/xlsxparser/issues/9). `diff::storage::DiffStore::save_diff` currently persists none of `old_style`/`new_style`/`merges`.
