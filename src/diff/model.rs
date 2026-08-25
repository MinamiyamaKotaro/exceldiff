// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! `WorkbookDiff`/`SheetDiff`/`CellDiff`/`MergeDiff`: the output shape
//! `diff::engine` produces and `diff::storage` persists (Issue #3, styled
//! and merge-cell changes added by Issue #8).

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

/// One changed cell. `row`/`col` are the coordinate shared by both
/// revisions from the default coordinate-based engine
/// (`diff::engine::diff_workbooks`, which never reports a cell moving from
/// one coordinate to another — see that function's doc comment for why).
/// `diff::col_alignment::diff_workbooks_aligned_columns` (Issue #5) reuses this
/// same type rather than introducing a parallel one: `col` there is the
/// cell's column in `target` (or in `base`, for a `Deleted` cell with no
/// `target` side — the same convention `row` already uses, since rows
/// never shift), and `old_col` (below) carries the pre-alignment column
/// whenever it differs.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CellDiff {
    pub row: u32,
    pub col: u32,
    pub status: DiffStatus,
    /// The cell's column before column alignment, present only when it
    /// differs from `col` — i.e. only `diff_workbooks_aligned_columns`
    /// ever populates this, and only for a `Modified` cell whose column was
    /// recognized as shifted (a matched-but-unshifted column pair, an
    /// `Added`, or a `Deleted` cell all leave this `None`). Always `None`
    /// from `diff::engine::diff_workbooks`, which never shifts columns.
    /// Mirrors `old_style`'s "only present when it actually differs"
    /// sparseness, not `old_value`/`new_value`'s "always both" convention —
    /// unlike a value change, which is always *why* a `CellDiff` exists at
    /// all, a column shift is incidental information most `Modified` cells
    /// (even under alignment) never carry, since most cells' columns don't
    /// move.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_col: Option<u32>,
    /// The cell's row before row alignment, present only when it differs
    /// from `row` — the row-alignment counterpart of `old_col` (Issue #4),
    /// with the identical sparseness convention: only
    /// `diff_workbooks_aligned_rows` ever populates this, and only for a
    /// `Modified` cell whose row was recognized as shifted. Always `None`
    /// from `diff::engine::diff_workbooks` and from
    /// `diff_workbooks_aligned_columns` (Issue #5), neither of which ever
    /// shifts rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_row: Option<u32>,
    /// Present for `Modified`/`Deleted`, absent for `Added`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_value: Option<JsonCellValue>,
    /// Present for `Modified`/`Added`, absent for `Deleted`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_value: Option<JsonCellValue>,
    /// Present for `Deleted` (whenever the removed cell carried a style at
    /// all) and for `Modified` (but, unlike `old_value`, *only* when the
    /// style actually differs from `new_style` — a `Modified` cell whose
    /// value changed but whose style didn't carries no style fields at
    /// all, deliberately more sparse than `old_value`/`new_value`'s
    /// "always both, even if equal" convention: unlike a value change,
    /// which is always the reason a diff exists at all, style is a
    /// secondary dimension most `Modified` cells never touch — see
    /// `diff::engine`'s doc comment for the full rationale, Issue #8).
    /// Never present for `Added`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_style: Option<JsonStyle>,
    /// Present for `Added` (whenever the added cell carries a style) and
    /// for `Modified` when the style actually changed. Never present for
    /// `Deleted`. See `old_style`'s doc comment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_style: Option<JsonStyle>,
}

/// A cell coordinate as `MergeDiff` reports it. A separate type from
/// `model::CellRef` (rather than reusing it directly) because `model/`
/// deliberately stays free of a `serde` dependency (the same reason
/// `json.rs`'s `alignment_tag`/`color_ref_to_json`/etc. convert into a
/// JSON-specific type instead of deriving `Serialize` on the `model::`
/// type directly) — this is that same convention applied to a coordinate
/// pair.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CellPos {
    pub row: u32,
    pub col: u32,
}

impl From<CellRef> for CellPos {
    fn from(r: CellRef) -> Self {
        CellPos {
            row: r.row,
            col: r.col,
        }
    }
}

/// One changed merged region, matched across revisions by its origin
/// coordinate (`start`) — the same coordinate-based matching philosophy
/// `CellDiff` uses for values (`diff::engine::diff_merges`). `start` is a
/// single shared field, not an `old_start`/`new_start` pair: a merge is
/// only ever recognized as the *same* merge across revisions when its
/// origin coordinate matches on both sides (see `diff::engine::
/// diff_merges`'s doc comment), so `old_start` would always equal
/// `new_start` for a `Modified` entry and neither exists for `Added`/
/// `Deleted` (there is only one side to report a start from).
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MergeDiff {
    pub status: DiffStatus,
    pub start: CellPos,
    /// The region's extent before the change. Present for `Modified`/
    /// `Deleted`, absent for `Added` — mirrors `CellDiff::old_value`
    /// exactly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_end: Option<CellPos>,
    /// The region's extent after the change. Present for `Modified`/
    /// `Added`, absent for `Deleted` — mirrors `CellDiff::new_value`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_end: Option<CellPos>,
}

/// One sheet's changes. Only ever constructed for a sheet that actually
/// changed — `diff::engine::diff_workbooks` omits a sheet entirely from
/// `WorkbookDiff::sheets` when it exists on both sides with the same
/// visibility and zero cell/merge diffs (the same "nothing to report, emit
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
    /// Merged-region changes on this sheet (Issue #8) — reported here,
    /// sheet-level, rather than folded into the origin cell's `CellDiff`
    /// (a deliberate asymmetry with the full-snapshot JSON, where a merge
    /// is instead embedded as the origin cell's `rowSpan`/`colSpan`; see
    /// `diff::engine`'s doc comment for why the diff format doesn't mirror
    /// that here). Empty (and omitted from JSON output) when nothing about
    /// this sheet's merges changed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub merges: Vec<MergeDiff>,
}

/// The full diff between two workbooks — the return type of
/// `diff::engine::diff_workbooks`/`diff_paths`.
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct WorkbookDiff {
    pub sheets: Vec<SheetDiff>,
}
