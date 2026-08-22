# `diff/model.rs` 設計書

*[English](model.en.md)*

`src/diff/model.rs` に対応する設計書。[`diff/engine.rs`](engine.md) が生成し、[`diff/storage.rs`](storage.md) が永続化する差分結果の出力形（`WorkbookDiff`/`SheetDiff`/`CellDiff`/`DiffStatus`）を定義する（[Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3)）。[`json.rs`](../json.md) が `model::Workbook` の完全なスナップショットをJSON化するのに対し、本ファイルはその**差分**をJSON化可能な形で表現する、いわば「json.rsの差分版」に相当する。

## 責務・スコープ

- 差分結果の型を定義する: セル1件の変更を表す `CellDiff`、シート1枚の変更（可視性変更・セル変更の集合）を表す `SheetDiff`、ワークブック全体の差分を表す `WorkbookDiff`、変更種別を表す `DiffStatus`（`Added`/`Modified`/`Deleted`）
- `CellDiff::old_value`/`new_value` の型として、[`json.rs`](../json.md) の `JsonCellValue`（本ファイルの都合で `pub` へ変更——依存関係セクション参照）をそのまま再利用する。独自の値表現を新設しない
- `serde::Serialize` を各型へ導出し、`WorkbookDiff` がそのままJSONへシリアライズ可能であることを保証する（[`diff/storage.rs`](storage.md) が `CellDiff::old_value`/`new_value` を個別に `serde_json::to_string` する際にもこの導出を利用する）
- 「報告すべき情報が無ければフィールド自体を省略する」という [json.rs](../json.md) 既存の疎な出力方針（`JsonCell::style` 等）を踏襲し、`CellDiff::old_value`/`new_value`（`Added`では`old_value`が、`Deleted`では`new_value`が存在しない）と `SheetDiff::old_visibility`/`new_visibility`（可視性に変更が無ければ両方とも省略）に `#[serde(skip_serializing_if = "Option::is_none")]` を適用する
- **含まない責務**: 差分の計算ロジックそのもの（これらの型を実際にどう構築するかは[`diff/engine.rs`](engine.md)の責務）、SQLiteへの永続化（[`diff/storage.rs`](storage.md)）

## 主要な型・関数（案）

```rust
use crate::JsonCellValue;
use serde::Serialize;

/// セル単位の変更種別。
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DiffStatus {
    Added,
    Modified,
    Deleted,
}

/// 変更されたセル1件。`row`/`col` は両リビジョンで共通の座標——行/列挿入
/// アライメントを行わないデフォルトのエンジン（`diff::engine::diff_workbooks`）
/// はセルが座標を移動したと報告することが無い(詳細は同関数のdocコメント
/// 参照)ため、旧/新座標のペアを別々に保持する必要が無い。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CellDiff {
    pub row: u32,
    pub col: u32,
    pub status: DiffStatus,
    /// `Modified`/`Deleted` では存在し、`Added` では存在しない。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_value: Option<JsonCellValue>,
    /// `Modified`/`Added` では存在し、`Deleted` では存在しない。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_value: Option<JsonCellValue>,
}

/// 1シート分の変更。実際に変更があったシートについてのみ構築される——
/// `diff::engine::diff_workbooks` は両側に同名シートが存在し、可視性が
/// 同一かつセル差分が0件の場合、そのシートを `WorkbookDiff::sheets` へ
/// 一切含めない（`json.rs` が `JsonCell::style` 等に既に適用している
/// 「報告すべき情報が無ければ何も出力しない」という規約と同一）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SheetDiff {
    pub name: String,
    pub status: DiffStatus,
    /// 変更前にシートが存在した場合（`Modified`/`Deleted`）のみ存在し、
    /// `Modified` の場合はさらに `new_visibility` と実際に異なる場合のみ
    /// 存在する。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_visibility: Option<&'static str>,
    /// 変更後にシートが存在する場合（`Modified`/`Added`）のみ存在し、
    /// `Modified` の場合はさらに `old_visibility` と実際に異なる場合のみ
    /// 存在する。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_visibility: Option<&'static str>,
    pub cells: Vec<CellDiff>,
}

/// 2つのワークブック間の差分全体——`diff::engine::diff_workbooks`/
/// `diff_paths` の返り値の型。
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct WorkbookDiff {
    pub sheets: Vec<SheetDiff>,
}
```

