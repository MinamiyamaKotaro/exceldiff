// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! `WorkbookDiff`/`SheetDiff`/`CellDiff`: the output shape `diff::engine`
//! produces and `diff::storage` persists (Issue #3).

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
/// revisions — unlike a row/column-insertion-aware alignment, the default
/// coordinate-based engine (`diff::engine::diff_workbooks`) never reports a
/// cell moving from one coordinate to another (see that function's doc
/// comment for why), so there is no separate old/new coordinate pair to
/// carry here.
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
