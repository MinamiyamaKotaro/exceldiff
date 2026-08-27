# `action.yml` 設計書

*[English](action.en.md)*

リポジトリルートの `action.yml`(`runs: using: composite`)に対応する設計書。[`.github/workflows/xlsx-diff.yml`](../../.github/workflows/xlsx-diff.yml)がこの自リポジトリ専用ワークフロー内に直接書いていたステップ群を、`uses:`で外部リポジトリからも呼び出せる再利用可能なcomposite actionとして切り出したもの([Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23))。

[`cli.md`](cli.md)の未決事項1に記載の通り、当初の[Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23)は「CLIを`src/bin/xlsxdiff.rs`へ昇格した上でcomposite action化する」という案を想定していたが、実際には[Issue #31](https://github.com/MinamiyamaKotaro/exceldiff/issues/31)・[Issue #32](https://github.com/MinamiyamaKotaro/exceldiff/issues/32)で独立ワークスペースメンバー`cli/`方式が採用された。本設計はその現状を前提に、`cli/`をソースからビルドしてcomposite action化する方針を採る——`cli`クレートを`crates.io`へ公開する必要はない(下記「未決事項」参照)。

## 責務・スコープ

- Rustツールチェーンのセットアップ・`cli/`(パッケージ`xlsxdiff`)のビルド・変更された`.xlsx`ファイルごとの差分計算・Markdownコメントの投稿(または既存コメントの更新)までを、composite actionの`steps`としてカプセル化する。
- 呼び出し元のワークフローファイルが個々のステップを重複して書く必要をなくす——本リポジトリ自身の`.github/workflows/xlsx-diff.yml`も、この`action.yml`を`uses: ./`で呼び出すことでセルフドッグフーディングする(下記「テスト方針」参照)。
- `visual: true`の場合、変更のあったシートごとに[`grid.rs`のExcelライクなグリッドHTML](grid.md)をPlaywright(ヘッドレスChromium)でスクリーンショットし、1回のjob実行分をまとめて1つのGitHub Actions artifact(`actions/upload-artifact@v4`)としてアップロードして、そのダウンロードリンクをテキスト差分の下に追記する(下記「ビジュアルモード」参照)。実際にスクリーンショットを撮影・公開する処理そのものは[`grid.rs`の責務には含まれていない](grid.md)ため、この配線は本actionが担う。
- **含まない責務**: `.xlsx`のパース・差分計算・Markdown整形・グリッドHTML生成そのもの(すべて[`exceldiff::diff_file_section_from_paths`](markdown.md)/[`grid_sections_from_paths`](grid.md)と、それを呼び出す[`cli/`](cli.md)の責務)、事前ビルド済みバイナリの配布([Issue #22](https://github.com/MinamiyamaKotaro/exceldiff/issues/22)・[Issue #28](https://github.com/MinamiyamaKotaro/exceldiff/issues/28)の検討事項、P2)、変更セル数の合計を返すoutput・コミット単位での差分表示(いずれも[Issue #24](https://github.com/MinamiyamaKotaro/exceldiff/issues/24)で後続タスクとして切り出し済み。下記「未決事項」参照)。

## inputs / outputs([Issue #24](https://github.com/MinamiyamaKotaro/exceldiff/issues/24))

| input | 型/既定値 | 内容 |
|---|---|---|
| `github-token` | string, `${{ github.token }}` | コメント投稿に使うトークン |
| `files` | string, `*.xlsx` | `git diff -- <files>`へそのまま渡す**gitパススペック**(シェルグロブではない。既定値は先頭に`**/`が無くても任意の深さのパスにマッチする) |
| `comment` | bool文字列, `'true'` | PRコメントとして投稿するか |
| `job-summary` | bool文字列, `'false'` | `$GITHUB_STEP_SUMMARY`へも書き出すか。`comment`と独立に指定可能——forkからのPR等`pull-requests: write`権限を付与できない環境では`comment: false`・`job-summary: true`にすることで、権限エラーを起こさずJob Summary上で差分を確認できる |
| `max-rows-per-sheet` | 数値文字列, `'30'` | [`MarkdownOptions::max_rows_per_sheet`](markdown.md)へ渡す(`cli/`の`--max-rows-per-sheet`フラグ経由) |
| `diff-mode` | string enum `auto`\|`coordinate`, `'auto'` | [`MarkdownOptions::diff_mode`](markdown.md)へ渡す(`cli/`の`--diff-mode`フラグ経由)。`auto`は現行の`diff_workbooks_best_effort`(座標一致/行アライメント/列アライメント自動選択)、`coordinate`はアライメント検出をスキップした単純座標比較 |
| `visual` | bool文字列, `'false'` | 変更のあったシートごとにExcelライクなグリッドのスクリーンショットを生成し、workflow artifactとして添付(コメントにはダウンロードリンクを掲載)するか。追加の`permissions:`は不要(下記「呼び出し元に要求する前提」)だが、Node.js・Playwright・Chromiumダウンロードが追加されジョブの実行時間に無視できない影響があるため既定はオフ |

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
  visual:                    # 既定値 'false'
outputs:
  has-changes:            # steps.diff.outputs.has-changes
  changed-files-count:   # steps.diff.outputs.changed-files-count
runs:
  using: composite
  steps:
    - dtolnay/rust-toolchain@stable
    - Swatinem/rust-cache@v2         # workspaces: 本action自身のパス起点
    - cargo build --release -p xlsxdiff --manifest-path ...
    - if: inputs.visual      # actions/setup-node@v4
    - if: inputs.visual      # action-scripts/ を npm install + playwright install --with-deps chromium
    - id: diff              # 変更ファイルごとにgit show + xlsxdiffを実行しMarkdownを組み立て、
                              # has-changes/changed-files-countを$GITHUB_OUTPUTへ書き出す。
                              # visual: trueならシートごとにスクリーンショットを撮り、
                              # ${{ runner.temp }}/xlsx-diff-visuals/へ集約し、
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

1. **スクリーンショット生成**: `xlsxdiff --grid-html-dir <dir>`で書き出したシートごとのHTML(`exceldiff::wrap_grid_page`でラップ済み)を、`action-scripts/screenshot.mjs`(Playwright)でPNG化する。要素スクリーンショット(`page.locator(".page-content").screenshot()`)を使う——`page.screenshot({ fullPage: true })`は既定の1280pxビューポート幅までしか幅を縮められず、シートの実際の幅(可変)より余白が大きく残ってしまうため、`wrap_grid_page`側で`.page-content`に`display: inline-block`を与えて内容にぴったり合わせている。
2. **成果物としてローカルに集約**: 生成したPNGは`git`へは一切コミットせず、`${{ runner.temp }}/xlsx-diff-visuals/{sanitize(ファイルパス)}/sheet-{sanitize(シート名)}.png`(`sanitize`は英数字`._-`以外を`_`へ潰す)へコピーするだけに留める。1件でも集まれば、diffステップの`has-visuals`outputを`true`にする。
3. **単一artifactとしてアップロード**: 全ファイル・全シート分の処理が終わった後、`has-visuals == 'true'`の場合のみ、`actions/upload-artifact@v4`で`xlsx-diff-visuals/`ディレクトリ全体を1つのartifact(`xlsx-diff-screenshots`)としてまとめてアップロードする——1シートにつき1回ではなく、そのjob実行1回につき1回。
4. **ダウンロードリンクの追記**: アップロードした`upload-artifact`ステップの`artifact-url`output(形式: `https://github.com/{owner}/{repo}/actions/runs/{run_id}/artifacts/{artifact_id}`)を、diffステップが既に書き終えたコメントMarkdownの末尾へ別ステップで追記する(`artifact-url`はartifactが実在して初めて決まるため、diffステップ自身では書けない)。このURLをダウンロードするには「GitHubにログイン済みであること」が要件——実質的にこのリポジトリへの閲覧権限を持つユーザーしかダウンロードできない([`actions/upload-artifact`のREADME](https://github.com/actions/upload-artifact)に明記)。

**1シート単位でのベストエフォート**: スクリーンショットの生成に失敗しても、そのシートの画像だけを諦めてstderrへ警告を出し、処理を続行する([`cli/`のエラー処理方針](cli.md)と同じ「1件の失敗が全体を止めない」方針)。アップロード自体はjob全体で1回だけなので、旧方式にあった「pushの競合」への対処(リトライ・rebase)は不要になった。

### 変更履歴: pushベースの配信からartifactへ(Issue #47)

当初の設計は次の通りだった: 生成したPNGを、コード本体の履歴とは無関係な独立ブランチ`xlsx-diff-images`(パラレルな`git worktree`——呼び出し元の作業ツリーには一切触れない)へコミットしてpushし、pushされた実コミットSHA(ブランチの最新tipではなく、そのpush自体が生んだSHA)を使って`https://raw.githubusercontent.com/{owner}/{repo}/{commit_sha}/{path}`をMarkdown画像としてテキスト差分の直後に埋め込んでいた。パス規則は`pr-{PR番号}/{headのSHA先頭7桁}/{sanitize(ファイルパス)}/sheet-{sanitize(シート名)}.png`、push競合は`push_image`関数が`git fetch`+`git rebase`をはさんだ最大5回の線形バックオフ(`sleep $attempt`)リトライで吸収していた。GitHub Pagesは使わなかった——単にファイルをpushするだけで、Pages自体の有効化・デプロイ設定が一切不要なため。

この方式には、**プライベートリポジトリで画像が見えない**という欠陥があった: `raw.githubusercontent.com`は`github.com`とは別ドメインであり、`github.com`のセッションCookieが自動では渡らないため、閲覧権限を持つユーザーが見てもbroken imageになる([Issue #47](https://github.com/MinamiyamaKotaro/exceldiff/issues/47)で報告・調査)。代替として検討し、実機で不採用と判断した2案:

- **base64データURI埋め込み**(`![](data:image/png;base64,...)`): GitHubのコメントサニタイザが`data:`スキームの`img src`を、Markdown記法・生HTML記法どちらでも完全に削除することを実際にissueコメントへ投稿して確認した(GitHub REST APIの`body_html`で検証)。
- **`uploads.github.com`経由のuser-attachmentsアップロード**: 実機検証では画像自体は正しく描画された(`private-user-images.githubusercontent.com`への署名付きJWT URLへ自動的に書き換わる)が、生成される添付URL(`https://github.com/user-attachments/assets/<uuid>`)へ未認証・Cookieなしでアクセスしても302で有効な署名付きURLへリダイレクトされることを確認した——アクセス制御が「リポジトリ権限ベース」ではなく「URLの推測困難性(obscurity)」のみである可能性が高く、業務データを扱う本ツールの要件には合わないと判断した。また非公式・非ドキュメント化のAPIであり、composite action内の`GITHUB_TOKEN`(`ghs_`)で動作するかも未検証だった。

最終的に、GitHubの正規の権限モデル(リポジトリの閲覧権限)にそのまま乗る`actions/upload-artifact@v4` + `artifact-url`の組み合わせを採用した。トレードオフとして、PRのタイムライン上にインライン表示されなくなり、閲覧にダウンロードという一手間が増える——「見えるが実は誰でも見える」よりも「確実に権限のある人だけが見える」ことを優先した判断。旧方式で残っていた「`xlsx-diff-images`ブランチの肥大化」という未決事項(下記参照)自体も、ブランチへのコミットをやめたことで解消した。検証の詳細・生ログは[Issue #47](https://github.com/MinamiyamaKotaro/exceldiff/issues/47)のコメント参照。

## 依存関係

- 依存先: [`cli/`](cli.md)(`cargo build -p xlsxdiff`でビルドし、`xlsxdiff`バイナリを1PR差分ファイルにつき1回、`--max-rows-per-sheet`/`--diff-mode`フラグ付きで起動する。`visual: true`時は`--grid-html-dir`も渡す。位置引数側の契約——`<display_path> <A|M|D> [base_file] [head_file]`——は変更しない)、`action-scripts/`(`visual: true`時のみ。`package.json`でPlaywrightを依存に持つ、公開クレートには含まれない独立したNodeパッケージ。`screenshot.mjs`が唯一のスクリプト)
- 依存元: [`.github/workflows/xlsx-diff.yml`](../../.github/workflows/xlsx-diff.yml)(本リポジトリ自身が`uses: ./`で参照する唯一の呼び出し元。将来、外部リポジトリが`uses: MinamiyamaKotaro/exceldiff@<tag>`で参照することも想定するが、現時点でそのような外部呼び出し元は存在しない)

## エラー処理方針

`cli/`側([`main`のエラー処理方針](cli.md)参照)と同じく、1ファイルのパースエラーが全体のコメント投稿を止めないことを前提にした設計を踏襲する——本action自体はビルド失敗以外で明示的に失敗させる箇所を持たない。フォークからのPRではGitHub Actionsの仕様により`GITHUB_TOKEN`が読み取り専用に強制されるため、コメント投稿ステップは黙って失敗する(`action.yml`内のコメントに明記。何かに依存されるステップではないため、job全体の失敗にはつながる可能性がある——`peter-evans/*`アクション自体が非ゼロ終了する場合、後続ステップがない本jobではそのまま失敗として報告される。これは移植元の`xlsx-diff.yml`から変わらない既存の挙動)。`visual: true`時のスクリーンショットアップロード(`actions/upload-artifact@v4`)自体は`GITHUB_TOKEN`の権限モデルに縛られない別経路(`ACTIONS_RUNTIME_TOKEN`)で認可されるため、フォークからのPRでも失敗しない見込み——ただしコメント投稿ステップ自体は上記の理由で引き続き失敗する(未検証、下記「未決事項」参照)。

## テスト方針

composite actionはYAML定義であり`cargo test`の対象にならないため、以下の方法で検証する:

1. **静的検証**: `action.yml`はYAMLとして構文検証する(`actionlint`は`.github/workflows/`配下のワークフローファイルのみを対象とし`action.yml`形式のcomposite actionメタデータには非対応のため、Python `yaml.safe_load`等で構文のみ検証)。呼び出し元ワークフロー側(`.github/workflows/xlsx-diff.yml`)は`actionlint`でも検証可能。
2. **シェルロジックの単体検証**: 「変更ファイルごとに`git show`でbase/headを取り出し`xlsxdiff`を起動してMarkdownへ連結し、`has-changes`/`changed-files-count`を`$GITHUB_OUTPUT`へ書く」というシェルスクリプト部分は`${{ github.action_path }}`・`${{ runner.temp }}`・`$GITHUB_OUTPUT`をローカルパスに置き換えれば`bash`だけでそのまま実行できる。実際に、ローカルの使い捨てgitリポジトリへA(追加)・M(変更)・D(削除)の3ステータスが混在する差分を作りこのスクリプトを実行して意図通りのMarkdownが生成されること、および変更あり/なし双方のケースで`has-changes`/`changed-files-count`が正しい値になることを確認済み。`--max-rows-per-sheet`/`--diff-mode`フラグが実際に`MarkdownOptions`へ届くことは、`cli/`側の統合テスト([`cli/tests/cli.rs`](../../cli/tests/cli.rs))で検証している(下記「依存関係」)——`action.yml`のシェルスクリプト部分としては、フラグの値をそのまま`"$BIN"`へ渡しているだけなので、フラグ自体の意味までは再検証しない。
3. **実際のGitHub Actions上での結合検証**: `.github/workflows/xlsx-diff.yml`自体を`uses: ./`で本actionを呼び出す形に書き換えた(下記「依存関係」参照)。これにより、`.xlsx`ファイルを変更する今後の任意のPRが本action全体(Rustツールチェーンのセットアップ・`github.action_path`起点でのビルド・`rust-cache`のワークスペース指定・コメント投稿)の実行結果を検証する回帰テストとして機能する——外部のテスト用リポジトリを別途用意しなくても、本リポジトリ自身がdogfoodingの場になる。**この結合検証だけが発見できた不具合が実際にあった**(上記「事後発見」参照)——単一ファイルのみ変更というケースは、ローカルのシェルスクリプト単体検証(複数ステータス混在の差分で検証していた)や静的検証では再現せず、GitHub Actions実機での複数回の試行によってのみ再現・特定できた。composite actionの検証において実機結合テストを省略できない理由の実例。
4. **`visual`モード固有の検証**: `screenshot.mjs`はローカルで実際にPlaywrightを実行し、生成したHTML→PNGの見た目を目視確認済み(Added/Modified双方、値変更・行列追加・結合・スタイル変更を含む)。`actions/upload-artifact@v4`の`artifact-url`が実際にコメント上でクリック可能なダウンロードリンクとして表示されること・私有(プライベート)リポジトリでリンク先が実際にダウンロードできることは、GitHub Actions実機上ではまだ確認できていない——下記「未決事項」参照。旧pushベース方式(`setup_images_worktree`/`push_image`)についてローカルの使い捨てbareリポジトリで検証していた内容は、その方式自体の廃止(上記「変更履歴」参照)に伴い意味を失ったため削除した。

## 未決事項 / オープンクエスチョン

1. **`cli`クレートの`crates.io`公開**: 本actionは`cli/`をソースからビルドする方式を採用しており、`cli/Cargo.toml`の`publish = false`は変更していない。公開する実利(例: 呼び出し元でのビルド時間短縮のため事前ビルド済みバイナリを配布する、[Issue #22](https://github.com/MinamiyamaKotaro/exceldiff/issues/22)・[Issue #28](https://github.com/MinamiyamaKotaro/exceldiff/issues/28))が生じるまでは現状維持とする。
2. **inputs/outputsの汎用化(続き)**: `files`/`comment`/`job-summary`/`max-rows-per-sheet`/`diff-mode`/`visual`inputsと`has-changes`/`changed-files-count`outputsは実装済み([Issue #24](https://github.com/MinamiyamaKotaro/exceldiff/issues/24))。以下は後続タスクとして残っている:
   - `changed-cells-count`output: 現状`xlsxdiff`はMarkdown文字列をstdoutへ書くのみで、追加/変更/削除セル数を機械可読な形で外に出していない。`cli/`側に集計出力(例: stderrへの`added=N modified=M deleted=D`行)を追加した上で、`action.yml`側でファイルごとに合算する必要がある。
   - `diff-scope`(コミット単位の差分表示): 現状は常にPRの`base.sha`⇔`head.sha`の累積差分のみ(新規追加されたファイルへのPR内修正は常に`Added`として扱われる——[Issue #23のコメント](https://github.com/MinamiyamaKotaro/exceldiff/issues/23)参照)。`push`(直前pushの`before`/`after`)や`commit`(PR内の各コミットを隣接ペアごとに差分)単位への切り替えは、コメント出力自体が「1PRにつき1セクション」から「複数セクション」へ構造が変わるため優先度を下げ、P2として別途着手する。
   - コメント文言・マーカー(`<!-- xlsx-diff-comment -->`)自体のカスタマイズは、具体的な要望が出るまでスコープ外のままとする。
   - `files`inputはgitパススペックとして実装した(シェルグロブではない)。GitHub Actionsの`paths:`トリガーフィルタ構文とは別物である点に注意——本actionはワークフローのトリガー自体を制御しない。
3. ~~**外部リポジトリからの実地検証**~~([Issue #23](https://github.com/MinamiyamaKotaro/exceldiff/issues/23)、解決済み): プレリリースタグ`v0.1.0-rc1`を切り、別リポジトリ(throwaway、`MinamiyamaKotaro/exceldiff-action-verify`)から`uses: MinamiyamaKotaro/exceldiff@v0.1.0-rc1`で実際に呼び出し、checkout→差分計算→PRコメント投稿までend-to-endで成功を確認した。この検証自体が実際にバグを発見した——README基本例どおり`permissions: pull-requests: write`のみを書いた呼び出し元では`contents`が黙って`none`になり`actions/checkout`が失敗する問題(上記「`permissions:`ブロック」参照、修正はPR #45)。セルフドッグフーディング(常に`visual: true`で`contents: write`を書く)だけでは決して発見できなかった不具合であり、外部からの実地検証を省略できない実例になった。
4. ~~**`xlsx-diff-images`ブランチの肥大化**~~(Issue #47の設計変更により解消): pushベース方式(コミットが無期限に増え続ける)自体を廃止し、artifactアップロード(90日で自動失効、`retention-days`で短縮も可能)へ置き換えたため、このリスクは前提ごと無くなった。
5. **`visual`モードのGitHub Actions実機検証**: ローカルでのシェルロジック検証・Playwrightでのスクリーンショット目視確認は完了しているが(上記「テスト方針」)、実際のGitHub Actions runner上で(a) Playwright/Chromiumのインストールが問題なく行われること、(b) `actions/upload-artifact@v4`のアップロードとその`artifact-url`がPRコメント上に正しいリンクとして表示されること、(c) プライベートリポジトリで実際にそのリンクをダウンロードできることは、本設計時点ではまだ確認できていない——次のステップとして実施する([Issue #47](https://github.com/MinamiyamaKotaro/exceldiff/issues/47))。
