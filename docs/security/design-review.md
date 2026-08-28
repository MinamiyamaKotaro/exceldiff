# `docs/design/`・CI設計 セキュリティレビュー: exceldiff固有領域 (2026-08-28)

*[English](design-review.en.md)*

**本レビューの位置づけ**: [code-review.md](code-review.md)冒頭に記載の通り、旧`docs/security/`はxlsxparser向けレビューの複製であり、exceldiff自身が追加した領域(`diff/`・`markdown.rs`・`grid.rs`・`cli/`・`action.yml`・`release.yml`)を一度もレビューしていなかった。旧ドキュメントは削除し、本レビューはこれらのexceldiff固有領域の設計・CI構成のみを対象とする。パーサー本体([architecture.md](../design/architecture.md)がカバーする5フェーズ・パイプライン)のセキュリティ設計レビューは、実装を共有する[`xlsxparser`側の`docs/security/`](https://github.com/MinamiyamaKotaro/xlsxparser/tree/master/docs/security)を参照。

コードレベルの発見的事実(実際に細工した`.xlsx`を通した検証)は[code-review.md](code-review.md)を参照——本レビューはそのFinding 1を踏まえた設計上の位置づけと、`action.yml`/`release.yml`(GitHub Actions構成そのもの)の設計レビューに焦点を当てる。

## 総合評価

`action.yml`/`xlsx-diff.yml`のシェルスクリプトは、GitHub Actions特有のスクリプトインジェクション(攻撃者が制御可能なPRタイトル・ブランチ名・コミットメッセージ等を`${{ }}`で`run:`ブロックへ直接埋め込み、シェルコマンドとして実行させる手口)への対策が一貫している——攻撃者が実際に制御しうるコンテキスト値(PRのブランチ名・タイトル等)は`run:`ブロックの`${{ }}`直接展開に一箇所も現れず、すべて`env:`経由でシェル変数化されてから`"$VAR"`として引用符付きで参照されているか(Issue #43で追加された`diff-scope: commit`のコミットsubjectも`git log`出力を`$(...)`で変数へ束縛し、直接YAML展開はしていない)、他Actionへの`with:`パラメータとして型付きで渡されているかのいずれかである。この規律は最近実装された`Resolve xlsxdiff binary`ステップ(Issue #28)にも一貫して適用されている。

一方、Issue #28で実装された事前ビルド済みバイナリのダウンロード経路には、設計上の既知の限界(Finding 1として記録するが、対応不要と判断)がある——チェックサム検証がダウンロード元と同一の信頼境界(同じGitHub Release)から取得されるため、正規のリリースパイプライン自体が侵害された場合の防御にはならない。

## Findings

### Finding 1(情報提供): 事前ビルド済みバイナリのチェックサム検証は、転送時の破損は検出できるが、リリース自体の侵害は検出できない

* **深刻度**: Informational(既知の設計上のトレードオフとして受容——対応不要)
* **対象**: `action.yml`の「Resolve xlsxdiff binary」ステップ、`.github/workflows/release.yml`。
* **詳細**: `action.yml`は`https://github.com/MinamiyamaKotaro/exceldiff/releases/download/{tag}/xlsxdiff-{tag}-{target}.tar.gz`とその`SHA256SUMS`を同一のGitHub Releaseから取得し、両者を突き合わせて検証してから実行する。この検証は転送時の破損・不完全なダウンロードは確実に検出するが、`SHA256SUMS`自体が攻撃者(例えばリポジトリのメンテナ権限やCIのシークレットを奪取した攻撃者)によって書き換えられたリリースからは、悪意あるバイナリとその「正しい」チェックサムの両方が一貫して取得されてしまい、検証は素通りする。
* **リスクシナリオ**: リポジトリの`GITHUB_TOKEN`やメンテナ権限が侵害された場合、攻撃者は`release.yml`を経由せず直接悪意あるバイナリを含むReleaseを作成・上書きでき、本Actionの利用者全員がそれをダウンロード・実行してしまう——ただしこれは「リポジトリ自体が侵害された場合」という、ソースからビルドする従来方式でも(悪意あるコミットがpushされれば)本質的に同じ影響を受ける前提であり、事前ビルド済みバイナリ配布によって新たに生じたリスクというより、既存の信頼境界(「このリポジトリのメンテナを信頼する」)が形を変えて現れたものと捉えるべきである。真に閉じるには、リリースと切り離された鍵(例: `cosign`によるkeyless署名、または本リポジトリのGitHub Releaseとは別の場所に保管したGPG鍵)による署名検証が必要になるが、現状の脅威モデル(信頼されたメンテナ1名が運営するプロジェクト)に対しては不釣り合いに重いと判断する。
* **対応**: 今回は対応不要と判断し、情報提供として記録するに留める。将来的に外部コントリビューターの増加やサプライチェーン攻撃への懸念が高まった場合に再検討する。

## 良好だった点

* **GitHub Actionsスクリプトインジェクション対策が一貫している**: `action.yml`/`xlsx-diff.yml`の全`run:`ブロックを確認したところ、攻撃者が制御可能な値(PRのブランチ名・タイトル・コミットメッセージ等)が`${{ }}`で直接シェルスクリプトへ展開されている箇所は無かった。`BASE_SHA`/`HEAD_SHA`(コミットSHA、攻撃者が任意文字列を選べる値ではない)を含め、必要な値はすべて`env:`経由でシェル変数化されてから引用符付きで参照される——GitHub Actions特有のスクリプトインジェクション手口に対する標準的な防御パターンが一貫して守られている。
* **`diff-scope: commit`のコミットsubject埋め込みは二重に安全**: Issue #43で追加された`echo "## Commit \`${short}\` — ${subject}"`は、`$subject`が`git log`の出力を`$(...)`でシェル変数へ束縛したものであり、YAML展開ではなくシェル変数展開である時点でGitHub Actions固有のスクリプトインジェクションの対象外(シェルの変数展開は、展開後の内容を新たなシェル構文として再解釈しない)。加えてMarkdownの不整合な閉じバックティックによるコメント破壊についても、実装時に対策済み(バックティックのエスケープ、PR #54)。
* **`release.yml`のパッケージング処理に危険な操作は無い**: `tar -czf`/`sha256sum`/`gh release create`はいずれも固定引数か`${{ github.ref_name }}`(タグをpushした本人が決める値であり、PR経由の攻撃者コンテキストではない)・`matrix.target`(ワークフロー定義自体に列挙された固定値)のみを使っており、攻撃者制御可能な値は一切関与しない。
* **`xlsxdiff`の依存グラフに`unsafe`なネイティブ拡張・Cツールチェーンが一切無い**(Issue #28検証時に`cargo tree`で確認済み——`zip`crateの`deflate`featureも`flate2`→`zlib-rs`という純Rust実装)ため、事前ビルド済みバイナリのクロスコンパイル自体が`cross`/Dockerを介さないシンプルな構成で完結しており、ビルドパイプライン自体の攻撃対象領域が小さい。

## 対象外

* パーサー本体(`container/`・`parse/`・`model/`・`resolve/`・`json.rs`・`pipeline.rs`・`error.rs`・`lib.rs`)の設計レビュー——xlsxparser側の`docs/security/`を参照。
* `quick-xml`・`zip`・`serde`/`serde_json`・`thiserror`・`rusqlite`・`dtolnay/rust-toolchain`・`Swatinem/rust-cache`・`peter-evans/*`・`actions/*`各Marketplace Actionのサプライチェーン/依存関係脆弱性——個別のバージョンピン管理・Dependabot等の一般的な運用課題であり、本プロジェクト固有の設計判断を伴わないため対象外。
* コードレベルの検証(実際に細工したファイルを通した再現)は[code-review.md](code-review.md)を参照——本ドキュメントは設計・CI構成レベルの観点に限定する。
