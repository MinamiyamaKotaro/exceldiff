# `action.yml` 設計書

*[English](action.en.md)*

リポジトリルートの `action.yml`(`runs: using: composite`)に対応する設計書。[`.github/workflows/xlsx-diff.yml`](../../.github/workflows/xlsx-diff.yml)がこの自リポジトリ専用ワークフロー内に直接書いていたステップ群を、`uses:`で外部リポジトリからも呼び出せる再利用可能なcomposite actionとして切り出したもの([Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23))。

[`cli.md`](cli.md)の未決事項1に記載の通り、当初の[Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23)は「CLIを`src/bin/xlsxdiff.rs`へ昇格した上でcomposite action化する」という案を想定していたが、実際には[Issue #31](https://github.com/MinamiyamaKotaro/exceldiff/issues/31)・[Issue #32](https://github.com/MinamiyamaKotaro/exceldiff/issues/32)で独立ワークスペースメンバー`cli/`方式が採用された。本設計はその現状を前提に、`cli/`をソースからビルドしてcomposite action化する方針を採る——`cli`クレートを`crates.io`へ公開する必要はない(下記「未決事項」参照)。

## 責務・スコープ

- `xlsxdiff`バイナリの解決(事前ビルド済みリリースの取得、ダウンロードできない場合のみRustツールチェーンのセットアップ+`cli/`ビルドへフォールバック——[Issue #28](https://github.com/MinamiyamaKotaro/exceldiff/issues/28)、下記「事前ビルド済みバイナリ配布」参照)・変更された`.xlsx`ファイルごとの差分計算・Markdownコメントの投稿(または既存コメントの更新)までを、composite actionの`steps`としてカプセル化する。
- 呼び出し元のワークフローファイルが個々のステップを重複して書く必要をなくす——本リポジトリ自身の`.github/workflows/xlsx-diff.yml`も、この`action.yml`を`uses: ./`で呼び出すことでセルフドッグフーディングする(下記「テスト方針」参照)。
- `visual: true`の場合、変更のあったシートごとに[`grid.rs`が生成するExcelライクなグリッドHTML](grid.md)ページを収集し、1回のjob実行分をまとめて1つのGitHub Actions artifact(`actions/upload-artifact@v4`)としてアップロードして、そのダウンロードリンクをテキスト差分の下に追記する(下記「ビジュアルモード」参照)。実際にHTMLを収集・公開する処理そのものは[`grid.rs`の責務には含まれていない](grid.md)ため、この配線は本actionが担う。
- **含まない責務**: `.xlsx`のパース・差分計算・Markdown整形・グリッドHTML生成そのもの(すべて[`exceldiff::diff_file_section_from_paths`](markdown.md)/[`grid_sections_from_paths`](grid.md)と、それを呼び出す[`cli/`](cli.md)の責務)、変更セル数の合計を返すoutput(`changed-cells-count`、[Issue #24](https://github.com/MinamiyamaKotaro/exceldiff/issues/24)で後続タスクとして切り出し済み、[Issue #43](https://github.com/MinamiyamaKotaro/exceldiff/issues/43)未着手分。下記「未決事項」参照)。コミット単位での差分表示は`diff-scope: commit`として実装済み(下記「inputs / outputs」)。事前ビルド済みバイナリの配布は`release.yml`+`action.yml`の「Resolve xlsxdiff binary」ステップとして実装済み(下記「事前ビルド済みバイナリ配布」参照)。

## inputs / outputs([Issue #24](https://github.com/MinamiyamaKotaro/exceldiff/issues/24))

| input | 型/既定値 | 内容 |
|---|---|---|
| `github-token` | string, `${{ github.token }}` | コメント投稿に使うトークン |
| `files` | string, `*.xlsx` | `git diff -- <files>`へそのまま渡す**gitパススペック**(シェルグロブではない。既定値は先頭に`**/`が無くても任意の深さのパスにマッチする) |
| `comment` | bool文字列, `'true'` | PRコメントとして投稿するか |
| `job-summary` | bool文字列, `'false'` | `$GITHUB_STEP_SUMMARY`へも書き出すか。`comment`と独立に指定可能——forkからのPR等`pull-requests: write`権限を付与できない環境では`comment: false`・`job-summary: true`にすることで、権限エラーを起こさずJob Summary上で差分を確認できる |
| `max-rows-per-sheet` | 数値文字列, `'30'` | [`MarkdownOptions::max_rows_per_sheet`](markdown.md)へ渡す(`cli/`の`--max-rows-per-sheet`フラグ経由) |
| `diff-mode` | string enum `auto`\|`coordinate`, `'auto'` | [`MarkdownOptions::diff_mode`](markdown.md)へ渡す(`cli/`の`--diff-mode`フラグ経由)。`auto`は現行の`diff_workbooks_best_effort`(座標一致/行アライメント/列アライメント自動選択)、`coordinate`はアライメント検出をスキップした単純座標比較 |
| `diff-scope` | string enum `pr`\|`commit`, `'pr'` | `pr`(既定)はPRの累積`base.sha`⇔`head.sha`差分を1ファイル1セクションとして出力する従来通りの挙動。`commit`はPRが導入した各コミットを`git log --reverse base.sha..head.sha`で列挙し、コミットごとに直前の親(`<commit>^1`)との差分を`## Commit <short-sha> — <subject>`見出しの下にサブセクションとして出力する——同一PR内で新規追加されたファイルへの修正が常に`Added`としてしか見えない問題([Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23))を解消する([Issue #43](https://github.com/MinamiyamaKotaro/exceldiff/issues/43))。`has-changes`/`changed-files-count`outputsへの影響はなく、常に累積diffベースのまま。`visual: true`と併用した場合、グリッドHTMLの保存先も`<commit-short-sha>/`でネームスペース分けされる(下記「ビジュアルモード」参照)。`push`(直前pushの`before`/`after`)モードは未実装(下記「未決事項」) |
| `visual` | bool文字列, `'false'` | 変更のあったシートごとにExcelライクなグリッドのビューを単体HTMLページとして生成し、workflow artifactとして添付(コメントにはダウンロードリンクを掲載)するか。追加の`permissions:`は不要(下記「呼び出し元に要求する前提」) |

| output | 型 | 内容 |
|---|---|---|
| `has-changes` | bool文字列 | `files`にマッチするファイルがPR内で変更されたか |
| `changed-files-count` | 数値文字列 | `files`にマッチする変更ファイル数 |

`has-changes`/`changed-files-count`はいずれも`git diff --name-status`の結果(`$changed`)だけで計算できるため、`cli/`側の変更なしに「差分計算」ステップ(`id: diff`)内で`$GITHUB_OUTPUT`へ直接書き出している。変更セル数の合計(`changed-cells-count`)は`xlsxdiff`が現状Markdown文字列しか返さないため機械可読な集計値を持たず、実現するには`cli/`側の変更が別途必要(下記「未決事項」参照)。

## 呼び出し元に要求する前提

composite actionは通常のワークフローjobと異なり、以下の2つを自分自身では宣言・実行できない——そのため呼び出し元のワークフロー側で用意してもらうことを前提とする(`action.yml`冒頭のコメントに同内容を明記):

- **`permissions:`ブロック**: composite actionの`action.yml`には`permissions:`キーが存在しない(ワークフロー/job単位のみで宣言可能)。
  - **`contents: read`は`comment`/`visual`の設定に関わらず常に必要**——`actions/checkout@v4`自体がリポジトリを取得するのに使う権限であり、これが無いとチェックアウト自体が失敗する。GitHub Actionsは`permissions:`ブロックを一つでも書くと、そこに列挙しなかった全スコープを(リポジトリの既定値ではなく)`none`にする仕様のため、呼び出し元が`permissions: pull-requests: write`だけを書くと`contents`が黙って`none`になり、`actions/checkout`が汎用的な「repository not found」エラーで失敗する——実際に外部リポジトリから`uses:`で呼び出す検証([Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23))で踏んだ不具合で、原因が権限エラーだと気づきにくい形で顕在化する。
  - `comment`入力が既定の`true`のままの場合、呼び出し元が`permissions: pull-requests: write`を設定していないと、後述のコメント投稿ステップはトークンの権限不足で失敗する(`comment: false`・`job-summary: true`にすればこの権限は不要——上記「inputs / outputs」参照)。
  - `visual: true`を使う場合も追加の`permissions:`は不要——スクリーンショットは`actions/upload-artifact@v4`でworkflow artifactとしてアップロードするのみで、`GITHUB_TOKEN`の権限モデルとは別の`ACTIONS_RUNTIME_TOKEN`で認可されるため(下記「ビジュアルモード」参照)。
- **チェックアウト**: composite actionは呼び出し元リポジトリを自動でcheckoutしない。差分計算ステップは`git show <sha>:<path>`でPRのbase/head双方のリビジョンを参照するため、呼び出し元が`actions/checkout@v4`を`fetch-depth: 0`付きで事前に実行している必要がある(shallow checkoutだとマージコミット以外のリビジョンが存在しない)。

いずれも本actionが`pull_request`イベント専用(差分計算ステップが`github.event.pull_request.base.sha`/`head.sha`を参照する)であることの帰結でもある——`workflow_dispatch`等の他イベントから呼び出しても意味のある結果は得られない。

## ブランディング・Marketplaceカテゴリ([Issue #27](https://github.com/MinamiyamaKotaro/exceldiff/issues/27))

`action.yml`の`branding`は`icon: grid`(Feather v4.28.0由来——本actionの`visual: true`モードが実際にExcelライクな「グリッド」のスクリーンショットを生成すること([`grid.rs`](grid.md))に対応させた)・`color: green`(製品判断として決定)とした。

Marketplace掲載時のカテゴリはリポジトリ内のファイルに保存する field が無く(掲載UI側で都度選択する)ため、ここに決定事項として記録する: プライマリカテゴリは**Code review**(PRに差分プレビューコメントを投稿するのが主機能のため)、セカンダリカテゴリは**Utilities**とする。

## 主要な構造

```yaml
# action.yml
inputs:
  github-token:        # 既定値 ${{ github.token }}
  files:                # 既定値 '*.xlsx'（gitパススペック）
  comment:               # 既定値 'true'
  job-summary:            # 既定値 'false'
  max-rows-per-sheet:      # 既定値 '30'
  diff-mode:                # 既定値 'auto'
  diff-scope:                # 既定値 'pr'（'pr' | 'commit'）
  visual:                    # 既定値 'false'
outputs:
  has-changes:            # steps.diff.outputs.has-changes
  changed-files-count:   # steps.diff.outputs.changed-files-count
runs:
  using: composite
  steps:
    - id: resolve_binary     # github.action_refが空でなければ対応ターゲットの
                              # 事前ビルド済みリリースバイナリをダウンロード+チェックサム検証。
                              # 成功すればfound=true・bin-pathを出力（Issue #28）
    - if: steps.resolve_binary.outputs.found != 'true'
      uses: dtolnay/rust-toolchain@stable
    - if: steps.resolve_binary.outputs.found != 'true'
      uses: Swatinem/rust-cache@v2   # workspaces: 本action自身のパス起点
    - id: build_fallback      # found != true の場合のみ: cargo build --release -p xlsxdiff
      if: steps.resolve_binary.outputs.found != 'true'
    - id: diff              # BIN = resolve_binary（成功時）または build_fallback（フォールバック時）のbin-path。
                              # has-changes/changed-files-countは常に累積base..headから$GITHUB_OUTPUTへ書き出す。
                              # diff-scope: pr（既定）なら変更ファイルごとに、
                              # diff-scope: commitならPRの各コミット×そのコミットで変更されたファイルごとに
                              # git show + xlsxdiffを実行しMarkdownを組み立てる（commitモードは
                              # "## Commit <short-sha> — <subject>" 見出しでサブセクション化）。
                              # visual: trueならシートごとの単体HTMLページを
                              # ${{ runner.temp }}/xlsx-diff-visuals/へ（commitモードは
                              # commit-short-sha/ でネームスペース分けして）集約し、
                              # has-visualsを$GITHUB_OUTPUTへ書き出す
    - if: inputs.visual && steps.diff.outputs.has-visuals == 'true'
      id: upload_visuals     # actions/upload-artifact@v4 でxlsx-diff-visuals/を1つのartifactへ
    - if: inputs.visual && steps.diff.outputs.has-visuals == 'true'
                              # upload_visuals.outputs.artifact-url をコメントMarkdownの末尾へ追記
    - if: inputs.job-summary # $GITHUB_STEP_SUMMARYへ書き出す
    - if: inputs.comment
      uses: peter-evans/find-comment@v3
    - if: inputs.comment
      uses: peter-evans/create-or-update-comment@v4
```

実装本体は[`action.yml`](../../action.yml)を参照。

## 設計上の要点(呼び出し元とアクション自身のディレクトリが分離される影響)

`uses: owner/repo@ref`でこのactionを外部リポジトリから呼び出した場合、GitHub Actionsランナーは本actionのリポジトリを、呼び出し元のワークフローが`actions/checkout`済みのディレクトリ(呼び出し元の`$PWD`/`GITHUB_WORKSPACE`)とは**別の**ディレクトリへ取得する。そのパスは`${{ github.action_path }}`コンテキストで参照できる。この分離が以下2箇所に影響する:

1. **ビルド生成物の参照パス**: `cargo build --manifest-path "${{ github.action_path }}/Cargo.toml"`のように`--manifest-path`を明示すると、Cargoの既定`target`ディレクトリは(CWDではなく)そのマニフェストが属するワークスペースのルート——すなわち`${{ github.action_path }}`——直下に作られる(`CARGO_TARGET_DIR`環境変数や`.cargo/config.toml`の`build.target-dir`で上書きしない限り)。そのため後続ステップがビルド済みバイナリを参照する際も`${{ github.action_path }}/target/release/xlsxdiff`を起点にする必要がある——呼び出し元の`target/`とは無関係な別ディレクトリになる(副次的な利点として、呼び出し元自身がRustリポジトリであっても`target/`を汚染・競合させない)。
2. **`Swatinem/rust-cache`のワークスペース指定**: `workspaces`入力の既定値は`". -> target"`で、`.`は呼び出し元のリポジトリルート(`GITHUB_WORKSPACE`)を指す。これは本action自身の`Cargo.lock`とは無関係な場所であり、既定のままだと(呼び出し元が非Rustリポジトリの場合は特に)`Cargo.lock`が見つからずキャッシュが効かない。`workspaces: "${{ github.action_path }} -> target"`と明示することで、本action自身のワークスペース(構文は`$workspace -> $target`、`$target`省略時は`target`)を基準にキャッシュさせる。

そのほかの調整点:

- 一時的なコメント本文ファイルは、呼び出し元の作業ツリーを汚さないよう`${{ runner.temp }}`(ランナーがjobごとに用意する一時ディレクトリ)配下に書き出す。
- コメント投稿に使うトークンは`github-token`入力として公開し、既定値は`${{ github.token }}`(呼び出し元workflowのGITHUB_TOKENをそのまま使う)とする。呼び出し元がカスタムPATを使いたい場合はこの入力を上書きできる。`peter-evans/find-comment`・`peter-evans/create-or-update-comment`双方とも同名の`token`入力(既定値`${{ github.token }}`)を持つため、そこへそのまま渡す。

### 事後発見: `while read ... done <<< "$var"`をスクリプト末尾に置くと`set -e`下で意図せず失敗しうる

[Issue #24](https://github.com/MinamiyamaKotaro/exceldiff/issues/24)実装(PR #38)のライブ検証中に発見・修正した問題。「差分計算」ステップの末尾は`if [ -z "$changed" ]; ... else; while IFS=$'\t' read -r file_status path; do ...; done <<< "$changed"; fi`という構造で、変更ファイルが**1件だけ**の場合にステップが`Process completed with exit code 1`として失敗することがあった(GitHub Actions実機上でのみ、複数回連続で再現。ローカルの`bash`では再現せず)——`while`ループがスクリプト内の最後の文であり、ループ終端(herestringを読み切った`read`の非ゼロ終了)がそのままスクリプト自体の終了コードとして扱われてしまう経路があったとみられる。ループの内容自体は正しく実行され、`$OUT`への書き込みも完了していたにもかかわらず、ステップ全体が失敗として報告されていた。

対処として、`if`/`fi`ブロックの直後に副作用のない`:`(no-op、常に終了コード0)を明示的なスクリプト末尾として追加した——ループの終端コードにスクリプト全体の終了コードが依存しないようにする、という一般的なシェルスクリプトの防御策。ローカルの`bash 3.2`では元々このパターンでも問題が再現しなかった(POSIX的にはループの終了コードはループ**本体**の最後のコマンドのものであり、ループ条件のread自体の失敗ではないはず)ため、GitHub Actionsランナー側の`bash`(Ubuntu、5系)固有の挙動である可能性が高いが、根本原因を完全には特定できていない——`:`追加によりCI上で3回連続の成功を確認済みで、実用上はこれで十分と判断した。

この経緯は、単一ファイルのみ変更されたPR(実運用で最も頻度が高いケース)に対して本actionが機能しなくなるリスクだった、という点で重要——複数ファイルの差分でのテストだけでは検出できなかった([Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23)のセルフドッグフーディング時は複数ファイル変更や偶然2ファイル以上のケースが多く、単一ファイル変更のケースを明示的に検証していなかった)。

## ビジュアルモード(`visual: true`)の設計

GitHubのPRコメントはHTML内の`style=`属性をサニタイズするため、[`grid.rs`](grid.md)が生成する色付き・罫線付きのグリッドHTMLをそのままコメント本文へ貼ることはできない。

**現在の配信経路**(検討の経緯は[Issue #23のコメント](https://github.com/MinamiyamaKotaro/exceldiff/issues/23)、後述する旧方式からの置き換えの経緯は[Issue #47](https://github.com/MinamiyamaKotaro/exceldiff/issues/47)参照):

1. **結合済みHTMLページの収集**: `xlsxdiff --grid-html-dir <dir>`が変更のあった全シートを1つに結合して書き出したHTML(`<dir>/grid.html`、`exceldiff::wrap_grid_page`でラップ済み——インライン`<style>`のみで外部アセットに依存しない自己完結ページ)を、加工せずそのまま`${{ runner.temp }}/xlsx-diff-visuals/{sanitize(ファイルパス)}.html`(`sanitize`は英数字`._-`以外を`_`へ潰す)へコピーする——変更ファイル1件につき1コピー(シートごとではない)。`git`へは一切コミットしない。`manifest.tsv`から集めたシート名はカンマ区切りで`$VISUALS_LIST`(`path\tsheet1,sheet2,...`)へ記録し、後述のコメント箇条書きに使う。1件でも集まれば、diffステップの`has-visuals`outputを`true`にする。
2. **単一artifactとしてアップロード**: 全ファイル分の処理が終わった後、`has-visuals == 'true'`の場合のみ、`actions/upload-artifact@v4`で`xlsx-diff-visuals/`ディレクトリ全体を1つのartifact(`xlsx-diff-grids`)としてまとめてアップロードする——変更ファイルごとに1回ではなく、そのjob実行1回につき1回。
3. **ダウンロードリンクの追記**: アップロードした`upload-artifact`ステップの`artifact-url`output(形式: `https://github.com/{owner}/{repo}/actions/runs/{run_id}/artifacts/{artifact_id}`)を、diffステップが既に書き終えたコメントMarkdownの末尾へ別ステップで追記する(`artifact-url`はartifactが実在して初めて決まるため、diffステップ自身では書けない)。このURLをダウンロードするには「GitHubにログイン済みであること」が要件——実質的にこのリポジトリへの閲覧権限を持つユーザーしかダウンロードできない([`actions/upload-artifact`のREADME](https://github.com/actions/upload-artifact)に明記)。

**変更ファイル単位でのベストエフォート**: HTMLのコピーに失敗しても、その変更ファイル分だけを諦めてstderrへ警告を出し、処理を続行する([`cli/`のエラー処理方針](cli.md)と同じ「1件の失敗が全体を止めない」方針)。アップロード自体はjob全体で1回だけなので、旧方式にあった「pushの競合」への対処(リトライ・rebase)は不要になった。

**`diff-scope: commit`との組み合わせ(Issue #43)**: 保存先パスがファイルパスのみでキーされていると(`{sanitize(ファイルパス)}.html`)、同じファイルが複数コミットで変更された場合に後のコミットの分が前のコミットの分を上書きしてしまう——Issue #43のPoCで実際にこの上書きを再現した上で確認済み。これを避けるため、`diff-scope: commit`モードでは保存先を`{commit-short-sha}/{sanitize(ファイルパス)}.html`とコミット単位でネームスペース分けし、`$VISUALS_LIST`にも先頭列としてコミットラベルを追加(`commit_label\tpath\tsheet1,sheet2,...`)して、コメント末尾の箇条書きもコミットごとにコミットの短縮SHAを太字見出しにしてグループ化する。`diff-scope: pr`(既定)ではこの列は`-`という固定文字列(空文字列ではない——`IFS`にタブだけを設定していても、タブは`read`にとって依然「IFS空白文字」として扱われ、行頭の空フィールドが黙って読み飛ばされ後続の列がすべて1つずつ前へずれる、というbashの挙動があるため。実装時にこの列を空文字列にしたところ`diff-scope: pr`側の出力が実際に壊れることを確認し、非空プレースホルダに変更して解消した)。

### 変更履歴: pushベースの配信からartifactへ(Issue #47)

当初の設計は次の通りだった: 生成したPNGを、コード本体の履歴とは無関係な独立ブランチ`xlsx-diff-images`(パラレルな`git worktree`——呼び出し元の作業ツリーには一切触れない)へコミットしてpushし、pushされた実コミットSHA(ブランチの最新tipではなく、そのpush自体が生んだSHA)を使って`https://raw.githubusercontent.com/{owner}/{repo}/{commit_sha}/{path}`をMarkdown画像としてテキスト差分の直後に埋め込んでいた。パス規則は`pr-{PR番号}/{headのSHA先頭7桁}/{sanitize(ファイルパス)}/sheet-{sanitize(シート名)}.png`、push競合は`push_image`関数が`git fetch`+`git rebase`をはさんだ最大5回の線形バックオフ(`sleep $attempt`)リトライで吸収していた。GitHub Pagesは使わなかった——単にファイルをpushするだけで、Pages自体の有効化・デプロイ設定が一切不要なため。

この方式には、**プライベートリポジトリで画像が見えない**という欠陥があった: `raw.githubusercontent.com`は`github.com`とは別ドメインであり、`github.com`のセッションCookieが自動では渡らないため、閲覧権限を持つユーザーが見てもbroken imageになる([Issue #47](https://github.com/MinamiyamaKotaro/exceldiff/issues/47)で報告・調査)。代替として検討し、実機で不採用と判断した2案:

- **base64データURI埋め込み**(`![](data:image/png;base64,...)`): GitHubのコメントサニタイザが`data:`スキームの`img src`を、Markdown記法・生HTML記法どちらでも完全に削除することを実際にissueコメントへ投稿して確認した(GitHub REST APIの`body_html`で検証)。
- **`uploads.github.com`経由のuser-attachmentsアップロード**: 実機検証では画像自体は正しく描画された(`private-user-images.githubusercontent.com`への署名付きJWT URLへ自動的に書き換わる)が、生成される添付URL(`https://github.com/user-attachments/assets/<uuid>`)へ未認証・Cookieなしでアクセスしても302で有効な署名付きURLへリダイレクトされることを確認した——アクセス制御が「リポジトリ権限ベース」ではなく「URLの推測困難性(obscurity)」のみである可能性が高く、業務データを扱う本ツールの要件には合わないと判断した。また非公式・非ドキュメント化のAPIであり、composite action内の`GITHUB_TOKEN`(`ghs_`)で動作するかも未検証だった。

最終的に、GitHubの正規の権限モデル(リポジトリの閲覧権限)にそのまま乗る`actions/upload-artifact@v4` + `artifact-url`の組み合わせを採用した。トレードオフとして、PRのタイムライン上にインライン表示されなくなり、閲覧にダウンロードという一手間が増える——「見えるが実は誰でも見える」よりも「確実に権限のある人だけが見える」ことを優先した判断。旧方式で残っていた「`xlsx-diff-images`ブランチの肥大化」という未決事項(下記参照)自体も、ブランチへのコミットをやめたことで解消した。検証の詳細・生ログは[Issue #47](https://github.com/MinamiyamaKotaro/exceldiff/issues/47)のコメント参照。

### 変更履歴その2: スクリーンショットPNGから単体HTMLページへ(Issue #47、artifact化の直後)

artifact化そのものは上記の通りPR #48で実装・実機検証(private repoでの認証テスト含む)まで完了したが、そのすぐ後、実際にプライベートリポジトリで大きめのシート(実データの「スキルシート」、1シートで25,517セル)を検証した際、生成されたスクリーンショットPNGが縮小されすぎて見た目を判別できないというフィードバックを受けた。Playwrightの要素スクリーンショット(`page.locator(".page-content").screenshot()`)はシートの実際の幅・高さそのままの解像度でPNG化するため、セル数が多いシートほど画像自体は巨大になる一方、一般的な画像ビューアはウィンドウ幅に収まるよう自動縮小して表示するため、大きなシートほどかえって内容が読めなくなるという逆効果があった。

対処として、スクリーンショット生成のステップ自体を廃止し、`wrap_grid_page`が生成する単体HTMLページ(インライン`<style>`のみで完結し、外部アセットへの依存が無い)をそのまま`xlsx-diff-visuals/`へコピーしてartifactへ含める方式に変更した。ダウンロードしたHTMLをブラウザで開けば、通常のWebページと同じようにスクロール・拡大縮小(ブラウザの標準ズーム)しながら閲覧できるため、シートの大きさに関わらず内容を判読できる。

この変更に伴い、`action-scripts/`(`screenshot.mjs`・`package.json`)ディレクトリ自体と、`action.yml`の`Install Node.js`/`Install screenshot dependencies`ステップ(Node.jsセットアップ・`npm install`・`npx playwright install --with-deps chromium`)を削除した——Playwright/Chromiumのインストールがそもそも不要になったため、`visual: true`時のジョブ実行時間も大きく短縮される副次効果があった。

### 変更履歴その3: シートごとの個別HTMLから、変更ファイル単位の結合HTMLへ(Issue #47)

「変更履歴その2」でPNGをHTMLへ置き換えた直後、今度は「シートごとに別々のHTMLファイルではなく、まとめて1つのHTMLで見たい」というフィードバックを受けた。当時の実装は、`write_grid_sections`(`cli/src/main.rs`)が`grid_sections_from_paths`の返す各シートのフラグメントに対して`wrap_grid_page`をシートごとに個別呼び出しし、`sheet-{i}.html`という別々のファイルへ書き出していた——1つの変更ファイルに複数の変更シートがあると、artifact内に同数のバラバラなHTMLファイルが並ぶ形になっていた。

`wrap_grid_page`は元々「1つ以上のフラグメントをまとめて1ページへラップする」設計(複数シートのフラグメントを連結した文字列を渡せる。`examples/xlsx_diff_grid.rs`が既にこのパターンを使用)だったため、`write_grid_sections`側を「シートごとに`wrap_grid_page`を呼ぶ」から「全シートのフラグメントを連結してから`wrap_grid_page`を1回だけ呼ぶ」方式に変更するだけで実現できた。出力ファイル名も`sheet-{i}.html`から固定名`grid.html`(変更ファイル1件につき1枚)に変わり、`manifest.tsv`は全行が同じ`grid.html`を指すようになった。

`action.yml`側もこれに合わせて更新: 変更ファイルごとの収集ループを「シートごとにコピー」から「`manifest.tsv`の最初の行から`html_path`を1回だけ取り出してコピーし、シート名はカンマ区切りで`$VISUALS_LIST`へまとめて記録する」方式へ変更した(`sanitize(path).html`という固定パスへコピー——`sanitize(path)/sheet-sanitize(name).html`というディレクトリ構造は不要になった)。PRコメントの箇条書きも、シートごとの行(`- path — sheet1`\n`- path — sheet2`)から、ファイルごとに1行へまとまった(`- path — sheet1,sheet2`)。

`cli/tests/cli.rs`に、2シートを変更した`.xlsx`ペア(専用ヘルパー`xlsx_zip_multi_sheet`で構築——[[feedback_test_fixture_determinism]]と同じ理由で、既存の無関係な実フィクスチャ2つを流用するのではなく最小構成をその場で組み立てている)を渡し、`manifest.tsv`の2行がどちらも同じ`grid.html`を指すこと、実際に書き出されるHTMLファイルが1つだけであること、そのファイル内に両シート分の`class="sheet"`セクションが含まれることを確認する専用テスト(`grid_html_dir_combines_every_changed_sheet_into_one_page`)を追加した。

## 事前ビルド済みバイナリ配布([Issue #28](https://github.com/MinamiyamaKotaro/exceldiff/issues/28))

composite action化(P0)は呼び出しのたびに`cargo build`を要求するため低速になる、というのが元々のIssue #28の懸念だった。実装前にPoCで実測したところ(Issue #28のコメント参照)、この懸念は部分的に正しいが原因は想定と異なっていた: `dtolnay/rust-toolchain`は約0.6秒、`Swatinem/rust-cache`のrestoreは約0.4秒とどちらも軽いが、**`Swatinem/rust-cache`は`xlsx-diff.yml`が`on: pull_request`専用(`master`へのpushでは実行されない)なためデフォルトブランチ側にキャッシュの実体が一度も作られず、新規PRの初回実行は必ず`No cache found.`のコールドビルドになる**——実測で`cargo build --release -p xlsxdiff`自体に約14秒かかっていた。

### 設計: ダウンロード優先+ソースビルドへの透過的フォールバック

`action.yml`の最初のステップ「Resolve xlsxdiff binary」が以下を行う(実装は[`action.yml`](../../action.yml)参照):

1. `${{ github.action_ref }}`(呼び出し元の`uses: owner/repo@ref`のref)が空でなく、かつ`runner.os`/`runner.arch`が既知の組み合わせ(後述の対応ターゲット表)であれば、`https://github.com/MinamiyamaKotaro/exceldiff/releases/download/{action_ref}/xlsxdiff-{action_ref}-{target}.tar.gz`とその`SHA256SUMS`を`curl -fsSL`で取得し、チェックサム検証(`sha256sum -c`、macOSには`sha256sum`が無いため`shasum -a 256 -c`にフォールバック)を通ったものだけ展開・`chmod +x`して使う。
2. 上記のいずれか(ref が空、プラットフォーム非対応、ダウンロード失敗、チェックサム不一致)が起きた場合は、**無条件に**`dtolnay/rust-toolchain`+`Swatinem/rust-cache`+`cargo build --release -p xlsxdiff`という従来のソースビルド経路にフォールバックする(`if: steps.resolve_binary.outputs.found != 'true'`をこれら3ステップに付けるだけで実現——ステップ自体を条件分岐で丸ごとスキップできるのはcomposite actionのステップ単位`if:`ならではの単純さ)。
3. **`action_ref`の形状による事前フィルタは意図的に行わない**——空文字列でない限り常にダウンロードを試みる。PoC初期案では`^v[0-9]+\.[0-9]+`という正規表現でバージョンタグらしいrefだけに絞り込んでいたが、これは本リポジトリ自身のREADMEが案内する呼び出し例`uses: MinamiyamaKotaro/exceldiff@v1`(メジャーバージョンのみ)にマッチせず、常にダウンロードをスキップしてソースビルドへ回ってしまうという実害のあるバグだった(Issue #28のコメントで実機検証・修正)。`curl`が404で失敗すればそのままフォールバックするため、形状チェック自体が不要という結論に至った。

この設計により、本リポジトリ自身の`.github/workflows/xlsx-diff.yml`(`uses: ./`、`action_ref`が常に空)は**自動的に**従来通りソースビルド経路を通る——事前ビルド済みバイナリへの移行が、この最重要のセルフドッグフーディング経路を壊さないことを保証する仕組みそのものになっている。

### `release.yml`(新設)

`.github/workflows/release.yml`が`v*`形式のタグpushをトリガーに、`ubuntu-latest`/`macos-latest`(2回、`aarch64-apple-darwin`とクロスビルドの`x86_64-apple-darwin`)/`windows-latest`の4ジョブをマトリクスビルドし、各ターゲットのバイナリを`xlsxdiff-{tag}-{target}.tar.gz`として`tar`で固めた上で(Windows含め全ターゲット`.tar.gz`に統一——`windows-latest`にも動く`tar`があるため`.zip`/`Compress-Archive`用の別経路は不要と判断)、最後の集約ジョブが全アセットの`SHA256SUMS`を生成し`gh release create`でGitHub Releaseへ添付する。

`xlsxdiff`の依存グラフ(`cargo tree`で確認済み: `quick-xml`/`serde`/`serde_json`/`thiserror`/`zip`——`zip`の`deflate`featureも`flate2`→`zlib-rs`という純Rust実装で、Cのzlibを要求しない)にはC/ネイティブ依存が一切無い(`exceldiff`のオプション機能`diff-storage`が使う`rusqlite`の`bundled`(C製SQLite同梱)featureは、`cli/Cargo.toml`が`exceldiff = { path = ".." }`とfeature指定なしで依存しているため`xlsxdiff`バイナリには含まれない)。そのため各ターゲットは対応するOSのGitHub-hostedランナー上でネイティブに`cargo build --release --target <トリプル>`するだけでビルドでき、`cross`やDockerは一切不要——実際にこのマシン(arm64 macOS)からmacOSの別アーキテクチャ(x86_64)へのクロスビルドが7秒で成功することも確認済み。

対応ターゲット(優先度順):

| ターゲット | 優先度 | 備考 |
|---|---|---|
| `x86_64-unknown-linux-gnu` | P0 | `ubuntu-latest`と一致。composite actionの呼び出し元の大多数を占める |
| `aarch64-apple-darwin` | P1 | `macos-latest`は現在arm64ホスト |
| `x86_64-apple-darwin` | P1 | 同一OS内クロスビルド |
| `x86_64-pc-windows-msvc` | P1 | `windows-latest` |
| `aarch64-unknown-linux-gnu` | P2(未実装) | クロスリンカが必要になる見込みで今回のスコープ外——下記「未決事項」参照 |

### 未検証の部分

このPoC・実装はいずれも(a) 実際の`curl`ダウンロード成功パスをローカルの簡易HTTPサーバー(`python3 -m http.server`)で模した`.tar.gz`+`SHA256SUMS`に対して検証(展開・`chmod +x`後のバイナリが実際に動作することまで確認)、(b) チェックサム不一致時に正しくフォールバックすることを、破損させたアーカイブに対して検証、(c) `runner.os`/`runner.arch`の全組み合わせに対するターゲットトリプル解決ロジックの検証、まではローカルで実施済み。しかし**実際にタグをpushして`release.yml`を走らせ、本物のGitHub Releaseアセットに対してダウンロード成功パスを実機検証することはまだ行っていない**——タグ・Releaseの作成は公開・準不可逆な操作のため、実施タイミングは別途判断する(下記「未決事項」参照)。

## 依存関係

- 依存先: [`cli/`](cli.md)(1PR差分ファイルにつき1回、`--max-rows-per-sheet`/`--diff-mode`フラグ付きで起動する。`visual: true`時は`--grid-html-dir`も渡す。位置引数側の契約——`<display_path> <A|M|D> [base_file] [head_file]`——は変更しない。バイナリの入手経路は上記「事前ビルド済みバイナリ配布」参照)。~~`action-scripts/`~~(Playwright依存のNodeパッケージ)は上記「変更履歴その2」の通り削除済み——`visual: true`もRustツールチェーンのみで完結する。
- 依存元: [`.github/workflows/xlsx-diff.yml`](../../.github/workflows/xlsx-diff.yml)(本リポジトリ自身が`uses: ./`で参照する唯一の呼び出し元。将来、外部リポジトリが`uses: MinamiyamaKotaro/exceldiff@<tag>`で参照することも想定するが、現時点でそのような外部呼び出し元は存在しない)、[`.github/workflows/release.yml`](../../.github/workflows/release.yml)(タグpushで`xlsxdiff`のビルド済みバイナリをGitHub Releaseへ公開する、新設の依存元)

## エラー処理方針

`cli/`側([`main`のエラー処理方針](cli.md)参照)と同じく、1ファイルのパースエラーが全体のコメント投稿を止めないことを前提にした設計を踏襲する——本action自体はビルド失敗以外で明示的に失敗させる箇所を持たない。フォークからのPRではGitHub Actionsの仕様により`GITHUB_TOKEN`が読み取り専用に強制されるため、コメント投稿ステップは黙って失敗する(`action.yml`内のコメントに明記。何かに依存されるステップではないため、job全体の失敗にはつながる可能性がある——`peter-evans/*`アクション自体が非ゼロ終了する場合、後続ステップがない本jobではそのまま失敗として報告される。これは移植元の`xlsx-diff.yml`から変わらない既存の挙動)。`visual: true`時のスクリーンショットアップロード(`actions/upload-artifact@v4`)自体は`GITHUB_TOKEN`の権限モデルに縛られない別経路(`ACTIONS_RUNTIME_TOKEN`)で認可されるため、フォークからのPRでも失敗しない見込み——ただしコメント投稿ステップ自体は上記の理由で引き続き失敗する(未検証、下記「未決事項」参照)。

## テスト方針

composite actionはYAML定義であり`cargo test`の対象にならないため、以下の方法で検証する:

1. **静的検証**: `action.yml`はYAMLとして構文検証する(`actionlint`は`.github/workflows/`配下のワークフローファイルのみを対象とし`action.yml`形式のcomposite actionメタデータには非対応のため、Python `yaml.safe_load`等で構文のみ検証)。呼び出し元ワークフロー側(`.github/workflows/xlsx-diff.yml`)は`actionlint`でも検証可能。
2. **シェルロジックの単体検証**: 「変更ファイルごとに`git show`でbase/headを取り出し`xlsxdiff`を起動してMarkdownへ連結し、`has-changes`/`changed-files-count`を`$GITHUB_OUTPUT`へ書く」というシェルスクリプト部分は`${{ github.action_path }}`・`${{ runner.temp }}`・`$GITHUB_OUTPUT`をローカルパスに置き換えれば`bash`だけでそのまま実行できる。実際に、ローカルの使い捨てgitリポジトリへA(追加)・M(変更)・D(削除)の3ステータスが混在する差分を作りこのスクリプトを実行して意図通りのMarkdownが生成されること、および変更あり/なし双方のケースで`has-changes`/`changed-files-count`が正しい値になることを確認済み。`--max-rows-per-sheet`/`--diff-mode`フラグが実際に`MarkdownOptions`へ届くことは、`cli/`側の統合テスト([`cli/tests/cli.rs`](../../cli/tests/cli.rs))で検証している(下記「依存関係」)——`action.yml`のシェルスクリプト部分としては、フラグの値をそのまま`"$BIN"`へ渡しているだけなので、フラグ自体の意味までは再検証しない。
3. **実際のGitHub Actions上での結合検証**: `.github/workflows/xlsx-diff.yml`自体を`uses: ./`で本actionを呼び出す形に書き換えた(下記「依存関係」参照)。これにより、`.xlsx`ファイルを変更する今後の任意のPRが本action全体(Rustツールチェーンのセットアップ・`github.action_path`起点でのビルド・`rust-cache`のワークスペース指定・コメント投稿)の実行結果を検証する回帰テストとして機能する——外部のテスト用リポジトリを別途用意しなくても、本リポジトリ自身がdogfoodingの場になる。**この結合検証だけが発見できた不具合が実際にあった**(上記「事後発見」参照)——単一ファイルのみ変更というケースは、ローカルのシェルスクリプト単体検証(複数ステータス混在の差分で検証していた)や静的検証では再現せず、GitHub Actions実機での複数回の試行によってのみ再現・特定できた。composite actionの検証において実機結合テストを省略できない理由の実例。
4. **`visual`モード固有の検証**: 使い捨てのgitリポジトリに対し、diffステップのシェルロジックを`VISUAL=true`で単体実行し、`xlsxdiff --grid-html-dir`が書き出したHTMLがそのまま`xlsx-diff-visuals/`へ収集され(実際に生成された`.html`ファイルの中身も目視確認)、`has-visuals`が正しく`true`になることをローカルで確認済み。`actions/upload-artifact@v4`の`artifact-url`が実際にコメント上でクリック可能なダウンロードリンクとして表示されること、プライベートリポジトリでのアクセス制御(未認証ではHTTP 404、認証済みならダウンロード可能)については、PR #48の時点でGitHub Actions実機・使い捨て外部リポジトリの双方で確認済み(下記「未決事項」参照)——本変更(PNG→HTML)によってartifactの中身が変わっただけで、配信経路自体の権限モデルは変わっていないため、その部分の再検証は不要と判断した。
5. **`diff-scope: commit`の検証(Issue #43)**: 実装前にPoC(`poc/issue43-poc/`、非コミット)で、使い捨てgitリポジトリ上で「Added→Modified→Added」のコミット列を作り、コミット単位ループが実際にIssue #23の問題を解消することと、`visual: true`併用時にファイル名衝突が起きること(グリッドHTMLが後のコミットの分で上書きされる)を確認した上で、コミット単位ネームスペース分けの修正案でその衝突が解消することを確認した(いずれもGitHub issueコメントとして記録: [最初のコメント](https://github.com/MinamiyamaKotaro/exceldiff/issues/43#issuecomment-5448294124)・[追加検証コメント](https://github.com/MinamiyamaKotaro/exceldiff/issues/43#issuecomment-5448338647))。実装後、`action.yml`から実際の2ステップ分のシェルスクリプトを抽出し(`yaml.safe_load`でパース→`run:`フィールドをファイル書き出し)、`bash -n`で構文検証した上で、ビルド済み`xlsxdiff`バイナリ・使い捨てgitリポジトリに対して`diff-scope=pr`/`commit`両方・`visual=true`/`false`両方・非`.xlsx`のみのコミット(スキップされるべき)を実際に実行し、コメントMarkdown・`$VISUALS_DIR`のディレクトリ構造・`$VISUALS_LIST`の内容・`$GITHUB_OUTPUT`をすべて目視確認した。この過程で、`$VISUALS_LIST`の先頭列(コミットラベル)を空文字列にしていたところ、`diff-scope: pr`(既定モード)側の出力が実際に壊れる不具合を発見した——`IFS=$'\t'`を設定していても、タブは`read`にとって「IFS空白文字」として扱われ続けるため行頭の空フィールドが黙って読み飛ばされ後続列が1つずつ前へずれる、というbash特有の挙動によるもの(上記「ビジュアルモード」参照)。プレースホルダを`-`に変更して解消・再検証済み。**ただし、この検証はすべてローカルのシェルスクリプト単体実行によるものであり、上記項目3で述べた「ローカル検証だけでは不十分で実際のGitHub Actions実行でしか再現しない不具合がある」という教訓を踏まえると、実際のGitHub Actions上での結合検証(複数コミットを持つ本物のPRでのdogfooding)はまだ実施していない**(下記「未決事項」参照)。

## 未決事項 / オープンクエスチョン

1. **`cli`クレートの`crates.io`公開**: 本actionは`cli/`をソースからビルドする方式を採用しており、`cli/Cargo.toml`の`publish = false`は変更していない。事前ビルド済みバイナリ配布([Issue #28](https://github.com/MinamiyamaKotaro/exceldiff/issues/28)、下記「事前ビルド済みバイナリ配布」参照)はGitHub Releaseのアセットとして直接配布する方式で実現したため、`crates.io`公開自体は依然として不要のまま——[Issue #22](https://github.com/MinamiyamaKotaro/exceldiff/issues/22)が想定していた「公開する実利」は生じなかった。公開する新たな実利が生じるまでは現状維持とする。
2. **inputs/outputsの汎用化(続き)**: `files`/`comment`/`job-summary`/`max-rows-per-sheet`/`diff-mode`/`visual`inputsと`has-changes`/`changed-files-count`outputsは実装済み([Issue #24](https://github.com/MinamiyamaKotaro/exceldiff/issues/24))。~~`diff-scope`(コミット単位の差分表示)~~([Issue #43](https://github.com/MinamiyamaKotaro/exceldiff/issues/43)、`commit`モードのみ実装済み): 新規`diff-scope`input(`pr`(既定・従来通り)/`commit`)を追加し、`commit`モードはPRが導入した各コミットを`git log --reverse base.sha..head.sha`で列挙して直前の親(`<commit>^1`)との差分をコミットごとのMarkdownサブセクションとして出力する——新規追加されたファイルへのPR内修正が常に`Added`として扱われる問題([Issue #23のコメント](https://github.com/MinamiyamaKotaro/exceldiff/issues/23))を解消する。`visual: true`併用時のグリッドHTML成果物もコミット単位でネームスペース分けする(上記「ビジュアルモード」参照)。設計はPoC(`poc/issue43-poc/`)で事前検証し、実装後もローカルでシェルスクリプト単体検証まで実施済みだが、**実際のGitHub Actions上での結合検証(dogfooding)はまだ未実施**(上記「テスト方針」項目5参照)。以下は引き続き未着手:
   - `changed-cells-count`output: 現状`xlsxdiff`はMarkdown文字列をstdoutへ書くのみで、追加/変更/削除セル数を機械可読な形で外に出していない。`cli/`側に集計出力(例: stderrへの`added=N modified=M deleted=D`行)を追加した上で、`action.yml`側でファイルごとに合算する必要がある。
   - `diff-scope: push`(直前pushの`before`/`after`単位への切り替え)は未実装のまま残っている。`pull_request`イベントの`synchronize`アクションのペイロードには`before`/`after`フィールドが存在する([octokit/webhooksのJSON Schemaで確認](https://github.com/octokit/webhooks))が、`opened`等それ以外のアクションには存在しないため、そのケースのフォールバック(例: `base.sha`/`head.sha`にフォールバックする)を含めて別途設計・検証が必要。
   - コメント文言・マーカー(`<!-- xlsx-diff-comment -->`)自体のカスタマイズは、具体的な要望が出るまでスコープ外のままとする。
   - `files`inputはgitパススペックとして実装した(シェルグロブではない)。GitHub Actionsの`paths:`トリガーフィルタ構文とは別物である点に注意——本actionはワークフローのトリガー自体を制御しない。
3. ~~**外部リポジトリからの実地検証**~~([Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23)、解決済み): プレリリースタグ`v0.1.0-rc1`を切り、別リポジトリ(throwaway、`MinamiyamaKotaro/exceldiff-action-verify`)から`uses: MinamiyamaKotaro/exceldiff@v0.1.0-rc1`で実際に呼び出し、checkout→差分計算→PRコメント投稿までend-to-endで成功を確認した。この検証自体が実際にバグを発見した——README基本例どおり`permissions: pull-requests: write`のみを書いた呼び出し元では`contents`が黙って`none`になり`actions/checkout`が失敗する問題(上記「`permissions:`ブロック」参照、修正はPR #45)。セルフドッグフーディング(常に`visual: true`で`contents: write`を書く)だけでは決して発見できなかった不具合であり、外部からの実地検証を省略できない実例になった。
4. ~~**`xlsx-diff-images`ブランチの肥大化**~~(Issue #47の設計変更により解消): pushベース方式(コミットが無期限に増え続ける)自体を廃止し、artifactアップロード(90日で自動失効、`retention-days`で短縮も可能)へ置き換えたため、このリスクは前提ごと無くなった。
5. ~~**`visual`モードのGitHub Actions実機検証**~~([Issue #47](https://github.com/MinamiyamaKotaro/exceldiff/issues/47)、[PR #48](https://github.com/MinamiyamaKotaro/exceldiff/pull/48)で解決済み): 使い捨ての`.xlsx`フィクスチャを一時的に追加するコミットで本リポジトリ自身の`uses: ./`ワークフローを実際にトリガーし([実行33122403504](https://github.com/MinamiyamaKotaro/exceldiff/actions/runs/33122403504))、`actions/upload-artifact@v4`のアップロード成功・PRコメントへの`artifact-url`リンク掲載・そのartifact自体の実在(`xlsx-diff-screenshots`、id `9666997603`、期限切れでない)をGitHub REST APIで確認した。検証用フィクスチャは確認後削除済み(Issue #23/#24と同じ手順)。**プライベートリポジトリでの検証も追加で実施済み**: マージ後、既存の使い捨て外部検証リポジトリ(`MinamiyamaKotaro/exceldiff-action-verify`、非公開)の新規PRから、マージコミット(`0c4f571`)を指す`uses:`で本actionを呼び出し、実際にコメント・artifactが生成されることを確認した上で、(a) 認証なしでの`artifact-url`アクセスがHTTP 404(APIは401)になり閲覧をブロックされること、(b) 権限を持つ側(オーナー自身のトークン)からは実際にzipをダウンロードでき、中身が期待通りのスクリーンショットPNGであることの両方をGitHub APIで確認した——旧`raw.githubusercontent.com`方式(閲覧権限があっても見えない)、および不採用にした`uploads.github.com`方式(認証なしでも到達できてしまう)のどちらとも異なり、意図通り「リポジトリ権限のある人だけが見える」が実現できていることの直接証拠。
6. **事前ビルド済みバイナリ配布の実機未検証部分**([Issue #28](https://github.com/MinamiyamaKotaro/exceldiff/issues/28)、上記「事前ビルド済みバイナリ配布」参照): 設計はPoC(`poc/issue28-poc/`)で検証し、ダウンロード成功パス・チェックサム不一致時のフォールバック・ターゲットトリプル解決ロジックはいずれもローカル(簡易HTTPサーバーでのモック含む)で確認済みだが、以下は未着手のまま残っている:
   - **実際のタグpush→`release.yml`実行→本物のGitHub Releaseアセットに対する`action.yml`側ダウンロードの実機検証**。項目3・5と同じ理由(ローカル検証だけでは見つからない不具合がある)でいずれ必要になるが、タグ・Releaseの作成は本actionにとって初めての公開・準不可逆な操作であり、実施タイミング(バージョン番号の付け方、`v0.1.0-rc1`と同様の検証専用プレリリースを切るか等)は別途判断する。
   - `aarch64-unknown-linux-gnu`(ARM64 Linux)ターゲットは未実装。クロスリンカが必要になる可能性が高く、`ubuntu-latest`にネイティブARM64版が一般提供されているかも含め別途調査が必要。
   - `changed-cells-count`output(上記項目2参照)とは独立した課題だが、`release.yml`が生成する`SHA256SUMS`のフォーマット・アセット命名規則が将来この手のoutputやその他のツール連携に流用可能かは未検討。
