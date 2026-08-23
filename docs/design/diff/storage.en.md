# `diff/storage.rs` Design Doc

*[日本語](storage.md)*

Design doc for `src/diff/storage.rs`. Implements [Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3)'s requirement to "persist diff content to SQLite" and to "output the complete JSON when HEAD is specified," acting on the `WorkbookDiff` [`diff/engine.rs`](engine.en.md) computes. Compiled only when the `diff-storage` Cargo feature is enabled (see [diff/mod.md Responsibility/Scope](mod.en.md)).

## Responsibility / Scope

- `DiffStore::open`, which opens a SQLite database and creates the `revisions`/`diff_records` schema if it doesn't already exist
- `save_revision`, which saves a `model::Workbook` as a named revision. Passing `is_head: true` first clears the `is_head` flag on every previously-flagged revision before inserting the new row (so exactly one revision is ever "the most recently saved HEAD"). The `full_json` it stores is exactly [`json.rs`](../json.en.md)'s `to_json_string` output — this file never introduces its own snapshot representation (satisfying [Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3)'s "output HEAD's complete JSON as-is" requirement directly)
- `save_diff`, which persists every cell diff in a `WorkbookDiff` as one row each in `diff_records`, inside a transaction
- `head_json`, which returns the `full_json` of whichever revision is currently flagged HEAD, verbatim
- **Not responsible for**: the diff computation itself ([`diff/engine.rs`](engine.en.md)), the diff result type definitions ([`diff/model.rs`](model.en.md)), querying/searching `diff_records` (the PoC in [Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3) had a query capability, but it goes beyond the user's explicit request scope — see Open Question 2)

## Key Types / Functions (draft)

```rust
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

pub struct DiffStore {
    conn: Connection,
}

impl DiffStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> { /* ... */ }

    pub fn save_revision(&mut self, name: &str, is_head: bool, workbook: &Workbook) -> Result<i64> {
        let full_json = crate::to_json_string(workbook)?;
        // If is_head, clear every existing HEAD flag first, then insert.
        // full_json is exactly json.rs::to_json_string's own output.
        /* ... */
    }

    pub fn save_diff(&mut self, base_revision_id: i64, target_revision_id: i64, diff: &WorkbookDiff) -> Result<()> {
        // INSERT diff.sheets[*].cells one row at a time, inside a transaction.
        /* ... */
    }

    pub fn head_json(&self) -> Result<Option<String>> {
        // SELECT full_json FROM revisions WHERE is_head = 1 ORDER BY id DESC LIMIT 1
        /* ... */
    }
}
```

(See `src/diff/storage.rs` for the complete implementation — only the skeleton is shown here.)

## Dependencies

- Depends on: [`diff/model.rs`](model.en.md) (`DiffStatus`, `WorkbookDiff`), [`json.rs`](../json.en.md) (via `crate::to_json_string`), [`error.rs`](../error.en.md) (`Error::DiffStorage` — a new variant added specifically for this file, see Error Handling Policy below), [`lib.rs`](../lib.en.md) (`crate::Result`, `crate::to_json_string`), the external crate `rusqlite` (`features = ["bundled"]` — statically links SQLite itself from C so the crate builds without a system-installed SQLite library), `serde_json` (used to stringify `CellDiff::old_value`/`new_value` individually)
- Depended on by: [`diff/mod.rs`](mod.en.md) (re-exports `DiffStore` under the `diff-storage` feature only)

`rusqlite` is marked `optional = true` in [`Cargo.toml`](../../../Cargo.toml), bundled under `[features] diff-storage = ["dep:rusqlite"]`. Given the package description self-identifies as "A lightweight, high-performance .xlsx (OOXML) parser library," a typical consumer who only ever calls `parse_workbook`/`to_json_string` should not be forced to pay `rusqlite`'s (bundled SQLite) added compile time — this answers [Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3)'s Open Question 1 ("make `rusqlite` a default dependency, or an optional Cargo feature?").

## Error Handling Policy

- `rusqlite::Error` is never propagated as the `Result` type directly; it is type-erased into a newly-added `Error::DiffStorage { source: Box<dyn std::error::Error + Send + Sync + 'static> }` variant (via the `storage_err` helper). Same rationale as [error.md](../error.en.md)'s type-erasure of `XmlParse::source` (`quick-xml`): putting `rusqlite`'s concrete error type directly on a public API field would make `rusqlite` a de facto public dependency. `Error::DiffStorage` itself is `#[cfg(feature = "diff-storage")]`-gated — the variant doesn't exist in the `Error` enum at all when the feature is off
- A failure stringifying `CellDiff::old_value`/`new_value` (`JsonCellValue`) individually via `serde_json::to_string` converts to `Error::JsonSerialize` (via the `json_err` helper) — reusing the existing variant [json.rs](../json.en.md) already uses to wrap `serde_json::Error`, rather than adding a new one
- `save_diff` runs every `INSERT` inside a SQLite transaction (`Connection::transaction`); nothing is committed until `tx.commit()` succeeds — if an error occurs partway through, no partial write to `diff_records` survives (fail closed)

## Testing Strategy

Unit tests inside `src/diff/storage.rs` (an `:memory:` SQLite database; `Workbook`s built directly via the public model API):

- `head_json` returns `None` before any revision has been saved
- Content saved via `save_revision(..., is_head: true, ...)` comes back from `head_json` as an **exact match** for `crate::to_json_string`'s own output (a direct regression test that no divergent, independent snapshot format has crept in)
- Saving a new revision with `is_head: true` clears the previous HEAD flag, and `head_json` reflects the newer content
- `save_diff` inserts exactly as many `diff_records` rows as there are cell diffs in the `WorkbookDiff`, with the `kind` column matching `DiffStatus` (`"added"`/`"modified"`/`"deleted"`)

[`tests/diff.rs`](../../../tests/diff.rs) (integration test, `diff-storage` feature only):

- Passes a real `Workbook` — obtained by parsing actual `.xlsx`-shaped bytes via `parse_workbook_reader` — into `save_revision`/`save_diff`, and confirms `head_json` matches that same workbook's `to_json_string` output (re-verifying, through the real parse pipeline, the same contract the unit tests already verify against synthetic `Workbook`s)

## Open Questions

1. **`full_json` retention policy**: currently every revision stores `full_json` (a complete JSON snapshot string) regardless of whether it's HEAD, so database size grows linearly with revision count — as already flagged in [Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3)'s PoC verification (`poc/issue3-poc/output/verification_report.md`). Pruning/compressing older revisions' `full_json` is left to the caller's own policy; this file provides no such mechanism. Whether a standard pruning helper (e.g. `DiffStore::prune_full_json_except_head`) is worth adding is to be judged against real-world usage requests.
2. **A search API over `diff_records`**: [Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3)'s PoC had a "look up the diff between two revision IDs from SQLite" query capability, but this implementation's actual direct request (the "create a `/diff` module" request text) asked only for "saving the diff" and "outputting HEAD's complete JSON" — so no query capability was implemented. A caller can issue `SELECT * FROM diff_records WHERE base_revision_id = ? AND target_revision_id = ?` directly; no need for a `DiffStore` wrapper method has arisen.
3. **Persisting `SheetDiff`'s visibility-change/sheet-add-delete info**: `save_diff` writes only `diff.sheets[*].cells` (cell-level changes) to `diff_records`; `SheetDiff::status`/`old_visibility`/`new_visibility` themselves are not saved (see [model.md](model.en.md)). A caller needing sheet-level history would need to persist `WorkbookDiff` itself separately. Whether to add a dedicated column/table (e.g. a `sheet_diffs` table) is to be decided once sheet-level tracking is actually needed.
4. **Concurrent access**: `rusqlite::Connection` is `Send` but not `Sync`; sharing one `DiffStore` across multiple threads requires the caller to add their own exclusion (e.g. a `Mutex`). This file assumes single-connection, single-thread use; whether connection pooling (e.g. `r2d2_sqlite`) is needed is to be judged once that requirement arises.
5. **Schema extension for style/merge diffs**: `save_diff` currently persists neither `CellDiff::old_style`/`new_style` nor `SheetDiff::merges`, both added by [Issue #8](https://github.com/MinamiyamaKotaro/xlsxparser/issues/8) — `diff_records` has no corresponding columns, and `merges` isn't even walked. Tracked separately as [Issue #9](https://github.com/MinamiyamaKotaro/xlsxparser/issues/9).
