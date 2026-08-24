# `diff/mod.rs` 設計書

*[English](mod.en.md)*

`src/diff/mod.rs` に対応する設計書。[architecture.md](../architecture.md) が定義する5フェーズ・パイプライン（rels解決→サニタイズ→ストリームパース→分析/遅延解決→JSON生成）の**外側に追加された第6の機能領域**であり、[Issue #3](https://github.com/MinamiyamaKotaro/exceldiff/issues/3) が要求する「2つの `Workbook` の差分計算・SQLiteへの永続化・HEADの完全JSON出力」を担う。architecture.mdの5フェーズはいずれも「1つの `.xlsx` を読んで1つの `Workbook`/JSONを返す」ことを前提とするのに対し、`diff/` は既にフェーズ1〜4を完了した**2つの** `Workbook` を受け取って比較する後段の機能であるため、既存フェーズへ割り込ませず独立したサブモジュールツリーとした。

## 責務・スコープ

- サブモジュールの宣言（`mod engine; mod model;`、および Cargo feature `diff-storage` でのみ有効になる `#[cfg(feature = "diff-storage")] mod storage;`）と公開型・関数の再エクスポート
- `diff::engine::{diff_paths, diff_workbooks}` と `diff::model::{CellDiff, DiffStatus, SheetDiff, WorkbookDiff}` を無条件に再エクスポートする
- `diff::storage::DiffStore` は `diff-storage` フィーチャーが有効な場合のみ再エクスポートする — [`Cargo.toml`](../../../Cargo.toml) で `rusqlite` を `optional = true` とし、`diff-storage = ["dep:rusqlite"]` フィーチャーで束ねている（詳細は[storage.md 責務・スコープ](storage.md)参照）。`parse_workbook` しか使わない一般的な利用者が `rusqlite`（bundled SQLiteを含む）のコンパイルコストを一切払わずに済むようにするための、クレート説明文「lightweight」という自己規定と整合する設計判断
- **含まない責務**: 差分の型定義そのもの（[`model.rs`](model.md)）、差分計算ロジックそのもの（[`engine.rs`](engine.md)）、SQLite永続化そのもの（[`storage.rs`](storage.md)）

## 主要な型・関数（案）

```rust
pub mod engine;
pub mod model;
#[cfg(feature = "diff-storage")]
pub mod storage;

pub use engine::{diff_paths, diff_workbooks};
pub use model::{CellDiff, DiffStatus, SheetDiff, WorkbookDiff};
#[cfg(feature = "diff-storage")]
pub use storage::DiffStore;
```

## 依存関係

- 依存先: [`diff/model.rs`](model.md)（`mod` 宣言）、[`diff/engine.rs`](engine.md)（`mod` 宣言）、[`diff/storage.rs`](storage.md)（`mod` 宣言、`diff-storage` フィーチャー時のみ）
- 依存元: [`lib.rs`](../lib.md)（`mod diff;` として非公開宣言した上で、本ファイルが再エクスポートする型・関数をさらに `pub use diff::{...};` でクレートルートへフラットに再エクスポートする）

`lib.rs` が `diff::` という名前空間パス（例: `exceldiff::diff::WorkbookDiff`）ではなく、`model/` 由来の型（`Cell`, `Sheet` 等）と同様にクレートルート直下（`exceldiff::WorkbookDiff`）へフラットに再エクスポートしているのは、[lib.md](../lib.md) が既に確立している「サブモジュールを非公開 `mod` として隠蔽し、外部公開したい型・関数だけをクレートルートへ集約する」という一貫した公開API方針にならったもの。[Issue #3](https://github.com/MinamiyamaKotaro/exceldiff/issues/3) の提案ディレクトリ構成コメント「`mod.rs` # 差分モジュールの公開インターフェース」は、ファイル分割そのものについての言及であり、`exceldiff::diff::` という独立した公開名前空間を要求するものではないと解釈した。

## エラー処理方針

- 本ファイル自身は `mod` 宣言と再エクスポートのみで、エラーを生成する処理を持たない。

## テスト方針

- 本ファイル自身の直接テストは持たない。再エクスポートが正しく機能していることは、[`tests/diff.rs`](../../../tests/diff.rs) が `exceldiff::{diff_workbooks, diff_paths, DiffStatus, JsonCellValue, WorkbookDiff}`（および `diff-storage` フィーチャー時は `exceldiff::DiffStore`）をクレートルートから直接 `use` して呼び出せていることで間接的に検証される。

## 未決事項 / オープンクエスチョン

1. **行/列挿入検出（2D LCSアライメント）モードの追加場所**: [Issue #4](https://github.com/MinamiyamaKotaro/exceldiff/issues/4)（行挿入/削除検出）・[Issue #5](https://github.com/MinamiyamaKotaro/exceldiff/issues/5)（列挿入/削除検出）が要求する、上限付きオプトインのアライメントベース差分を実装する場合、`diff::engine` 内に関数を追加するのか（例: `diff_workbooks_aligned`）、`diff::alignment` のような新規サブモジュールとして分離するのかは未決定。[engine.md 未決事項](engine.md)参照。
2. **`DiffStore` 以外のストレージバックエンドの要否**: 現状SQLite（`rusqlite`）のみをサポートする。[Issue #3](https://github.com/MinamiyamaKotaro/exceldiff/issues/3) の検討事項1「`rusqlite` を本体のデフォルト依存にするか、Cargo featureとしてオプショナルにするか」は「オプショナルにする」で解決したが、他のストレージ（例: JSON Lines ファイルへの追記）を求める要望が生じた場合、`diff::storage` をトレイト抽象化するかは未決定。
