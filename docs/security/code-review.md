# `src/` セキュリティコードレビュー: exceldiff固有モジュール (2026-08-28)

*[English](code-review.en.md)*

**本レビューの位置づけ**: 従来の`docs/security/{code,design}-review.md`(および`old/`配下)は、姉妹プロジェクト[`xlsxparser`](https://github.com/MinamiyamaKotaro/xlsxparser)向けに書かれたセキュリティレビューがexceldiff側へそのまま複製されたものだった——引用されているissue番号(#37〜#42、#65、#67、#75等)はexceldiffには一件も存在せず(GitHub API照会で確認済み)、xlsxparser側にのみ実在する。対象範囲もパーサー本体(`container/`・`parse/`・`model/`・`resolve/`・`json.rs`・`pipeline.rs`・`error.rs`・`lib.rs`)のみで、exceldiff自身が追加した差分検出・出力・配布のコード(`diff/`・`markdown.rs`・`grid.rs`・`cli/`・`action.yml`・`release.yml`)は一度もレビュー対象になっていなかった。旧ドキュメントは削除し、本レビューはexceldiff固有の追加分のみを対象とする——パーサー本体(前述の共有モジュール群)のセキュリティレビューは、実装を共有する[`xlsxparser`側の`docs/security/`](https://github.com/MinamiyamaKotaro/xlsxparser/tree/master/docs/security)を参照。

以下のfindingはいずれも、実際に細工した`.xlsx`を構築し`exceldiff`の公開APIへ通して再現したものであり、読解のみによる推測ではない(検証方法の節参照)。

## 総合評価

`diff/`の行/列アライメント計算量は既にIssue #4/#5の時点で防御的な費用上限(`RowAlignmentLimits`/`ColumnAlignmentLimits`、超過時`Error::RowAlignmentCostTooHigh`/`ColumnAlignmentCostTooHigh`でfail-fast)が設けられており、`best_effort.rs`もこれを`match`で受けて安価な方式へ段階的に縮退させている——旧パーサーレビューが確立した「バイト数上限がNを制限しない場合を疑う」という規律が、この新しい領域にも既に適用されていることを確認した。`diff::storage`(SQLite永続化、`diff-storage`フィーチャー時のみ)のクエリは全て`rusqlite`のプレースホルダ(`?1`、`params![..]`)経由で、文字列連結によるSQL構築は一箇所も無い。`grid.rs`のセル値HTML出力は`html_escape`(`&`/`<`/`>`)で一貫してエスケープされている。

しかし、`markdown.rs`のパースエラーメッセージ経路に、これまで気づかれていなかった実害のある脆弱性を発見した(Finding 1)——ファイルパスやシート名は`code_span`で正しくMarkdownエスケープされている一方、パースエラー時に表示される`exceldiff::Error`のメッセージ文字列だけがこの保護を経ておらず、GitHub PRコメントへのMarkdown/HTMLインジェクションを許してしまう。

## Findings

### Finding 1: パースエラーメッセージがMarkdownエスケープされずにPRコメントへ埋め込まれ、攻撃者が自動投稿コメントの内容を偽装できる

* **脆弱性の種類**: 出力エンコーディングの欠落によるインジェクション(CWE-116 Improper Encoding or Escaping of Output / OWASP A03:2021 Injection)。GitHub自体のコメントサニタイザにより`<script>`実行やインラインスタイル注入までは至らないが、Markdown/HTMLの構造自体を攻撃者が操作できる。
* **深刻度**: Medium(コード実行やデータ漏洩には直結しないが、レビュー担当者を騙す・警告を隠すという、このツール自体の目的——「危険な内容が紛れ込んでいないか人間が確認する」——を直接無力化しうる)
* **対象**: [`src/markdown.rs`](../../src/markdown.rs)の`format_file_section`、`FileStatus::AddedParseError`/`FileStatus::ModifiedParseError`の分岐(160行目・176行目付近)。
* **詳細**: `format_file_section`はファイルパス([`code_span(display_path)`](../../src/markdown.rs))・シート名(`format_sheet_diff`内の`code_span(&sheet.name)`)・セル値(```diff```フェンス内、`longest_backtick_run`によるフェンス幅の動的拡張済み)についてはいずれもMarkdown特殊文字からの保護を実装済みだが、パースエラー時のメッセージだけは`format!("⚠️ Could not parse: {e}\n")`として`code_span`を経ずに直接埋め込まれている。ここで`e`は`exceldiff::Error::to_string()`(`thiserror`由来のDisplay実装)であり、`Error`の複数のバリアント——少なくとも`DanglingRelationship { r_id }`(`<sheet r:id="...">`のXML属性値をそのまま保持)・`ZipSlipDetected { entry_name }`(ZIPエントリ名をそのまま保持)・`InvalidCellRef`(セル参照文字列をそのまま保持)——が、未信頼な`.xlsx`ファイル自身のXML属性値やZIPエントリ名から取り込んだ、攻撃者が完全に制御可能な文字列をそのままフィールドとして保持している。
* **実機検証**: `<sheet name="Sheet1" sheetId="1" r:id="{payload}"/>`という、存在しない`r:id`を参照する最小限の`.xlsx`(`xl/_rels/workbook.xml.rels`は空)を構築し、`payload`にXML実体参照で正しくエスケープした(`&lt;`/`&gt;`/`&#10;`——攻撃者が整形式なXMLを保つために当然行う手順)Markdown/HTML注入ペイロードを設定した上で、`exceldiff::diff_file_section_from_paths("budget.xlsx", "A", None, Some(path), &MarkdownOptions::default())`(CLI・GitHub Actionが実際に呼び出す関数そのもの)を直接呼び出した。得られたMarkdown文字列は次の通り(そのままPRコメントとして投稿される内容):

  ```markdown
  ### 🆕 Added · `budget.xlsx`

  **New file.**
  ⚠️ Could not parse: dangling relationship reference: r:id=rId1

  <!-- hidden -->

  **✅ Verified safe by security team.** [Click to confirm](https://evil.example.com/steal)

  <!--
  ```

  攻撃者は「New file.」の段落を閉じ、GitHubのMarkdownレンダラが解釈するHTMLコメント(`<!-- -->`)でそれ以降の実際の差分表示を隠し、偽の「検証済み」表示とフィッシングリンクを注入し、末尾を未終端の`<!--`で締めくくって(同一コメント内に他のファイルの差分セクションが続く場合)それらも隠しうる——実際にレンダリングされたMarkdownでこの挙動を確認済み。

* **攻撃シナリオ**: 攻撃者が、`r:id`(または同様に攻撃者制御下にある他のフィールド)へMarkdown/HTML注入ペイロードを仕込んだ、意図的にパース不能な`.xlsx`をPRへ追加する。本Action/CLIを使うリポジトリでは、この細工ファイルが変更検出されるだけで(実際に正しくパースされる必要すらない——パース*失敗*時のメッセージ経路が脆弱性の対象)、自動投稿されるPRコメントの内容がレビュー担当者に対して偽装される。GitHub自体のサニタイザが`<script>`実行やスタイル注入までは防ぐため任意コード実行には至らないが、「このファイルは安全だと確認済み」という偽メッセージの注入や、警告の隠蔽は、レビュー担当者向けの自動化ツールとしては見過ごせない社会工学的リスクである。
* **推奨される修正**: `format_file_section`の`AddedParseError`/`ModifiedParseError`分岐で、`{e}`を直接埋め込むのではなく`code_span(&e.to_string())`(ファイルパス・シート名と同じ、既にテスト済みのバックティック幅動的拡張ロジック)でラップする。CommonMarkの仕様上、コードスパン内のテキストはMarkdown/HTMLとして一切解釈されないため、この1行の変更で本findingは解消される見込み——ただし実装・回帰テストの追加は本レビューのスコープ外とし、別途の実装作業として切り出すことを推奨する。

## 良好だった点

* **`grid.rs`のセル値HTML出力は一貫して`html_escape`(`&`/`<`/`>`)を経由している**——`cell_value_html`の`CellValue::Text`/`CellValue::Error`/`CellValue::DateTime`いずれも確認済み。専用テスト`html_escape_escapes_reserved_characters`も存在する。また`grid.rs`はハイパーリンク・埋め込み画像を一切レンダリングしないため(スタイル・罫線・結合セルのみ)、`href`/`src`属性経由のインジェクション経路自体が存在しない。
* **`markdown.rs`のファイルパス・シート名・セル値は`code_span`/フェンス幅の動的拡張により正しく保護されている**——`code_span`(`longest_backtick_run(text) + 1`本のバックティックで囲む)と、```diff```フェンス自体の幅を`body`全体の最長バックティック連続数+1(最低3)に拡張する`format_sheet_diff`のロジックの両方を確認した。Finding 1はこの保護機構自体の欠陥ではなく、この保護機構が適用され*ていない*別経路(エラーメッセージ)の存在である。
* **`diff::storage`(SQLite、`diff-storage`フィーチャー時のみ)のクエリは全てプレースホルダ経由**——`?1`・`params![..]`によるバインドのみで、文字列連結によるSQL文構築は`src/diff/storage.rs`のどこにも見つからなかった。SQLインジェクションのリスクなし。なお本フィーチャーは既定で無効であり、CLI/GitHub Actionが実際に使うコードパス(`diff_file_section_from_paths`/`grid_sections_from_paths`)からは到達しない。
* **`diff/row_alignment.rs`/`col_alignment.rs`の計算量上限が既に存在し、`best_effort.rs`が正しく段階的縮退させている**——Issue #4/#5で導入された`RowAlignmentLimits`/`ColumnAlignmentLimits`の`max_cost`超過は`Error::RowAlignmentCostTooHigh`/`ColumnAlignmentCostTooHigh`でfail-fastし、`best_effort.rs`はこれを`match`で受けて安価な方式(単純座標比較等)へ縮退させる——`unwrap`によるパニックや無条件のエラー伝播ではない。パーサー旧レビューが確立した「攻撃者制御可能な軸に上限が無い場合を疑う」という規律が、既にこの領域へも適用されていることを確認した。
* **`cli/src/main.rs`に危険な操作は無い**——argvの手動パース(`clap`等への依存を避けた設計、[cli.md](../design/cli.md)参照)のみで、シェルアウト・`eval`・動的コード実行は一切無い。ファイルパスはすべて呼び出し元(`action.yml`)が用意した信頼できる一時ファイルパスであり、攻撃者制御下のパスをそのままファイルシステム操作へ渡す経路は無い。

## 対象外

* パーサー本体(`container/`・`parse/`・`model/`・`resolve/`・`json.rs`・`pipeline.rs`・`error.rs`のバリアント定義自体・`lib.rs`)——xlsxparserと実装を共有するモジュール群であり、そちら側の`docs/security/`が既にレビュー済み。本レビューはこれらのモジュールを対象としない(ただし`Error`の各バリアントが攻撃者制御可能な文字列を保持しうるという事実そのものはFinding 1の前提として利用した)。
* `quick-xml`・`zip`・`serde`/`serde_json`・`thiserror`・`rusqlite`のサプライチェーン/依存関係脆弱性——xlsxparser側のレビュー・`cargo audit`の対象。
* `action.yml`/`release.yml`のGitHub Actions固有のセキュリティ観点(スクリプトインジェクション・サプライチェーン)は[design-review.md](design-review.md)で扱う。

## 検証方法

Finding 1は、`poc/security-review-poc/`(使い捨て、非コミット——`poc/README.md`の方針通り)に実際に`.xlsx`を構築するRustプログラムを書き、`exceldiff::diff_file_section_from_paths`(CLI/GitHub Actionが実際に呼び出す関数そのもの)へ通して得られた実際の出力を目視確認した。攻撃者が実際に用意する形——XML属性値としてwell-formedなまま`<`/`>`/改行を`&lt;`/`&gt;`/`&#10;`で表現したペイロード——を用いており、理論上の懸念ではなく実際に生成される出力で確認している。
