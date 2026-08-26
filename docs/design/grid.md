# `grid.rs` 設計書

*[English](grid.en.md)*

`src/grid.rs` に対応する設計書。[`markdown.rs`](markdown.md)([Issue #22](https://github.com/MinamiyamaKotaro/exceldiff/issues/22)・[Issue #31](https://github.com/MinamiyamaKotaro/exceldiff/issues/31))の検証過程で派生した、もう1つのdiff出力形式。`diff::WorkbookDiff`のシート単位の差分(`SheetDiff`)を、実際のExcelのグリッド(列幅・結合セル・罫線・塗りつぶし・折り返しをすべて忠実に再現)として、Before/Afterを左右に並べたHTMLへ整形する。

## 背景・目的

[markdown.mdの「設計判断」節](markdown.md)で述べた通り、GitHubはPRコメントに投稿されたHTML内の`style=`属性をサニタイズして無効化するため、`markdown.rs`が生成する```` ```diff ````フェンスは色付き表示ができる一方、実際のExcelのグリッド(列幅・結合セル・塗りつぶし色・罫線)をそのままの見た目で表示することはできない。本モジュールはその制約を受けない別の出力先(スクリーンショット画像、GitHub Pagesへの静的ホスティング、CIの成果物としてのダウンロード等——いずれも本モジュール自体のスコープ外)向けに、実際のExcelのグリッドに忠実なHTMLを生成する。

「方眼紙Excel」(細かく均一なセルグリッドに結合セルで見出し・枠を表現する、日本のビジネス文書に多いスタイル。[`lib.rs`](lib.md)自身がクレートの主眼としている対象)を主眼に置いており、その特性——数千行/数千列に及ぶことがある——を踏まえ、変更のない行/列が離れて存在する場合は`git diff`のコンテキスト行と同じ考え方で間を省略する。

## 責務・スコープ

- [`diff::SheetDiff`](diff/model.md)（+ diffの元になった `base`/`head` の[`Workbook`](model/workbook.md)、およびその中の対象[`Sheet`](model/sheet.md)）を受け取り、Before(base)/After(head)を左右に並べたHTMLの`<section class="sheet">`フラグメントを返す(`render_sheet_split`)。`<style>`ブロックやページ全体のHTML(`<html>`/`<head>`等)は含まない——`examples/xlsx_diff_grid.rs`のような呼び出し側が、返されたHTMLフラグメントに対応するスタイルシートを自前で用意する
- 各セルについて、その面(Before/After)の**実際の**解決済みスタイル(塗りつぶし色は`resolve_color`で実RGBへ、太字、`wrapText`、罫線)をそのまま描画する。変更セルの背景を緑/赤/黄色一色で塗り潰すのではなく、実際の見た目はそのまま保ちつつ、変更種別を1px枠線のCSSクラス(`border-added`/`border-deleted`/`border-value`/`border-style`)で重ねて示す
- [`Sheet::merged_region_at`](model/sheet.md)を用いて、各面が実際に持つ結合セル構造をそのままHTMLの`rowspan`/`colspan`として描画する。結合セルの追加/解除/リサイズもモデルの構造情報として正しく反映されるが、専用の視覚マーカーは持たない(レビューフィードバックにより削除——結合セルの変更は`SheetDiff.merges`が保持する集計上「modified」に算入されるのみ)
- `wrapText`が有効なセルは折り返し表示、無効なセルは実際のExcelの挙動(セル境界で切り詰め、隣接セルが空なら自然にはみ出す)を再現する(`next_cell_is_empty`)
- 列幅を文字単位からpx単位へExcelの実際の変換式で換算し(`excel_width_to_px`)、`<table>`自体に総px幅を明示指定してブラウザが内容に合わせて列を広げるのを防ぐ(`table-layout: fixed`だけでは不十分——詳細は`render_table`のドキュメントコメント参照)
- 変更のあった行/列の前後2行/2列だけを残し、それ以外の連続した未変更の行/列を`⋯ N row(s)/column(s) omitted ⋯`という1行/1列に畳み込む(`build_line_plan`)。行・列どちらの軸にも同じロジックを適用する
- `grid_sections_from_paths`(Issue #23の後続、[`action.yml`の`visual`input](action.md))は`markdown.rs::diff_file_section_from_paths`と同じ「パス→パース→diff→整形」の高水準APIを、本モジュール独自に提供する——A/Dステータスは空の`Workbook`を片側に見立てた`diff_workbooks`呼び出しだけで(専用の差分計算ロジックを新設せずに)表現できる。返り値`Vec<GridSection>`はシート単位のHTMLフラグメントの集合で、パースエラー・変更なしの場合は空になる
- `wrap_grid_page`は`render_sheet_split`/`GridSection::html`が返すフラグメント(群)を、スタイルシート・凡例付きの単体HTMLページへ包む——`examples/xlsx_diff_grid.rs`と`cli/`の`--grid-html-dir`(スクリーンショット用)の双方が共有する
- **含まない責務**: 生成したHTML/PNGの配信・公開そのもの(実際にスクリーンショットを撮影してリポジトリへコミットする処理は[`cli/`](cli.md)と[`action.yml`](action.md)の責務——下記「未決事項」参照)

## 主要な型・関数（案）

```rust
use crate::diff::{DiffStatus, SheetDiff};
use crate::markdown::DiffMode;
use crate::model::{Sheet, Workbook};

pub fn render_sheet_split(
    sheet_diff: &SheetDiff,
    base: &Workbook,
    head: &Workbook,
    base_sheet: Option<&Sheet>,
    head_sheet: Option<&Sheet>,
) -> String;

pub fn wrap_grid_page(sections: &str) -> String;

pub struct GridSection {
    pub sheet_name: String,
    pub html: String,
}

pub fn grid_sections_from_paths(
    git_status: &str,
    base_path: Option<&str>,
    head_path: Option<&str>,
    diff_mode: DiffMode,
) -> Vec<GridSection>;
```

内部の`LineSlot`/`CellChange`/`Side`列挙型、`render_table`/`render_cell`/`resolve_visual_style`/`border_sides_css`/`excel_width_to_px`/`column_pixel_width`等のヘルパーはすべて非公開。実装本体は[`src/grid.rs`](../../src/grid.rs)を参照。

## 依存関係

- 依存先: [`diff/model.rs`](diff/model.md)（`DiffStatus`, `SheetDiff`）、[`model/`](model/)（`Borders`, `Cell`, `CellRef`, `CellValue`, `ColorRef`, `ResolvedStyle`, `Rgb`, `Sheet`, `ThemePalette`, `Workbook`）、[`resolve/color.rs`](resolve/color.md)（`resolve_color`——`ColorRef`を実際のRGB値へ解決する。テーマカラー・インデックスカラーを含む）、[`json.rs`](json.md)（`format_date_time`——`DateTime`セルの値を、`DateTimeValue`の導出`Debug`表現ではなく`json.rs`と同じタイムゾーンなしISO 8601形式で表示する）
- 依存先(`grid_sections_from_paths`のみ): [`markdown.rs`](markdown.md)（`DiffMode`——`M`ステータスの差分計算アルゴリズムを選択するためだけに参照し、`markdown.rs`側の型・関数は一切呼ばない。本モジュールの独立性は保たれる)、[`lib.rs`](lib.md)（`parse_workbook`）
- 依存元: [`lib.rs`](lib.md)（`render_sheet_split`/`wrap_grid_page`/`GridSection`/`grid_sections_from_paths`を公開APIとして再エクスポート）、`examples/xlsx_diff_grid.rs`（`parse_workbook`/`diff_workbooks`の呼び出しと`wrap_grid_page`によるページ組み立て）、[`cli/`](cli.md)の`--grid-html-dir`フラグ（`grid_sections_from_paths`+`wrap_grid_page`でシートごとのHTMLファイルを書き出す）

## 設計判断: なぜ`markdown.rs`と統合せず別モジュールにしたか

`markdown.rs`と本モジュールはどちらも`diff::WorkbookDiff`/`SheetDiff`を入力に取るが、出力の性質が根本的に異なる——`markdown.rs`はGitHubのPRコメントというサニタイズされた環境に耐える出力(装飾的なHTML/CSSを一切使わない)を目的とし、本モジュールは逆に、装飾的なCSS(色・罫線・列幅)そのものが価値の中心である出力を目的とする。両者を1つの関数にまとめると、常に両方のスタイルを計算するコストが生じる上、片方だけを呼びたい呼び出し側にとって無関係な引数(`base`/`head`の`Workbook`全体——本モジュールはセルの実際のスタイル・列幅・結合構造を読むために必要とするが、`markdown.rs`は`WorkbookDiff`だけで完結し不要)を渡す必要が生じる。実際、`examples/xlsx_diff_cli.rs`(`markdown.rs`を使う)と`examples/xlsx_diff_grid.rs`(本モジュールを使う)は、同じ`diff_workbooks`の呼び出し結果を全く異なる形へ整形しており、共有すべき実装はほとんど無い。

## テスト方針

`Sheet::new`/`insert_cell`/`insert_merge`/`set_col_widths`(いずれも`pub(crate)`)を直接呼び出して合成の`Sheet`/`Workbook`を構築し、実ファイルのパース・プロセス起動を一切必要としない単体テストとする。

- 未変更セルが実際の値をそのまま描画し、変更系のCSSクラス(`border-*`)が一切付与されないことの確認
- 追加セルがBefore面で`not-present`(ハッチング)、After面で`border-added`となることの確認、削除セルの逆向き対称性の確認
- 値が変わったセルは`border-value`、値は同じでスタイルだけが変わったセルは`border-style`が付与され、両者が排他的であることの確認(`CellChange::ValueChanged`/`StyleOnly`の判定ロジックの検証)
- 結合セルの範囲変更(`old_end`/`new_end`)が、Before/Afterそれぞれの面で実際の`colspan`属性として正しく反映されること、かつ結合セル専用の視覚マーカー(`merge-changed`等)が一切出力されないこと(レビューで削除された機能の回帰確認)、集計行の`modified`件数に結合セルの変更が算入されることの確認
- 離れた変更間の未変更行が`gap-row`として畳み込まれること、逆にシートが小さく畳み込む必要が無い場合は`gap-row`が一切出力されないことの確認
- ある軸(行または列)に変更が全く無い`SheetDiff`(可視性のみの変更など)は、畳み込むことで実際に表示量を削減できるほど大きい場合はその軸全体を単一のgapへ畳み込み、そうでなければそのまま全行を表示すること——「その軸に変更が無い」というだけの理由で無条件に全行を表示してしまわないことの確認
- 複数シートを持つワークブックで、シート見出しの位置番号(`sheet1：`/`sheet2：`)がheadのシート順を正しく反映することの確認
- `excel_width_to_px`が既知の換算値(方眼紙Excelでよく使われる2.14文字幅 → 15px)を正しく返すことの確認(Excel公式の変換式の実装そのものの検証)
- `column_letters`が複数文字の列(Z列の次のAA列等)を正しく変換することの確認
- `html_escape`が`&`/`<`/`>`を正しくエスケープすることの確認
- スタイル付きセルの太字フラグ(`font.bold`)がインラインCSSの`font-weight:700;`として反映されることの確認
- `grid_sections_from_paths`(パスベース、`markdown.rs`の`diff_file_section_from_paths`テストと同じくin-memory `.xlsx`を組み立てて検証): A/M/Dそれぞれが期待通りの件数の`GridSection`を返すこと、パースエラー・変更なし・未対応ステータス・パス欠落の場合に空`Vec`を返すこと

## 未決事項 / オープンクエスチョン

1. ~~**生成したHTMLの配信経路**~~ **解決済み**: [Issue #24](https://github.com/MinamiyamaKotaro/exceldiff/issues/24)の後続として実装した。CI上でPlaywright(ヘッドレスChromium)によりスクリーンショットPNGを生成し、専用のorphanブランチ(`xlsx-diff-images`)へコミット・pushして`raw.githubusercontent.com`のURL経由でPRコメントへMarkdown画像として埋め込む方式を採用(GitHub Pagesは使わない——`action.yml`の`visual`input、詳細は[action.md](action.md)参照)。
2. **本モジュールを`markdown.rs`と同様に確定した公開APIとして扱ってよいか**: `grid_sections_from_paths`/`wrap_grid_page`の追加により本番のワークフロー(`action.yml`の`visual`モード)から実際に消費されるようになったため、`render_sheet_split`のシグネチャ(`base`/`head`の`Workbook`全体を要求する形)は実運用を経て妥当と確認できた。今後大きく変更する予定はない。
3. **列方向の省略と結合セルの相互作用**: `render_table`のドキュメントコメントに記載の通り、結合セルの起点が省略された行/列の中に位置する場合、`covered`の追跡が正しく機能しない既知の制約がある。方眼紙Excelの典型的な使い方(結合セルは通常、変更が集中する見出し・ラベル周辺にあり、それ自体が「変更に近い行/列」としてコンテキストに含まれやすい)では実際上問題になりにくいと考えているが、確証はない。
