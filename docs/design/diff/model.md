# `diff/model.rs` 設計書

*[English](model.en.md)*

`src/diff/model.rs` に対応する設計書。[`diff/engine.rs`](engine.md) が生成し、[`diff/storage.rs`](storage.md) が永続化する差分結果の出力形（`WorkbookDiff`/`SheetDiff`/`CellDiff`/`MergeDiff`/`CellPos`/`DiffStatus`）を定義する（[Issue #3](https://github.com/MinamiyamaKotaro/exceldiff/issues/3)、スタイル・セル結合差分は[Issue #8](https://github.com/MinamiyamaKotaro/exceldiff/issues/8)）。[`json.rs`](../json.md) が `model::Workbook` の完全なスナップショットをJSON化するのに対し、本ファイルはその**差分**をJSON化可能な形で表現する、いわば「json.rsの差分版」に相当する。

## 責務・スコープ

- 差分結果の型を定義する: セル1件の変更を表す `CellDiff`、結合セル1件の変更を表す `MergeDiff`、シート1枚の変更（可視性変更・セル変更・結合変更の集合）を表す `SheetDiff`、ワークブック全体の差分を表す `WorkbookDiff`、変更種別を表す `DiffStatus`（`Added`/`Modified`/`Deleted`）、座標を表す `CellPos`
- `CellDiff::old_value`/`new_value` の型として [`json.rs`](../json.md) の `JsonCellValue` を、`CellDiff::old_style`/`new_style` の型として同じく `JsonStyle` を再利用する（いずれも本ファイルの都合で `pub` へ変更——依存関係セクション参照）。独自の値・スタイル表現を新設しない
- `serde::Serialize` を各型へ導出し、`WorkbookDiff` がそのままJSONへシリアライズ可能であることを保証する
- 「報告すべき情報が無ければフィールド自体を省略する」という [json.rs](../json.md) 既存の疎な出力方針を踏襲しつつ、`old_value`/`new_value` と `old_style`/`new_style` とで**意図的に粒度を変える**（詳細は`CellDiff`のdocコメントおよび[engine.md](engine.md)「スタイル差分の疎さ」参照）
- **含まない責務**: 差分の計算ロジックそのもの（これらの型を実際にどう構築するかは[`diff/engine.rs`](engine.md)の責務）、SQLiteへの永続化（[`diff/storage.rs`](storage.md)。`old_style`/`new_style`/`merges` も[Issue #9](https://github.com/MinamiyamaKotaro/exceldiff/issues/9)で永続化された）

## 主要な型・関数

```rust
use crate::json::JsonStyle;
use crate::model::CellRef;
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

/// 変更されたセル1件。`row`/`col` は両リビジョンで共通の座標。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CellDiff {
    pub row: u32,
    pub col: u32,
    pub status: DiffStatus,
    /// `Modified`/`Deleted` では存在し、`Added` では存在しない。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_value: Option<JsonCellValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_value: Option<JsonCellValue>,
    /// `Added`(スタイルを持つ場合)と、スタイルが実際に変わった`Modified`
    /// のみ存在する——`old_value`/`new_value`とは異なり「値が同じでも
    /// Modifiedなら常に両方出力」ではない、意図的により疎な規約
    /// （Issue #8。理由は`diff::engine`のdocコメント参照）。`Deleted`
    /// では存在しない。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_style: Option<JsonStyle>,
    /// `Deleted`(スタイルを持っていた場合)と、スタイルが実際に変わった
    /// `Modified`のみ存在する。`Added`では存在しない。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_style: Option<JsonStyle>,
}

/// `MergeDiff`が報告する座標。`model::CellRef`を直接使わないのは、
/// `model/`にserde依存を持ち込まない方針を守るため（json.rsの
/// `alignment_tag`等の変換関数群と同じ理由）。
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CellPos {
    pub row: u32,
    pub col: u32,
}

impl From<CellRef> for CellPos {
    fn from(r: CellRef) -> Self {
        CellPos { row: r.row, col: r.col }
    }
}

/// 変更された結合範囲1件。起点座標（`start`）でリビジョン間を対応付ける
/// ——`Modified`でも`old_start`/`new_start`のような別々の対を持たない
/// (常に同一の`start`になるため。詳細は`diff::engine::diff_merges`の
/// docコメント参照)。
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MergeDiff {
    pub status: DiffStatus,
    pub start: CellPos,
    /// `Modified`/`Deleted`で存在。`old_value`と同じ対称性。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_end: Option<CellPos>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_end: Option<CellPos>,
}

/// 1シート分の変更。実際に変更があったシートについてのみ構築される。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SheetDiff {
    pub name: String,
    pub status: DiffStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_visibility: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_visibility: Option<&'static str>,
    pub cells: Vec<CellDiff>,
    /// このシートの結合範囲の変更(Issue #8)。セル単位の`CellDiff`には
    /// 折り込まず、シート単位の配列として持つ——完全スナップショットの
    /// JSON（`json.rs`）が結合を起点セルの`rowSpan`/`colSpan`として
    /// 埋め込むのとは意図的に異なる表現（理由は`diff::engine`のdoc
    /// コメント参照）。変更が無ければ空（JSON上も省略）。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub merges: Vec<MergeDiff>,
}

/// 2つのワークブック間の差分全体。
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct WorkbookDiff {
    pub sheets: Vec<SheetDiff>,
}
```

## 依存関係

- 依存先: [`json.rs`](../json.md)（`JsonCellValue`、`JsonStyle`——いずれも`pub`化して再利用。`JsonStyle`が内部で持つ`JsonFont`/`JsonColorRef`/`JsonBorders`も同様に`pub`化し、かつ構造体の全フィールドを`pub`にした——`CellDiff`/`SheetDiff`同様、外部からフィールドを直接読める全公開データ型という設計方針にJsonStyle一族を揃えるため）、[`model/cell.rs`](../model/cell.md)（`CellRef`——`CellPos`への変換元）。外部クレート`serde`。
- 依存元: [`diff/engine.rs`](engine.md)（各型を構築して返す）、[`diff/storage.rs`](storage.md)（`CellDiff::old_value`/`new_value`/`old_style`/`new_style`・`DiffStatus`・`SheetDiff::merges`をSQLへ変換する際に参照——[Issue #9](https://github.com/MinamiyamaKotaro/exceldiff/issues/9)でスタイル・結合差分も参照対象に加わった）、[`lib.rs`](../lib.md)（`CellDiff`/`CellPos`/`DiffStatus`/`MergeDiff`/`SheetDiff`/`WorkbookDiff`を[`diff/mod.rs`](mod.md)経由でクレートルートへ再エクスポート）

`JsonCellValue`/`JsonStyle`を独自に複製せず再利用する設計は、同一のセル値・スタイルが`to_json_string`（完全スナップショット）経由でも`diff_workbooks`（差分）経由でも同じ形でシリアライズされることを型レベルで保証し、2つの独立した表現が将来ズレていくリスクを構造的に排除する（[Issue #3](https://github.com/MinamiyamaKotaro/exceldiff/issues/3)のPoCが独自の`JsonValue`型を新設していた点からの意図的な変更を、スタイルにも一貫して適用したもの）。

`CellDiff`に`old_row`/`old_col`を持たせず、`MergeDiff`にも`old_start`/`new_start`の別対を持たせなかったのは、現状のデフォルトエンジンが座標移動を検出しない設計であるため（[engine.md](engine.md)参照）。

## エラー処理方針

- 本ファイルはデータ型の定義のみであり、エラーを生成する処理を持たない。

## テスト方針

- 本ファイル単体の直接テストは持たない。各型が期待通り構築・シリアライズされることは、[`diff/engine.rs`](engine.md)の単体テスト（`style_only_change_is_reported_as_modified_with_new_style_populated`、`value_only_change_carries_no_style_diff`、`added_cell_with_a_style_reports_new_style_only`、`merge_added_is_detected_even_with_no_cell_changes`、`merge_deleted_is_detected`、`merge_extent_change_is_reported_as_modified`、`unchanged_merge_produces_no_diff_at_all`、`sheet_added_reports_its_merges_as_added_too`、`sheet_deleted_reports_its_merges_as_deleted_too`）、および[`tests/diff.rs`](../../../tests/diff.rs)の実パースパイプライン経由の統合テスト（`style_only_change_is_reported_as_modified_end_to_end`、`merge_addition_is_detected_even_with_no_cell_changes_end_to_end`）で間接的に検証する。

## 未決事項 / オープンクエスチョン

1. **行/列挿入アライメントモード導入時の型拡張**: [Issue #4](https://github.com/MinamiyamaKotaro/exceldiff/issues/4)/[Issue #5](https://github.com/MinamiyamaKotaro/exceldiff/issues/5)が要求するアライメントベースの差分を実装する場合、`CellDiff`/`MergeDiff`の座標フィールドをどう拡張するかは未決定（変更なし）。
2. ~~スタイル・結合セルの差分~~ → **部分的に解決**（[Issue #8](https://github.com/MinamiyamaKotaro/exceldiff/issues/8)）: `CellDiff::old_style`/`new_style`（fill色・フォント・罫線・配置・書式）と`SheetDiff::merges`を追加した。数式・列幅・画像の差分は依然未着手。
3. ~~SQLite永続化へのスタイル・結合差分の反映~~ → **解決**（[Issue #9](https://github.com/MinamiyamaKotaro/exceldiff/issues/9)）: `diff::storage::DiffStore::save_diff`が`old_style`/`new_style`を`diff_records`へ、`merges`を新設の`merge_diff_records`テーブルへ保存するようになった。詳細は[storage.md](storage.md)参照。
