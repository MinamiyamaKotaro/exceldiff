// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! Diffs two `Workbook`s and, optionally, persists the result to SQLite
//! (Issue #3). Builds on top of `pipeline::run` (via `parse_workbook`) the
//! same way `json.rs` does — this module never touches the ZIP/XML layers
//! directly.
//!
//! - `model`: `WorkbookDiff`/`SheetDiff`/`CellDiff` — the diff output shape.
//! - `engine`: `diff_workbooks`/`diff_paths` — computes it, in
//!   O(base_cells + target_cells) (see `engine`'s doc comment for why this
//!   crate does not use a row/column-alignment algorithm by default).
//! - `col_alignment`: `diff_workbooks_aligned_columns` (Issue #5) — the
//!   capped, opt-in alternative that matches columns by content first, so
//!   an inserted/deleted column doesn't cascade into spurious diffs for
//!   every column after it (see `col_alignment`'s doc comment for the full
//!   design).
//! - `row_alignment`: `diff_workbooks_aligned_rows` (Issue #4) — the
//!   capped, opt-in alternative that matches rows by content first, so an
//!   inserted/deleted row doesn't cascade into spurious diffs for every
//!   row after it (see `row_alignment`'s doc comment for the full design).
//! - `storage` (Cargo feature `diff-storage`): `DiffStore` — persists
//!   revisions and diffs to SQLite, and serves a revision's full JSON back
//!   out verbatim when it's flagged HEAD.

pub mod col_alignment;
pub mod engine;
pub mod model;
pub mod row_alignment;
#[cfg(feature = "diff-storage")]
pub mod storage;

pub use col_alignment::{diff_workbooks_aligned_columns, ColumnAlignmentLimits};
pub use engine::{diff_paths, diff_workbooks};
pub use model::{CellDiff, CellPos, DiffStatus, MergeDiff, SheetDiff, WorkbookDiff};
pub use row_alignment::{diff_workbooks_aligned_rows, RowAlignmentLimits};
#[cfg(feature = "diff-storage")]
pub use storage::DiffStore;
