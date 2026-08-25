# `diff/row_alignment.rs` 設計書

*[English](row_alignment.en.md)*

`src/diff/row_alignment.rs` に対応する設計書。[`diff/engine.rs`](engine.md) の座標一致ベースの差分（`diff_workbooks`）に対し、上限付き・オプトインの行アライメントベース差分（`diff_workbooks_aligned_rows`）を提供する（[Issue #4](https://github.com/MinamiyamaKotaro/exceldiff/issues/4)）。行の挿入・削除が起きても、それより下側のセル全てがカスケードして誤差分化されることを避けるのが目的。

## 背景・経緯

[Issue #3](https://github.com/MinamiyamaKotaro/exceldiff/issues/3)のPoC（`poc/issue3-poc`）が実装していた行+列同時の2D LCSアライメントは、上限チェックが一切無く、O(distinct_rows² + distinct_cols²)（4,000行で約13秒・128MB）というコストが本クレートの設計目標（行・列数が極端に多い「方眼紙Excel」への最適化）と正面から矛盾していたため、`engine.rs`はこれを採用せず座標一致ベースをデフォルトとした（詳細は[engine.md](engine.md)参照）。アライメントベースの差分自体は上限付きオプトイン機能として、列（[Issue #5](https://github.com/MinamiyamaKotaro/exceldiff/issues/5)、実装済み）と行（本Issue #4）に分けて別途検討することとされていた。

列アライメントで採用したO(distinct_cols²)のDPは、そのまま行に転用できない。Excelは1シートあたり最大1,048,576行を許容する（列は最大16,384）ため、列では安全に上限化できたアルゴリズムが、実務でよくある行数では通用しない。Issue #3のPoCが検証した通り、O(distinct_rows²)は数万行規模で数分〜数時間、数十GBのメモリを要求する。

Issue #4では、`poc/issue4-poc`から`poc/issue4-poc-v8`まで8ラウンドにわたるPoC検証（いずれも使い捨てで非コミット、GitHub Issueのコメント履歴に詳細あり）を経て、以下の設計に到達した:

1. **ハッシュアンカー + patience diff（`git diff --patience`と同じ考え方）でO(n log n)に抑える**: 内容が両側で一意な行をアンカーとし、その順序保存部分列（LIS）を確定マッチとする。O(n)で構築、O(k log k)（kはアンカー数）で整列できる。O(distinct_rows²)のDPと異なり、実務規模（100万行）でも高速（favorable caseで1秒未満）。
2. **アンカー間の未解決区間はMyers diffで解決するが、バックトレースは全ステップをデコードする**: `poc/issue4-poc-v2`で見つかった不具合——スネーク（対角線）ステップだけを記録し、残りを配列インデックスでの位置合わせに頼っていた——は、Myersの予算内（`found == true`）でも大規模な誤`Modified`カスケードを再現した(5,000行の重複行シートで18,240件の誤検出、詳細はIssue #4コメント参照)。`poc/issue4-poc-v4`で全ステップ(対角線・垂直・水平)を直接デコードするよう修正し、この不具合を解消した。
3. **予算超過時のフォールバックは「安全な全削除+全追加」**: 位置合わせによるフォールバック（v1/v2の不具合と同じ危険性を持つ）ではなく、対応関係を確認できなかった行はすべて素朴なAdded/Deletedとして報告する——誤`Modified`を絶対に生まない代わりに最小差分ではなくなる、という`MIN_DISTINCT_FOR_CONTENT_MATCH`と同種の「安全だが最適でない」トレードオフ。
4. **内容類似度によるペアリングは、区間サイズに上限を設けて初めて安全になる**: `poc/issue4-poc-v6`は隣接性ではなく内容類似度（列値の一致率）でDeleted/Insertedをペアリングする改善を提案したが、区間全体に総当たりで適用するとO(span²)のコストが乗り、`poc/issue4-poc-v7`の実測では未解決区間が大きい場合(シートの一部が丸ごと置き換わるケース)にv5比29〜36倍の速度低下（4,000行規模のブロックで9秒超）を引き起こした。`poc/issue4-poc-v8`で区間サイズに上限(`CONTENT_SIMILARITY_SPAN_CAP`)を設け、この問題を解消した(v5相当の速度まで回復)。
5. **内容類似度が同点になる場合の対応関係は原理的に一意に決まらない**: `poc/issue4-poc-v7`/`v8`で、行の並び順という偶然の要因で対応関係(どの行がどの行に変化したか)が入れ替わることを最小構成で実証した。集計される差分件数自体は正しいままだが、この曖昧さはヒューリスティックでは解消できない構造的な限界であり、「既知の限界」として受け入れる(下記「内容類似度ペアリングの一意性」参照)。

本実装は、上記の設計と、実装スコープを「行のみ」に限定した設計判断（下記「列は再整列しない」参照）を反映している。

## 責務・スコープ

- 座標一致ではなく内容一致で行を対応付けた上でセル差分を計算する `diff_workbooks_aligned_rows` を提供する
- 対応付けの予算制チェック（`RowAlignmentLimits`）を、実際のO(gap²)照合処理を始める前に行い、超過時は`Err(Error::RowAlignmentCostTooHigh)`を返す（黙って`diff_workbooks`相当へフォールバックしない——`diff::col_alignment`と同じ設計判断）
- 片側にしか存在しないシートは`diff::engine::diff_sheet`をそのまま再利用する
- 結合セルの差分（`SheetDiff::merges`）は`diff::engine::diff_merges`をそのまま再利用し、行アライメントの対象にしない
- **含まない責務**: 座標一致ベースの差分計算そのもの（[`engine.rs`](engine.md)）、列アライメント（[`col_alignment.rs`](col_alignment.md)、Issue #5で実装済み）との統合（下記「列は再整列しない」参照）、`diff::storage`へのアライメント結果（特に`CellDiff::old_row`）の永続化（[storage.md 未決事項6](storage.md)と同種、未着手）

## 列は再整列しない（Issue #5は別issue・実装済みだが未統合）

本実装は行のみをアライメントし、列は常に座標一致のまま扱う（`diff::col_alignment`が列のみをアライメントし行を座標一致のまま扱っていたのと対称）。行と列の挿入が同時に起きるシートを両方アライメントする組み合わせはスコープ外——`diff::col_alignment`のdocコメント「行は再整列しない」節が既に指摘している統合ポイント（`diff_matched_columns`のマージジョイン、`count_matching_rows`への行マッピングの反映）を、逆方向（行アライメント側から列マッピングを受け取る）にも同様に用意する必要があり、双方向の統合は将来の課題として残す。

## 内容類似度ペアリングの一意性（既知の限界）

複数の候補行が削除された行に対して**同点の**類似度を持つ場合（例: 複数行が同じ説明列を共有し、1つの値列だけが異なる）、どの行がペアになるかはMyersのバックトレースが生成する順序——ハッシュ値・配列順序に依存する偶然の要因——で決まる。`poc/issue4-poc-v7`/`v8`で最小構成により実証済み: 集計される`added`/`modified`/`deleted`件数自体は変わらないが、「どちらの行がどちらに変化したか」という対応関係がこの同点ケースでは曖昧になり得る。位置の近さ等どんなタイブレークルールを使っても一般には解消しない——類似度ベースのマッチング手法に内在する限界であり、`diff::col_alignment`の`MIN_DISTINCT_FOR_CONTENT_MATCH`ゲートが「偶然の一致と真の低カーディナリティ変化を区別できない」ことを受け入れているのと同じ性質のトレードオフとして受容する。

## 主要な型・関数

```rust
pub struct RowAlignmentLimits {
    pub max_gap_myers_d: usize, // 1ギャップあたりのMyers編集距離予算(MAX_GAP_MYERS_D_CEILING独立チェックあり)
    pub max_cost: usize,        // 2 * max(distinct_rows_base, distinct_rows_target) * max_gap_myers_d の上限
}

pub fn diff_workbooks_aligned_rows(
    base: &Workbook,
    target: &Workbook,
    limits: RowAlignmentLimits,
) -> Result<WorkbookDiff> { ... }
```

アルゴリズム（1シートあたり、両側に存在する場合）:

1. 予算チェックはメモリ上限を先にチェックする: `limits.max_gap_myers_d > MAX_GAP_MYERS_D_CEILING`なら即座にエラーを返す。これは行数に依存しないメモリ上限（`myers_diff_gap`の`flat_trace`バッファはO(max_gap_myers_d²)で、行数を掛けた時間予算だけでは、呼び出し側が`max_cost`と`max_gap_myers_d`を両方大きく設定した場合にこのバッファ単体がGB単位に膨れ上がるのを防げない——`diff::col_alignment::MAX_COLUMN_PAIR_COUNT`が列アライメントの`max_cost`単体では不十分だったのと同じ理由。PR #21のレビューで指摘）。
2. `iter_cells()`を1回走査し、distinct行数をbase/target双方について求める（`distinct_row_count`、真のO(cells)——`Sheet::iter_cells()`が既に行昇順であることを利用し、行番号の遷移回数を数えるだけで済ませる。`BTreeSet`への挿入はO(log distinct_rows)の追加コストを払うため使わない。PR #21のレビューで指摘）。
3. 時間予算チェック: `2 * max(distinct_rows_base, distinct_rows_target) * limits.max_gap_myers_d > limits.max_cost`なら、実際の照合処理を始める前に即座にエラーを返す（`MAX_ROW_ALIGNMENT_COST`のdocコメントに実測根拠あり）。
4. 行ごとの内容を真のO(cells)の単一走査で抽出する（`row_contents`）——`iter_cells()`が既に行昇順であることを利用し、行番号が変わるたびに直前の行の蓄積を確定させる方式で、`BTreeMap`によるバケツ分けは行わない（`distinct_row_count`と同じ理由でPR #21のレビューにより修正）。各`RowContent`は列→セルの`Vec`、`RandomState`（プロセスごとにランダム化されたシード）でハッシュした内容シグネチャ（セル走査と同時にインクリメンタルに計算）、書式のみセルを除いた実データ数を持つ。
5. 共通prefix/suffixをO(1)/行でトリムする——両端から見て同一シグネチャが続く限り、O(n²)の作業を一切せずに確定マッチとする。
6. トリム後の「アクティブ領域」内で、シグネチャが両側で一意な行をアンカー候補とし、patience-sort LIS（`lis_indices`）で順序を保った最大の確定マッチ集合を求める（`align_rows`）。
7. 確定マッチの間の各ギャップを`myers_diff_gap`でMyers diffにより解決する。バックトレースは対角線（Match）・垂直（Inserted）・水平（Deleted）の全ステップを直接デコードする——スネークだけ記録して残りを位置合わせで穴埋めする近道は取らない。予算(`max_gap_myers_d`)を超えた場合は`fill_gap_no_match`で対象区間全体を安全に全削除+全追加として報告する。
8. Myersが解決したがシグネチャの完全一致では説明できなかった残余のDeleted/Insertedの連続区間は、`merge_leftover_spans_by_content_similarity`で内容類似度によるペアリングを試みる——区間長が`CONTENT_SIMILARITY_SPAN_CAP`を超える場合はO(span²)コストを避けるためスキップし、安全な全削除+全追加のまま残す。類似度計算自体（`row_similarity`）は1区間あたり最大`CONTENT_SIMILARITY_SPAN_CAP²`回呼ばれるため、`HashMap`を毎回確保する実装ではなく、両側の`RowContent::cells`が既に列昇順であることを利用したマージジョインで実装している（PR #21のレビューで指摘・修正）。
9. マッチした行ペアはマージジョインでセル差分を計算し(`diff_matched_rows`)、行がシフトしていれば`CellDiff::old_row`を付与する。マッチしなかった行の全populatedセルはAdded/Deleted扱いにする。

## 依存関係

- 依存先: [`diff/engine.rs`](engine.md)（`diff_sheet`/`diff_merges`/`visibility_diff`を`pub(crate)`化して再利用）、[`diff/model.rs`](model.md)（`CellDiff`/`SheetDiff`/`WorkbookDiff`/`DiffStatus`。`diff::col_alignment`と同じ`CellDiff`型を再利用し`old_row`のみ実際に populate する）、[`error.rs`](../error.md)（`Error::RowAlignmentCostTooHigh`）、[`json.rs`](../json.md)（`cell_value_to_json`/`style_to_json`）、[`model/sheet.rs`](../model/sheet.md)（`Sheet::iter_cells`）
- 依存元: [`diff/mod.rs`](mod.md)（`diff_workbooks_aligned_rows`/`RowAlignmentLimits`を再エクスポート）

## エラー処理方針

- `diff_workbooks_aligned_rows`は`Result<WorkbookDiff>`を返す（`diff_workbooks`と異なりfallible）——予算超過時に`Error::RowAlignmentCostTooHigh`を返すため。呼び出し側がオプトインした処理が実際に完了したかどうかを黙って隠さない、という`diff::col_alignment`と同じ設計判断。自動フォールバックが必要な呼び出し側は、このエラーを`match`して`diff_workbooks`を自分で呼び出せばよい。

## テスト方針

`src/diff/row_alignment.rs`内の単体テスト（`Sheet`/`Workbook`を公開モデルAPI経由で直接構築）:

- 行挿入・削除でカスケードが起きないこと（`row_insertion_does_not_cascade_when_aligned`、`row_deletion_does_not_cascade_when_aligned`）。シフトした行内の真の値変更が`old_row`付きで正しく検出されること、シフトしていない行では`old_row`が`None`のままであること（`old_row_is_absent_when_the_matched_row_did_not_shift`）
- 低カーディナリティ・重複行シートに編集が散在するケースでカスケードが起きないこと（`low_cardinality_duplicated_rows_with_scattered_insertion_do_not_cascade`）——`poc/issue4-poc-v2`で見つかった不具合の直接の回帰テスト
- 2行以上連続する変更行がそれぞれ独立して`Modified`として検出されること（`consecutive_modified_rows_are_each_detected_as_modified`）——`poc/issue4-poc-v6`で見つかった限界（隣接性のみに基づくマージ規則の失敗）の直接の回帰テスト
- 予算超過が正しく`Error::RowAlignmentCostTooHigh`を返すこと（`row_alignment_cost_too_high_is_reported_fail_fast`）
- 片側のみに存在するシートがアライメント経由でも座標一致エンジンへ委譲されること（`sheet_present_on_only_one_side_reuses_the_coordinate_engine_through_alignment`）
- `diff_workbooks`（デフォルトエンジン）が本機能の追加によって挙動を変えていないこと（`diff_workbooks_default_behavior_is_unaffected_by_row_alignment_existing`）

[`tests/diff.rs`](../../../tests/diff.rs): `row_insertion_does_not_cascade_when_aligned_end_to_end`が、実際の`.xlsx`相当バイト列を`parse_workbook_reader`でパースした上でカスケード回避を再検証する。

パフォーマンス（実測、release build、Apple Silicon。PoC実装に対する計測——本体実装への移植後の再計測は今後の課題）:

- Myers diffの単体コスト（`poc/issue4-poc-v7`、共有シグネチャの無い完全置換ブロック、ブロックサイズB、コスト=4B²）: B=4,000（コスト64,000,000）で282.44ms、コスト正規化した単価は約4.4e-6〜5.5e-6ms/単位でほぼ一定。`MAX_ROW_ALIGNMENT_COST`はこの実測単価に余裕を見て導出した（詳細は`src/diff/row_alignment.rs`の`MAX_ROW_ALIGNMENT_COST`docコメント参照）。
- 内容類似度ペアリングの区間サイズ上限なし版は、同条件でv5（隣接ペアのみマージ）比29〜36倍遅い（`poc/issue4-poc-v7`）。`CONTENT_SIMILARITY_SPAN_CAP`導入後（`poc/issue4-poc-v8`）はv5比0.8〜1.4倍まで回復。
- 実務的な編集パターン（局所的な挿入・削除・変更が散らばる、重複の少ないシート）では、1,000,000行でも1秒未満で完走する（`poc/issue4-poc`の一連の計測）。

## 未決事項 / オープンクエスチョン

1. **列アライメントとの統合**（[Issue #5](https://github.com/MinamiyamaKotaro/exceldiff/issues/5)）: 上記「列は再整列しない」節の通り、双方向の統合は未設計・未実装。
2. **`old_row`のSQLite永続化**: `old_col`（[storage.md 未決事項6](storage.md)）と同様、`diff::storage`は現状`old_row`を永続化しない。呼び出し側が`diff_workbooks_aligned_rows`の結果を永続化したい場合、現状は`WorkbookDiff`自体を別途JSONとして保存する必要がある。
3. **内容類似度ペアリングの一意性**: 上記「内容類似度ペアリングの一意性」節の通り、現状は既知の限界として受容している。実用上より厳密な対応付けが必要になった場合（例えば行に安定した識別子列がある場合にそれを優先的なマッチングシグナルとして使う等）は別途検討する。
4. **`RowAlignmentLimits`のデフォルト値の妥当性**: `MAX_ROW_ALIGNMENT_COST`/`DEFAULT_MAX_GAP_MYERS_D`はPoCでの実測に基づくが、本体実装に移植した後の再計測（特に`row_contents`のメモリコストを含めたエンドツーエンドの計測）はまだ行っていない。実運用のフィードバックを踏まえて調整する可能性がある。
