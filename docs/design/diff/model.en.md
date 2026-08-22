# `diff/model.rs` Design Doc

*[日本語](model.md)*

Design doc for `src/diff/model.rs`. Defines the output shape ([`diff/engine.rs`](engine.en.md) produces, [`diff/storage.rs`](storage.en.md) persists) of a diff result — `WorkbookDiff`/`SheetDiff`/`CellDiff`/`DiffStatus` ([Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3)). Where [`json.rs`](../json.en.md) turns a full `model::Workbook` snapshot into JSON, this file makes its **diff** representable as JSON — effectively "json.rs's diff counterpart."

## Responsibility / Scope

- Defines the diff result types: `CellDiff` (one cell's change), `SheetDiff` (one sheet's changes — visibility change plus the set of cell changes), `WorkbookDiff` (the whole workbook's diff), and `DiffStatus` (`Added`/`Modified`/`Deleted`)
- Reuses [`json.rs`](../json.en.md)'s `JsonCellValue` as-is for `CellDiff::old_value`/`new_value`'s type (widened to `pub` for this purpose — see Dependencies) rather than defining its own value representation
- Derives `serde::Serialize` on every type, guaranteeing `WorkbookDiff` is directly JSON-serializable ([`diff/storage.rs`](storage.en.md) also relies on this derive when it individually `serde_json::to_string`s `CellDiff::old_value`/`new_value`)
- Follows [json.rs](../json.en.md)'s existing sparse-output convention ("omit a field entirely when there's nothing to report", e.g. `JsonCell::style`): `#[serde(skip_serializing_if = "Option::is_none")]` on `CellDiff::old_value`/`new_value` (absent for `Added`/`Deleted` respectively) and on `SheetDiff::old_visibility`/`new_visibility` (both omitted when visibility is unchanged)
- **Not responsible for**: the diff computation logic itself (how these types actually get built is [`diff/engine.rs`](engine.en.md)'s job), persistence to SQLite ([`diff/storage.rs`](storage.en.md))

## Key Types / Functions (draft)

```rust
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

/// One changed cell. `row`/`col` are the coordinate shared by both
/// revisions — the default engine (`diff::engine::diff_workbooks`), which
/// does no row/column-insertion alignment, never reports a cell moving
/// from one coordinate to another (see that function's doc comment), so
/// there is no need to carry a separate old/new coordinate pair here.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CellDiff {
    pub row: u32,
    pub col: u32,
    pub status: DiffStatus,
    /// Present for `Modified`/`Deleted`, absent for `Added`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_value: Option<JsonCellValue>,
    /// Present for `Modified`/`Added`, absent for `Deleted`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_value: Option<JsonCellValue>,
}

/// One sheet's changes. Only ever constructed for a sheet that actually
/// changed — `diff::engine::diff_workbooks` omits a sheet entirely from
/// `WorkbookDiff::sheets` when it exists on both sides with the same
/// visibility and zero cell diffs (the same "nothing to report, emit
/// nothing" convention `json.rs` already applies to e.g. `JsonCell::style`).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SheetDiff {
    pub name: String,
    pub status: DiffStatus,
    /// Present only when the sheet existed pre-change (`Modified`/`Deleted`)
    /// and, for `Modified`, only when visibility actually differs from
    /// `new_visibility`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_visibility: Option<&'static str>,
    /// Present only when the sheet exists post-change (`Modified`/`Added`)
    /// and, for `Modified`, only when visibility actually differs from
    /// `old_visibility`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_visibility: Option<&'static str>,
    pub cells: Vec<CellDiff>,
}

/// The full diff between two workbooks — the return type of
/// `diff::engine::diff_workbooks`/`diff_paths`.
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct WorkbookDiff {
    pub sheets: Vec<SheetDiff>,
}
```

## Dependencies

- Depends on: [`json.rs`](../json.en.md) (`JsonCellValue`). `JsonCellValue` was widened from private to `pub` specifically to be reusable from this file — [json.md's Key Types section](../json.en.md) originally kept it as an internal implementation detail of `to_json_writer`/`to_json_string` only (that document's own Open Questions haven't been updated to reflect this — it's a post-hoc visibility relaxation driven by this file's reuse need, not a change to json.rs's own design intent). Depends on the external crate `serde` (deriving `Serialize`).
- Depended on by: [`diff/engine.rs`](engine.en.md) (constructs and returns each type), [`diff/storage.rs`](storage.en.md) (reads `CellDiff::old_value`/`new_value`/`DiffStatus` when converting to SQL rows), [`lib.rs`](../lib.en.md) (re-exports `diff::model::{CellDiff, DiffStatus, SheetDiff, WorkbookDiff}` onto the crate root via [`diff/mod.rs`](mod.en.md))

Reusing `JsonCellValue` rather than duplicating it is a deliberate departure from [Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3)'s PoC (`poc/issue3-poc`), which defined its own parallel `JsonValue` enum. This guarantees, at the type level, that the same cell value serializes identically whether it reaches JSON via `to_json_string` (a full snapshot) or via `diff_workbooks` (a diff), structurally ruling out the two representations ever drifting apart (e.g. `DateTime`'s format changing on only one side).

`CellDiff`/`SheetDiff` deliberately carry no `old_row`/`old_col` (a pre-move coordinate) because the current default engine never detects coordinate shifts in the first place (see [engine.md](engine.en.md)) — a field that would go unused is not added preemptively (should Issue #4/#5's alignment mode ever land, it can be added then).

## Error Handling Policy

- This file only defines data types; it has no logic that generates errors.

## Testing Strategy

- No direct unit tests of this file itself (only type definitions and derives). That each type is constructed and serialized as expected is verified indirectly via [`diff/engine.rs`](engine.en.md)'s and [`diff/storage.rs`](storage.en.md)'s own unit tests, plus [`tests/diff.rs`](../../../tests/diff.rs)'s integration tests through the real parse pipeline.

## Open Questions

1. **Type extension for a future row/column-alignment mode**: if the alignment-based diff [Issue #4](https://github.com/MinamiyamaKotaro/xlsxparser/issues/4)/[Issue #5](https://github.com/MinamiyamaKotaro/xlsxparser/issues/5) ask for is implemented, whether to add `old_row`/`old_col` to `CellDiff` (reinterpreting the existing `row`/`col` as always the new coordinate) or introduce a separate type (e.g. `AlignedCellDiff`) is undecided. The former changes the existing fields' meaning — from "always the same coordinate under the default engine" to "possibly moved under the alignment engine" — so the backward-compatibility impact needs evaluating at implementation time.
2. **Style/formula/column-width/image diffs**: [Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3)'s Open Question 3 ("how far to extend beyond cell value — style, formula, merges, images, column widths") is not addressed yet. Currently `CellDiff` holds only cell value as `old_value`/`new_value`; [`diff/engine.rs`](engine.en.md) detects a style change and flags `Modified`, but *what* changed (font size vs. fill color, say) is not represented in the JSON at all. Whether to add fields to `CellDiff` is to be decided once the frontend's actual granularity needs are known.
