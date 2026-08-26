# `diff/best_effort.rs` 設計書

*[English](best_effort.en.md)*

`src/diff/best_effort.rs` に対応する設計書。[`diff::engine::diff_workbooks`](engine.md)(座標一致)・[`diff::row_alignment::diff_workbooks_aligned_rows`](row_alignment.md)(Issue #4)・[`diff::col_alignment::diff_workbooks_aligned_columns`](col_alignment.md)(Issue #5)という3つの既存アルゴリズムの中から、呼び出し側がモードを事前に選ばなくても、シートごとに最もノイズの少ない結果を自動選択する([Issue #25](https://github.com/MinamiyamaKotaro/exceldiff/issues/25))。`.github/workflows/xlsx-diff.yml`が投稿する差分プレビューコメントは、変更されたファイルがどんな編集を受けたか(値の変更・行の挿入・列の挿入)を事前に知る術が無いため、この自動選択が必要だった。

## 背景・設計判断の経緯

Issue #25では4回のPoCラウンド([comment #5418963848](https://github.com/MinamiyamaKotaro/exceldiff/issues/25#issuecomment-5418963848) → [#5419091215](https://github.com/MinamiyamaKotaro/exceldiff/issues/25#issuecomment-5419091215) → [#5419182619](https://github.com/MinamiyamaKotaro/exceldiff/issues/25#issuecomment-5419182619) → [#5419237433](https://github.com/MinamiyamaKotaro/exceldiff/issues/25#issuecomment-5419237433))を経て、以下の設計に収束した:

1. **ワークブック単位ではなく、シート単位で方式を選ぶ**: ワークブック全体で1つの方式を選ぶ設計(初期案)は、「シート1は行挿入、シート2は列挿入」のように異なる編集が混在するワークブックで、片方のシートのカスケードが解消されないまま残ることが実証された([comment #5419091215](https://github.com/MinamiyamaKotaro/exceldiff/issues/25#issuecomment-5419091215))。
2. **座標一致の変更数が1件以下ならアライメントを試さない(ショートサーキット)**: 実データを持つ行/列の挿入・削除が実際に起きていれば、座標一致diffは新規セル分として最低1件は報告するため(それ未満にはなりようがない)、総数が1件以下の時点でアライメントによる改善余地は理論上ない。5,000行×20列のシートで約30倍の高速化を実測。
3. **行アライメントが`Ok(None)`(0件)に到達したら列アライメントを試さない**: 0はどんな非負の件数よりも小さいか等しいため、これ以上の改善余地は無い。当初「実データでは滅多に起きない」と評価していたが、**空行の挿入**(セルを一切持たない行の挿入。純粋かつ連続的な行シフトで、新規に報告すべき内容が無い)という現実的なケースで実際に到達することを確認した(本ファイルのテスト参照)。

## 責務・スコープ

- 2つの[`Workbook`](../model/workbook.md)を受け取り、両者に存在するシート名それぞれについて、[`diff::engine::diff_sheet`](engine.md)(座標一致)・[`diff::row_alignment::align_sheet_rows`](row_alignment.md)・[`diff::col_alignment::align_sheet_columns`](col_alignment.md)を評価し、最も変更数(`sheet_total_changes`——セル変更数+結合セル変更数+可視性変更の有無)が少ない`SheetDiff`を採用して`WorkbookDiff`へ合成する(`diff_workbooks_best_effort`)
- 座標一致の変更数が1件以下のシートは、両アライメント方式の呼び出し自体を省略する(ショートサーキット)
- 行アライメントが`Ok(None)`(0件)を返した場合、列アライメントの呼び出しを省略する(早期打ち切り)
- 行/列いずれかのアライメントがコスト超過(`Error::RowAlignmentCostTooHigh`/`Error::ColumnAlignmentCostTooHigh`)した場合、そのシートについてはそれ以外の候補(座標一致、または成功した方のアライメント)にフォールバックする——`Result`を返さず、常に`WorkbookDiff`を返す
- 片側のみに存在するシート(追加/削除)は、[`diff_sheet`](engine.md)に委譲する(アライメントの対象外——他の`diff_workbooks_*`関数と同じ扱い)
- **含まない責務**: 個々のアルゴリズムの実装そのもの(それぞれの担当ファイル)、行と列を同時にアライメントする新しい統合アルゴリズム(未決事項参照。既存3方式の中から選ぶだけで、既存方式に無い新しい解を作り出すものではない)、呼び出し側(CLI/`markdown.rs`)への結線([markdown.md](../markdown.md)参照)

## 主要な型・関数

```rust
pub fn diff_workbooks_best_effort(
    base: &Workbook,
    target: &Workbook,
    row_limits: RowAlignmentLimits,
    col_limits: ColumnAlignmentLimits,
) -> WorkbookDiff;

fn sheet_total_changes(s: &SheetDiff) -> usize; // 非公開ヘルパー
```

実装本体は[`src/diff/best_effort.rs`](../../../src/diff/best_effort.rs)を参照。

## 依存関係

- 依存先: [`diff/engine.rs`](engine.md)(`diff_sheet`、`pub(crate)`)、[`diff/row_alignment.rs`](row_alignment.md)(`align_sheet_rows`。本issue向けに`pub(crate)`化)、[`diff/col_alignment.rs`](col_alignment.md)(`align_sheet_columns`。同じく`pub(crate)`化)、[`diff/model.rs`](model.md)(`SheetDiff`/`WorkbookDiff`)、[`diff/mod.rs`](mod.md)(`RowAlignmentLimits`/`ColumnAlignmentLimits`の再エクスポート経由)、[`model/workbook.rs`](../model/workbook.md)(`Workbook`)
- 依存元: [`diff/mod.rs`](mod.md)(`diff_workbooks_best_effort`を再エクスポート)、[`markdown.rs`](../markdown.md)の`diff_file_section_from_paths`(`"M"`分岐で本関数を呼び出す。[Issue #25](https://github.com/MinamiyamaKotaro/exceldiff/issues/25))

## エラー処理方針

`diff_workbooks_best_effort`自体は`Result`を返さない。行/列アライメントの`Err`(コスト超過)はその場で握りつぶし、そのシートについては別の候補にフォールバックする——[`markdown.rs`のエラー処理方針](../markdown.md)・[Issue #32で確立した「1ファイルの失敗で全体を止めない」方針](../markdown.md)と同じ、失敗を**データとして**吸収する設計。

## テスト方針

- 行挿入・列挿入それぞれが単体でカスケードを起こさなくなることの確認(座標一致との比較込み)
- 複数シート混在ワークブック(行挿入シート・列挿入シートが同時に存在する場合)で、両方のシートが独立して最適化されることの確認——ワークブック単位で1方式を選ぶ設計では解決できなかった問題そのものの検証
- 変更の無いシートが完全に短絡され、結果から省略されることの確認
- 単一セル変更(変更数1件)が座標一致の結果とバイト単位で一致し、アライメントが試みられていないことの確認(短絡の正しさ)
- **空行の挿入が`Ok(None)`の下限に到達し、結果からシートごと省略されることの確認**——この分岐が実データで到達しうることの具体的な裏付け
- 行・列アライメント双方に極端に小さいコスト上限を与え、コスト超過時に座標一致の結果へ安全にフォールバックし、パニック・エラー伝播が起きないことの確認
- シートの追加/削除(片側のみ存在)が他の`diff_workbooks_*`関数と同じ結果になることの確認

## 未決事項 / オープンクエスチョン

1. **行と列を同時にアライメントする統合アルゴリズム**: [`diff/mod.rs`未決事項1](mod.md)/[col_alignment.md 未決事項1](col_alignment.md)/[row_alignment.md 未決事項1](row_alignment.md)から引き継ぐ、根本的に未解決の課題。本モジュールは既存3方式の中から選ぶだけなので、同一シートで行・列が同時にずれる編集(Issue #25の検証では約32%の改善に留まった)を完全には解消できない。
2. **採用した方式をMarkdown出力へ注記するか**: デバッグ・信頼性の観点では、どの方式が選ばれたかを出力に含めるのは有用だが、コメント本文のノイズにもなりうる。現状は注記しない設計とした([`markdown.rs`](../markdown.md)は`FileStatus::Modified`に`WorkbookDiff`のみを渡し、選ばれた方式の情報は`diff_workbooks_best_effort`の呼び出し元へ伝播しない)。
3. **`RowAlignmentLimits`/`ColumnAlignmentLimits`をAction入力として調整可能にするか**: 現状は`::default()`を渡す想定。Action inputs/outputsの汎用設計([Issue #24](https://github.com/MinamiyamaKotaro/exceldiff/issues/24))で改めて検討する。
