# `diff/engine.rs` 設計書

*[English](engine.en.md)*

`src/diff/engine.rs` に対応する設計書。[`diff/model.rs`](model.md) が定義する `WorkbookDiff` を、2つの `model::Workbook` から実際に計算するロジックを担う（[Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3)）。

## アルゴリズム選定の経緯

[Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3) のPoC（`poc/issue3-poc`）は、行/列の挿入・削除によるセル座標のシフトを検出する「2D LCSアライメント」（列のLCS→行のLCSの2段階）を実装していた。これを実際にビルド・実行して機能面での正しさを検証した結果、小規模サンプルでは主張通り正しく動作することを確認した（[Issue #3コメント](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3#issuecomment-5382524419)、`poc/issue3-poc/output/verification_report.md`）一方、`align_columns`/`align_rows_2d` を直接ベンチマークしたところ、明確な **O(distinct_rows² + distinct_cols²)** の時間・メモリ挙動を実測した:

| 対象 | 入力サイズ | 実行時間 |
|---|---|---|
| align_rows_2d | 500行 × 50列一致 | 200ms |
| align_rows_2d | 1,000行 × 50列一致 | 787ms（×3.9） |
| align_rows_2d | 2,000行 × 50列一致 | 3.16s（×4.0） |
| align_rows_2d | 4,000行 × 50列一致 | 12.98s（×4.1） |

サイズを倍にする度に実行時間が約4倍（=2²）で増加しており、dpテーブル自体のメモリも `(R+1)×(R+1)` の `usize` で行数の2乗に比例する（4,000行で約128MB、100,000行では約80GB相当と試算）。これは `lib.rs` が明言する「行・列数が極端に多い方眼紙Excelに最適化」という本クレートの設計目標と正面から矛盾し、また `resolve::merge::MAX_MERGE_REGIONS`/`resolve::column_width::MAX_COLUMN_WIDTH_RANGES`（[resolve/merge.md](../resolve/merge.md)/[resolve/column_width.md](../resolve/column_width.md)）が「O(N²)になりうる構造には上限を設けてfail-fastする」と既に確立している防御パターンとも整合しない。

このため、本ファイルはPoCの2D LCSアライメントをそのまま移植せず、**座標一致ベースの軽量差分をデフォルトとして採用**した。行/列挿入検出（アライメントベースの差分）は上限付きオプトイン機能として別issue（[#4](https://github.com/MinamiyamaKotaro/xlsxparser/issues/4) 行、[#5](https://github.com/MinamiyamaKotaro/xlsxparser/issues/5) 列）で管理し、本ファイルのスコープには含めない。

## 責務・スコープ

- ファイルパスから直接差分を計算する `diff_paths`（内部で [`parse_workbook`](../lib.md) を2回呼び、`diff_workbooks` へ委譲する）
- 既にパース済みの2つの `Workbook` を比較する `diff_workbooks`（公開APIの中核）
- シート名の和集合を走査し、片側にのみ存在するシート（`Added`/`Deleted`）、両側に存在するシート（`Modified`——可視性変更やセル差分の有無を判定）をそれぞれ処理する
- 1シート内のセル差分を、`Sheet::iter_cells` が返す `CellRef` 昇順（行→列）のイテレータを2本同時に前進させる「マージジョイン」方式で計算する（`diff_cells`）。座標を比較し、一致すれば値・スタイルを比較して `Modified` の要否を判定、片側にしか無い座標はそのまま `Added`/`Deleted` とする
- **含まない責務**: 差分結果の型定義そのもの（[`diff/model.rs`](model.md)）、SQLiteへの永続化（[`diff/storage.rs`](storage.md)）、行/列挿入を検出するアライメントベースの差分（未決事項1、Issue #4/#5参照）

## 主要な型・関数（案）

```rust
use crate::diff::model::{CellDiff, DiffStatus, SheetDiff, WorkbookDiff};
use crate::error::Result;
use crate::json::{cell_value_to_json, visibility_tag};
use crate::model::{Cell, CellRef, Sheet, Workbook};
use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::path::Path;

/// `base_path`/`target_path` をパースして差分を計算する利便性関数。
pub fn diff_paths(
    base_path: impl AsRef<Path>,
    target_path: impl AsRef<Path>,
) -> Result<WorkbookDiff> {
    let base = crate::parse_workbook(base_path)?;
    let target = crate::parse_workbook(target_path)?;
    Ok(diff_workbooks(&base, &target))
}

/// 2つの `Workbook` をシート名で対応付けて差分計算する中核関数。
pub fn diff_workbooks(base: &Workbook, target: &Workbook) -> WorkbookDiff {
    let mut sheet_names: BTreeSet<&str> = BTreeSet::new();
    sheet_names.extend(base.sheets().iter().map(|s| s.name.as_str()));
    sheet_names.extend(target.sheets().iter().map(|s| s.name.as_str()));

    let sheets = sheet_names
        .into_iter()
        .filter_map(|name| diff_sheet(name, base.sheet(name), target.sheet(name)))
        .collect();

    WorkbookDiff { sheets }
}

/// 1シート分の差分を計算する。両側に存在し可視性も同一かつセル差分が
/// 0件の場合は `None`（何も報告しない）。
fn diff_sheet(name: &str, base: Option<&Sheet>, target: Option<&Sheet>) -> Option<SheetDiff> {
    // (None, Some), (Some, None), (Some, Some), (None, None) の4パターンを
    // 処理する。詳細は src/diff/engine.rs 参照。
    todo!()
}

/// `base`/`target` のセルを1回ずつ線形走査するマージジョイン。
/// `Sheet::iter_cells` が既に `CellRef` 昇順(行優先)であることを前提とする。
/// O(base_cells + target_cells)、出力以外の追加メモリはO(1)。
fn diff_cells(base: &Sheet, target: &Sheet) -> Vec<CellDiff> {
    let mut out = Vec::new();
    let mut b = base.iter_cells().peekable();
    let mut t = target.iter_cells().peekable();

    loop {
        match (b.peek(), t.peek()) {
            (Some(&(br, bc)), Some(&(tr, tc))) => match br.cmp(&tr) {
                Ordering::Less => { /* base側のみ -> Deleted */ }
                Ordering::Greater => { /* target側のみ -> Added */ }
                Ordering::Equal => { /* 値・スタイル比較 -> Modified or 変化なし */ }
            },
            (Some(&(br, bc)), None) => { /* Deleted */ }
            (None, Some(&(tr, tc))) => { /* Added */ }
            (None, None) => break,
        }
    }
    out
}
```

（`diff_sheet`/`diff_cells` の完全な実装は `src/diff/engine.rs` を参照。本ドキュメントでは骨子のみ示す。）

## 依存関係

- 依存先: [`diff/model.rs`](model.md)（`CellDiff`, `DiffStatus`, `SheetDiff`, `WorkbookDiff`）、[`json.rs`](../json.md)（`cell_value_to_json`, `visibility_tag`——いずれも本ファイルからの再利用のために `pub(crate)` へ変更）、[`model/sheet.rs`](../model/sheet.md)（`Sheet::iter_cells`）、[`model/workbook.rs`](../model/workbook.md)（`Workbook::sheets`, `Workbook::sheet`）、[`lib.rs`](../lib.md)（`parse_workbook`——`diff_paths` から）、[`error.rs`](../error.md)（`Result`）
- 依存元: [`diff/mod.rs`](mod.md)（`diff_paths`/`diff_workbooks` を再エクスポート）、[`diff/storage.rs`](storage.md)の単体テスト（`diff_workbooks` を呼んで `WorkbookDiff` を得た上で `save_diff` へ渡す）

`json.rs` の `cell_value_to_json`/`visibility_tag` を再利用しているのは、[model.md 依存関係](model.md) が述べる `JsonCellValue` 再利用と同じ理由——完全スナップショット（`to_json_string`）と差分（`diff_workbooks`）で、同じセル値・同じ可視性が常に同じ文字列表現になることを型・実装レベルで保証するため。

## エラー処理方針

- `diff_workbooks` はエラーを返さない（`Result` を返さない）——2つの `Workbook` は既にパース済みで、比較処理自体に失敗しうる外部要因（I/O・不正なXML等）が存在しないため。
- `diff_paths` は内部の `parse_workbook` 呼び出し2回分のエラーをそのまま `?` で伝播する。`base_path`/`target_path` いずれのパースが失敗しても、[pipeline.md エラー処理方針](../pipeline.md) が定義する対応する `Error` バリアントがそのまま呼び出し元へ返る。

## テスト方針

`src/diff/engine.rs` 内の単体テスト（`Sheet`/`Workbook` を公開モデルAPI経由で直接構築、ZIP/XMLには触れない）:

- 完全に一致する2つのワークブックが空の `WorkbookDiff`（`sheets: []`）を返すこと
- セル値変更が `Modified`（`old_value`/`new_value` 双方を伴う）として検出されること
- セルの追加/削除がそれぞれ `Added`/`Deleted` として検出されること
- シート自体の追加/削除が、そのシートの全セルを `Added`/`Deleted` として報告すること
- セル差分が無くても可視性変更のみで `SheetDiff` が報告されること（`cells: []` でも省略しない）
- スタイルのみの変更（値は不変）が `Modified` として検出されること
- **本アルゴリズムの意図したトレードオフの回帰テスト**: 1行挿入により以降の行が全てシフトするケースで、シフトしただけの行も含め全セルがカスケードして差分化されることを明示的に確認する（`row_insertion_cascades_into_shift_diffs_by_design`）——これは「バグ」ではなく上記「アルゴリズム選定の経緯」で述べた設計上のトレードオフそのものであることを、テスト名と共にドキュメント化する

[`tests/diff.rs`](../../../tests/diff.rs)（[tests/fixtures/diff.rs](../../../tests/fixtures/diff.rs) 経由の統合テスト、実際のZIP/XMLパイプラインを通す）:

- 上記シナリオ（セル変更・追加・削除、シート追加・削除、可視性変更、スタイルのみ変更、完全一致）を、実際に `.xlsx` 相当のバイト列を組み立てて `parse_workbook_reader` でパースした上で再検証
- `diff_paths` が一時ファイル経由で正しく動作することの確認

パフォーマンス（実測、release build。コミット対象のテストコードではなく、検証時の一時ベンチマークによる）:

| セル数 | 変更率 | 実行時間 |
|---:|---:|---:|
| 100,000 | 0.1% | 1.8ms |
| 800,000 | 0.1% | 25.8ms |
| 4,000,000 | 0.1% | 105ms |

セル数に対しほぼ線形（O(n)）にスケールすることを確認済み。ワーストケース（1行挿入によるシート全体カスケード、50万セル）でも約19ms で完走し、「計算自体が破綻する」ことはなく「diff結果の件数が多くなる」だけであることを確認した（PoCのO(n²)実装との対比）。

## 未決事項 / オープンクエスチョン

1. **行/列挿入アライメントモードの実装場所・API形状**: [Issue #4](https://github.com/MinamiyamaKotaro/xlsxparser/issues/4)（行）・[Issue #5](https://github.com/MinamiyamaKotaro/xlsxparser/issues/5)（列）が要求する上限付きオプトインのアライメントベース差分を、本ファイルへ関数追加（例: `diff_workbooks_aligned(base, target, limits) -> Result<WorkbookDiff, Error>`）する形にするか、独立したサブモジュール（`diff::alignment`）に分離するかは未決定。後者を選ぶ場合、[diff/mod.md 未決事項1](mod.md)と連動する。
2. **`similarity_score` ヒューリスティックの頑健性**: PoCの列アライメントが採用していた「1セルでも値が一致すれば候補」という緩い一致判定は、疎/重複値の多いシートで誤マッチする懸念がある（[Issue #5 検討事項](https://github.com/MinamiyamaKotaro/xlsxparser/issues/5)参照）。アライメントモードを実装する際、より頑健な列/行シグネチャ（例: 複数セルのハッシュ組み合わせ）に置き換えるかは実装時に検討する。
3. **スタイル差分の詳細化**: `diff_cells` は `bc.style != tc.style` の真偽のみで `Modified` を判定し、何が変わったか（フォント/塗り色/罫線等）はJSON上に表現しない。[model.md 未決事項2](model.md)と同一の論点。
4. **シート順序変更の扱い**: `diff_workbooks` はシートを名前（`BTreeSet<&str>` によるソート順）で対応付けており、`workbook.xml` の `<sheets>` 定義順が入れ替わった場合でも「シート順序が変わった」という差分は一切報告しない（同名シートの中身が同一なら無視される）。ワークブックレベルでシート順序の変更を追跡する要求が生じた場合、`WorkbookDiff` へ別途フィールドを追加するかは未決定。
