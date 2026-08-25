# `markdown.rs` 設計書

*[English](markdown.en.md)*

`src/markdown.rs` に対応する設計書。[architecture.md](architecture.md) が定義する5フェーズ・パイプラインの外側に位置する後段の機能で、[`diff/`](diff/mod.md)（Issue #3）が計算した `WorkbookDiff` を、`.github/workflows/xlsx-diff.yml` がPRコメントとして投稿するGitHub Flavored Markdownへ整形する（[Issue #22](https://github.com/MinamiyamaKotaro/exceldiff/issues/22)・[Issue #31](https://github.com/MinamiyamaKotaro/exceldiff/issues/31)）。

`examples/xlsx_diff_cli.rs` に元々同居していたMarkdown整形ロジック(`print_added`/`print_deleted`/`print_modified`/`print_diff`/`format_value`/`escape_table_cell`)をライブラリ側へ切り出したもの。CLI側は標準出力へ直接 `println!` していたため、整形結果を検証するにはプロセスを実際に起動してstdoutをキャプチャし直すしかなかった。本モジュールの各関数は代わりに `String` を返すため、`WorkbookDiff` を渡して文字列を検証するだけの単体テストが書ける（[Issue #31が要求するテスト容易性](https://github.com/MinamiyamaKotaro/exceldiff/issues/31)）。

## 責務・スコープ

- [`diff::WorkbookDiff`](diff/model.md)（+ ファイルのgit状態A/M/D、表示パス）を受け取り、GitHub Flavored MarkdownのMarkdown文字列を返す（[`format_file_section`](#主要な型関数案)）
- 変更セルの一覧を ```` ```diff ```` フェンス内の `@@ <A1座標> @@` ハンク＋`-`/`+`行として整形する(`format_cell_hunk`)。Markdownテーブルではなくこの形式を選んだ理由は下記「設計判断: なぜMarkdownテーブルではなくdiffフェンスか」を参照
- 結合セルの変更（[`diff::MergeDiff`](diff/model.md)、Issue #8）を、セルのハンクと同じ ```` ```diff ```` フェンス内に `@@ <始点>:<終点> (merge) @@` ハンクとして整形する(`format_merge_hunk`)。集計行(`{added} added, {modified} modified, {deleted} deleted`)では、結合セルの追加/解除/リサイズをすべて「modified」に算入する — 結合の変更は特定の1セルの追加・削除ではなく「既存セルのグルーピングが変わった」ことなので、どちらから見ても一種の変更と捉える方が座りが良いため
- `MarkdownOptions::max_rows_per_sheet` で1シートあたりのセルハンク表示件数上限を呼び出し側から指定可能にする（[Issue #24](https://github.com/MinamiyamaKotaro/exceldiff/issues/24)のinput化と接続。結合セルハンクは上限の対象外 — 理由は下記コード中のドキュメントコメント参照）
- ファイルの見出し(`` ### <バッジ> · `path` ``)に、追加/変更/削除を一目で判別できる絵文字バッジ(🆕/✏️/🗑️/❓)を付与する(`file_status_badge`) — パスだけでは複数ファイルが変更されたPRで状態を見分けにくいため
- **含まない責務**: `.xlsx` のパース・差分計算そのもの（[`parse_workbook`](lib.md)・[`diff::diff_workbooks`](diff/engine.md)。呼び出し側の責務）、GitHub Actionsワークフロー自体の実装（`.github/workflows/xlsx-diff.yml`）、方眼紙Excelを実際のExcelグリッドのような見た目でHTML表示する機能（別issueの検討事項。GitHubのPRコメントは投稿されたHTML内の `style=` 属性をサニタイズして無効化するため、色付きの罫線・塗りつぶしをコメント本文へ直接埋め込むことはできない — この制約により、そうした視覚的なグリッド表示は別の出力先（例: スクリーンショット画像＋GitHub Pagesのリンク）を要する設計上の判断であり、本モジュールのスコープ外）

## 主要な型・関数（案）

```rust
use crate::diff::{CellDiff, CellPos, DiffStatus, MergeDiff, SheetDiff, WorkbookDiff};
use crate::json::JsonCellValue;
use crate::model::CellRef;

#[derive(Debug, Clone, Copy)]
pub struct MarkdownOptions {
    pub max_rows_per_sheet: usize, // デフォルト30
}

#[derive(Debug, Clone, Copy)]
pub struct AddedSummary {
    pub sheet_count: usize,
    pub cell_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionSide { Base, Head }

pub enum FileStatus<'a> {
    Added(Option<AddedSummary>),
    AddedParseError(&'a str),
    Deleted,
    Modified(&'a WorkbookDiff),
    ModifiedMissingContent,
    ModifiedParseError(RevisionSide, &'a str),
    Unrecognized(&'a str),
}

pub fn format_file_section(display_path: &str, status: &FileStatus, options: &MarkdownOptions) -> String;
pub fn format_workbook_diff(diff: &WorkbookDiff, options: &MarkdownOptions) -> String;
```

実装本体は[`src/markdown.rs`](../../src/markdown.rs)を参照。`format_sheet_diff`・`format_cell_hunk`・`format_merge_hunk`・`format_value`・`code_span`・`longest_backtick_run` は非公開のヘルパー。`code_span`はファイルパスやシート名のような呼び出し側/ユーザー由来の文字列をMarkdownのインラインコードスパンとして安全に埋め込む（CommonMarkの規則に従い、内容中の最長バッククォート連続より長いフェンスを選び、内容がバッククォートで始まる/終わる場合はパディング用のスペースを追加する）。`format_sheet_diff`が組み立てる ```` ```diff ```` ブロックフェンスも同じ理由で固定長ではなく、`longest_backtick_run`でレンダリング済みハンク本文中の最長バッククォート連続を測り、それより長いフェンス（3本以上）を動的に選ぶ — セルの`Error`値はバッククォートで囲んで整形されるため、これを怠るとフェンスが早期に閉じてPRコメントの表示が壊れうる。

## 設計判断: なぜMarkdownテーブルではなくdiffフェンスか

CLIの元々の実装は `| | Cell | Before | After |` 形式のMarkdownテーブルへ、`➕`/`✏️`/`➖` の絵文字を状態マーカーとして埋め込んでいた。この形式には次の問題がある。

- GitHubはPRコメントに投稿されたHTML/Markdown内の `style=` 属性をサニタイズして無効化するため、テーブルのセルに背景色を付けて `git diff` のような視覚的な赤/緑の強調表示をすることができない
- 一方 ```` ```diff ```` フェンス内で行頭が `-`/`+` の行には、GitHub自身のシンタックスハイライトが自動的に赤/緑の背景色を適用する。これは実際の `git diff` の出力と全く同じ仕組みであり、追加のスタイル指定を一切必要としない

そのため本モジュールは、値の変更を `-`/`+` 行として表現する ```` ```diff ```` フェンス形式へ切り替えた。`@@ <A1座標> @@` というハンク見出しも、統一diff形式のハンク見出し(`@@ -a,b +c,d @@`)を模したもので、GitHubはこの行にも紫系の強調表示を適用する。結合セルの変更(`format_merge_hunk`)には、同じ見出し行に `(merge)` タグを付けて、値の変更ハンクと視覚的に区別できるようにしている（`@@ B1:F1 @@` だけでは単なるセル範囲表記と誤読されうるため）。

## 依存関係

- 依存先: [`diff/model.rs`](diff/model.md)（`CellDiff`, `CellPos`, `DiffStatus`, `MergeDiff`, `SheetDiff`, `WorkbookDiff`）、[`json.rs`](json.md)（`JsonCellValue` — `CellDiff::old_value`/`new_value`/`format_value`の変換先として再利用。[json.mdの設計判断](json.md)通り、値の種別タグ付き表現をdiffの世界でも一貫させる）、[`model/cell.rs`](model/cell.md)（`CellRef::to_a1` — 座標をA1形式へ変換）
- 依存元: [`lib.rs`](lib.md)（`FileStatus`/`MarkdownOptions`/`AddedSummary`/`RevisionSide`/`format_file_section`/`format_workbook_diff`を再エクスポートし、クレートの公開APIとする）、`examples/xlsx_diff_cli.rs`（引数パース・`parse_workbook`/`diff_workbooks`の呼び出し・結果を`FileStatus`へ詰め替える薄いラッパーとして、本モジュールの関数を呼び出す。CLI自体をさらに薄くする作業自体は[Issue #32](https://github.com/MinamiyamaKotaro/exceldiff/issues/32)のスコープ）

## エラー処理方針

本モジュールの関数はすべて `Result` を返さない — 入力として受け取る `WorkbookDiff`/`FileStatus` は既に呼び出し側（`parse_workbook`/`diff_workbooks`）で解決済みの正常なデータであり、失敗しうる操作（ファイルパース・差分計算）は一切行わない。パース失敗そのものは `FileStatus::AddedParseError`/`ModifiedParseError` という**データとして**表現し、呼び出し側がその情報を渡せば、本モジュールはエラーメッセージをそのままMarkdown文字列へ整形するだけである（[`CellDiff::old_value`/`new_value`](diff/model.md)が「結果を列挙子で保持する」慣習と同じ設計）。

## テスト方針

- `WorkbookDiff`を直接構築し、`format_workbook_diff`/`format_file_section`へ渡して文字列を検証する（プロセス起動・fixtureファイル不要 — Issue #31が要求するテスト容易性そのものの検証）
- 値が変更されたセル1件を持つ`WorkbookDiff`が正しい ```` ```diff ```` フェンス(`@@ A1 @@`/`- 1`/`+ 2`)へ整形されることの確認
- シートが0件の`WorkbookDiff`が`_No differences detected._`として整形されることの確認
- `max_rows_per_sheet`がセルハンクの件数を正しく上限し、超過分を`_...and N more change(s) in this sheet._`として報告することの確認
- 結合セルの追加/解除/リサイズ3パターンすべてが、それぞれ正しい`@@ ... (merge) @@`ハンク形状(`+ merged`/`- merged`/`- merged A:B` + `+ merged A:C`)へ整形されること、および集計行の`modified`件数に結合セルの変更が正しく算入されることの確認
- シートの可視性が変わった場合に`` _Visibility: `old` → `new`_ ``行が出力されることの確認
- `FileStatus`の全バリアント(`Added`/`AddedParseError`/`Deleted`/`Modified`/`ModifiedMissingContent`/`ModifiedParseError`/`Unrecognized`)それぞれについて、見出しのバッジ文言と本文が期待通りに整形されることの確認。`ModifiedParseError`は`RevisionSide::Base`/`Head`両方で、エラーメッセージにどちら側が失敗したか正しい文言(the previous version/the new version)が現れることを確認
- テキスト値中の改行がdiff行の1行構造を壊さないようエスケープされること、一方で`|`はMarkdownテーブル構文ではなくなったため一切エスケープされないことの確認（旧テーブル形式からの意図的な変更点）

## 未決事項 / オープンクエスチョン

1. **公開関数名・型名の最終決定**: `format_file_section`/`FileStatus`は本実装での名称。[Issue #31本文](https://github.com/MinamiyamaKotaro/exceldiff/issues/31)も「公開関数名は要検討」としていたが、レビューを経て確定した現状の名称をここに記録する。
2. **`Write`への書き込み対応**: Issue #31本文は「出力は`String`(または`Write`への書き込み)」としていたが、本実装は`String`のみを提供する。[`json.rs`](json.md)の`to_json_writer`/`to_json_string`と同じ「Writer版を主、String版はラッパー」というパターンに合わせるかどうかは、実際のワークフローでの出力サイズ・メモリ使用量が問題になった場合に再検討する。
3. **方眼紙Excelのグリッド表示との関係**: 「設計判断」節で触れた通り、GitHubのPRコメントは装飾的なHTML/CSSをサニタイズするため、方眼紙Excelを実際のExcelのグリッドのような見た目で表示するには、スクリーンショット画像＋GitHub Pagesへの静的ホスティングという別経路が必要になる。この経路の具体的な実装（CI側でのヘッドレスブラウザによるレンダリング、`actions/upload-pages-artifact`/`actions/deploy-pages`の導入）は、本モジュールのスコープ外の別issueとして切り出す。
