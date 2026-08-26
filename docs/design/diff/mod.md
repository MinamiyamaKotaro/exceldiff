# `diff/mod.rs` 設計書

*[English](mod.en.md)*

`src/diff/mod.rs` に対応する設計書。[architecture.md](../architecture.md) が定義する5フェーズ・パイプライン（rels解決→サニタイズ→ストリームパース→分析/遅延解決→JSON生成）の**外側に追加された第6の機能領域**であり、[Issue #3](https://github.com/MinamiyamaKotaro/exceldiff/issues/3) が要求する「2つの `Workbook` の差分計算・SQLiteへの永続化・HEADの完全JSON出力」を担う。architecture.mdの5フェーズはいずれも「1つの `.xlsx` を読んで1つの `Workbook`/JSONを返す」ことを前提とするのに対し、`diff/` は既にフェーズ1〜4を完了した**2つの** `Workbook` を受け取って比較する後段の機能であるため、既存フェーズへ割り込ませず独立したサブモジュールツリーとした。

## 責務・スコープ

- サブモジュールの宣言（`mod best_effort; mod col_alignment; mod engine; mod model; mod row_alignment;`、および Cargo feature `diff-storage` でのみ有効になる `#[cfg(feature = "diff-storage")] mod storage;`）と公開型・関数の再エクスポート
- `diff::engine::{diff_paths, diff_workbooks}`、`diff::col_alignment::{diff_workbooks_aligned_columns, ColumnAlignmentLimits}`（Issue #5）、`diff::row_alignment::{diff_workbooks_aligned_rows, RowAlignmentLimits}`（Issue #4）、`diff::best_effort::diff_workbooks_best_effort`（Issue #25）、`diff::model::{CellDiff, DiffStatus, SheetDiff, WorkbookDiff}` を無条件に再エクスポートする
- `diff::storage::DiffStore` は `diff-storage` フィーチャーが有効な場合のみ再エクスポートする — [`Cargo.toml`](../../../Cargo.toml) で `rusqlite` を `optional = true` とし、`diff-storage = ["dep:rusqlite"]` フィーチャーで束ねている（詳細は[storage.md 責務・スコープ](storage.md)参照）。`parse_workbook` しか使わない一般的な利用者が `rusqlite`（bundled SQLiteを含む）のコンパイルコストを一切払わずに済むようにするための、クレート説明文「lightweight」という自己規定と整合する設計判断
- **含まない責務**: 差分の型定義そのもの（[`model.rs`](model.md)）、座標一致ベースの差分計算ロジックそのもの（[`engine.rs`](engine.md)）、列アライメントベースの差分計算ロジックそのもの（[`col_alignment.rs`](col_alignment.md)）、行アライメントベースの差分計算ロジックそのもの（[`row_alignment.rs`](row_alignment.md)）、3方式を組み合わせるベストエフォート戦略そのもの（[`best_effort.rs`](best_effort.md)）、SQLite永続化そのもの（[`storage.rs`](storage.md)）

## 主要な型・関数（案）

```rust
pub mod best_effort;
pub mod col_alignment;
pub mod engine;
pub mod model;
pub mod row_alignment;
#[cfg(feature = "diff-storage")]
pub mod storage;

pub use best_effort::diff_workbooks_best_effort;
pub use col_alignment::{diff_workbooks_aligned_columns, ColumnAlignmentLimits};
pub use engine::{diff_paths, diff_workbooks};
pub use model::{CellDiff, DiffStatus, SheetDiff, WorkbookDiff};
pub use row_alignment::{diff_workbooks_aligned_rows, RowAlignmentLimits};
#[cfg(feature = "diff-storage")]
pub use storage::DiffStore;
```

## 依存関係

- 依存先: [`diff/model.rs`](model.md)（`mod` 宣言）、[`diff/engine.rs`](engine.md)（`mod` 宣言）、[`diff/col_alignment.rs`](col_alignment.md)（`mod` 宣言、Issue #5）、[`diff/row_alignment.rs`](row_alignment.md)（`mod` 宣言、Issue #4）、[`diff/best_effort.rs`](best_effort.md)（`mod` 宣言、Issue #25）、[`diff/storage.rs`](storage.md)（`mod` 宣言、`diff-storage` フィーチャー時のみ）
- 依存元: [`lib.rs`](../lib.md)（`mod diff;` として非公開宣言した上で、本ファイルが再エクスポートする型・関数をさらに `pub use diff::{...};` でクレートルートへフラットに再エクスポートする）

`lib.rs` が `diff::` という名前空間パス（例: `exceldiff::diff::WorkbookDiff`）ではなく、`model/` 由来の型（`Cell`, `Sheet` 等）と同様にクレートルート直下（`exceldiff::WorkbookDiff`）へフラットに再エクスポートしているのは、[lib.md](../lib.md) が既に確立している「サブモジュールを非公開 `mod` として隠蔽し、外部公開したい型・関数だけをクレートルートへ集約する」という一貫した公開API方針にならったもの。[Issue #3](https://github.com/MinamiyamaKotaro/exceldiff/issues/3) の提案ディレクトリ構成コメント「`mod.rs` # 差分モジュールの公開インターフェース」は、ファイル分割そのものについての言及であり、`exceldiff::diff::` という独立した公開名前空間を要求するものではないと解釈した。

## エラー処理方針

- 本ファイル自身は `mod` 宣言と再エクスポートのみで、エラーを生成する処理を持たない。

## テスト方針

- 本ファイル自身の直接テストは持たない。再エクスポートが正しく機能していることは、[`tests/diff.rs`](../../../tests/diff.rs) が `exceldiff::{diff_workbooks, diff_paths, diff_workbooks_aligned_columns, diff_workbooks_aligned_rows, diff_workbooks_best_effort, ColumnAlignmentLimits, RowAlignmentLimits, DiffStatus, JsonCellValue, WorkbookDiff}`（および `diff-storage` フィーチャー時は `exceldiff::DiffStore`）をクレートルートから直接 `use` して呼び出せていることで間接的に検証される。

## 未決事項 / オープンクエスチョン

1. ~~行/列挿入検出（2D LCSアライメント）モードの追加場所~~ → **列・行とも解決**: 列は[Issue #5](https://github.com/MinamiyamaKotaro/exceldiff/issues/5)で`diff::col_alignment`という新規サブモジュールに分離し、`diff_workbooks_aligned_columns`/`ColumnAlignmentLimits`を公開した（[col_alignment.md](col_alignment.md)参照）。行は[Issue #4](https://github.com/MinamiyamaKotaro/exceldiff/issues/4)で`diff::row_alignment`という新規サブモジュールに分離し、`diff_workbooks_aligned_rows`/`RowAlignmentLimits`を公開した（[row_alignment.md](row_alignment.md)参照）。両者を**同一シート上で同時に**使う(行と列の挿入が同じシートで同時に起きる)組み合わせは依然として未統合——[col_alignment.md 未決事項1](col_alignment.md)/[row_alignment.md 未決事項1](row_alignment.md)参照。[engine.md 未決事項](engine.md)も参照。
2. ~~呼び出し側がどの方式(座標一致/行アライメント/列アライメント)を選ぶべきかの判断~~ → **解決**: [Issue #25](https://github.com/MinamiyamaKotaro/exceldiff/issues/25)で`diff::best_effort::diff_workbooks_best_effort`を新設し、シートごとに3方式を評価して最もノイズの少ない結果を採用する形にした([best_effort.md](best_effort.md)参照)。ただしこれは項目1の「同一シートで行・列両方が同時にずれる」ケースを解決するものではない——シート単位でどちらか一方の軸を選ぶだけであり、両軸を組み合わせた新しいアライメントアルゴリズムではない点に注意。
3. **`DiffStore` 以外のストレージバックエンドの要否**: 現状SQLite（`rusqlite`）のみをサポートする。[Issue #3](https://github.com/MinamiyamaKotaro/exceldiff/issues/3) の検討事項1「`rusqlite` を本体のデフォルト依存にするか、Cargo featureとしてオプショナルにするか」は「オプショナルにする」で解決したが、他のストレージ（例: JSON Lines ファイルへの追記）を求める要望が生じた場合、`diff::storage` をトレイト抽象化するかは未決定。
