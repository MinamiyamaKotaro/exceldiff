# `diff/storage.rs` 設計書

*[English](storage.en.md)*

`src/diff/storage.rs` に対応する設計書。[Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3) が要求する「差分内容のSQLiteへの保存」および「HEAD指定時の完全JSON出力」を、[`diff/engine.rs`](engine.md) が計算した `WorkbookDiff` を対象に実装する。[Issue #8](https://github.com/MinamiyamaKotaro/xlsxparser/issues/8) で追加された `CellDiff::old_style`/`new_style`・`SheetDiff::merges` の永続化は[Issue #9](https://github.com/MinamiyamaKotaro/xlsxparser/issues/9)で追加した。Cargo feature `diff-storage` が有効な場合のみコンパイルされる（[diff/mod.md 責務・スコープ](mod.md)参照）。

## 責務・スコープ

- SQLiteデータベースを開き、`revisions`/`diff_records`/`merge_diff_records` の3テーブルからなるスキーマを（存在しなければ）作成する `DiffStore::open`。既存のデータベースファイルが[Issue #9](https://github.com/MinamiyamaKotaro/xlsxparser/issues/9)以前のスキーマ（`diff_records` に `old_style_json`/`new_style_json` カラムが無い）である場合は、`ALTER TABLE ... ADD COLUMN` で追加カラムを非破壊的に補う（マイグレーション方針セクション参照）
- `model::Workbook` を名前付きリビジョンとして保存する `save_revision`。`is_head: true` を渡すと、それ以前に `is_head` フラグの立っていた全リビジョンを先にクリアしてから新規行を挿入する(常に「直近に保存されたHEAD」が1件だけになる)。保存する `full_json` は [`json.rs`](../json.md) の `to_json_string` の出力そのものであり、本ファイル独自のスナップショット表現を新設しない([Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3)「HEAD 指定時は完全な JSON 出力をそのまま行う」要件をそのまま満たす)
- `WorkbookDiff` の各セル差分(`old_style`/`new_style` を含む)を `diff_records` テーブルへ、各結合セル差分を `merge_diff_records` テーブルへ、1行ずつトランザクション内で永続化する `save_diff`([Issue #9](https://github.com/MinamiyamaKotaro/xlsxparser/issues/9))
- HEADフラグが立っている最新リビジョンの `full_json` をそのまま返す `head_json`
- **含まない責務**: 差分計算そのもの([`diff/engine.rs`](engine.md))、差分結果の型定義([`diff/model.rs`](model.md))、`diff_records`/`merge_diff_records` からの差分検索・クエリ機能([Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3) のPoCが持っていたクエリ機能は、ユーザーの明示的な要求範囲を超えるため実装していない——未決事項2参照)、`SheetDiff::status`/`old_visibility`/`new_visibility`(シート単位の可視性変更・追加削除)の永続化(未決事項3参照——[Issue #9](https://github.com/MinamiyamaKotaro/xlsxparser/issues/9)の追加PoC検証で `sheet_diff_records` テーブル案が提案されたが、Issue #9自体のスコープ(`old_style`/`new_style`/`merges`)を超えるため今回は見送り、別issueでの追跡を推奨する)

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

pub struct DiffStore {
    conn: Connection,
}

impl DiffStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        // SCHEMA_SQL を実行(CREATE TABLE IF NOT EXISTS)した後、
        // 既存の diff_records に old_style_json が無ければ ALTER TABLE で
        // 追加する(マイグレーション方針セクション参照)。
        /* ... */
    }

    pub fn save_revision(&mut self, name: &str, is_head: bool, workbook: &Workbook) -> Result<i64> {
        let full_json = crate::to_json_string(workbook)?;
        // is_head なら既存のHEADフラグを全てクリアしてから新規行を挿入。
        // full_json は json.rs::to_json_string の出力そのもの。
        /* ... */
    }

    pub fn save_diff(&mut self, base_revision_id: i64, target_revision_id: i64, diff: &WorkbookDiff) -> Result<()> {
        // トランザクション内で diff.sheets[*].cells(old_style/new_style込み)
        // を diff_records へ、diff.sheets[*].merges を merge_diff_records へ、
        // それぞれ1件ずつINSERT。
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

- 依存先: [`diff/model.rs`](model.md)（`DiffStatus`, `WorkbookDiff`——[Issue #9](https://github.com/MinamiyamaKotaro/xlsxparser/issues/9)以降は `CellDiff::old_style`/`new_style`・`SheetDiff::merges`・`MergeDiff`・`CellPos` も参照する）、[`json.rs`](../json.md)（`crate::to_json_string` 経由）、[`error.rs`](../error.md)（`Error::DiffStorage`——本ファイル専用の新設バリアント、下記エラー処理方針参照）、[`lib.rs`](../lib.md)（`crate::Result`, `crate::to_json_string`）、外部クレート `rusqlite`（`features = ["bundled"]`——システムにSQLiteライブラリがインストールされていなくてもビルドできるよう、SQLite自体をCから静的リンクする）、`serde_json`（`CellDiff::old_value`/`new_value`/`old_style`/`new_style` を個別に文字列化する際に使用）
- 依存元: [`diff/mod.rs`](mod.md)（`diff-storage` フィーチャー時のみ `DiffStore` を再エクスポート）

`rusqlite` は [`Cargo.toml`](../../../Cargo.toml) で `optional = true` とし、`[features] diff-storage = ["dep:rusqlite"]` で束ねている。`Cargo.toml` のパッケージ説明文が「A lightweight, high-performance .xlsx (OOXML) parser library」と自己規定している以上、`parse_workbook`/`to_json_string` しか使わない一般的な利用者に、`rusqlite`（bundled SQLiteのビルドを含む）のコンパイル時間増加を強制すべきではないという判断（[Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3) 検討事項1「`rusqlite` を本体のデフォルト依存にするか、Cargo featureとしてオプショナルにするか」への回答）。

## エラー処理方針

- `rusqlite::Error` を直接 `Result` の型として伝播させず、新設した `Error::DiffStorage { source: Box<dyn std::error::Error + Send + Sync + 'static> }` バリアントへ型消去して包む（`storage_err` ヘルパー）。理由は[error.md](../error.md) が `XmlParse::source`（`quick-xml`）を型消去している設計判断と同一——`rusqlite` の具体的なエラー型をクレート公開APIのフィールド型に直接置くと、`rusqlite` が事実上パブリック依存になってしまう。`Error::DiffStorage` 自体は `#[cfg(feature = "diff-storage")]` でガードされており、フィーチャー無効時は `Error` enumにこのバリアントが存在しない
- `CellDiff::old_value`/`new_value`（`JsonCellValue`）を個別に `serde_json::to_string` する際の失敗は `Error::JsonSerialize` へ変換する（`json_err` ヘルパー）。[json.rs](../json.md) が `serde_json::Error` を包む際に使っているのと同じ既存バリアントを再利用し、新しいバリアントを増やさない
- `save_diff` はSQLite側のトランザクション（`Connection::transaction`）内で全`INSERT`を実行し、`tx.commit()` に成功するまでいずれのINSERTも確定しない——処理途中でエラーが発生した場合、部分的に書き込まれた `diff_records`/`merge_diff_records` が残らない(fail closed)

## マイグレーション方針（[Issue #9](https://github.com/MinamiyamaKotaro/xlsxparser/issues/9)）

`DiffStore::open` は `SCHEMA_SQL` を実行した後、`PRAGMA table_info('diff_records')`（正確には `pragma_table_info` テーブル関数）で `old_style_json` カラムの有無を確認する。既存の `diff_records` テーブルに対する `CREATE TABLE IF NOT EXISTS` はテーブルが既に存在する場合何もしない(カラムを追加しない)ため、[Issue #9](https://github.com/MinamiyamaKotaro/xlsxparser/issues/9)以前のスキーマで作られたデータベースファイルをそのまま `DiffStore::open` した場合、明示的な `ALTER TABLE diff_records ADD COLUMN old_style_json TEXT` / `... new_style_json TEXT`(いずれも `NOT NULL`/`DEFAULT` を付けない nullable カラム)を実行しない限り新しいカラムは生まれない。

SQLiteの `ALTER TABLE ... ADD COLUMN`(nullable・`DEFAULT` 無し)はテーブル全体を書き換えないメタデータのみの変更であり、既存行は書き換えられず新カラムは単に `NULL` として読み出される——`poc/issue9-poc`/`poc/issue9-poc-v2` の両PoCで、10万行規模の既存データに対しても数ミリ秒で完了し、既存データが非破壊であることを確認済み(該当issueコメント参照)。`merge_diff_records` テーブル自体は元々存在しないため、`SCHEMA_SQL` の `CREATE TABLE IF NOT EXISTS merge_diff_records` がそのまま新規作成として機能する。

## テスト方針

`src/diff/storage.rs` 内の単体テスト（`:memory:` SQLiteデータベース、`Workbook` は公開モデルAPI経由で直接構築）:

- 何もリビジョンを保存していない状態で `head_json` が `None` を返すこと
- `save_revision(..., is_head: true, ...)` で保存した内容が、`head_json` から `crate::to_json_string` の出力と**完全に一致**する形で取得できること(独自スナップショット形式との乖離が無いことの直接的な回帰テスト)
- 新たに `is_head: true` でリビジョンを保存すると、直前のHEADフラグがクリアされ `head_json` が新しい方の内容を返すこと
- `save_diff` が `WorkbookDiff` のセル差分件数と同数の行を `diff_records` へ挿入し、`kind` カラムが `DiffStatus` と対応する文字列(`"added"`/`"modified"`/`"deleted"`)になっていること
- （[Issue #9](https://github.com/MinamiyamaKotaro/xlsxparser/issues/9)）`CellDiff::old_style`/`new_style` が設定されている場合、`diff_records.old_style_json`/`new_style_json` に `serde_json::to_string` の出力と完全一致する形で保存されること
- （[Issue #9](https://github.com/MinamiyamaKotaro/xlsxparser/issues/9)）スタイル変更を伴わない `Modified` セル(値のみ変更)では、`old_style_json`/`new_style_json` が `NULL` のまま保存されること(空文字列や偽のスタイルを書き込まない)
- （[Issue #9](https://github.com/MinamiyamaKotaro/xlsxparser/issues/9)）`SheetDiff::merges` の `Added`/`Modified`/`Deleted` それぞれが `merge_diff_records` へ1行ずつ、`kind`・`start_row`/`start_col`・`old_end_row`/`old_end_col`・`new_end_row`/`new_end_col` が対応する値で保存されること
- （[Issue #9](https://github.com/MinamiyamaKotaro/xlsxparser/issues/9)）[Issue #9](https://github.com/MinamiyamaKotaro/xlsxparser/issues/9)以前のスキーマ(`old_style_json`/`new_style_json` カラム・`merge_diff_records` テーブルの無い `diff_records`)を持つ実ファイルに対して `DiffStore::open` を呼ぶと、既存行を保持したまま新カラム・新テーブルが追加されること(マイグレーション方針セクション参照)

[`tests/diff.rs`](../../../tests/diff.rs)（`diff-storage` フィーチャー時のみ有効な統合テスト）:

- 実際に `.xlsx` 相当のバイト列を `parse_workbook_reader` でパースして得た本物の `Workbook` を `save_revision`/`save_diff` へ渡し、`head_json` が対象ワークブックの `to_json_string` 出力と一致することを確認する（単体テストが合成 `Workbook` で検証している契約を、実パースパイプライン経由でも再確認する）
- （[Issue #9](https://github.com/MinamiyamaKotaro/xlsxparser/issues/9)）実パースパイプライン経由で得たスタイル差分・結合差分（`tests/fixtures/diff.rs` の `style_only_change`/`merge_added` フィクスチャ）を `save_diff` へ渡してもエラーにならないこと——`diff_records`/`merge_diff_records` の中身の詳細な検証は単体テスト（`self.conn` への直接アクセスが可能）側の責務とし、本テストは実パース由来のデータで `serde_json`/`rusqlite` のバインディングが破綻しないことの回帰確認に留める

## 未決事項 / オープンクエスチョン

1. **`full_json` の保持ポリシー**: 現状、HEADかどうかに関わらず全てのリビジョンで `full_json`（完全なJSONスナップショット文字列）を保存する。リビジョンを重ねるほどDBサイズが線形に増加するため、[Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3) の PoC検証(`poc/issue3-poc/output/verification_report.md`)でも指摘した通り、古いリビジョンの `full_json` を間引く・圧縮するなどの保持ポリシーは呼び出し側の判断に委ねており、本ファイルは提供しない。将来的に標準的な間引き関数（例: `DiffStore::prune_full_json_except_head`）を提供する価値があるかは、実運用での要望を踏まえて判断する。
2. **`diff_records` からの差分検索API**: [Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3) のPoCは「2つのリビジョンID間の差分をSQLiteから逆引き取得する」クエリ機能を持っていたが、本実装のユーザーからの直接の要求（`/diff`モジュール作成の依頼文）は「差分の保存」と「HEADの完全JSON出力」のみであったため、クエリ機能自体は実装していない。呼び出し側は `SELECT * FROM diff_records WHERE base_revision_id = ? AND target_revision_id = ?` を直接発行すればよく、`DiffStore` にラッパーメソッドを追加する必要性は生じていない。
3. **`SheetDiff` の可視性変更・シート追加削除情報の永続化**: `save_diff` は `diff.sheets[*].cells`/`merges`（セル単位・結合セル単位の変更）のみを `diff_records`/`merge_diff_records` へ書き込み、`SheetDiff::status`/`old_visibility`/`new_visibility` そのものは保存しない（`diff/storage.md` 依存関係セクション、[model.md](model.md)参照）。シート単位の履歴が必要な呼び出し側は、`WorkbookDiff` 自体を別途JSONとして保存する必要がある。[Issue #9](https://github.com/MinamiyamaKotaro/xlsxparser/issues/9)の追加PoC検証（`poc/issue9-poc-v2`）では、この用途向けに `sheet_diff_records` テーブル(オーバーヘッドは実測で保存時間+5%未満・ディスクサイズ+8KB程度と小さい)を追加する案が提案・検証されたが、Issue #9自体の要求範囲（`old_style`/`new_style`/`merges`）を超えるため今回は採用を見送った。専用カラム・テーブルを追加するかは、実際にシート単位の追跡が必要になった時点で（可能であれば `poc/issue9-poc-v2` の実測結果を出発点として）改めて検討する。
4. **並行アクセス**: `rusqlite::Connection` は `Send` だが `Sync` ではなく、複数スレッドから同一の `DiffStore` を共有するには呼び出し側で `Mutex` 等による排他が必要になる。本ファイルは単一接続・単一スレッドでの利用を前提としており、コネクションプーリング（例: `r2d2_sqlite`）の要否は要求が生じた時点で検討する。
5. ~~スタイル・セル結合差分のスキーマ拡張~~ → **解決**（[Issue #9](https://github.com/MinamiyamaKotaro/xlsxparser/issues/9)）: [Issue #8](https://github.com/MinamiyamaKotaro/xlsxparser/issues/8)で追加した `CellDiff::old_style`/`new_style` は `diff_records.old_style_json`/`new_style_json`（インライン方式）へ、`SheetDiff::merges` は新設の `merge_diff_records` テーブルへ、それぞれ `save_diff` が永続化するようになった。設計は2回のPoCベンチマーク（`poc/issue9-poc`: スキーマA「インライン」対スキーマB「`style_diff_records` 分離」の比較、`poc/issue9-poc-v2`: スキーマC「スタイルカタログ辞書テーブル」を加えた4方式・4ワークロードでの追加比較）で検証済み——インライン方式（スキーマA）が保存時間・ディスクサイズの双方で一貫して優位（3〜18%高速・3〜5%小さい）だった一方、メモリ使用量には設計間で有意差が無かった（ワークブックのパース・diff計算側が支配的）。スタイルカタログ方式（スキーマC）は同一スタイルが多数セルで共有される場合にディスクサイズを大幅削減できる（実測で約68%減）が、セル毎に一意なスタイルが多いケースでは保存時間が約2.7倍に悪化するトレードオフがあり、本クレートの「軽量・シンプル・依存最小」という設計方針とも整合しないため採用しなかった。既存データベースへのマイグレーション（`ALTER TABLE ADD COLUMN`）も両PoCで安全性・性能を確認済み（マイグレーション方針セクション参照）。
