// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! SQLite persistence for workbook revisions and diffs (Issue #3). Only
//! compiled with the `diff-storage` Cargo feature, which pulls in
//! `rusqlite` (bundled SQLite) — kept optional so a consumer that only
//! calls `parse_workbook` never pays that compile-time cost.

use crate::diff::model::{DiffStatus, WorkbookDiff};
use crate::error::Error;
use crate::model::Workbook;
use crate::Result;
use rusqlite::{params, Connection};
use std::path::Path;

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS revisions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    revision_name TEXT NOT NULL,
    is_head INTEGER NOT NULL DEFAULT 0,
    full_json TEXT NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS diff_records (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    base_revision_id INTEGER NOT NULL,
    target_revision_id INTEGER NOT NULL,
    sheet_name TEXT NOT NULL,
    row INTEGER NOT NULL,
    col INTEGER NOT NULL,
    kind TEXT NOT NULL,
    old_value_json TEXT,
    new_value_json TEXT,
    FOREIGN KEY(base_revision_id) REFERENCES revisions(id),
    FOREIGN KEY(target_revision_id) REFERENCES revisions(id)
);

CREATE INDEX IF NOT EXISTS idx_diff_target ON diff_records(target_revision_id);
CREATE INDEX IF NOT EXISTS idx_diff_base_target ON diff_records(base_revision_id, target_revision_id);
";

/// A SQLite-backed store of workbook revisions and the diffs between them.
///
/// `full_json` is stored per revision — not only for HEAD — so that any
/// saved revision can later become HEAD (e.g. after a rollback) and still
/// have its complete JSON available. This trades storage (linear in
/// revision count × sheet size) for query simplicity, an explicit
/// open question from Issue #3 the caller should weigh for their own
/// retention needs (e.g. periodically pruning `full_json` for revisions
/// that are neither HEAD nor referenced by a diff still worth keeping —
/// left to the caller, since only they know their own retention policy).
pub struct DiffStore {
    conn: Connection,
}

impl DiffStore {
    /// Opens (creating if absent) a SQLite database at `path` and ensures
    /// the schema exists.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path).map_err(storage_err)?;
        // SQLite does not enforce `FOREIGN KEY` constraints unless this
        // pragma is set on the connection (it defaults to off, and is not
        // persisted in the database file itself — every connection must
        // set it again). Without it, `diff_records.base_revision_id`/
        // `target_revision_id` could silently reference a `revisions.id`
        // that doesn't exist (code review on PR #6).
        conn.pragma_update(None, "foreign_keys", true)
            .map_err(storage_err)?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(SCHEMA_SQL).map_err(storage_err)
    }

    /// Saves `workbook` as a revision named `name`. When `is_head` is true,
    /// every previously-flagged HEAD revision is cleared first, so
    /// `head_json` always reflects the most recently saved HEAD. The
    /// stored `full_json` is exactly `crate::to_json_string(workbook)`'s
    /// output — Issue #3's "HEAD 指定時は完全な JSON 出力をそのまま行う"
    /// requirement is met by reusing that existing serialization directly
    /// rather than re-deriving a separate (and potentially
    /// lossy/divergent) snapshot representation.
    ///
    /// Returns the new revision's row id, for use as `save_diff`'s
    /// `base_revision_id`/`target_revision_id`.
    pub fn save_revision(&mut self, name: &str, is_head: bool, workbook: &Workbook) -> Result<i64> {
        let full_json = crate::to_json_string(workbook)?;

        // Clearing the previous HEAD and inserting the new revision run in
        // one transaction so a failure between the two statements can
        // never leave the database with zero HEAD revisions (code review
        // on PR #6) — mirrors `save_diff`'s use of a transaction below.
        let tx = self.conn.transaction().map_err(storage_err)?;
        if is_head {
            tx.execute("UPDATE revisions SET is_head = 0", [])
                .map_err(storage_err)?;
        }
        tx.execute(
            "INSERT INTO revisions (revision_name, is_head, full_json) VALUES (?1, ?2, ?3)",
            params![name, is_head as i64, full_json],
        )
        .map_err(storage_err)?;
        let id = tx.last_insert_rowid();
        tx.commit().map_err(storage_err)?;

        Ok(id)
    }

    /// Persists every cell diff in `diff` as rows in `diff_records`,
    /// tagged with `base_revision_id`/`target_revision_id` (the ids
    /// `save_revision` returned for the two revisions being compared).
    /// Sheet-level add/delete/visibility-change info (`SheetDiff`'s own
    /// fields, beyond its `cells`) is not persisted — `diff_records` is a
    /// flat, per-cell table by design (Issue #3's proposed schema); a
    /// caller that also needs sheet-level history should store
    /// `WorkbookDiff` itself (e.g. as its own JSON blob) rather than rely
    /// on reconstructing it from `diff_records` alone.
    pub fn save_diff(
        &mut self,
        base_revision_id: i64,
        target_revision_id: i64,
        diff: &WorkbookDiff,
    ) -> Result<()> {
        let tx = self.conn.transaction().map_err(storage_err)?;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO diff_records (
                        base_revision_id, target_revision_id, sheet_name, row, col, kind,
                        old_value_json, new_value_json
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                )
                .map_err(storage_err)?;

            for sheet in &diff.sheets {
                for cell in &sheet.cells {
                    let kind = match cell.status {
                        DiffStatus::Added => "added",
                        DiffStatus::Modified => "modified",
                        DiffStatus::Deleted => "deleted",
                    };
                    let old_json = cell
                        .old_value
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()
                        .map_err(json_err)?;
                    let new_json = cell
                        .new_value
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()
                        .map_err(json_err)?;

                    stmt.execute(params![
                        base_revision_id,
                        target_revision_id,
                        sheet.name,
                        cell.row,
                        cell.col,
                        kind,
                        old_json,
                        new_json,
                    ])
                    .map_err(storage_err)?;
                }
            }
        }
        tx.commit().map_err(storage_err)?;
        Ok(())
    }

    /// The full JSON snapshot (`to_json_string`'s exact output) of the most
    /// recently saved HEAD revision, or `None` if no revision has ever been
    /// flagged as HEAD.
    pub fn head_json(&self) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT full_json FROM revisions WHERE is_head = 1 ORDER BY id DESC LIMIT 1")
            .map_err(storage_err)?;
        let mut rows = stmt.query([]).map_err(storage_err)?;
        match rows.next().map_err(storage_err)? {
            Some(row) => Ok(Some(row.get(0).map_err(storage_err)?)),
            None => Ok(None),
        }
    }
}

