# `cli/` 設計書

*[English](cli.en.md)*

`cli/`(バイナリクレート `xlsxdiff`)に対応する設計書。`.github/workflows/xlsx-diff.yml` が変更PRへ投稿する`.xlsx`差分プレビューコメントを生成するための、プロセスとして起動されるCLI本体([Issue #22](https://github.com/MinamiyamaKotaro/exceldiff/issues/22)・[Issue #32](https://github.com/MinamiyamaKotaro/exceldiff/issues/32))。

元々は `examples/xlsx_diff_cli.rs` として存在し、Markdown整形ロジックの[`markdown.rs`](markdown.md)への切り出し([Issue #31](https://github.com/MinamiyamaKotaro/exceldiff/issues/31))後もパース・差分計算・`FileStatus`への詰め替えというオーケストレーションはCLI側に残っていた。Issue #32でこのオーケストレーション自体を[`markdown.rs::diff_file_section_from_paths`](markdown.md)へ集約し、CLIを「argvを5引数へ詰め替えてstdoutへ書くだけ」の薄いラッパーへ縮小した上で、独立したワークスペースメンバー `cli/` へ移動した。

## 責務・スコープ

- argv(`<display_path> <A|M|D> [base_file] [head_file]`)をパースし、[`exceldiff::diff_file_section_from_paths`](markdown.md)の引数へ詰め替えて呼び出し、返ってきたMarkdown文字列をstdoutへ書き出す(`main.rs`)
- ワークフロー側の慣習である「該当リビジョンにファイルが存在しない」ことを表す空文字列引数(`git show`が失敗した場合に空ファイルへリダイレクトする)を、`None`として`diff_file_section_from_paths`へ渡す(`.filter(|s| !s.is_empty())`)
- 引数が3個未満(プログラム名 + `display_path` + `status`)の場合、使用方法をstderrへ出力し非ゼロ終了する
- **含まない責務**: `.xlsx`のパース・差分計算・Markdown整形そのもの(すべて[`exceldiff::diff_file_section_from_paths`](markdown.md)の責務)、GitHub Actionsワークフロー自体の実装(`.github/workflows/xlsx-diff.yml`)、`cargo install`可能な形での配布やcomposite action化(別issueの検討事項。下記「`exceldiff`本体との関係」参照)

## 主要な型・関数

```rust
// cli/src/main.rs
fn main() -> std::process::ExitCode;
```

`main`一関数のみの薄いバイナリクレート。実装本体は[`cli/src/main.rs`](../../cli/src/main.rs)を参照。

## `exceldiff`本体との関係: なぜ`examples/`でも`src/bin/`でもなく独立クレートか

CLIをどこに置くかについては3案が検討された(Issue #32のPRレビュー参照):

1. **`examples/`のまま**(移行前の状態): `cargo install`や素の`cargo build`の対象外だが、`cargo build --example xlsx_diff_cli`のように明示的な指定が必要で、独立した統合テスト(`tests/`ディレクトリのようなクレートレベルのテスト)を持てない
2. **`src/bin/xlsxdiff.rs`へ昇格**([Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23)が当初想定していた案): Cargoの`autobins`既定動作により、`exceldiff`ライブラリクレート自体の`cargo install`/素の`cargo build`/`cargo package`の対象に自動的に含まれてしまう — ライブラリの公開バイナリ面を意図せず広げる
3. **独立したワークスペースメンバー`cli/`**(採用案): `exceldiff`を`path`依存として参照するだけの別パッケージなので、`exceldiff`単体に対する`cargo build`/`cargo install`/`cargo package`には一切現れない。同時に、独自の`Cargo.toml`・`tests/`ディレクトリを持てるため、`main.rs`自体のargv処理を実プロセス起動で検証する統合テスト(下記「テスト方針」)が書ける

3を採用したことで、ライブラリの公開面を汚さずに済む1の利点と、実プロセスを使った統合テストが書ける(1にはできなかった)利点を両立している。将来`cargo install xlsxdiff`や`action.yml`によるcomposite action化([Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23))を行う場合も、このクレートを起点にすればよい(`cli/Cargo.toml`の`publish = false`を外し、必要なら`crates.io`へ公開する)。

## 依存関係

- 依存先: [`exceldiff`](lib.md)(`path`依存。`diff_file_section_from_paths`・`MarkdownOptions`のみを使用)
- 依存元: `.github/workflows/xlsx-diff.yml`(`cargo build --release -p xlsxdiff`でビルドし、`target/release/xlsxdiff`をPRごとに変更された`.xlsx`ファイル1件につき1回起動して、その出力をコメント本文へ連結する)

## エラー処理方針

`main`は`ExitCode`を返す——argvの検証に失敗した場合(引数3個未満)のみ`ExitCode::FAILURE`で、使用方法をstderrへ出力する。それ以外(パースエラー・未知のgit statusを含む)は`exceldiff::diff_file_section_from_paths`が**データとして**表現し正常系のMarkdown文字列を返すため([`markdown.rs`のエラー処理方針](markdown.md)参照)、CLI自体が`panic!`したり非ゼロ終了したりすることはない——ワークフロー側が1ファイルのパースエラーで全体のコメント投稿を止めてしまわないようにするための意図的な設計。

## テスト方針

- [`markdown.rs`の単体テスト](markdown.md)が`diff_file_section_from_paths`自体の全`FileStatus`分岐(正常系・パースエラー系・リビジョン指定)をプロセス起動なしで検証済みのため、`cli/tests/cli.rs`はそれを再検証しない
- `cli/tests/cli.rs`は本クレート固有のロジック、すなわち**プロセスとして実際に起動した場合のargv処理**のみを検証する対象とする(`env!("CARGO_BIN_EXE_xlsxdiff")`経由でビルド済みバイナリを起動し、終了コード・stdout/stderrを検証):
  - 引数が3個未満の場合に使用方法がstderrへ出力され非ゼロ終了すること
  - 空文字列の`base_file`/`head_file`引数が「省略」と同一に扱われること(ワークフローが`git show`失敗時に空文字列を渡す慣習の検証)
  - 各git status(`A`/`D`/`M`/未知の文字)を渡した場合に、対応する見出しバッジが出力へ現れること(詳細な整形内容そのものは[`markdown.rs`側で検証済み](markdown.md)なので、ここでは「正しい引数が正しく渡っていること」の確認に留める)
- 実ファイルの用意には`tests/fixtures/`配下の既存フィクスチャ(`normal/basic_types.xlsx`・`other/date.xlsx`・`error/corrupted_xml.xlsx`)を、本クレートの`CARGO_MANIFEST_DIR`からの相対パスで参照する — クレートレベルの統合テストが`tests/fixtures/`を直接使う既存の慣習([`tests/error.rs`](../../tests/error.rs)等)に合わせたもので、[`src/`配下の単体テストがin-memoryなデータのみを使う慣習](markdown.md)とは異なる層であることに注意

## 未決事項 / オープンクエスチョン

1. **`cargo install`可能な形での配布・composite action化**: [Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23)が検討していたテーマ。本クレートを`crates.io`へ公開する(`publish = false`を外す)か、`action.yml`からこのリポジトリを直接ビルドして使うかは未決。
2. **バージョニング方針**: `cli/Cargo.toml`のバージョンは`exceldiff`本体とは独立に`0.1.0`から開始した。両者のバージョンを連動させる必要が生じた場合(例: `cli`が`exceldiff`の特定バージョン以降の公開APIに依存し始めた場合)は再検討する。
