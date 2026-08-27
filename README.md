# exceldiff

*[English](README.en.md)*

[![Rust CI](https://github.com/MinamiyamaKotaro/exceldiff/actions/workflows/rust-ci.yml/badge.svg)](https://github.com/MinamiyamaKotaro/exceldiff/actions/workflows/rust-ci.yml)
[![Docs](https://github.com/MinamiyamaKotaro/exceldiff/actions/workflows/docs.yml/badge.svg)](https://github.com/MinamiyamaKotaro/exceldiff/actions/workflows/docs.yml)
[![exceldiff on crates.io](https://img.shields.io/crates/v/exceldiff.svg)](https://crates.io/crates/exceldiff)
[![codecov](https://codecov.io/gh/MinamiyamaKotaro/exceldiff/branch/master/graph/badge.svg)](https://codecov.io/gh/MinamiyamaKotaro/exceldiff)
[![License](https://img.shields.io/github/license/MinamiyamaKotaro/exceldiff)](LICENSE)

Rustで書かれた、軽量・高速な `.xlsx`(OOXML)パーサーライブラリです。加えて、PRで変更された `.xlsx` ファイルの差分プレビューコメントを自動投稿する[GitHub Action](#github-actionとして使う)としても利用できます。

## 開発動機

`.xlsx` ファイルは日本のビジネスシステムでよくgit管理・PRレビューの対象になりますが、`git diff` はそこから何も意味のある情報を返しません——`.xlsx` はZIP圧縮されたXMLパーツの集合であり、セル1つを変えただけでも共有文字列テーブルの並びやZIP圧縮結果全体が変わり得るため、gitからはただのバイナリ差分にしか見えないからです。

`exceldiff` はこのギャップを埋めます。変更前後の `.xlsx` をパースして2つの `Workbook` として復元し、セル単位の追加・変更・削除を比較する差分エンジンを核に、その結果をMarkdownテキストやExcelライクなグリッドのHTMLビューとしてPRコメントへ要約するCLI・GitHub Actionまで一貫して提供します。対象は日本のビジネスシステムでよく見られる、行・列数が極端に多い(「方眼紙Excel」)シートや結合セルを多用したファイルです——パーサー自体がこれらをフルのインメモリ2次元グリッドを構築せず低メモリ・高速に扱えることは、単にパーサー自身の軽さにとどまらず、差分計算そのものの速度と正確さ(1行/1列の挿入だけで大量の`Modified`を誤検出しないための行/列アライメント検出を含む)に直結しています。

この基盤パーサー部分は、姉妹プロジェクトである[`xlsxparser`](https://github.com/MinamiyamaKotaro/xlsxparser)と同じ設計・実装をベースにしています。パース処理そのものの詳細なアーキテクチャ・対応OOXMLパーツ・パース性能のベンチマーク(`calamine`との比較等)は`xlsxparser`側のREADMEにまとまっているため、そちらを参照してください。

## ステータス

コア実装は完了しています——以下に示す設計上の全モジュールが実装済みで、`docs/design/` の設計書どおりにテストされています。公開API(`parse_workbook`、`parse_workbook_reader`、`to_json_string`、`to_json_writer`、`resolve_color`)は `src/lib.rs` に結線されています。

```rust
let workbook = exceldiff::parse_workbook("book.xlsx")?;
let json = exceldiff::to_json_string(&workbook)?;
```

- [docs/requirement/requirements.md](docs/requirement/requirements.md) —
  機能要件と、後述する5フェーズ・パイプラインの要約([English](docs/requirement/requirements.en.md)版もあります)。
- [docs/design/architecture.md](docs/design/architecture.md) — `src/`
  ディレクトリ全体の構成・各モジュールの責務・設計方針([English](docs/design/architecture.en.md)版もあります)。
  ここから、全ファイルそれぞれの設計書(責務・スコープ、主要な型・関数シグネチャ、依存関係、エラー処理方針、テスト方針、未決事項を記載)にリンクしており、各設計書は日英両方(`*.md` / `*.en.md`)で書かれています。実装が設計書のドラフトと異なる形に落ち着いた場合(外部APIの詳細が想定と違う形で確定した、テスト作成中にバグが見つかった等)は、何がどう変わったかを記録するため設計書自体をその場で更新しています。

## GitHub Actionとして使う

`.xlsx`ファイルを変更するPRに対して、シートごとの変更点をまとめたコメントを自動投稿するGitHub Action(composite action)として使えます。

### 前提条件

composite actionは以下の2つを自分自身では宣言・実行できないため、呼び出し元のワークフロー側で用意してください:

- `actions/checkout@v4` を `fetch-depth: 0` 付きで実行しておくこと——差分計算がPRのbase/head双方のリビジョンを `git show` で参照するため、shallowチェックアウトでは動作しません。
- `permissions: contents: read`——`comment`/`visual`の設定に関わらず常に必要です。`permissions:`ブロックを一つでも書くと、列挙しなかったスコープは(リポジトリの既定値ではなく)`none`になるというGitHub Actionsの仕様があり、`pull-requests: write`だけを書くと`contents`が黙って`none`になって`actions/checkout`自体が「repository not found」という分かりにくいエラーで失敗します(外部リポジトリからの実地検証で実際に踏んだ不具合です)。
- `permissions: pull-requests: write`——`comment`入力を既定の`true`のままコメント投稿するために必要です(`comment: false`・`job-summary: true`にすればこの権限は不要)。`visual: true`を使う場合も追加の権限は不要です(下記参照)。

### 使用例

```yaml
name: xlsx diff preview

on:
  pull_request:

permissions:
  contents: read
  pull-requests: write

jobs:
  xlsx-diff:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - uses: MinamiyamaKotaro/exceldiff@v1
```

Excelライクなグリッドのビューも見たい場合は `visual: true` を指定します。追加の`permissions:`は不要です——グリッドはPRコメントへの直接埋め込みではなく、変更ファイルごとに全シートを1つにまとめた単体HTMLページとしてワークフローのartifactへ添付され、コメントにはダウンロードリンクが載ります(このリポジトリの閲覧権限を持つ人だけがダウンロードできます。プライベートリポジトリで確実に見えるようにするための設計です。詳細は [action.yml](action.yml) 冒頭のコメントと [Issue #47](https://github.com/MinamiyamaKotaro/exceldiff/issues/47) を参照)。スクリーンショット画像ではなくHTMLなので、大きなシートでも縮小されて潰れず、ブラウザでスクロール・拡大しながら閲覧できます:

```yaml
permissions:
  contents: read
  pull-requests: write

steps:
  - uses: actions/checkout@v4
    with:
      fetch-depth: 0
  - uses: MinamiyamaKotaro/exceldiff@v1
    with:
      visual: 'true'
```

### inputs

| input | 型/既定値 | 内容 |
|---|---|---|
| `github-token` | string, `${{ github.token }}` | コメント投稿に使うトークン |
| `files` | string, `'*.xlsx'` | `git diff -- <files>` へそのまま渡す**gitパススペック**(シェルグロブではありません) |
| `comment` | bool文字列, `'true'` | PRコメントとして投稿するか |
| `job-summary` | bool文字列, `'false'` | `$GITHUB_STEP_SUMMARY` へも書き出すか。forkからのPRなど `pull-requests: write` を付与できない環境では、`comment: false`・`job-summary: true` にすると権限エラーを避けつつ差分を確認できます |
| `max-rows-per-sheet` | 数値文字列, `'30'` | 1シートあたりに表示するセル変更ハンクの上限数 |
| `diff-mode` | `auto` \| `coordinate`, `'auto'` | `auto`(既定)は座標一致/行アライメント/列アライメントのうち最も変更が少なく報告される方式を自動選択。`coordinate`はアライメント検出をスキップした単純な座標比較を強制します |
| `visual` | bool文字列, `'false'` | 変更のあったシートごとにExcelライクなグリッドのBefore/AfterビューをHTMLページとして生成し、ワークフローのartifactとして添付(コメントにはダウンロードリンクを掲載)するか。追加の`permissions:`は不要 |

### outputs

| output | 型 | 内容 |
|---|---|---|
| `has-changes` | bool文字列 | `files` にマッチするファイルがPR内で変更されたか |
| `changed-files-count` | 数値文字列 | `files` にマッチする変更ファイル数 |

より詳細な設計・トレードオフは [docs/design/action.md](docs/design/action.md) を参照してください。

## CLIとして使う

上記のGitHub Actionは、内部で `cli/`(パッケージ名 `xlsxdiff`。crates.ioには非公開)が提供するバイナリを、PRで変更された `.xlsx` ファイル1件につき1回起動しています。このバイナリは単独でもビルド・実行でき、1ファイルのgit差分をGitHub Flavored MarkdownのセクションとしてMarkdown文字列で出力します。

### ビルド

```bash
git clone https://github.com/MinamiyamaKotaro/exceldiff.git
cd exceldiff
cargo build --release -p xlsxdiff
```

### 使い方

```text
xlsxdiff [--max-rows-per-sheet <N>] [--diff-mode <auto|coordinate>] [--grid-html-dir <dir>] <display_path> <A|M|D> [base_file] [head_file]
```

- `display_path`: Markdownの見出しに表示するパス(リポジトリ内でのファイルパス)。
- `A`/`M`/`D`: gitのステータス(追加/変更/削除)。
- `base_file`/`head_file`: 変更前/変更後の実際のファイルシステムパス。`A`に`base_file`、`D`に`head_file`は無く、その場合は省略するか空文字列を渡します。
- `--max-rows-per-sheet <N>`(既定 `30`): 1シートあたりに表示するセル変更ハンクの上限数。
- `--diff-mode <auto|coordinate>`(既定 `auto`): `auto`はアライメント自動選択、`coordinate`は単純な座標比較を強制。
- `--grid-html-dir <dir>`: 指定すると、変更のあった全シートを1つにまとめた単体HTMLページ(`<dir>/grid.html`)と、シート名の一覧 `<dir>/manifest.tsv`(`sheet_name\thtml_path`、全行が同じ`grid.html`を指す)を追加で書き出します(`action.yml`の`visual: true`がこのHTMLページをそのままworkflow artifactへ添付します)。

例(`M`——変更前後どちらのファイルも存在するケース):

```bash
xlsxdiff "budget.xlsx" M /tmp/base/budget.xlsx /tmp/head/budget.xlsx
```

パースエラーや未知のgit statusも(プロセスを異常終了させず)Markdown内のエラー表示として出力されます——1ファイルの問題が他のファイルの差分表示を止めないための設計です。詳細は [docs/design/cli.md](docs/design/cli.md) を参照してください。

## 埋め込み画像

`images` はシート単位の、セルに固定された埋め込み画像(`xl/drawings/drawingN.xml`)の配列です——実際の出力例(`tests/fixtures/complex/embedded_image.xlsx` から。`B2:E9` に固定されハイパーリンクを持つ画像が1つ。以下では簡潔さのため `cells`/`columns` を省略しています):

```json
{
  "images": [
    {
      "anchor": {
        "type": "twoCell",
        "from": { "row": 2, "col": 2, "colOff": 10000, "rowOff": 20000 },
        "to": { "row": 9, "col": 5, "colOff": 0, "rowOff": 0 }
      },
      "target": "xl/media/image1.png",
      "hyperlink": "https://example.com/sample-image"
    }
  ]
}
```

- `anchor` は `type` でタグ付けされます: `"twoCell"`(2つのセル角の間に伸びる、`from`/`to`)または `"oneCell"`(`from` に加えEMU単位のサイズ `ext: {"cx", "cy"}`——`oneCell` アンカーはどのセル境界にもサイズが依存しないため `to` マーカーを持ちません)。`row`/`col` は1始まりで、本crateが出力する他の全セル座標と同じです。`colOff`/`rowOff` はそのセル*内*でのEMU単位オフセットです(丸めて捨てずに保持することで、数ピクセルずれた画像と全く動いていない画像を差分で区別できます)。
- `target` は埋め込みメディアパーツの解決済みパス(例: `"xl/media/image1.png"`)です——画像自体のバイト列は完全にスコープ外です(差分検出ツールにピクセルデータは不要であり、読み込むとメモリ使用量がセル数ではなく画像数にスケールしてしまいます)。
- `hyperlink` は画像自身のハイパーリンク(`a:hlinkClick`)で、セルのハイパーリンク(`JsonCell` レベルのフィールド。前述)とは別物です。画像がハイパーリンクを持たない場合は省略されます。`Internal`(パッケージ内)ターゲットは `target` と同じ方法でZIPエントリ名相当のパスへ解決され、`External`(上記のようなURL)はそのまま保持されます。
- グループ化された画像(`<xdr:grpSp>`)は、内包する各 `<xdr:pic>` のアンカーを所属グループ相対で解決した上で、同じシート単位の `images` 配列に平坦化されます——別途グループ構造が公開されることはありません。

## 表示色の解決

上記の `fillFgColor`/`fillBgColor` は、exceldiffの主目的が描画ではなく差分検出であるため生のまま保持されています——しかし、呼び出し側がセルの実際の表示色を(変化したかどうかだけでなく)知る必要がある場合、`resolve_color` が3つの `ColorRef` 形式(`rgb` / `theme`+`tint` / `indexed`)のいずれもオンデマンドで実際の `Rgb { r, g, b }` 値に変換します:

```rust
use exceldiff::{parse_workbook, resolve_color, CellRef};

let workbook = parse_workbook("book.xlsx")?;
let sheet = &workbook.sheets()[0];
let cell = sheet.get(CellRef { row: 1, col: 1 }).unwrap();

if let Some(color_ref) = cell.style.as_ref().and_then(|s| s.fill_fg_color.as_ref()) {
    let rgb = resolve_color(color_ref, workbook.theme());
    // 例: Some(Rgb { r: 0x4F, g: 0x81, b: 0xBD })
}
```

- `theme`+`tint` 参照は、ワークブックの `xl/theme/theme{N}.xml` の `<clrScheme>`(`Workbook::theme()`)に対して解決され、ECMA-376のtint輝度補正を適用します。ワークブックにテーマパーツが全く無い場合や、参照先のスロットインデックスが範囲外の場合は `None` を返します。
- `indexed` 参照はレガシーなECMA-376の64色パレットに対して解決されます。`indexed=64`/`65`(「システム前景色」/「システム背景色」の特殊値)は、OSのシステムパレットに依存せず固定の `#000000`/`#FFFFFF` に解決されます(本crateはヘッドレスで動作するため)。
- `resolve_color` は不正な入力(範囲外のテーマインデックス、非有限な `tint`、不正な16進数)に対してパニックせず、代わりに `None` を返します。
- `xl/theme/theme{N}.xml` は、ワークブックのスタイルシートが実際にテーマ色を参照している場合のみ読み込み・パースされます(「使う分だけ払う」)——一度もテーマ色を使わないワークブックは、ファイル内にパーツが存在していてもこの機能のI/O・CPUコストを一切払いません。

## アーキテクチャ

1. **リレーションシップ解決** — `_rels` パーツをパースし、シートの `r:id` からワークシートファイルパスへのルーティングマップを構築した上で、中間データを即座に破棄します。
2. **サニタイズ** — 信頼できないコンテンツをパースする前に、zip bomb・zip-slipパストラバーサル・XXEから防御します。
3. **ストリームパース** — SAXスタイルのリーダーが `<sheetData>` を `<row>` 単位で処理し、シート全体のXML DOMをメモリ上に保持しません。
4. **解決** — 共有文字列(`t="s"`)とセルスタイルはSST/スタイルシートに対して解決され、`<mergeCells>` 範囲はストリームパス完了後に収集済みセルに対して解決されます。
5. **JSON出力** — 解決済みのデータモデルは(結合セル用の `row_span`/`col_span` を含む)構造化JSONへシリアライズされます。これは主要な `Workbook` を返すAPIとは別の独立したステップです。

設計を駆動する中核要件:

- **疎なストレージ** — セルは密な2次元配列ではなく座標キー付きマップに保持されるため、疎な「方眼紙」シートもメモリ上で低コストに保持できます。
- **結合セルの透過性** — 結合範囲内の任意の座標は(バウンディングボックスによるO(1)事前チェックとシートの結合範囲群に対する幾何的包含スキャンにより)、その範囲の起点セルと同じ値・結合メタデータに解決されます。
- **I/Oとドメインロジックの分離を維持** — XML/ZIP処理(`container/`、`parse/`)は解決ロジック(`resolve/`)と決して混在しません。解決ロジックはインメモリデータのみで純粋に動作し、単体テストにI/Oを必要としません。

モジュール構成(コア5フェーズパイプラインの各ファイルの責務は [docs/design/architecture.md](docs/design/architecture.md)、差分計算・Markdown整形・グリッド描画は [docs/design/diff/mod.md](docs/design/diff/mod.md)・[docs/design/markdown.md](docs/design/markdown.md)・[docs/design/grid.md](docs/design/grid.md) 参照):

```text
src/
  lib.rs        # 公開APIのエントリポイント (parse_workbook, diff_workbooks_best_effort, diff_file_section_from_paths, ...)
  error.rs      # クレート全体の共通エラー型
  pipeline.rs   # 5フェーズ・パイプラインとリソースの生存期間のオーケストレーション

  container/    # ZIP (OPC) 展開、zip-bomb/zip-slip防御
  parse/        # XMLパース (quick-xml の使用はここに閉じ込める)、XXE対策
  model/        # 純粋なデータ構造 (Workbook, Sheet, Cell, CellValue, ...)
  resolve/      # 共有文字列/スタイル/結合セルの解決 + オンデマンドの色解決、I/O非依存
  json.rs       # 解決済み Workbook を JSON へシリアライズ

  diff/         # 2つの Workbook を比較する差分エンジン(座標一致/行アライメント/列アライメント/ベストエフォート自動選択)
  markdown.rs   # WorkbookDiff を GitHub Flavored MarkdownのPRコメントへ整形(CLIのエントリポイント diff_file_section_from_paths)
  grid.rs       # 変更のあったシートをExcelライクなグリッドHTMLとして描画(action.ymlの visual: true モードがそのままartifactへ添付)
```

## 対応OOXMLパーツ

- `xl/_rels/workbook.xml.rels`
- `xl/workbook.xml`(`<workbookPr date1904="...">` を含む。日付/時刻セルのシリアル値を1900年方式/1904年方式のどちらで解決するか判定するのに必要)
- `xl/sharedStrings.xml`(リッチテキストランの連結、`xml:space="preserve"` の処理、CDATAラン、Excelがリテラルな改行に使う `_x000D_` エスケープ)
- `xl/styles.xml`(フォントサイズ/太字、水平方向配置、折り返し、書式コード——組み込みnumFmtIdテーブル(ECMA-376 §18.8.30)とカスタム `<numFmt>` コード両方——塗りつぶし色(生の `rgb`/`theme`+`tint`/`indexed` 形式のまま保持。実RGB値への変換は前述の[表示色の解決](#表示色の解決)参照)、辺ごとの罫線有無——線のスタイル/太さ/色や `<diagonal>` は読みません)
- `xl/theme/theme{N}.xml`(`<clrScheme>` の12色。スタイルが実際にテーマ色を参照している場合のみ読み込みます。前述の[表示色の解決](#表示色の解決)参照)
- `xl/worksheets/sheetX.xml`(`<sheetData>`——他の全ての日付/時刻セルが使う数値シリアル日付と並んで、`t="d"` のISO 8601日付セルも同じ `"dateTime"` 出力に統一されます——`<mergeCells>`、および生のまま未解決で保持する `<hyperlinks>`(前述の `hyperlink` フィールド参照))
- `xl/worksheets/_rels/sheetX.xml.rels`(`<hyperlink r:id="...">` を生のTarget文字列に解決します——シートが `r:id` 付きのハイパーリンクを少なくとも1つ宣言している場合のみ読み込みます。`location` のみの内部ハイパーリンクではこの読み込みは発生しません)
- `xl/drawings/drawingN.xml` とその `_rels`(セルに固定された埋め込み画像——アンカー形状、埋め込みメディアの解決済みパス、画像自身のハイパーリンク。`<xdr:grpSp>` グループにネストした画像を含む。前述の[埋め込み画像](#埋め込み画像)参照)

`[Content_Types].xml` は一切読みません——ワークブックパーツの実際のパスは、慣習的な `xl/workbook.xml` と仮定するのではなく `_rels/.rels` の `officeDocument` リレーションシップ経由で解決します(Issue #55)が、この解決はパーツの宣言されたContent-Typeを `[Content_Types].xml` と突き合わせて検証することは一切ありません(この判断の理由と厳密なOPC準拠とのトレードオフについては [pipeline.md 未決事項3](docs/design/pipeline.md) 参照)。

## ベンチマーク

パース性能そのもの(疎な「方眼紙Excel」でのメモリ使用量、結合セルが多いファイルでの計算量、`calamine` との比較など)は、基盤パーサーを共有する[`xlsxparser`のREADMEのベンチマークセクション](https://github.com/MinamiyamaKotaro/xlsxparser#benchmarks)を参照してください。`exceldiff` 固有の差分計算・Markdown整形・グリッド描画のベンチマークは、必要になった時点で別途ここに追加します。

## セキュリティに関する注記

- **Zip Bomb / Zip Slip / XXE**: パース時に防御しています(前述の[アーキテクチャ](#アーキテクチャ)、および完全な分析は [docs/security/design-review.md](docs/security/design-review.md) 参照)。
- **CSV / 数式インジェクション**: セルの文字列値(数式の計算結果文字列を含む)は、いかなる段階でもエスケープされず、そのまま通過します——これはJSON出力としては安全ですが、パース結果をCSVや他のスプレッドシート形式へ再出力する呼び出し側は、自身で数式インジェクション対策(`=`、`+`、`-`、`@` で始まる値のエスケープなど)を行う責任があります。`.xlsx` 入力は信頼できないものであり、本ライブラリはセル内容の書き換えを一切行わないためです。

## ライセンス

本プロジェクトは GNU Affero General Public License v3.0(AGPL-3.0)の下でライセンスされています。詳細は [LICENSE](LICENSE) ファイルを参照してください。

### Actionとしての利用とAGPLについて

`uses: MinamiyamaKotaro/exceldiff@<tag>` として本Actionを無改変のまま呼び出すだけの場合、呼び出し元のリポジトリ・ワークフローに新たなAGPL-3.0上の義務は生じません。composite actionは本リポジトリのソースをそのままCIランナー上でビルド・実行するだけであり、呼び出し元は本ソフトウェアを「改変」していないためです——AGPL-3.0第13条のネットワーク経由コピーレフト条項は、Programを*改変*したバージョンをネットワーク越しに利用者へ提供する場合に生じる義務であり、本Action自体の対応ソースも、それを呼び出しているこの公開リポジトリとしてすでに利用可能です。

一方、本ソフトウェアをフォーク・改変した上で、その改変版を自身のAction・サービスとしてネットワーク越しに他者へ提供する場合は、AGPL-3.0第13条によりその改変後のソースを利用者へ提供する義務が生じます——これは通常のAGPLの挙動であり、本プロジェクト固有の追加条件ではありません。

これは一般的な整理であり法的助言ではありません。確実な判断が必要な場合は専門家にご相談ください。AGPL-3.0の義務自体を避けたい場合は、下記の商用ライセンスをご検討ください。

### 商用ライセンス

`exceldiff` はデュアルライセンスです: 上記のAGPL-3.0の条件が既定で適用されますが、クローズドソース/プロプライエタリなシステムで、またはAGPL-3.0のコピーレフト・ネットワーク経由でのソース公開義務なしに本ソフトウェアを利用したい場合、別途商用ライセンスをご利用いただけます。

商用ライセンスが具体的に何をカバーするか、また申請方法については [COMMERCIAL-LICENSE.md](COMMERCIAL-LICENSE.md) を参照してください。
