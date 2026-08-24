# `diff/mod.rs` Design Doc

*[日本語](mod.md)*

Design doc for `src/diff/mod.rs`. This is a **sixth functional area layered on top of** the 5-phase pipeline (rels resolution → sanitization → stream parsing → analysis/deferred resolution → JSON generation) [architecture.md](../architecture.en.md) defines — it implements [Issue #3](https://github.com/MinamiyamaKotaro/exceldiff/issues/3)'s requirement for "diffing two `Workbook`s, persisting the result to SQLite, and outputting HEAD's complete JSON." Every one of architecture.md's 5 phases assumes "read one `.xlsx`, return one `Workbook`/JSON," whereas `diff/` is a downstream capability that takes **two** already-Phase-1–4-completed `Workbook`s and compares them — so it was kept as an independent module subtree rather than spliced into the existing phases.

## Responsibility / Scope

- Declares submodules (`mod alignment; mod engine; mod model;`, plus `#[cfg(feature = "diff-storage")] mod storage;`, active only under the `diff-storage` Cargo feature) and re-exports their public types/functions
- Unconditionally re-exports `diff::engine::{diff_paths, diff_workbooks}`, `diff::alignment::{diff_workbooks_aligned_columns, ColumnAlignmentLimits}` (Issue #5), and `diff::model::{CellDiff, DiffStatus, SheetDiff, WorkbookDiff}`
- Re-exports `diff::storage::DiffStore` only when the `diff-storage` feature is enabled — [`Cargo.toml`](../../../Cargo.toml) marks `rusqlite` `optional = true`, bundled under the `diff-storage = ["dep:rusqlite"]` feature (see [storage.md Responsibility/Scope](storage.en.md)). This design choice keeps a typical `parse_workbook`-only consumer from ever paying `rusqlite`'s (bundled SQLite) compile cost, consistent with the crate's own "lightweight" self-description
- **Not responsible for**: the diff type definitions themselves ([`model.rs`](model.en.md)), the coordinate-based diff computation logic itself ([`engine.rs`](engine.en.md)), the column-alignment-based diff computation logic itself ([`alignment.rs`](alignment.en.md)), SQLite persistence itself ([`storage.rs`](storage.en.md))

## Key Types / Functions (draft)

```rust
pub mod alignment;
pub mod engine;
pub mod model;
#[cfg(feature = "diff-storage")]
pub mod storage;

pub use alignment::{diff_workbooks_aligned_columns, ColumnAlignmentLimits};
pub use engine::{diff_paths, diff_workbooks};
pub use model::{CellDiff, DiffStatus, SheetDiff, WorkbookDiff};
#[cfg(feature = "diff-storage")]
pub use storage::DiffStore;
```

## Dependencies

- Depends on: [`diff/model.rs`](model.en.md) (`mod` declaration), [`diff/engine.rs`](engine.en.md) (`mod` declaration), [`diff/alignment.rs`](alignment.en.md) (`mod` declaration, Issue #5), [`diff/storage.rs`](storage.en.md) (`mod` declaration, `diff-storage` feature only)
- Depended on by: [`lib.rs`](../lib.en.md) (declares `mod diff;` privately, then re-exports this file's re-exported types/functions flatly onto the crate root via `pub use diff::{...};`)

`lib.rs` re-exports these flatly at the crate root (`exceldiff::WorkbookDiff`) rather than through a `diff::` namespace path (`exceldiff::diff::WorkbookDiff`), the same way `model/`-derived types (`Cell`, `Sheet`, etc.) already are — following [lib.md](../lib.en.md)'s already-established public API policy of "hide submodules behind private `mod`s, and funnel everything meant to be public onto the crate root." [Issue #3](https://github.com/MinamiyamaKotaro/exceldiff/issues/3)'s proposed directory layout comment ("`mod.rs` # the diff module's public interface") is read as being about file organization, not a requirement for an independent `exceldiff::diff::` public namespace.

## Error Handling Policy

- This file only declares `mod`s and re-exports; it has no logic that generates errors.

## Testing Strategy

- No direct tests of this file itself. That the re-exports work correctly is verified indirectly by [`tests/diff.rs`](../../../tests/diff.rs) `use`-ing `exceldiff::{diff_workbooks, diff_paths, diff_workbooks_aligned_columns, ColumnAlignmentLimits, DiffStatus, JsonCellValue, WorkbookDiff}` (and, under the `diff-storage` feature, `exceldiff::DiffStore`) directly from the crate root and calling them successfully.

## Open Questions

1. ~~Where a row/column-insertion detection (2D LCS alignment) mode would live~~ → **Resolved for columns** ([Issue #5](https://github.com/MinamiyamaKotaro/exceldiff/issues/5)): split into a new submodule, `diff::alignment`, exposing `diff_workbooks_aligned_columns`/`ColumnAlignmentLimits`. See [alignment.en.md](alignment.en.md). Row insertion/deletion detection ([Issue #4](https://github.com/MinamiyamaKotaro/exceldiff/issues/4)) remains unimplemented. See also [engine.md Open Questions](engine.en.md).
2. **Whether storage backends other than `DiffStore` are needed**: currently only SQLite (`rusqlite`) is supported. [Issue #3](https://github.com/MinamiyamaKotaro/exceldiff/issues/3)'s Open Question 1 ("make `rusqlite` a default dependency, or an optional Cargo feature?") was resolved as "optional"; whether `diff::storage` should be abstracted behind a trait if a request for another backend (e.g. appending to a JSON Lines file) ever arises is undecided.
