# `action.yml` 設計書

*[English](action.en.md)*

リポジトリルートの `action.yml`(`runs: using: composite`)に対応する設計書。[`.github/workflows/xlsx-diff.yml`](../../.github/workflows/xlsx-diff.yml)がこの自リポジトリ専用ワークフロー内に直接書いていたステップ群を、`uses:`で外部リポジトリからも呼び出せる再利用可能なcomposite actionとして切り出したもの([Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23))。

[`cli.md`](cli.md)の未決事項1に記載の通り、当初の[Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23)は「CLIを`src/bin/xlsxdiff.rs`へ昇格した上でcomposite action化する」という案を想定していたが、実際には[Issue #31](https://github.com/MinamiyamaKotaro/exceldiff/issues/31)・[Issue #32](https://github.com/MinamiyamaKotaro/exceldiff/issues/32)で独立ワークスペースメンバー`cli/`方式が採用された。本設計はその現状を前提に、`cli/`をソースからビルドしてcomposite action化する方針を採る——`cli`クレートを`crates.io`へ公開する必要はない(下記「未決事項」参照)。

## 責務・スコープ

- Rustツールチェーンのセットアップ・`cli/`(パッケージ`xlsxdiff`)のビルド・変更された`.xlsx`ファイルごとの差分計算・Markdownコメントの投稿(または既存コメントの更新)までを、composite actionの`steps`としてカプセル化する。
- 呼び出し元のワークフローファイルが個々のステップを重複して書く必要をなくす——本リポジトリ自身の`.github/workflows/xlsx-diff.yml`も、この`action.yml`を`uses: ./`で呼び出すことでセルフドッグフーディングする(下記「テスト方針」参照)。
- **含まない責務**: `.xlsx`のパース・差分計算・Markdown整形そのもの(すべて[`exceldiff::diff_file_section_from_paths`](markdown.md)と、それを呼び出す[`cli/`](cli.md)の責務)、inputs/outputsの汎用設計([Issue #24](https://github.com/MinamiyamaKotaro/exceldiff/issues/24)のスコープ。本actionは現行`xlsx-diff.yml`と同じ固定挙動——対象は`**/*.xlsx`固定、コメントマーカーも固定——をそのまま移植するのみ)、事前ビルド済みバイナリの配布([Issue #22](https://github.com/MinamiyamaKotaro/exceldiff/issues/22)・[Issue #28](https://github.com/MinamiyamaKotaro/exceldiff/issues/28)の検討事項、P2)。

## 呼び出し元に要求する前提

composite actionは通常のワークフローjobと異なり、以下の2つを自分自身では宣言・実行できない——そのため呼び出し元のワークフロー側で用意してもらうことを前提とする(`action.yml`冒頭のコメントに同内容を明記):

- **`permissions:`ブロック**: composite actionの`action.yml`には`permissions:`キーが存在しない(ワークフロー/job単位のみで宣言可能)。呼び出し元が`permissions: pull-requests: write`を設定していない場合、後述のコメント投稿ステップはトークンの権限不足で失敗する。
- **チェックアウト**: composite actionは呼び出し元リポジトリを自動でcheckoutしない。差分計算ステップは`git show <sha>:<path>`でPRのbase/head双方のリビジョンを参照するため、呼び出し元が`actions/checkout@v4`を`fetch-depth: 0`付きで事前に実行している必要がある(shallow checkoutだとマージコミット以外のリビジョンが存在しない)。

いずれも本actionが`pull_request`イベント専用(差分計算ステップが`github.event.pull_request.base.sha`/`head.sha`を参照する)であることの帰結でもある——`workflow_dispatch`等の他イベントから呼び出しても意味のある結果は得られない。

## 主要な構造

```yaml
# action.yml
inputs:
  github-token:  # 既定値 ${{ github.token }}
runs:
  using: composite
  steps:
    - dtolnay/rust-toolchain@stable
    - Swatinem/rust-cache@v2         # workspaces: 本action自身のパス起点
    - cargo build --release -p xlsxdiff --manifest-path ...
    - 変更された.xlsxファイルごとにgit show + xlsxdiffを実行しMarkdownを組み立て
    - peter-evans/find-comment@v3
    - peter-evans/create-or-update-comment@v4
```

実装本体は[`action.yml`](../../action.yml)を参照。

## 設計上の要点(呼び出し元とアクション自身のディレクトリが分離される影響)

`uses: owner/repo@ref`でこのactionを外部リポジトリから呼び出した場合、GitHub Actionsランナーは本actionのリポジトリを、呼び出し元のワークフローが`actions/checkout`済みのディレクトリ(呼び出し元の`$PWD`/`GITHUB_WORKSPACE`)とは**別の**ディレクトリへ取得する。そのパスは`${{ github.action_path }}`コンテキストで参照できる。この分離が以下2箇所に影響する:

1. **ビルド生成物の参照パス**: `cargo build --manifest-path "${{ github.action_path }}/Cargo.toml"`のように`--manifest-path`を明示すると、Cargoの既定`target`ディレクトリは(CWDではなく)そのマニフェストが属するワークスペースのルート——すなわち`${{ github.action_path }}`——直下に作られる(`CARGO_TARGET_DIR`環境変数や`.cargo/config.toml`の`build.target-dir`で上書きしない限り)。そのため後続ステップがビルド済みバイナリを参照する際も`${{ github.action_path }}/target/release/xlsxdiff`を起点にする必要がある——呼び出し元の`target/`とは無関係な別ディレクトリになる(副次的な利点として、呼び出し元自身がRustリポジトリであっても`target/`を汚染・競合させない)。
2. **`Swatinem/rust-cache`のワークスペース指定**: `workspaces`入力の既定値は`". -> target"`で、`.`は呼び出し元のリポジトリルート(`GITHUB_WORKSPACE`)を指す。これは本action自身の`Cargo.lock`とは無関係な場所であり、既定のままだと(呼び出し元が非Rustリポジトリの場合は特に)`Cargo.lock`が見つからずキャッシュが効かない。`workspaces: "${{ github.action_path }} -> target"`と明示することで、本action自身のワークスペース(構文は`$workspace -> $target`、`$target`省略時は`target`)を基準にキャッシュさせる。

そのほかの調整点:

- 一時的なコメント本文ファイルは、呼び出し元の作業ツリーを汚さないよう`${{ runner.temp }}`(ランナーがjobごとに用意する一時ディレクトリ)配下に書き出す。
- コメント投稿に使うトークンは`github-token`入力として公開し、既定値は`${{ github.token }}`(呼び出し元workflowのGITHUB_TOKENをそのまま使う)とする。呼び出し元がカスタムPATを使いたい場合はこの入力を上書きできる。`peter-evans/find-comment`・`peter-evans/create-or-update-comment`双方とも同名の`token`入力(既定値`${{ github.token }}`)を持つため、そこへそのまま渡す。

## 依存関係

- 依存先: [`cli/`](cli.md)(`cargo build -p xlsxdiff`でビルドし、`xlsxdiff`バイナリを1PR差分ファイルにつき1回起動する。本action自身は`cli/`のargv契約——`<display_path> <A|M|D> [base_file] [head_file]`——を変更しない)
- 依存元: [`.github/workflows/xlsx-diff.yml`](../../.github/workflows/xlsx-diff.yml)(本リポジトリ自身が`uses: ./`で参照する唯一の呼び出し元。将来、外部リポジトリが`uses: MinamiyamaKotaro/exceldiff@<tag>`で参照することも想定するが、現時点でそのような外部呼び出し元は存在しない)

## エラー処理方針

`cli/`側([`main`のエラー処理方針](cli.md)参照)と同じく、1ファイルのパースエラーが全体のコメント投稿を止めないことを前提にした設計を踏襲する——本action自体はビルド失敗以外で明示的に失敗させる箇所を持たない。フォークからのPRではGitHub Actionsの仕様により`GITHUB_TOKEN`が読み取り専用に強制されるため、コメント投稿ステップは黙って失敗する(`action.yml`内のコメントに明記。何かに依存されるステップではないため、job全体の失敗にはつながる可能性がある——`peter-evans/*`アクション自体が非ゼロ終了する場合、後続ステップがない本jobではそのまま失敗として報告される。これは移植元の`xlsx-diff.yml`から変わらない既存の挙動)。

## テスト方針

composite actionはYAML定義であり`cargo test`の対象にならないため、以下の方法で検証する:

1. **静的検証**: `action.yml`はYAMLとして構文検証する(`actionlint`は`.github/workflows/`配下のワークフローファイルのみを対象とし`action.yml`形式のcomposite actionメタデータには非対応のため、Python `yaml.safe_load`等で構文のみ検証)。呼び出し元ワークフロー側(`.github/workflows/xlsx-diff.yml`)は`actionlint`でも検証可能。
2. **シェルロジックの単体検証**: 「変更された`.xlsx`ファイルごとに`git show`でbase/headを取り出し`xlsxdiff`を起動してMarkdownへ連結する」というシェルスクリプト部分は`${{ github.action_path }}`・`${{ runner.temp }}`をローカルパスに置き換えれば`bash`だけでそのまま実行できる。実際に、ローカルの使い捨てgitリポジトリへA(追加)・M(変更)・D(削除)の3ステータスが混在する差分を作り、このスクリプトをそのまま実行して意図通りのMarkdownが生成されることを確認済み。
3. **実際のGitHub Actions上での結合検証**: `.github/workflows/xlsx-diff.yml`自体を`uses: ./`で本actionを呼び出す形に書き換えた(下記「依存関係」参照)。これにより、`.xlsx`ファイルを変更する今後の任意のPRが本action全体(Rustツールチェーンのセットアップ・`github.action_path`起点でのビルド・`rust-cache`のワークスペース指定・コメント投稿)の実行結果を検証する回帰テストとして機能する——外部のテスト用リポジトリを別途用意しなくても、本リポジトリ自身がdogfoodingの場になる。

## 未決事項 / オープンクエスチョン

1. **`cli`クレートの`crates.io`公開**: 本actionは`cli/`をソースからビルドする方式を採用しており、`cli/Cargo.toml`の`publish = false`は変更していない。公開する実利(例: 呼び出し元でのビルド時間短縮のため事前ビルド済みバイナリを配布する、[Issue #22](https://github.com/MinamiyamaKotaro/exceldiff/issues/22)・[Issue #28](https://github.com/MinamiyamaKotaro/exceldiff/issues/28))が生じるまでは現状維持とする。
2. **inputs/outputsの汎用化**: 対象パス(現状`**/*.xlsx`固定)やコメントの文言・マーカーのカスタマイズは[Issue #24](https://github.com/MinamiyamaKotaro/exceldiff/issues/24)のスコープ。本actionは現行workflowと同じ固定挙動の移植に留めている。
3. **外部リポジトリからの実地検証**: 本設計時点ではセルフドッグフーディング(`uses: ./`)のみで検証しており、実際に別リポジトリから`uses: MinamiyamaKotaro/exceldiff@<tag>`で参照した動作確認はまだ行っていない。タグ付けされたリリースを用意した上で実施する。
