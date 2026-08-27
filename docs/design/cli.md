# `cli/` 設計書

*[English](cli.en.md)*

`cli/`(バイナリクレート `xlsxdiff`)に対応する設計書。`.github/workflows/xlsx-diff.yml` が変更PRへ投稿する`.xlsx`差分プレビューコメントを生成するための、プロセスとして起動されるCLI本体([Issue #22](https://github.com/MinamiyamaKotaro/exceldiff/issues/22)・[Issue #32](https://github.com/MinamiyamaKotaro/exceldiff/issues/32))。

元々は `examples/xlsx_diff_cli.rs` として存在し、Markdown整形ロジックの[`markdown.rs`](markdown.md)への切り出し([Issue #31](https://github.com/MinamiyamaKotaro/exceldiff/issues/31))後もパース・差分計算・`FileStatus`への詰め替えというオーケストレーションはCLI側に残っていた。Issue #32でこのオーケストレーション自体を[`markdown.rs::diff_file_section_from_paths`](markdown.md)へ集約し、CLIを「argvを5引数へ詰め替えてstdoutへ書くだけ」の薄いラッパーへ縮小した上で、独立したワークスペースメンバー `cli/` へ移動した。

## 責務・スコープ

- argv(`[--max-rows-per-sheet <N>] [--diff-mode <auto|coordinate>] [--grid-html-dir <dir>] <display_path> <A|M|D> [base_file] [head_file]`)をパースし、[`exceldiff::diff_file_section_from_paths`](markdown.md)の引数(`display_path`/`status`/`base_path`/`head_path`/`MarkdownOptions`)へ詰め替えて呼び出し、返ってきたMarkdown文字列をstdoutへ書き出す(`main.rs`)
- 先頭の`--max-rows-per-sheet <N>`/`--diff-mode <auto|coordinate>`/`--grid-html-dir <dir>`(いずれも省略可・順不同ではなく先頭からの`--flag value`ペアとして認識)を`MarkdownOptions::max_rows_per_sheet`/`diff_mode`、および[`exceldiff::grid_sections_from_paths`](grid.md)呼び出しへ反映する([`action.yml`の同名inputs](action.md)を橋渡しするため、[Issue #24](https://github.com/MinamiyamaKotaro/exceldiff/issues/24))。値が不正(`--max-rows-per-sheet`が数値でない、`--diff-mode`が`auto`/`coordinate`のいずれでもない、いずれかのフラグに値が続かない)な場合は使用方法をstderrへ出力し非ゼロ終了する——`clap`等の引数解析クレートは追加せず(下記「`exceldiff`本体との関係」参照)、`parse_options`という手書きのループで完結させている
- `--grid-html-dir <dir>`が指定された場合、標準出力のMarkdown契約には一切影響を与えずに、変更のあったシートごとの独立したHTMLページ(`<dir>/sheet-{N}.html`、`exceldiff::wrap_grid_page`でラップ)と、`sheet_name\thtml_path`形式のTSV一覧(`<dir>/manifest.tsv`、`git diff --name-status`と同じTSV慣習)を書き出す(`write_grid_sections`)。グリッド生成に失敗しても(不正なパス等)stderrへ警告を出すのみで、プロセス自体は正常終了する——標準出力のMarkdownがこのバイナリの主目的であり、`--grid-html-dir`はその上に乗るベストエフォートの追加出力という位置づけ
- ワークフロー側の慣習である「該当リビジョンにファイルが存在しない」ことを表す空文字列引数を、`None`として`diff_file_section_from_paths`へ渡す(`.filter(|s| !s.is_empty())`)——`.github/workflows/xlsx-diff.yml`は`base_file`/`head_file`をまず空文字列で初期化し、該当しない側(例: `A`の`base_file`、`D`の`head_file`)は`git show`自体を一切実行せず空文字列のまま`xlsxdiff`へ渡す。`git show`が失敗した場合の空ファイルへのフォールバックではない
- フラグ解析後の位置引数が3個未満(`display_path` + `status`)の場合、使用方法をstderrへ出力し非ゼロ終了する
- **含まない責務**: `.xlsx`のパース・差分計算・Markdown整形そのもの(すべて[`exceldiff::diff_file_section_from_paths`](markdown.md)の責務)、GitHub Actionsワークフロー自体の実装(`.github/workflows/xlsx-diff.yml`・[`action.yml`](action.md))、`cargo install`可能な形での配布(下記「`exceldiff`本体との関係」参照)

## 主要な型・関数

```rust
// cli/src/main.rs
struct Options {
    markdown: MarkdownOptions,
    grid_html_dir: Option<String>,
}
fn parse_options(args: &[String]) -> Option<(Options, &[String])>;
fn write_grid_sections(
    dir: &str,
    status: &str,
    base_path: Option<&str>,
    head_path: Option<&str>,
    diff_mode: DiffMode,
) -> std::io::Result<()>;
fn main() -> std::process::ExitCode;
```

`parse_options`が3つの`--`フラグを消費して`Options`と残りの位置引数を返し(不正な値は`None`)、`main`がそれを使って`diff_file_section_from_paths`(常時)と`write_grid_sections`(`--grid-html-dir`指定時のみ)を呼ぶ、という構成。実装本体は[`cli/src/main.rs`](../../cli/src/main.rs)を参照。

## `exceldiff`本体との関係: なぜ`examples/`でも`src/bin/`でもなく独立クレートか

CLIをどこに置くかについては3案が検討された(Issue #32のPRレビュー参照):

1. **`examples/`のまま**(移行前の状態): `cargo install`や素の`cargo build`の対象外だが、`cargo build --example xlsx_diff_cli`のように明示的な指定が必要で、独立した統合テスト(`tests/`ディレクトリのようなクレートレベルのテスト)を持てない
2. **`src/bin/xlsxdiff.rs`へ昇格**([Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23)が当初想定していた案): Cargoの`autobins`既定動作により、`exceldiff`ライブラリクレート自体の`cargo install`/素の`cargo build`/`cargo package`の対象に自動的に含まれてしまう — ライブラリの公開バイナリ面を意図せず広げる
3. **独立したワークスペースメンバー`cli/`**(採用案): `exceldiff`を`path`依存として参照するだけの別パッケージなので、`exceldiff`単体に対する`cargo build`/`cargo install`/`cargo package`には一切現れない。同時に、独自の`Cargo.toml`・`tests/`ディレクトリを持てるため、`main.rs`自体のargv処理を実プロセス起動で検証する統合テスト(下記「テスト方針」)が書ける

3を採用したことで、ライブラリの公開面を汚さずに済む1の利点と、実プロセスを使った統合テストが書ける(1にはできなかった)利点を両立している。[Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23)でのcomposite action化([`action.yml`設計書](action.md)参照)もこのクレートを起点にソースからビルドする方式を採用しており、`cli/Cargo.toml`の`publish = false`は変更していない。

## 依存関係

- 依存先(通常): [`exceldiff`](lib.md)(`path`依存。`diff_file_section_from_paths`・`MarkdownOptions`・[`grid_sections_from_paths`/`wrap_grid_page`](grid.md)・`DiffMode`を使用)
- 依存先(devのみ): `zip`(`cli/tests/cli.rs`が制御されたテスト用`.xlsx`ペアをin-memoryで組み立てるためだけに使用。下記「テスト方針」参照。バイナリ本体には含まれない)
- 依存元: [`action.yml`](action.md)(`cargo build --release -p xlsxdiff`でビルドし、`target/release/xlsxdiff`をPRごとに変更された`.xlsx`ファイル1件につき1回起動して、その出力をコメント本文へ連結する。`visual: true`の場合は`--grid-html-dir`も渡し、書き出されたHTMLをそのままworkflow artifactへ添付する)。[`.github/workflows/xlsx-diff.yml`](../../.github/workflows/xlsx-diff.yml)は`action.yml`を`uses: ./`で呼び出す薄いワークフローになっており、本クレートを直接ビルドしない

## エラー処理方針

`main`は`ExitCode`を返す——argvの検証に失敗した場合(フラグの値が不正、または位置引数が2個未満)のみ`ExitCode::FAILURE`で、使用方法をstderrへ出力する。それ以外(パースエラー・未知のgit statusを含む)は`exceldiff::diff_file_section_from_paths`が**データとして**表現し正常系のMarkdown文字列を返すため([`markdown.rs`のエラー処理方針](markdown.md)参照)、CLI自体が`panic!`したり非ゼロ終了したりすることはない——ワークフロー側が1ファイルのパースエラーで全体のコメント投稿を止めてしまわないようにするための意図的な設計。`write_grid_sections`(`--grid-html-dir`指定時)の失敗は`main`側でstderrへ警告するのみで、こちらも`ExitCode::FAILURE`にはつながらない——標準出力のMarkdownは既に書き終えており、それこそが失われてはならない主要な成果物であるため。

## テスト方針

- [`markdown.rs`の単体テスト](markdown.md)が`diff_file_section_from_paths`自体の全`FileStatus`分岐(正常系・パースエラー系・リビジョン指定)をプロセス起動なしで検証済みのため、`cli/tests/cli.rs`はそれを再検証しない
- `cli/tests/cli.rs`は本クレート固有のロジック、すなわち**プロセスとして実際に起動した場合のargv処理**のみを検証する対象とする(`env!("CARGO_BIN_EXE_xlsxdiff")`経由でビルド済みバイナリを起動し、終了コード・stdout/stderrを検証):
  - 引数が3個未満の場合に使用方法がstderrへ出力され非ゼロ終了すること
  - 空文字列の`base_file`/`head_file`引数が「省略」と同一に扱われること(ワークフローが該当しない側の引数として空文字列をそのまま渡す慣習の検証。上記「責務・スコープ」参照)
  - 各git status(`A`/`D`/`M`/未知の文字)を渡した場合に、対応する見出しバッジが出力へ現れること(詳細な整形内容そのものは[`markdown.rs`側で検証済み](markdown.md)なので、ここでは「正しい引数が正しく渡っていること」の確認に留める)
  - `--max-rows-per-sheet`/`--diff-mode`が実際に`MarkdownOptions`へ届き出力を変えること、および不正な値(非数値・未知のモード名)が使用方法エラーになること(フラグの意味そのもの——`max_rows_per_sheet`の上限計算・`DiffMode`ごとの差分計算アルゴリズムの違い——は[`markdown.rs`側で単体テスト済み](markdown.md)なので、ここでは argv からの橋渡しが正しいことのみ確認する)
  - `--grid-html-dir`が指定された場合に`manifest.tsv`とシートごとのHTMLファイルが実際に書き出されること、変更が無い場合は`manifest.tsv`が空になること、フラグに値が続かない(位置引数の直前で終わる)場合も他の2フラグと同じく使用方法エラーになること
- 実ファイルが必要なテスト(単純に「実在する`.xlsx`を渡すと正常/エラーとして処理される」ことの確認)には`tests/fixtures/`配下の既存フィクスチャ(`normal/basic_types.xlsx`・`error/corrupted_xml.xlsx`)を、本クレートの`CARGO_MANIFEST_DIR`からの相対パスで参照する — クレートレベルの統合テストが`tests/fixtures/`を直接使う既存の慣習([`tests/error.rs`](../../tests/error.rs)等)に合わせたもの
- 一方、「値が変更された1セルが`@@`ハンクとして出力へ現れること」を確認するテストは、`tests/fixtures/`配下の無関係な2ファイルではなく、`cli/tests/cli.rs`内で最小限の`.xlsx`ペア(A1セルの値だけが違う)をin-memoryで組み立てる(`zip`クレートをdev依存として使用)。2つの無関係な実ファイルのどのセルがどう違うかはテストが制御・保証できる性質のものではなく、実際に依存関係の解決結果が変わった際にCI上でのみ差分内容が変わって失敗する事例が起きたため([`tests/fixtures/diff.rs`の`cell_modified()`](../../tests/fixtures/diff.rs)や[`src/markdown.rs`の単体テスト](markdown.md)が同じ理由でin-memory構築を採用しているのと同じ判断)

## 未決事項 / オープンクエスチョン

1. **`cargo install`可能な形での配布・composite action化**: [Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23)が検討していたテーマ。composite action化そのものは[`action.yml`](action.md)として実装済み(このリポジトリをソースから直接ビルドする方式)。本クレートを`crates.io`へ公開するか(`publish = false`を外すか)は、事前ビルド済みバイナリ配布([Issue #22](https://github.com/MinamiyamaKotaro/exceldiff/issues/22)・[Issue #28](https://github.com/MinamiyamaKotaro/exceldiff/issues/28))が必要になるまで未決のまま。
2. **バージョニング方針**: `cli/Cargo.toml`のバージョンは`exceldiff`本体とは独立に`0.1.0`から開始した。両者のバージョンを連動させる必要が生じた場合(例: `cli`が`exceldiff`の特定バージョン以降の公開APIに依存し始めた場合)は再検討する。
