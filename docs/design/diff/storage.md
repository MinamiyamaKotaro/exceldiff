# `diff/storage.rs` 設計書

*[English](storage.en.md)*

`src/diff/storage.rs` に対応する設計書。[Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3) が要求する「差分内容のSQLiteへの保存」および「HEAD指定時の完全JSON出力」を、[`diff/engine.rs`](engine.md) が計算した `WorkbookDiff` を対象に実装する。Cargo feature `diff-storage` が有効な場合のみコンパイルされる（[diff/mod.md 責務・スコープ](mod.md)参照）。

## 責務・スコープ

- SQLiteデータベースを開き、`revisions`/`diff_records` の2テーブルからなるスキーマを（存在しなければ）作成する `DiffStore::open`
- `model::Workbook` を名前付きリビジョンとして保存する `save_revision`。`is_head: true` を渡すと、それ以前に `is_head` フラグの立っていた全リビジョンを先にクリアしてから新規行を挿入する（常に「直近に保存されたHEAD」が1件だけになる）。保存する `full_json` は [`json.rs`](../json.md) の `to_json_string` の出力そのものであり、本ファイル独自のスナップショット表現を新設しない（[Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3)「HEAD 指定時は完全な JSON 出力をそのまま行う」要件をそのまま満たす）
- `WorkbookDiff` の各セル差分を `diff_records` テーブルへ1行ずつ、トランザクション内で永続化する `save_diff`
- HEADフラグが立っている最新リビジョンの `full_json` をそのまま返す `head_json`
- **含まない責務**: 差分計算そのもの（[`diff/engine.rs`](engine.md)）、差分結果の型定義（[`diff/model.rs`](model.md)）、`diff_records` からの差分検索・クエリ機能（[Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3) のPoCが持っていたクエリ機能は、ユーザーの明示的な要求範囲を超えるため実装していない——未決事項2参照）