## 依存関係

- 依存先: [`json.rs`](../json.md)（`JsonCellValue`）。`JsonCellValue` はこのファイルからの再利用のために、[json.mdの主要な型](../json.md) が本来 `to_json_writer`/`to_json_string` 実装専用の内部詳細として非公開のままにしていたところを、`pub` へ変更した（[json.rs 未決事項](../json.md)にはこの変更は反映されていない——json.rs自体の設計意図を変えるものではなく、本ファイルからの再利用要求により事後的に可視性のみを緩和したもの）。外部クレート `serde`（`Serialize` の導出）に依存する。
- 依存元: [`diff/engine.rs`](engine.md)（各型を構築して返す）、[`diff/storage.rs`](storage.md)（`CellDiff::old_value`/`new_value`・`DiffStatus` をSQLへ変換する際に参照する）、[`lib.rs`](../lib.md)（`diff::model::{CellDiff, DiffStatus, SheetDiff, WorkbookDiff}` を[`diff/mod.rs`](mod.md)経由でクレートルートへ再エクスポートする）

`JsonCellValue` を独自に複製せず再利用する設計は、[Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3) のPoC（`poc/issue3-poc`）が独自の `JsonValue` enumを新設していた点からの意図的な変更である。同一のセル値が `to_json_string`（完全スナップショット）経由でも `diff_workbooks`（差分）経由でも同じ形でシリアライズされることを型レベルで保証し、2つの独立した値表現が将来ズレていく（例えば `DateTime` のフォーマットが片方だけ変更される）リスクを構造的に排除する。

`CellDiff`/`SheetDiff` に `old_row`/`old_col`（座標が移動した場合の旧座標）を持たせなかったのは、現状のデフォルトエンジンがそもそも座標移動を検出しない設計であるため（[engine.md](engine.md) 参照）——使われない見込みのフィールドを先回りして追加しない、という判断（Issue #4/#5のアライメントモードが実装される際に必要になれば、その時点で追加する）。

## エラー処理方針

- 本ファイルはデータ型の定義のみであり、エラーを生成する処理を持たない。

## テスト方針

- 本ファイル単体の直接テストは持たない(型定義とderiveのみのため)。各型が期待通り構築・シリアライズされることは、[`diff/engine.rs`](engine.md) と [`diff/storage.rs`](storage.md) それぞれの単体テスト、および[`tests/diff.rs`](../../../tests/diff.rs) の実パースパイプライン経由の統合テストで間接的に検証する。

## 未決事項 / オープンクエスチョン

1. **行/列挿入アライメントモード導入時の型拡張**: [Issue #4](https://github.com/MinamiyamaKotaro/xlsxparser/issues/4)/[Issue #5](https://github.com/MinamiyamaKotaro/xlsxparser/issues/5) が要求するアライメントベースの差分を実装する場合、`CellDiff` に `old_row`/`old_col` を追加する（既存の `row`/`col` は新座標を表すよう再解釈する）か、別の型（例 `AlignedCellDiff`）を新設するかは未決定。前者は既存フィールドの意味が「デフォルトエンジンでは常に同一座標」から「アライメントエンジンでは移動しうる」へ変わるため、後方互換性への影響を実装時に評価する必要がある。
2. **スタイル・数式・列幅・画像の差分**: [Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3) の検討事項3が挙げる「セル値に加えて、スタイル・数式・結合セル・画像・列幅の差分を含める範囲」は未着手。現状 `CellDiff` はセル値のみを `old_value`/`new_value` として保持し、[`diff/engine.rs`](engine.md) はスタイルの変更を検知して `Modified` フラグは立てるものの、何がどう変わったか（フォントサイズが変わったのか塗り色が変わったのか等）はJSON上に表現されない。フロントエンド側が実際に必要とする粒度が判明した時点で `CellDiff` へフィールドを追加するか検討する。
