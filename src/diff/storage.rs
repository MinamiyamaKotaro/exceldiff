// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! SQLite persistence for workbook revisions and diffs (Issue #3), including
//! the style- and merged-region-diff columns/table added for Issue #9. Only
//! compiled with the `diff-storage` Cargo feature, which pulls in
//! `rusqlite` (bundled SQLite) — kept optional so a consumer that only
//! calls `parse_workbook` never pays that compile-time cost.

use crate::diff::model::{DiffStatus, WorkbookDiff};
use crate::error::Error;
use crate::model::Workbook;
use crate::Result;
use rusqlite::{params, Connection};
use std::path::Path;

/// Schema for a freshly created database. `diff_records.old_style_json`/
/// `new_style_json` and the `merge_diff_records` table persist
/// `CellDiff::old_style`/`new_style` and `SheetDiff::merges` (Issue #9) —
/// computed by `diff::engine` since Issue #8 but, before Issue #9,
/// silently dropped by `save_diff`.
///
/// Style is stored as two extra columns directly on `diff_records`
/// ("inline") rather than in a separate table keyed by cell, and merges get
/// their own table (`merge_diff_records`) rather than a column on
/// `diff_records` — a merged region is never a single cell, so folding it
/// into the per-cell table would be unnatural. Both choices were verified
/// against a rejected alternative (a normalized `style_diff_records` table
/// plus a `style_catalog` dictionary table) by two rounds of PoC
/// benchmarking (`poc/issue9-poc`, `poc/issue9-poc-v2`, referenced from
/// Issue #9): the inline column design was consistently faster to write
/// (3-18% lower wall-clock `save_diff` time) and smaller on disk (3-5%
/// fewer bytes) than the normalized alternatives, with no measurable
/// memory difference between designs (dominated by workbook parsing/diffing,
/// not the storage layer) — see `storage.md`'s Open Questions for the full
/// writeup. A dictionary table only wins when the same style JSON repeats
/// across a very large number of cells (e.g. shared table formatting), and
/// even then only in disk size, not write latency once catalog lookups are
/// on the hot path — not worth the extra JOIN this crate's "lightweight,
/// simple, minimal dependencies" design goal exists to avoid.
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
    old_style_json TEXT,
    new_style_json TEXT,
    FOREIGN KEY(base_revision_id) REFERENCES revisions(id),
    FOREIGN KEY(target_revision_id) REFERENCES revisions(id)
);

CREATE INDEX IF NOT EXISTS idx_diff_target ON diff_records(target_revision_id);
CREATE INDEX IF NOT EXISTS idx_diff_base_target ON diff_records(base_revision_id, target_revision_id);

CREATE TABLE IF NOT EXISTS merge_diff_records (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    base_revision_id INTEGER NOT NULL,
    target_revision_id INTEGER NOT NULL,
    sheet_name TEXT NOT NULL,
    kind TEXT NOT NULL,
    start_row INTEGER NOT NULL,
    start_col INTEGER NOT NULL,
    old_end_row INTEGER,
    old_end_col INTEGER,
    new_end_row INTEGER,
    new_end_col INTEGER,
    FOREIGN KEY(base_revision_id) REFERENCES revisions(id),
    FOREIGN KEY(target_revision_id) REFERENCES revisions(id)
);