## 主要な型・関数（案）

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
        // is_head なら既存のHEADフラグを全てクリアしてから新規行を挿入。
        // full_json は json.rs::to_json_string の出力そのもの。
        /* ... */
    }

    pub fn save_diff(&mut self, base_revision_id: i64, target_revision_id: i64, diff: &WorkbookDiff) -> Result<()> {
        // トランザクション内で diff.sheets[*].cells を1件ずつINSERT。
        /* ... */
    }

    pub fn head_json(&self) -> Result<Option<String>> {
        // SELECT full_json FROM revisions WHERE is_head = 1 ORDER BY id DESC LIMIT 1
        /* ... */
    }
}
```

（完全な実装は `src/diff/storage.rs` を参照。本ドキュメントでは骨子のみ示す。）

## 依存関係

- 依存先: [`diff/model.rs`](model.md)（`DiffStatus`, `WorkbookDiff`）、[`json.rs`](../json.md)（`crate::to_json_string` 経由）、[`error.rs`](../error.md)（`Error::DiffStorage`——本ファイル専用の新設バリアント、下記エラー処理方針参照）、[`lib.rs`](../lib.md)（`crate::Result`, `crate::to_json_string`）、外部クレート `rusqlite`（`features = ["bundled"]`——システムにSQLiteライブラリがインストールされていなくてもビルドできるよう、SQLite自体をCから静的リンクする）、`serde_json`（`CellDiff::old_value`/`new_value` を個別に文字列化する際に使用）
- 依存元: [`diff/mod.rs`](mod.md)（`diff-storage` フィーチャー時のみ `DiffStore` を再エクスポート）

`rusqlite` は [`Cargo.toml`](../../../Cargo.toml) で `optional = true` とし、`[features] diff-storage = ["dep:rusqlite"]` で束ねている。`Cargo.toml` のパッケージ説明文が「A lightweight, high-performance .xlsx (OOXML) parser library」と自己規定している以上、`parse_workbook`/`to_json_string` しか使わない一般的な利用者に、`rusqlite`（bundled SQLiteのビルドを含む）のコンパイル時間増加を強制すべきではないという判断（[Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3) 検討事項1「`rusqlite` を本体のデフォルト依存にするか、Cargo featureとしてオプショナルにするか」への回答）。

## エラー処理方針

- `rusqlite::Error` を直接 `Result` の型として伝播させず、新設した `Error::DiffStorage { source: Box<dyn std::error::Error + Send + Sync + 'static> }` バリアントへ型消去して包む（`storage_err` ヘルパー）。理由は[error.md](../error.md) が `XmlParse::source`（`quick-xml`）を型消去している設計判断と同一——`rusqlite` の具体的なエラー型をクレート公開APIのフィールド型に直接置くと、`rusqlite` が事実上パブリック依存になってしまう。`Error::DiffStorage` 自体は `#[cfg(feature = "diff-storage")]` でガードされており、フィーチャー無効時は `Error` enumにこのバリアントが存在しない
- `CellDiff::old_value`/`new_value`（`JsonCellValue`）を個別に `serde_json::to_string` する際の失敗は `Error::JsonSerialize` へ変換する（`json_err` ヘルパー）。[json.rs](../json.md) が `serde_json::Error` を包む際に使っているのと同じ既存バリアントを再利用し、新しいバリアントを増やさない
- `save_diff` はSQLite側のトランザクション（`Connection::transaction`）内で全`INSERT`を実行し、`tx.commit()` に成功するまでいずれのINSERTも確定しない——処理途中でエラーが発生した場合、部分的に書き込まれた `diff_records` が残らない(fail closed)

## テスト方針

`src/diff/storage.rs` 内の単体テスト（`:memory:` SQLiteデータベース、`Workbook` は公開モデルAPI経由で直接構築）:

- 何もリビジョンを保存していない状態で `head_json` が `None` を返すこと
- `save_revision(..., is_head: true, ...)` で保存した内容が、`head_json` から `crate::to_json_string` の出力と**完全に一致**する形で取得できること(独自スナップショット形式との乖離が無いことの直接的な回帰テスト)
- 新たに `is_head: true` でリビジョンを保存すると、直前のHEADフラグがクリアされ `head_json` が新しい方の内容を返すこと
- `save_diff` が `WorkbookDiff` のセル差分件数と同数の行を `diff_records` へ挿入し、`kind` カラムが `DiffStatus` と対応する文字列(`"added"`/`"modified"`/`"deleted"`)になっていること

[`tests/diff.rs`](../../../tests/diff.rs)（`diff-storage` フィーチャー時のみ有効な統合テスト）:

- 実際に `.xlsx` 相当のバイト列を `parse_workbook_reader` でパースして得た本物の `Workbook` を `save_revision`/`save_diff` へ渡し、`head_json` が対象ワークブックの `to_json_string` 出力と一致することを確認する（単体テストが合成 `Workbook` で検証している契約を、実パースパイプライン経由でも再確認する）

## 未決事項 / オープンクエスチョン

1. **`full_json` の保持ポリシー**: 現状、HEADかどうかに関わらず全てのリビジョンで `full_json`（完全なJSONスナップショット文字列）を保存する。リビジョンを重ねるほどDBサイズが線形に増加するため、[Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3) の PoC検証(`poc/issue3-poc/output/verification_report.md`)でも指摘した通り、古いリビジョンの `full_json` を間引く・圧縮するなどの保持ポリシーは呼び出し側の判断に委ねており、本ファイルは提供しない。将来的に標準的な間引き関数（例: `DiffStore::prune_full_json_except_head`）を提供する価値があるかは、実運用での要望を踏まえて判断する。
2. **`diff_records` からの差分検索API**: [Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3) のPoCは「2つのリビジョンID間の差分をSQLiteから逆引き取得する」クエリ機能を持っていたが、本実装のユーザーからの直接の要求（`/diff`モジュール作成の依頼文）は「差分の保存」と「HEADの完全JSON出力」のみであったため、クエリ機能自体は実装していない。呼び出し側は `SELECT * FROM diff_records WHERE base_revision_id = ? AND target_revision_id = ?` を直接発行すればよく、`DiffStore` にラッパーメソッドを追加する必要性は生じていない。
3. **`SheetDiff` の可視性変更・シート追加削除情報の永続化**: `save_diff` は `diff.sheets[*].cells`（セル単位の変更）のみを `diff_records` へ書き込み、`SheetDiff::status`/`old_visibility`/`new_visibility` そのものは保存しない（`diff/storage.md` 依存関係セクション、[model.md](model.md)参照）。シート単位の履歴が必要な呼び出し側は、`WorkbookDiff` 自体を別途JSONとして保存する必要がある。専用カラム（例: `sheet_diffs` テーブル）を追加するかは、実際にシート単位の追跡が必要になった時点で検討する。
4. **並行アクセス**: `rusqlite::Connection` は `Send` だが `Sync` ではなく、複数スレッドから同一の `DiffStore` を共有するには呼び出し側で `Mutex` 等による排他が必要になる。本ファイルは単一接続・単一スレッドでの利用を前提としており、コネクションプーリング（例: `r2d2_sqlite`）の要否は要求が生じた時点で検討する。
