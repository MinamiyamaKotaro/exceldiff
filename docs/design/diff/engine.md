# `diff/engine.rs` 設計書

*[English](engine.en.md)*

`src/diff/engine.rs` に対応する設計書。[`diff/model.rs`](model.md) が定義する `WorkbookDiff` を、2つの `model::Workbook` から実際に計算するロジックを担う（[Issue #3](https://github.com/MinamiyamaKotaro/xlsxparser/issues/3)、スタイル・セル結合差分は[Issue #8](https://github.com/MinamiyamaKotaro/xlsxparser/issues/8)）。

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

## スタイル差分の疎さ（Issue #8）

`CellDiff::old_style`/`new_style` は、スタイルが実際に異なる場合のみ出力する。これは `old_value`/`new_value`（`Modified` なら値自体が同一でも常に両方出力する——スタイルのみの変更でも `old_value == new_value` になる、既存のテスト済み仕様）とは非対称な、意図的により疎な規約である。値の変更は `CellDiff` が存在する理由そのものであるため両方見せてもコストが無い一方、スタイルは大半の `Modified` セルが触れない副次的な次元であり、常に一対で添付すると通常ケースが無駄に肥大化する。既存の `old_value`/`new_value` の規約をこの疎い方式に遡って揃えることは、既に出荷・テスト済みの挙動を黙って変えてしまうため見送った（Issue #8 PRレビュー議論）。

## セル結合差分はシート単位（Issue #8）

`diff_merges` は結合の変更を起点セルの `CellDiff` へ折り込まず、`SheetDiff::merges` というシート単位のリストとして報告する。完全スナップショットのJSON（`json.rs`）が結合を起点セルの `rowSpan`/`colSpan` として埋め込むのとは意図的に異なる表現である。diffの役割は離散的な変更を報告することであり、値もスタイルも変わっていない `Added`/`Deleted` の結合には、そもそも対応する `CellDiff` が自然には存在しない（空の `CellDiff` を結合のためだけに合成するのは別の不自然さを生む）。`images`/`columns` を `json.rs` がシート単位配列として扱っているのと同じ「1つのセルに自然に属さない」という理由により、シート単位のリストを採用した（Issue #8 PRレビュー議論）。

## 責務・スコープ

- ファイルパスから直接差分を計算する `diff_paths`（内部で [`parse_workbook`](../lib.md) を2回呼び、`diff_workbooks` へ委譲する）
- 既にパース済みの2つの `Workbook` を比較する `diff_workbooks`（公開APIの中核）
- シート名の和集合を走査し、片側にのみ存在するシート（`Added`/`Deleted`）、両側に存在するシート（`Modified`——可視性変更やセル/結合差分の有無を判定）をそれぞれ処理する
- 1シート内のセル差分を、`Sheet::iter_cells` が返す `CellRef` 昇順（行→列）のイテレータを2本同時に前進させる「マージジョイン」方式で計算する（`diff_cells`）。座標を比較し、一致すれば値・スタイルを比較して `Modified` の要否を判定、片側にしか無い座標はそのまま `Added`/`Deleted` とする
- 1シート内の結合差分を、`Sheet::merged_regions()`（Issue #8で新設した `pub(crate)` アクセサ）が返す `HashMap<CellRef, MergedRegion>` を起点座標でルックアップ・比較することで計算する（`diff_merges`）。詳細度・計算量は下記参照
- **含まない責務**: 差分結果の型定義そのもの（[`diff/model.rs`](model.md)）、SQLiteへの永続化（[`diff/storage.rs`](storage.md)。スタイル・結合差分も[Issue #9](https://github.com/MinamiyamaKotaro/xlsxparser/issues/9)で永続化された）、行/列挿入を検出するアライメントベースの差分（未決事項1、Issue #4/#5参照）

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

結合差分は以下の形（骨子）で計算する:

```rust
fn diff_merges(base: &Sheet, target: &Sheet) -> Vec<MergeDiff> {
    let base_merges = base.merged_regions();   // &HashMap<CellRef, MergedRegion>
    let target_merges = target.merged_regions();

    let mut out = Vec::new();
    for (&origin, base_region) in base_merges {
        match target_merges.get(&origin) {
            Some(target_region) if target_region.end != base_region.end => {
                out.push(MergeDiff { status: Modified, start: origin.into(), .. });
            }
            Some(_) => {} // 変化なし
            None => out.push(MergeDiff { status: Deleted, .. }),
        }
    }
    for (&origin, target_region) in target_merges {
        if !base_merges.contains_key(&origin) {
            out.push(MergeDiff { status: Added, .. });
        }
    }

    // merged_regions は HashMap で順序を持たないため、実際に差分が
    // あった件数分だけ最後にソートする（全結合をソートするより安い）。
    out.sort_by_key(|m| (m.start.row, m.start.col));
    out
}
```

（`diff_sheet`/`diff_cells`/`diff_merges`/`cell_diff_added`/`cell_diff_deleted`/`cell_diff_modified` の完全な実装は `src/diff/engine.rs` を参照。本ドキュメントでは骨子のみ示す。）

## 依存関係

- 依存先: [`diff/model.rs`](model.md)（`CellDiff`, `DiffStatus`, `MergeDiff`, `SheetDiff`, `WorkbookDiff`）、[`json.rs`](../json.md)（`cell_value_to_json`, `style_to_json`, `visibility_tag`——いずれも本ファイルからの再利用のために `pub(crate)` へ変更）、[`model/sheet.rs`](../model/sheet.md)（`Sheet::iter_cells`, `Sheet::merged_regions`——Issue #8で新設）、[`model/workbook.rs`](../model/workbook.md)（`Workbook::sheets`, `Workbook::sheet`）、[`lib.rs`](../lib.md)（`parse_workbook`——`diff_paths` から）、[`error.rs`](../error.md)（`Result`）
- 依存元: [`diff/mod.rs`](mod.md)（`diff_paths`/`diff_workbooks` を再エクスポート）、[`diff/storage.rs`](storage.md)の単体テスト（`diff_workbooks` を呼んで `WorkbookDiff` を得た上で `save_diff` へ渡す）

`json.rs` の `cell_value_to_json`/`style_to_json`/`visibility_tag` を再利用しているのは、[model.md 依存関係](model.md) が述べる `JsonCellValue`/`JsonStyle` 再利用と同じ理由——完全スナップショット（`to_json_string`）と差分（`diff_workbooks`）で、同じセル値・同じスタイル・同じ可視性が常に同じ表現になることを型・実装レベルで保証するため。

`Sheet::merged_regions()`（新設）を使うのは、外部クレートという制約下にあった `poc/issue8-poc` の `iter_cells` ベースの復元（O(セル数)）を、クレート内部実装ではO(結合数)へ最適化するため——実測でセル数300,000・結合数10件のケースで前者が約4.3msかかることを確認しており（下記パフォーマンス参照）、`diff_cells` が既に1回セル全体を走査済みであることを踏まえると、結合検出のためだけにもう1回フル走査するのは無駄である。

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
- スタイルのみの変更（値は不変）が `Modified` として検出され、`new_style` に変更後のスタイルが（`old_style` は元がスタイル無しなら `None` のまま）正しく含まれること（`style_only_change_is_reported_as_modified_with_new_style_populated`）
- 値のみの変更（スタイルは不変）では `old_style`/`new_style` が両方とも `None` のままであること（`value_only_change_carries_no_style_diff`）——上記との非対称性の直接的な回帰テスト
- スタイル付きセルの追加/削除で `new_style`/`old_style` のみが（もう一方は`None`のまま）設定されること（`added_cell_with_a_style_reports_new_style_only`）
- 結合の追加・削除・範囲変更・変更なしがそれぞれ `Added`/`Deleted`/`Modified`/（差分なし）として検出されること（`merge_added_is_detected_even_with_no_cell_changes`、`merge_deleted_is_detected`、`merge_extent_change_is_reported_as_modified`、`unchanged_merge_produces_no_diff_at_all`）——値・スタイルの変更が一切無い「結合のみの変更」でも `SheetDiff` が報告されることを含む
- シート自体の追加/削除時、そのシートの結合も全て `Added`/`Deleted` として報告されること（`sheet_added_reports_its_merges_as_added_too`、`sheet_deleted_reports_its_merges_as_deleted_too`）
- **本アルゴリズムの意図したトレードオフの回帰テスト**: 1行挿入により以降の行が全てシフトするケースで、シフトしただけの行も含め全セルがカスケードして差分化されることを明示的に確認する（`row_insertion_cascades_into_shift_diffs_by_design`）——これは「バグ」ではなく上記「アルゴリズム選定の経緯」で述べた設計上のトレードオフそのものであることを、テスト名と共にドキュメント化する

[`tests/diff.rs`](../../../tests/diff.rs)（[tests/fixtures/diff.rs](../../../tests/fixtures/diff.rs) 経由の統合テスト、実際のZIP/XMLパイプラインを通す）:

- 上記シナリオ（セル変更・追加・削除、シート追加・削除、可視性変更、スタイルのみ変更、結合のみの追加、完全一致）を、実際に `.xlsx` 相当のバイト列を組み立てて `parse_workbook_reader` でパースした上で再検証（`style_only_change_is_reported_as_modified_end_to_end` はフォントサイズ・太字の具体的な新旧値まで検証する）
- `diff_paths` が一時ファイル経由で正しく動作することの確認

パフォーマンス（実測、release build。コミット対象のテストコードではなく、検証時の一時ベンチマークによる）:

| セル数 | 変更率 | 実行時間 |
|---:|---:|---:|
| 100,000 | 0.1% | 1.8ms |
| 800,000 | 0.1% | 25.8ms |
| 4,000,000 | 0.1% | 105ms |

セル数に対しほぼ線形（O(n)）にスケールすることを確認済み。ワーストケース（1行挿入によるシート全体カスケード、50万セル）でも約19ms で完走し、「計算自体が破綻する」ことはなく「diff結果の件数が多くなる」だけであることを確認した（PoCのO(n²)実装との対比）。

`diff_merges`（Issue #8）の計算量は `poc/issue8-poc` で以下の通り実測した:

| 結合数 | 実行時間（`diff_merges` 相当） |
|---:|---:|
| 1,000 | 128〜143µs |
| 5,000 | 420〜476µs |
| 10,000 | 702〜766µs |
| 20,000（`MAX_MERGE_REGIONS`上限） | 1.46〜1.51ms |

明確な線形（O(結合数)）挙動で、上限件数でも1.5ms程度。さらに「セル数は多いが結合はごく少数」という本クレートが想定するシナリオで、PoCのO(セル数)実装（クレート外部という制約による代替）のコストを直接計測し、`Sheet::merged_regions()` による直接アクセスへの最適化が実際に有効であることを確認した:

| セル数 | 結合数 | O(セル数)実装のコスト |
|---:|---:|---:|
| 10,000 | 10 | 145µs |
| 100,000 | 10 | 1.30ms |
| 300,000 | 10 | 4.32ms |

## 未決事項 / オープンクエスチョン

1. **行/列挿入アライメントモードの実装場所・API形状**: [Issue #4](https://github.com/MinamiyamaKotaro/xlsxparser/issues/4)（行）・[Issue #5](https://github.com/MinamiyamaKotaro/xlsxparser/issues/5)（列）が要求する上限付きオプトインのアライメントベース差分を、本ファイルへ関数追加（例: `diff_workbooks_aligned(base, target, limits) -> Result<WorkbookDiff, Error>`）する形にするか、独立したサブモジュール（`diff::alignment`）に分離するかは未決定。後者を選ぶ場合、[diff/mod.md 未決事項1](mod.md)と連動する。
2. **`similarity_score` ヒューリスティックの頑健性**: PoCの列アライメントが採用していた「1セルでも値が一致すれば候補」という緩い一致判定は、疎/重複値の多いシートで誤マッチする懸念がある（[Issue #5 検討事項](https://github.com/MinamiyamaKotaro/xlsxparser/issues/5)参照）。アライメントモードを実装する際、より頑健な列/行シグネチャ（例: 複数セルのハッシュ組み合わせ）に置き換えるかは実装時に検討する。
3. ~~スタイル差分の詳細化~~ → **解決**（[Issue #8](https://github.com/MinamiyamaKotaro/xlsxparser/issues/8)）: `CellDiff::old_style`/`new_style` を追加し、fill色・フォント・罫線・配置・書式の新旧を出力するようにした（本ファイル「スタイル差分の疎さ」節参照）。
4. **シート順序変更の扱い**: `diff_workbooks` はシートを名前（`BTreeSet<&str>` によるソート順）で対応付けており、`workbook.xml` の `<sheets>` 定義順が入れ替わった場合でも「シート順序が変わった」という差分は一切報告しない（同名シートの中身が同一なら無視される）。ワークブックレベルでシート順序の変更を追跡する要求が生じた場合、`WorkbookDiff` へ別途フィールドを追加するかは未決定。
5. **`MergeDiff` の上限**: `diff_merges` 自体には `resolve::merge::MAX_MERGE_REGIONS` のような専用の上限チェックは無い——ただし結合登録自体が `resolve::merge::resolve` の時点で既にその上限（20,000件）で制限されているため、diff計算時点で追加の上限を設ける必要は無いと判断した（実測上も20,000件で1.5ms程度）。将来的に上限自体が緩和された場合は再検討が必要。
6. **非表示（hidden/veryHidden）シートを差分対象から除外するオプションの要否**（[Issue #16](https://github.com/MinamiyamaKotaro/exceldiff/issues/16)）: `diff_workbooks`/`diff_sheet` は現状 `SheetVisibility` によるフィルタを一切行わず、`Hidden`/`VeryHidden` のシートも `Visible` のシートと全く同じにセル差分・結合差分の対象にする（`hidden_and_very_hidden_sheets_are_all_included`（`src/pipeline.rs`）が示す通り、パース段階でも非表示シートは除外されない）。これは「非表示シートだから除外する」という要求が現時点で無いことによる、意図的な現状維持の判断——将来「非表示シートは差分ノイズになるので除外したい」あるいは逆に「非表示シートにこそ意図的に隠された変更が多い」といった要求が具体化した場合、オプトインのフィルタ（例: `veryHidden`/`hidden`を個別に制御可能な引数）を追加するかを検討する。`hidden_sheet_cell_changes_are_diffed_just_like_visible_ones`（`src/diff/engine.rs`）がこの「常時対象」という現在の契約を固定するリグレッションテストとして機能する。