CREATE INDEX IF NOT EXISTS idx_merge_target ON merge_diff_records(target_revision_id);
CREATE INDEX IF NOT EXISTS idx_merge_base_target ON merge_diff_records(base_revision_id, target_revision_id);
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
        self.conn.execute_batch(SCHEMA_SQL).map_err(storage_err)?;
        self.migrate_pre_issue_9_schema()
    }

    /// A database created by a pre-Issue #9 version of this crate has a
    /// `diff_records` table without `old_style_json`/`new_style_json`, and
    /// no `merge_diff_records` table at all — `CREATE TABLE IF NOT EXISTS`
    /// above leaves such a table exactly as it already is, since it exists.
    /// Detects that case via `PRAGMA table_info` and adds the missing
    /// columns with `ALTER TABLE ... ADD COLUMN` (nullable, no `DEFAULT`),
    /// which SQLite performs as an O(1) metadata-only change — it neither
    /// rewrites existing rows nor requires them to backfill a value, so
    /// every pre-existing row simply reads back with `NULL` in the new
    /// columns. Verified safe and fast (2ms for 100k pre-existing rows) by
    /// `poc/issue9-poc`'s/`poc/issue9-poc-v2`'s migration benchmarks
    /// (Issue #9). A fresh database never takes this path: its
    /// `diff_records` already has both columns from `SCHEMA_SQL` above, so
    /// the `PRAGMA table_info` check below finds them immediately.
    ///
    /// The two columns are checked and added **independently** rather than
    /// gating both `ALTER TABLE`s behind a single `old_style_json`
    /// existence check: `execute_batch` (`sqlite3_exec`) does not wrap
    /// multiple statements in an implicit transaction, so a process that
    /// crashed (or hit any other error) between the two `ALTER TABLE`s on
    /// a previous `open` could have left `old_style_json` added but
    /// `new_style_json` still missing. Gating on `old_style_json` alone
    /// would then see it already present and skip re-attempting
    /// `new_style_json` forever, permanently breaking every future
    /// `save_diff` (its `INSERT` always references `new_style_json`) —
    /// checking each column on every `open` call makes this self-healing
    /// regardless of what a prior run left behind (code review on PR #13).
    fn migrate_pre_issue_9_schema(&self) -> Result<()> {
        let mut missing_column_statements = String::new();
        for column in ["old_style_json", "new_style_json"] {
            let has_column: bool = self
                .conn
                .prepare("SELECT 1 FROM pragma_table_info('diff_records') WHERE name = ?1")
                .map_err(storage_err)?
                .exists(params![column])
                .map_err(storage_err)?;
            if !has_column {
                missing_column_statements.push_str(&format!(
                    "ALTER TABLE diff_records ADD COLUMN {column} TEXT;\n"
                ));
            }
        }
        if !missing_column_statements.is_empty() {
            self.conn
                .execute_batch(&missing_column_statements)
                .map_err(storage_err)?;
        }
        Ok(())
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

    /// Persists every cell diff in `diff` as rows in `diff_records`
    /// (including `CellDiff::old_style`/`new_style`, Issue #9) and every
    /// merged-region diff as rows in `merge_diff_records` (Issue #9), all
    /// tagged with `base_revision_id`/`target_revision_id` (the ids
    /// `save_revision` returned for the two revisions being compared).
    /// Sheet-level add/delete/visibility-change info (`SheetDiff`'s own
    /// fields, beyond `cells`/`merges`) is still not persisted — a caller
    /// that also needs sheet-level history should store `WorkbookDiff`
    /// itself (e.g. as its own JSON blob) rather than rely on
    /// reconstructing it from `diff_records`/`merge_diff_records` alone
    /// (see `storage.md`'s Open Questions).
    pub fn save_diff(
        &mut self,
        base_revision_id: i64,
        target_revision_id: i64,
        diff: &WorkbookDiff,
    ) -> Result<()> {
        let tx = self.conn.transaction().map_err(storage_err)?;
        {
            let mut cell_stmt = tx
                .prepare(
                    "INSERT INTO diff_records (
                        base_revision_id, target_revision_id, sheet_name, row, col, kind,
                        old_value_json, new_value_json, old_style_json, new_style_json
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                )
                .map_err(storage_err)?;
            let mut merge_stmt = tx
                .prepare(
                    "INSERT INTO merge_diff_records (
                        base_revision_id, target_revision_id, sheet_name, kind,
                        start_row, start_col, old_end_row, old_end_col, new_end_row, new_end_col
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                )
                .map_err(storage_err)?;

            for sheet in &diff.sheets {
                for cell in &sheet.cells {
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
                    let old_style_json = cell
                        .old_style
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()
                        .map_err(json_err)?;
                    let new_style_json = cell
                        .new_style
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()
                        .map_err(json_err)?;

                    cell_stmt
                        .execute(params![
                            base_revision_id,
                            target_revision_id,
                            sheet.name,
                            cell.row,
                            cell.col,
                            diff_status_str(cell.status),
                            old_json,
                            new_json,
                            old_style_json,
                            new_style_json,
                        ])
                        .map_err(storage_err)?;
                }

                for merge in &sheet.merges {
                    merge_stmt
                        .execute(params![
                            base_revision_id,
                            target_revision_id,
                            sheet.name,
                            diff_status_str(merge.status),
                            merge.start.row,
                            merge.start.col,
                            merge.old_end.map(|p| p.row),
                            merge.old_end.map(|p| p.col),
                            merge.new_end.map(|p| p.row),
                            merge.new_end.map(|p| p.col),
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

/// `diff_records.kind`/`merge_diff_records.kind`'s shared string form —
/// both `CellDiff::status` and `MergeDiff::status` are the same
/// `DiffStatus` type.
fn diff_status_str(status: DiffStatus) -> &'static str {
    match status {
        DiffStatus::Added => "added",
        DiffStatus::Modified => "modified",
        DiffStatus::Deleted => "deleted",
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

    #[test]
    fn save_diff_persists_added_and_deleted_kinds_too() {
        // save_diff_persists_one_row_per_cell_diff above only exercises
        // the Modified arm of save_diff's DiffStatus -> &str match; this
        // hand-builds a WorkbookDiff carrying Added and Deleted cells too
        // so both remaining arms are covered.
        use crate::diff::model::{CellDiff, SheetDiff};
        use crate::JsonCellValue;

        let mut store = DiffStore::open(":memory:").unwrap();
        let base = Workbook::new(vec![sheet_with_one_cell("Sheet1", 1.0)], None);
        let target = Workbook::new(vec![sheet_with_one_cell("Sheet1", 2.0)], None);
        let base_id = store.save_revision("base", false, &base).unwrap();
        let target_id = store.save_revision("target", true, &target).unwrap();

        let diff = WorkbookDiff {
            sheets: vec![SheetDiff {
                name: "Sheet1".to_string(),
                status: DiffStatus::Modified,
                old_visibility: None,
                new_visibility: None,
                cells: vec![
                    CellDiff {
                        row: 1,
                        col: 1,
                        status: DiffStatus::Added,
                        old_col: None,
                        old_row: None,
                        old_value: None,
                        new_value: Some(JsonCellValue::Number(1.0)),
                        old_style: None,
                        new_style: None,
                    },
                    CellDiff {
                        row: 2,
                        col: 1,
                        status: DiffStatus::Deleted,
                        old_col: None,
                        old_row: None,
                        old_value: Some(JsonCellValue::Number(2.0)),
                        new_value: None,
                        old_style: None,
                        new_style: None,
                    },
                ],
                merges: Vec::new(),
            }],
        };
        store.save_diff(base_id, target_id, &diff).unwrap();

        let mut kinds: Vec<String> = store
            .conn
            .prepare("SELECT kind FROM diff_records ORDER BY row")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        kinds.sort();
        assert_eq!(kinds, vec!["added".to_string(), "deleted".to_string()]);
    }

    #[test]
    fn save_diff_persists_old_and_new_style_json_when_present() {
        // Issue #9: `CellDiff::old_style`/`new_style` (Issue #8) must now
        // round-trip through `diff_records.old_style_json`/
        // `new_style_json`, not be silently dropped.
        use crate::diff::model::{CellDiff, SheetDiff};
        use crate::{JsonCellValue, JsonColorRef, JsonFont, JsonStyle};

        let mut store = DiffStore::open(":memory:").unwrap();
        let base = Workbook::new(vec![sheet_with_one_cell("Sheet1", 1.0)], None);
        let target = Workbook::new(vec![sheet_with_one_cell("Sheet1", 1.0)], None);
        let base_id = store.save_revision("base", false, &base).unwrap();
        let target_id = store.save_revision("target", true, &target).unwrap();

        let old_style = JsonStyle {
            font: JsonFont {
                size_pt: 11.0,
                bold: false,
            },
            wrap_text: false,
            alignment: "general",
            number_format: None,
            fill_fg_color: Some(JsonColorRef::Rgb("FFFF0000".to_string())),
            fill_bg_color: None,
            borders: None,
        };
        let new_style = JsonStyle {
            font: JsonFont {
                size_pt: 14.0,
                bold: true,
            },
            ..old_style.clone()
        };

        let diff = WorkbookDiff {
            sheets: vec![SheetDiff {
                name: "Sheet1".to_string(),
                status: DiffStatus::Modified,
                old_visibility: None,
                new_visibility: None,
                cells: vec![CellDiff {
                    row: 1,
                    col: 1,
                    status: DiffStatus::Modified,
                    old_col: None,
                    old_row: None,
                    old_value: Some(JsonCellValue::Number(1.0)),
                    new_value: Some(JsonCellValue::Number(1.0)),
                    old_style: Some(old_style.clone()),
                    new_style: Some(new_style.clone()),
                }],
                merges: Vec::new(),
            }],
        };
        store.save_diff(base_id, target_id, &diff).unwrap();

        let (old_style_json, new_style_json): (String, String) = store
            .conn
            .query_row(
                "SELECT old_style_json, new_style_json FROM diff_records LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(old_style_json, serde_json::to_string(&old_style).unwrap());
        assert_eq!(new_style_json, serde_json::to_string(&new_style).unwrap());
    }

    #[test]
    fn save_diff_persists_style_json_as_null_when_absent() {
        // A `Modified` cell whose value changed but whose style didn't
        // carries no style fields at all (`CellDiff::old_style`'s doc
        // comment) — the persisted row must reflect that as NULL, not an
        // empty string or a spurious style.
        let mut store = DiffStore::open(":memory:").unwrap();
        let base = Workbook::new(vec![sheet_with_one_cell("Sheet1", 1.0)], None);
        let target = Workbook::new(vec![sheet_with_one_cell("Sheet1", 2.0)], None);
        let base_id = store.save_revision("base", false, &base).unwrap();
        let target_id = store.save_revision("target", true, &target).unwrap();
        let diff = diff_workbooks(&base, &target);

        store.save_diff(base_id, target_id, &diff).unwrap();

        let (old_style_json, new_style_json): (Option<String>, Option<String>) = store
            .conn
            .query_row(
                "SELECT old_style_json, new_style_json FROM diff_records LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(old_style_json, None);
        assert_eq!(new_style_json, None);
    }

    #[test]
    fn save_diff_persists_one_row_per_merge_diff_with_correct_kind_and_extent() {
        // Issue #9: `SheetDiff::merges` (Issue #8) must now be walked and
        // persisted into `merge_diff_records` — before Issue #9, save_diff
        // never even looked at this field.
        use crate::diff::model::{CellPos, MergeDiff, SheetDiff};

        let mut store = DiffStore::open(":memory:").unwrap();
        let base = Workbook::new(vec![sheet_with_one_cell("Sheet1", 1.0)], None);
        let target = Workbook::new(vec![sheet_with_one_cell("Sheet1", 1.0)], None);
        let base_id = store.save_revision("base", false, &base).unwrap();
        let target_id = store.save_revision("target", true, &target).unwrap();

        let diff = WorkbookDiff {
            sheets: vec![SheetDiff {
                name: "Sheet1".to_string(),
                status: DiffStatus::Modified,
                old_visibility: None,
                new_visibility: None,
                cells: Vec::new(),
                merges: vec![
                    MergeDiff {
                        status: DiffStatus::Added,
                        start: CellPos { row: 1, col: 1 },
                        old_end: None,
                        new_end: Some(CellPos { row: 1, col: 2 }),
                    },
                    MergeDiff {
                        status: DiffStatus::Modified,
                        start: CellPos { row: 3, col: 1 },
                        old_end: Some(CellPos { row: 3, col: 2 }),
                        new_end: Some(CellPos { row: 3, col: 3 }),
                    },
                    MergeDiff {
                        status: DiffStatus::Deleted,
                        start: CellPos { row: 5, col: 1 },
                        old_end: Some(CellPos { row: 6, col: 1 }),
                        new_end: None,
                    },
                ],
            }],
        };
        store.save_diff(base_id, target_id, &diff).unwrap();

        #[derive(Debug, PartialEq)]
        struct MergeRow {
            kind: String,
            start_row: u32,
            start_col: u32,
            old_end_row: Option<u32>,
            old_end_col: Option<u32>,
            new_end_row: Option<u32>,
            new_end_col: Option<u32>,
        }

        let mut stmt = store
            .conn
            .prepare(
                "SELECT kind, start_row, start_col, old_end_row, old_end_col, new_end_row, new_end_col
                 FROM merge_diff_records ORDER BY start_row",
            )
            .unwrap();
        let rows: Vec<MergeRow> = stmt
            .query_map([], |row| {
                Ok(MergeRow {
                    kind: row.get(0)?,
                    start_row: row.get(1)?,
                    start_col: row.get(2)?,
                    old_end_row: row.get(3)?,
                    old_end_col: row.get(4)?,
                    new_end_row: row.get(5)?,
                    new_end_col: row.get(6)?,
                })
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();

        assert_eq!(
            rows,
            vec![
                MergeRow {
                    kind: "added".to_string(),
                    start_row: 1,
                    start_col: 1,
                    old_end_row: None,
                    old_end_col: None,
                    new_end_row: Some(1),
                    new_end_col: Some(2),
                },
                MergeRow {
                    kind: "modified".to_string(),
                    start_row: 3,
                    start_col: 1,
                    old_end_row: Some(3),
                    old_end_col: Some(2),
                    new_end_row: Some(3),
                    new_end_col: Some(3),
                },
                MergeRow {
                    kind: "deleted".to_string(),
                    start_row: 5,
                    start_col: 1,
                    old_end_row: Some(6),
                    old_end_col: Some(1),
                    new_end_row: None,
                    new_end_col: None,
                },
            ]
        );
    }

    #[test]
    fn save_diff_keeps_two_sheets_diffs_distinguishable_at_identical_coordinates() {
        // `save_diff` commits at the Workbook level — a single call always
        // walks every sheet in `diff.sheets` together, there is no
        // per-sheet commit. That must never blur two different sheets'
        // diffs together: "SheetA" and "SheetB" here each carry a cell
        // diff AND a merge diff at the exact same (row, col)/start
        // coordinate but with different old/new content, which would be
        // indistinguishable if `sheet_name` weren't part of every row (or
        // weren't threaded through correctly by the `for sheet in
        // &diff.sheets` loop).
        use crate::diff::model::{CellDiff, CellPos, MergeDiff, SheetDiff};
        use crate::JsonCellValue;

        let mut store = DiffStore::open(":memory:").unwrap();
        let base = Workbook::new(
            vec![
                sheet_with_one_cell("SheetA", 1.0),
                sheet_with_one_cell("SheetB", 10.0),
            ],
            None,
        );
        let target = Workbook::new(
            vec![
                sheet_with_one_cell("SheetA", 2.0),
                sheet_with_one_cell("SheetB", 20.0),
            ],
            None,
        );
        let base_id = store.save_revision("base", false, &base).unwrap();
        let target_id = store.save_revision("target", true, &target).unwrap();

        let sheet_diff = |name: &str, old: f64, new: f64, merge_new_end_col: u32| SheetDiff {
            name: name.to_string(),
            status: DiffStatus::Modified,
            old_visibility: None,
            new_visibility: None,
            cells: vec![CellDiff {
                row: 1,
                col: 1,
                status: DiffStatus::Modified,
                old_col: None,
                old_row: None,
                old_value: Some(JsonCellValue::Number(old)),
                new_value: Some(JsonCellValue::Number(new)),
                old_style: None,
                new_style: None,
            }],
            merges: vec![MergeDiff {
                status: DiffStatus::Added,
                start: CellPos { row: 1, col: 1 },
                old_end: None,
                new_end: Some(CellPos {
                    row: 1,
                    col: merge_new_end_col,
                }),
            }],
        };
        let diff = WorkbookDiff {
            sheets: vec![
                sheet_diff("SheetA", 1.0, 2.0, 2),
                sheet_diff("SheetB", 10.0, 20.0, 3),
            ],
        };
        store.save_diff(base_id, target_id, &diff).unwrap();

        #[derive(Debug, PartialEq)]
        struct CellRow {
            sheet_name: String,
            old_value_json: String,
            new_value_json: String,
        }
        let mut cell_stmt = store
            .conn
            .prepare(
                "SELECT sheet_name, old_value_json, new_value_json FROM diff_records
                 WHERE row = 1 AND col = 1 ORDER BY sheet_name",
            )
            .unwrap();
        let cell_rows: Vec<CellRow> = cell_stmt
            .query_map([], |row| {
                Ok(CellRow {
                    sheet_name: row.get(0)?,
                    old_value_json: row.get(1)?,
                    new_value_json: row.get(2)?,
                })
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            cell_rows,
            vec![
                CellRow {
                    sheet_name: "SheetA".to_string(),
                    old_value_json: serde_json::to_string(&JsonCellValue::Number(1.0)).unwrap(),
                    new_value_json: serde_json::to_string(&JsonCellValue::Number(2.0)).unwrap(),
                },
                CellRow {
                    sheet_name: "SheetB".to_string(),
                    old_value_json: serde_json::to_string(&JsonCellValue::Number(10.0)).unwrap(),
                    new_value_json: serde_json::to_string(&JsonCellValue::Number(20.0)).unwrap(),
                },
            ]
        );

        #[derive(Debug, PartialEq)]
        struct MergeRow {
            sheet_name: String,
            new_end_col: u32,
        }
        let mut merge_stmt = store
            .conn
            .prepare(
                "SELECT sheet_name, new_end_col FROM merge_diff_records
                 WHERE start_row = 1 AND start_col = 1 ORDER BY sheet_name",
            )
            .unwrap();
        let merge_rows: Vec<MergeRow> = merge_stmt
            .query_map([], |row| {
                Ok(MergeRow {
                    sheet_name: row.get(0)?,
                    new_end_col: row.get(1)?,
                })
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            merge_rows,
            vec![
                MergeRow {
                    sheet_name: "SheetA".to_string(),
                    new_end_col: 2,
                },
                MergeRow {
                    sheet_name: "SheetB".to_string(),
                    new_end_col: 3,
                },
            ]
        );
    }

    #[test]
    fn open_adds_style_columns_and_merge_table_to_a_pre_issue_9_database() {
        // A database file created by a crate version predating Issue #9
        // has `diff_records` without `old_style_json`/`new_style_json` and
        // no `merge_diff_records` table at all. `DiffStore::open` on that
        // same file must migrate it in place (via `ALTER TABLE ADD
        // COLUMN`/`CREATE TABLE IF NOT EXISTS`) without disturbing the
        // pre-existing row — `:memory:` can't exercise this since each
        // `Connection::open(":memory:")` starts a fresh empty database, so
        // a real temp file is required.
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "exceldiff-test-{}-{unique}-pre_issue_9_migration.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        {
            // Hand-build the pre-Issue #9 schema directly, bypassing
            // `DiffStore` entirely (it would already create the new
            // columns/table).
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE revisions (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    revision_name TEXT NOT NULL,
                    is_head INTEGER NOT NULL DEFAULT 0,
                    full_json TEXT NOT NULL,
                    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
                );
                CREATE TABLE diff_records (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    base_revision_id INTEGER NOT NULL,
                    target_revision_id INTEGER NOT NULL,
                    sheet_name TEXT NOT NULL,
                    row INTEGER NOT NULL,
                    col INTEGER NOT NULL,
                    kind TEXT NOT NULL,
                    old_value_json TEXT,
                    new_value_json TEXT
                );
                INSERT INTO revisions (revision_name, is_head, full_json) VALUES ('v1', 1, '{}');
                INSERT INTO diff_records (
                    base_revision_id, target_revision_id, sheet_name, row, col, kind,
                    old_value_json, new_value_json
                ) VALUES (1, 1, 'Sheet1', 1, 1, 'modified', '1', '2');",
            )
            .unwrap();
        }

        let store = DiffStore::open(&path).unwrap();

        // The pre-existing row survived, and reads back with NULL in the
        // two new columns rather than an error or a forced rewrite.
        let (kind, old_style_json): (String, Option<String>) = store
            .conn
            .query_row(
                "SELECT kind, old_style_json FROM diff_records WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(kind, "modified");
        assert_eq!(old_style_json, None);

        // merge_diff_records now exists and is usable.
        let merge_table_exists: bool = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'merge_diff_records'",
                [],
                |row| row.get(0),
            )
            .map(|count: i64| count > 0)
            .unwrap();
        assert!(merge_table_exists);

        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn open_heals_a_partially_migrated_database_missing_only_new_style_json() {
        // Regression test for code review on PR #13: `execute_batch`
        // (`sqlite3_exec`) does not wrap multiple `ALTER TABLE` statements
        // in an implicit transaction, so a process that failed between
        // adding `old_style_json` and `new_style_json` on a prior `open`
        // could leave a database with `old_style_json` present but
        // `new_style_json` still missing. `migrate_pre_issue_9_schema`
        // must check (and add) each column independently rather than
        // gating both on a single `old_style_json` existence check — the
        // latter would see `old_style_json` already present, skip
        // `new_style_json` forever, and break every future `save_diff`
        // (its `INSERT` always references `new_style_json`).
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "exceldiff-test-{}-{unique}-partial_migration.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        {
            // Hand-build a database stuck exactly mid-migration: the
            // pre-Issue #9 `diff_records` shape, plus `old_style_json`
            // already added but not `new_style_json` — the state a crash
            // between the two `ALTER TABLE`s in a prior `open` would leave.
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE revisions (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    revision_name TEXT NOT NULL,
                    is_head INTEGER NOT NULL DEFAULT 0,
                    full_json TEXT NOT NULL,
                    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
                );
                CREATE TABLE diff_records (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    base_revision_id INTEGER NOT NULL,
                    target_revision_id INTEGER NOT NULL,
                    sheet_name TEXT NOT NULL,
                    row INTEGER NOT NULL,
                    col INTEGER NOT NULL,
                    kind TEXT NOT NULL,
                    old_value_json TEXT,
                    new_value_json TEXT,
                    old_style_json TEXT
                );",
            )
            .unwrap();
        }

        let mut store = DiffStore::open(&path).unwrap();

        // `new_style_json` must now exist — verified the only way that's
        // actually load-bearing: a `save_diff` call whose `CellDiff`
        // carries a `new_style` succeeds instead of failing with "no such
        // column: new_style_json".
        use crate::diff::model::{CellDiff, SheetDiff};
        use crate::{JsonCellValue, JsonColorRef, JsonFont, JsonStyle};

        let base = Workbook::new(vec![sheet_with_one_cell("Sheet1", 1.0)], None);
        let target = Workbook::new(vec![sheet_with_one_cell("Sheet1", 1.0)], None);
        let base_id = store.save_revision("base", false, &base).unwrap();
        let target_id = store.save_revision("target", true, &target).unwrap();
        let diff = WorkbookDiff {
            sheets: vec![SheetDiff {
                name: "Sheet1".to_string(),
                status: DiffStatus::Modified,
                old_visibility: None,
                new_visibility: None,
                cells: vec![CellDiff {
                    row: 1,
                    col: 1,
                    status: DiffStatus::Modified,
                    old_col: None,
                    old_row: None,
                    old_value: Some(JsonCellValue::Number(1.0)),
                    new_value: Some(JsonCellValue::Number(1.0)),
                    old_style: None,
                    new_style: Some(JsonStyle {
                        font: JsonFont {
                            size_pt: 11.0,
                            bold: true,
                        },
                        wrap_text: false,
                        alignment: "general",
                        number_format: None,
                        fill_fg_color: Some(JsonColorRef::Rgb("FFFF0000".to_string())),
                        fill_bg_color: None,
                        borders: None,
                    }),
                }],
                merges: Vec::new(),
            }],
        };
        store.save_diff(base_id, target_id, &diff).unwrap();

        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn save_diff_with_unknown_revision_id_fails_with_diff_storage_error() {
        // Exercises storage_err's own body, only reachable when a rusqlite
        // call genuinely fails — and behaviorally confirms the `PRAGMA
        // foreign_keys = ON` fix (code review on PR #6) actually rejects a
        // diff_records row whose revision ids don't correspond to any
        // revision ever saved via save_revision.
        let mut store = DiffStore::open(":memory:").unwrap();
        let base = Workbook::new(vec![sheet_with_one_cell("Sheet1", 1.0)], None);
        let target = Workbook::new(vec![sheet_with_one_cell("Sheet1", 2.0)], None);
        let diff = diff_workbooks(&base, &target);

        let err = store.save_diff(999, 1000, &diff).unwrap_err();
        assert!(matches!(err, Error::DiffStorage { .. }));
    }
}