fn storage_err(source: rusqlite::Error) -> Error {
    Error::DiffStorage {
        source: Box::new(source),
    }
}

fn json_err(source: serde_json::Error) -> Error {
    Error::JsonSerialize {
        source: Box::new(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::engine::diff_workbooks;
    use crate::model::{Cell, CellRef, CellValue, Sheet, SheetVisibility};

    fn sheet_with_one_cell(name: &str, value: f64) -> Sheet {
        let mut sheet = Sheet::new(name.to_string(), SheetVisibility::Visible);
        sheet.insert_cell(
            CellRef { row: 1, col: 1 },
            Cell {
                value: Some(CellValue::Number(value)),
                style: None,
            },
        );
        sheet
    }

    #[test]
    fn head_json_is_none_before_any_revision_is_saved() {
        let store = DiffStore::open(":memory:").unwrap();
        assert_eq!(store.head_json().unwrap(), None);
    }

    #[test]
    fn head_json_matches_to_json_string_output_exactly() {
        let mut store = DiffStore::open(":memory:").unwrap();
        let workbook = Workbook::new(vec![sheet_with_one_cell("Sheet1", 42.0)], None);

        store.save_revision("v1", true, &workbook).unwrap();

        let expected = crate::to_json_string(&workbook).unwrap();
        assert_eq!(store.head_json().unwrap(), Some(expected));
    }

    #[test]
    fn saving_a_new_head_clears_the_previous_one() {
        let mut store = DiffStore::open(":memory:").unwrap();
        let v1 = Workbook::new(vec![sheet_with_one_cell("Sheet1", 1.0)], None);
        let v2 = Workbook::new(vec![sheet_with_one_cell("Sheet1", 2.0)], None);

        store.save_revision("v1", true, &v1).unwrap();
        store.save_revision("v2", true, &v2).unwrap();

        assert_eq!(
            store.head_json().unwrap(),
            Some(crate::to_json_string(&v2).unwrap())
        );
    }

    #[test]
    fn save_diff_persists_one_row_per_cell_diff() {
        let mut store = DiffStore::open(":memory:").unwrap();
        let base = Workbook::new(vec![sheet_with_one_cell("Sheet1", 1.0)], None);
        let target = Workbook::new(vec![sheet_with_one_cell("Sheet1", 2.0)], None);

        let base_id = store.save_revision("base", false, &base).unwrap();
        let target_id = store.save_revision("target", true, &target).unwrap();
        let diff = diff_workbooks(&base, &target);
        store.save_diff(base_id, target_id, &diff).unwrap();

        let count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM diff_records", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);

        let kind: String = store
            .conn
            .query_row("SELECT kind FROM diff_records LIMIT 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(kind, "modified");
    }
}
