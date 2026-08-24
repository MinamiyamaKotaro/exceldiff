# `diff/alignment.rs` 設計書

*[English](alignment.en.md)*

`src/diff/alignment.rs` に対応する設計書。[`diff/engine.rs`](engine.md) の座標一致ベースの差分（`diff_workbooks`）に対し、上限付き・オプトインの列アライメントベース差分（`diff_workbooks_aligned_columns`）を提供する（[Issue #5](https://github.com/MinamiyamaKotaro/exceldiff/issues/5)）。列の挿入・削除が起きても、それより右側のセル全てがカスケードして誤差分化されることを避けるのが目的。

## 背景・経緯

[Issue #3](https://github.com/MinamiyamaKotaro/exceldiff/issues/3)のPoC（`poc/issue3-poc`）が実装していた行+列同時の2D LCSアライメントは、上限チェックが一切無く、O(distinct_rows² + distinct_cols²)（4,000行で約13秒・128MB）というコストが本クレートの設計目標（行・列数が極端に多い「方眼紙Excel」への最適化）と正面から矛盾していたため、`engine.rs`はこれを採用せず座標一致ベースをデフォルトとした（詳細は[engine.md](engine.md)参照）。アライメントベースの差分自体は上限付きオプトイン機能として、行（[Issue #4](https://github.com/MinamiyamaKotaro/exceldiff/issues/4)、未着手）と列（本Issue #5）に分けて別途検討することとされていた。

Issue #5では、`poc/issue5-poc`から`poc/issue5-poc-v4`まで5ラウンドにわたるPoC検証（いずれも使い捨てで非コミット、GitHub Issueのコメント履歴に詳細あり）を経て、以下2点の未決事項に対する具体的な結論が得られた:

1. **上限値の決め方**: `align_columns`のコストは distinct列数の2乗ではなく、`distinct_cols_base × distinct_cols_target × max_row`に比例する。500行のシートで安全な列数上限（約200列）を50,000行のシートにそのまま適用すると約10倍危険になるため、単一の列数上限（`MAX_MERGE_REGIONS`型のパターン）では不十分——distinct列数と行数の積で予算制する必要がある。
2. **マッチングヒューリスティックの頑健性**: 「1セルでも値が一致すれば候補」という緩い判定は精度15〜38%しかない。「一致セル数が閾値（列長の20%、最小2）以上」への置き換えでdistinct値10以上のケースは精度100%まで改善するが、超低カーディナリティ列（distinct値2〜4、真偽値/ステータスフラグ等）はヘッダー無しでは**どんな内容ベースの類似度スコアでも**安全に判定できない（無制限・全対比較のSequence-LCSでも誤マッチ率122%を記録）。この場合に必要なのは閾値の調整ではなく、**列自体のカーディナリティが低ければコンテンツベースの照合対象から事前に除外する**という設計（`MIN_DISTINCT_FOR_CONTENT_MATCH`）だった。

本実装は、この2つの結論と、実装スコープを「列のみ」に限定した設計判断（下記「行は再整列しない」参照）を反映している。

## 責務・スコープ

- 座標一致ではなく内容一致で列を対応付けた上でセル差分を計算する `diff_workbooks_aligned_columns` を提供する
- 対応付けの予算制チェック（`ColumnAlignmentLimits`）を、実際のO(cols²)照合処理を始める前に行い、超過時は`Err(Error::ColumnAlignmentCostTooHigh)`を返す（黙って`diff_workbooks`相当へフォールバックしない——呼び出し側が明示的にオプトインした処理である以上、実際にアライメントが行われたかどうかを呼び出し側から見えなくすべきではないという判断）
- 片側にしか存在しないシートは`diff::engine::diff_sheet`をそのまま再利用する（新規/削除されたシート全体には、そもそもアライメントする対象が無い）
- 結合セルの差分（`SheetDiff::merges`）は`diff::engine::diff_merges`をそのまま再利用し、列アライメントの対象にしない(下記「結合セルは列アライメント非対応」参照)
- **含まない責務**: 座標一致ベースの差分計算そのもの（[`engine.rs`](engine.md)）、行アライメント（[Issue #4](https://github.com/MinamiyamaKotaro/exceldiff/issues/4)、未着手——下記「行は再整列しない」参照）、`diff::storage`へのアライメント結果（特に`CellDiff::old_col`）の永続化（[storage.md 未決事項6](storage.md)参照）

## 行は再整列しない（Issue #4は別issue・未実装）

Issue #4（行挿入/削除検出）は本設計時点でコメント0件・実装ゼロの未着手issueである。そのため本実装は列のみをアライメントし、行は常に座標一致のまま扱う。これは、PoCが検討していたBag-of-Values・Sequence-LCSといった「行シフトに対して不変な」列マッチング手法が、そもそも**行が同時にシフトする**という前提を解決するために存在していたことと関係する——行が動かない前提であれば、単純な「同一行番号でのセル値比較」で十分であり、より複雑な手法は必要ない。将来Issue #4が実装される際は、行の対応付け（`base_row -> target_row`のマッピング）を、`diff_matched_columns`のマージジョインだけでなく`count_matching_rows`（`column_match_score`が閾値経路・完全一致救済の両方で使う、同じ「同一行番号での値比較」を行う関数）にも反映する必要がある——PRレビュー(Copilot)で「列マッチング自体の作り直しは不要」という以前の記述が不正確であると指摘された。`count_matching_rows`を更新しないと、行がシフトした列同士は`diff_matched_columns`に到達する前の時点でスコアが低すぎて一致とすら認識されない。

## 結合セルは列アライメント非対応

`SheetDiff::merges`は`diff::engine::diff_merges`（座標一致ベース）をそのまま呼び出す。列シフトをまたぐ結合セルの対応付け（例: 列挿入前に存在した結合の起点セルが列シフトした場合）はスコープ外——Issue #5自体がセル差分のカスケード回避を主眼としており、結合セルのアライメントは別途要求されていない。

## 主要な型・関数

```rust
pub struct ColumnAlignmentLimits {
    pub max_cost: usize,         // distinct_cols_base * distinct_cols_target * sample_rows の上限
    pub max_column_pairs: usize, // distinct_cols_base * distinct_cols_target 単体の上限(行数に依存しないメモリ上限)
}

pub fn diff_workbooks_aligned_columns(
    base: &Workbook,
    target: &Workbook,
    limits: ColumnAlignmentLimits,
) -> Result<WorkbookDiff> { ... }
```

アルゴリズム（1シートあたり、両側に存在する場合）:

1. `iter_cells()`を1回走査し、distinct列インデックスの集合をbase/target双方について求める。
2. 予算チェックは2段階（PR #20のレビューで判明——後述「実装レビューで見つかった問題」参照）: まず`cols_base.len() * cols_target.len() > limits.max_column_pairs`（行数に依存しない、`scores`/`dp`行列のメモリ上限）、次に`cols_base.len() * cols_target.len() * sample_rows > limits.max_cost`（照合時間の上限）。いずれかを超えたら、O(cols²)の照合処理を始める前に即座にエラーを返す。
3. 列ごとの内容を抽出する（`ColumnContent`: 行→セルの`Vec`（`iter_cells()`が既に行昇順であることを利用し、`BTreeMap`より安い）、ヘッダー(1行目の`Text`値のみ——数値データが1行目から始まる「ヘッダー無し」シートで数値の偶然一致を誤ってヘッダー一致と扱わないため)、コンテンツ照合可否(`MIN_DISTINCT_FOR_CONTENT_MATCH`以上のdistinct値を持つか))。
4. 候補ペアをスコアリング(`column_match_score`): スコアは単一の`u64`ではなく、レキシコグラフィック（辞書式）に比較される`(u64, u64)`のタプル(`Score`)。両側のヘッダーが一意かつ一致すれば`(1, match_count)`（第1要素`1`により、他のどんな量のコンテンツ一致の合計よりも常に優先される——後述「実装レビューで見つかった問題」18番参照）。双方が`MIN_DISTINCT_FOR_CONTENT_MATCH`以上のdistinct値を持ち、一致セル数が閾値（列長の20%、最小2）以上なら`(0, match_count)`。どちらにも該当しない（低カーディナリティ）場合でも、**完全一致**（両側の行数が等しく、全populated行が一致）なら`(0, populated_count)`として候補とする——後述「実装レビューで見つかった問題」参照。
5. 候補ペアの重み付きLCS的DPで、順序を保ったまま最適な列対応付けを求める（`align_columns`）。DPの遷移は各セルで「対角線（マッチ採用）」「上」「左」の3値の最大値を取る標準的な重み付きLCS漸化式——これも後述のレビューで修正した箇所。
6. マッチした列ペアはマージジョインでセル差分を計算し(`diff_matched_columns`)、列がシフトしていれば`CellDiff::old_col`を付与する。マッチしなかった列のうち、同一の列番号を共有するDeleted/Insertedペアは座標ベースで直接diffし(上記「実装レビューで見つかった問題」10番参照)、それ以外は全セルをAdded/Deleted扱いにする。

## 実装レビューで見つかった問題（PR #20）

GitHub Copilotの自動PRレビューが6ラウンドにわたり計18件の重大な問題を指摘し、いずれも検証の上で修正した:

1. **DPの遷移が正しい重み付きLCS漸化式になっていなかった**: `score > 0`の場合に無条件で対角線を採用しており、「上」「左」との比較を行っていなかった。1つの base 列に対し複数の候補 target 列があり、弱いマッチ（例: スコア2）が強いマッチ（例: スコア10）より先に評価されると、弱い方が採用され、本来の強いマッチが誤って挿入として報告される具体的な反例が指摘された。3値の`max`を取る標準的な漸化式に修正。
2. **書式のみのセル（値が`None`）が誤って「一致」扱いされていた**: `Option<CellValue>`の派生`PartialEq`では`None == None`が`true`になるため、`count_matching_rows`が値の無い書式専用セル同士を無条件に一致カウントしていた。両側とも`Some`である場合のみ一致とみなすよう修正。
3. **予算チェックが行数に依存しないメモリ量を考慮していなかった**: `max_cost`は行数で重み付けされるため、行数が極端に少なく列数が極端に多いシート（例: 1行×3,162列×3,162列）でもコスト上は上限内に収まる一方、`scores`/`dp`行列自体は行数に関わらずO(cols²)のメモリ（実測約160MB）を要求してしまう。`max_column_pairs`という行数非依存の第2の上限を追加。
4. **低カーディナリティゲートが「変化していない列」まで対象外にしていた**: ヘッダー無し・distinct値8未満の列は元実装では一律コンテンツマッチング対象外としていたため、実際には一切変化していない（単に列がシフトしただけの）低カーディナリティ列まで、丸ごと削除+丸ごと追加として報告されてしまっていた——これはまさに本機能が回避すべきカスケードそのものである。完全一致（全populated行が両側で一致し、行数も一致）の場合のみ例外的に許可する「完全一致救済」を追加し、この回帰を修正した。
5. **閾値・完全一致救済の判定に`cells.len()`（書式のみのセルを含む総数）を使っていた**: `min_len`（部分一致閾値の分母）と`long_enough`（完全一致救済のゲート）の両方が、実データを持つセル数ではなく書式のみのセルも含めた総エントリ数を使っていた。Excelでは取り込んだ表の未使用範囲全体に罫線・背景色だけを適用しているようなケースが珍しくなく、そうした大きな「書式のみの空白範囲」があると、実データ8個で本来一致するはずの列が`min_len`の水増しにより閾値を満たせず誤って棄却されたり、逆に実データ数個+書式のみセルで水増しされた列が完全一致救済の最低サンプル数（統計的安全マージン）を不当にクリアしてしまったりする。`ColumnContent::populated_count`（実データを持つセル数、列ごとに1回計算）を追加し、両方のゲートをこちらに切り替えた。
6. **命名・ドキュメントの整合性**: `Error::TooManyDistinctColumnsForAlignment`を`Error::ColumnAlignmentCostTooHigh`へ改名した（`count`/`limit`→`cost`/`limit`。実際にはコスト積であって列数そのものではないため）。また、存在しない`ColumnAlignmentLimits::MAX_COLUMN_ALIGNMENT_COST`（実際は`diff::alignment`モジュールレベルの定数であり、関連定数ではない）を指すdocコメントの誤りも修正した。
7. **完全一致救済のDPスコアに`cells.len()`（書式のみセル込み）を使っていた**: 5番の修正で適格性ゲートは`populated_count`に切り替えたが、DPに渡すスコア自体は依然`b.cells.len()`のままだった。`align_columns`はスコアの合計を最大化するため、書式のみの巨大な空白範囲を持つ完全一致候補が、水増しされたスコアで本来の（閾値経路による、より正当な）マッチを押しのけてしまう恐れがあった。スコアも`populated_count`ベースに修正。
8. **完全一致救済が「定数列」を許容していた**: `long_enough`（`populated_count >= 8`）はセル数のみを見ており、値の多様性を一切見ていなかった。全行が同一値（例: 全て`0`）の「定数列」は、8行以上あれば`long_enough`を満たしてしまうが、これは「偶然の一致確率が無視できるほど低い」という完全一致救済の安全性の前提そのものが成立しないケース——同じ定数値を持つ別の無関係な列とは**確率ではなく必然的に**完全一致してしまう。具体例: base列Aが全て`0`、target側で新規の無関係な列Aも全て`0`、Aの本当の継続である（列Bへシフトした）列は値が全て`1`に変化——この場合、修正前はbase列Aが誤って新規のtarget列Aとマッチし、本来検出すべき列Bへの変化（0→1）が握りつぶされてしまう。`ColumnContent::has_at_least_two_distinct_values`を追加し、完全一致救済の前提条件に加えた。
9. **`identical_low_cardinality_column_with_a_shared_blank_cell_still_produces_no_diff`テストが完全一致救済経路を実際には検証していなかった**: テストデータが8個の*相異なる*実数値を使っていたため、`eligible_for_content_match`（distinct値8以上）を満たしてしまい、実際には低カーディナリティ・完全一致救済経路ではなく通常の閾値経路を通過していた。真に低カーディナリティ（2値のboolean風パターン）なデータに修正し、`populated_count`が7個（閾値未達）の否定側テスト（`seven_populated_values_plus_a_blank_does_not_clear_the_exact_match_floor`）も追加した。
10. **列がシフトしていない（コンテンツ照合が成立しないだけの）未整列列が、座標一致ベース差分にフォールバックしていなかった**: マッチしなかった列は無条件で丸ごとDeleted/Insertedとして報告していたため、ヘッダー無し・低カーディナリティ列が同じ座標に留まったまま1セルだけ変化したケースでも、本来なら`diff_workbooks`と同じ「1件のModified」で済むはずが、列全体の削除+再追加（例: 8セルなら16件）として報告されていた——これは本モジュール自身がdocコメントで約束していた「座標ベースの通常差分にフォールバックする」という契約に反する、アライメントしない場合より悪い結果だった。`align_sheet_columns`に、DPで未整列のまま残ったDeleted/Inserted列のうち**同一の列番号を共有するペア**だけを座標ベースで直接diffする後処理を追加し、契約を実際に満たすよう修正した（列番号が異なる、真にシフトした列同士は引き続きコンテンツベースの整列に委ねられ、この後処理の対象外）。
11. **`docs/design/error.md`/`.en.md`が新しい`Error::ColumnAlignmentCostTooHigh`バリアントを含んでいなかった**: `src/error.rs`のEnum再現スニペットに追記した。
12. **重複ヘッダーが無条件のマッチとして扱われていた**: `header_match`は「行1のヘッダーが一致すれば、コンテンツの一致度に関わらず`HEADER_MATCH_BONUS`（他のどんなスコアより優先される）を与える」という設計だったが、これは列見出しが**重複**する場合（例: 「備考」列が2つある）に破綻する——変化したbase列と、たまたま同じ見出しを持つ無関係なtarget列とがDPにより誤って結び付けられ、その列の本当の継続（実際に変化した可能性もある）が未整列のまま残り、実際の変化が「新規追加」として誤報告される恐れがある。`ColumnContent::header_is_unique`（そのシート内で同じヘッダー値を持つ列が他に無いか、列全体を対象に事後計算）を追加し、`header_match`は両側のヘッダーが一意である場合のみ成立するよう修正した。重複ヘッダーの列同士は、通常のコンテンツベースマッチング（閾値経路または完全一致救済）で競い合う扱いになる。
13. **行アライメント統合時の主張が不正確だった**: 本モジュールのdocコメントは「Issue #4実装時は`diff_matched_columns`のマージジョインに行マッピングを渡すだけでよく、列マッチング自体の作り直しは不要」と述べていたが、`column_match_score`が閾値経路・完全一致救済の両方で使う`count_matching_rows`自体も同じ「同一行番号での値比較」を行っているため、行がシフトした場合はこちらも更新しないと、`diff_matched_columns`に到達する前の時点で一致スコアが低すぎて一致と認識されない。docコメントを修正し、両方の更新が必要であることを明記した。
14. **`CellDiff::old_col`追加はバージョンポリシー上明示すべき破壊的変更**: [model.md](model.md)の未決事項1に追記——既存の慣例（Issue #8）に従いバージョンは上げず、方針を明示的に記録するのみとした。
15. **予算チェックより先にO(cols²)の`mark_unique_headers`が走っていた**: `align_sheet_columns`は予算チェックの前に`column_contents`（内部で重複ヘッダー検出のためO(headered_cols²)の`mark_unique_headers`後処理を行う）を呼んでいたため、上限を超える幅の広いシートに対しても、エラーを返す前にこの二次コストを支払ってしまっていた。行数非依存の軽量な`distinct_column_count`（`BTreeSet<u32>`によるO(cells)走査）を新設し、`align_sheet_columns`をこの安価な集計→両予算チェック→（両方通過した場合のみ）`column_contents`の順に並べ替えた。
16. **モジュールdocコメントの誤字（"same same"）**: 上記「行は再整列しない」節に対応するdocコメント内の説明で単語が重複していた。修正。
17. **座標ベースフォールバックのdocコメントが過剰に一般化されていた**: 「未マッチの列は座標ベース差分にフォールバックする」という記述が無条件であるかのように読めたが、実際にこのフォールバックが働くのは上記10番の通り**同一の列番号を共有する**Deleted/Insertedペアに限られる（列番号が異なる未整列列同士は対象外で、丸ごとAdded/Deleted扱いになる）。docコメントをこの条件付きの挙動が明確に伝わるよう修正した。
18. **集約スコアリングが単一の`u64`ボーナスに依存しており、規模次第でヘッダー一致が負けうる**: `HEADER_MATCH_BONUS`（固定値`2,000,000`）は「他のどんなコンテンツ一致の組み合わせよりボーナスの方が大きい」という前提の上に成り立っていたが、この前提はExcelの実際の行数上限（1,048,576行）では崩れる——20%閾値の下でも2つの通常コンテンツマッチが合計で約2,097,150の一致セル数に達し得る一方、ボーナス込みのヘッダー一致は約2,000,001に留まる（base=[H, B, C], target=[X, Y, H]の例で、LCSの順序保存制約によりH↔HとB↔X・C↔Yは両立できない）。この場合、DPは正しいヘッダー一致ではなく2つの誤ったコンテンツ一致を選んでしまう。固定ボーナスを撤廃し、`(u64, u64)`のレキシコグラフィックな`Score`タプルに置き換えた——第1要素（ヘッダー一致なら`1`、それ以外は`0`）が常に第2要素（一致セル数）より優先されるため、この種の「ボーナスが十分大きいか」というクラスの不具合自体を構造的に排除した。回帰テスト`a_unique_header_match_outranks_a_larger_combined_content_match_score`を追加。

## 依存関係

- 依存先: [`diff/engine.rs`](engine.md)（`diff_sheet`/`diff_merges`/`visibility_diff`を`pub(crate)`化して再利用——片側のみのシート・結合セル・可視性の扱いを座標一致エンジンと完全に一致させるため、独自に再実装しない）、[`diff/model.rs`](model.md)（`CellDiff`/`SheetDiff`/`WorkbookDiff`/`DiffStatus`。同じ`CellDiff`型を再利用し`old_col`のみ実際に populate する）、[`error.rs`](../error.md)（`Error::ColumnAlignmentCostTooHigh`）、[`json.rs`](../json.md)（`cell_value_to_json`/`style_to_json`）、[`model/sheet.rs`](../model/sheet.md)（`Sheet::iter_cells`、`max_row`/`max_col`）
- 依存元: [`diff/mod.rs`](mod.md)（`diff_workbooks_aligned_columns`/`ColumnAlignmentLimits`を再エクスポート）

## エラー処理方針

- `diff_workbooks_aligned_columns`は`Result<WorkbookDiff>`を返す（`diff_workbooks`と異なり fallible）——予算超過時に`Error::ColumnAlignmentCostTooHigh`を返すため。呼び出し側がオプトインした処理が実際に完了したかどうかを黙って隠さない、という設計判断（ユーザーとの合意事項）。自動フォールバックが必要な呼び出し側は、このエラーを`match`して`diff_workbooks`を自分で呼び出せばよい。

## テスト方針

`src/diff/alignment.rs`内の単体テスト（`Sheet`/`Workbook`を公開モデルAPI経由で直接構築）:

- 列挿入・削除でカスケードが起きないこと（`column_insertion_does_not_cascade_when_aligned`、`column_deletion_does_not_cascade_when_aligned`）——`engine.rs`の`column_insertion_cascades_into_shift_diffs_by_design`（本Issue #5対応の一環として新規追加、デフォルトエンジンの列版カスケードテスト）と対をなす
- シフトした列内の真の値変更が`old_col`付きで正しく検出されること（`genuine_modification_survives_column_alignment`）
- シフトしていない列では`old_col`が`None`のままであること（`old_col_is_absent_when_the_matched_column_did_not_shift`）
- 短すぎる（`MIN_DISTINCT_FOR_CONTENT_MATCH`未満の行数）低カーディナリティ・ヘッダー無し列が安全に座標ベース差分へフォールバックすること（`low_cardinality_headerless_columns_fall_back_to_coordinate_diff_safely`）、ヘッダーがあれば同条件でも正しく整列できること（`header_match_rescues_low_cardinality_column_alignment`）
- 完全一致救済（上記「実装レビューで見つかった問題」4番）: 変化・シフトの無い低カーディナリティ列が誤差分ゼロになること（`identical_low_cardinality_headerless_column_produces_no_diff`、完全一致救済の陽性ケース）、行数が異なる列同士は完全一致になり得ないこと（`different_length_low_cardinality_columns_are_never_an_exact_match`）
- 低カーディナリティ列の意図的な非マッチ（誤検知防止）: シフトした低カーディナリティ列でも、実際に1行でも内容が変化していれば完全一致救済の対象外となり、他の内容一致シグナルも無い（`MIN_DISTINCT_FOR_CONTENT_MATCH`未満）ため、あえてマッチさせずAdded/Deletedのまま残ること（`shifted_low_cardinality_column_with_a_real_change_stays_unmatched`）——「マッチングヒューリスティック」節が定める、誤マッチより安全な未整列を優先する設計方針の直接的な検証
- 書式のみセル・`populated_count`（上記5・7番）: 書式のみセルの共有が部分一致閾値を水増ししないこと（`formatting_only_blank_cells_do_not_inflate_the_partial_match_threshold`）、書式のみセルを含んでいても真に同一の低カーディナリティ列は誤差分ゼロのままであること（`identical_low_cardinality_column_with_a_shared_blank_cell_still_produces_no_diff`）、`populated_count`が7個で閾値未達なら完全一致でも救済されないこと（`seven_populated_values_plus_a_blank_does_not_clear_the_exact_match_floor`）、大きな書式のみ範囲があっても実データによる一致判定が届くこと（`a_large_formatted_blank_range_does_not_raise_the_match_threshold_out_of_reach`）
- 定数列の多様性ゲート（上記8番）: 全行同一値の列が、無関係な別の定数列と誤って完全一致救済されず、本当の変化（0→1）を握りつぶさないこと（`constant_column_exact_match_rescue_requires_at_least_two_distinct_values`）
- 同一座標での座標ベースフォールバック（上記10番）: シフトしていない低カーディナリティ・ヘッダー無し列が1セルだけ変化した場合に、列全体の削除+再追加ではなく`diff_workbooks`と同じ「1件のModified」になること（`unmatched_low_cardinality_column_at_the_same_coordinate_falls_back_to_coordinate_diffing`）
- 重複ヘッダーの非優遇（上記12番）: 同じ見出しを持つ列が複数ある場合、コンテンツの裏付けが無ければ無条件マッチにならず、無関係な列への誤った対応付け（Modifiedの誤帰属）が起きないこと（`duplicate_header_does_not_win_an_unconditional_match`）
- レキシコグラフィックなScoreによる集約安全性（上記18番）: 一意なヘッダー一致が、規模の大きい（合計一致セル数が上回る）2つの通常コンテンツマッチの組み合わせより常に優先されること（`a_unique_header_match_outranks_a_larger_combined_content_match_score`）
- マージジョインの全分岐（`count_matching_rows`/`diff_matched_columns`の行が一方にしか存在しないケース含む、`matched_columns_with_sparse_non_overlapping_rows_exercise_every_merge_join_branch`）、片側のみに存在するシートがアライメント経由でも座標一致エンジンへ委譲されること（`sheet_present_on_only_one_side_reuses_the_coordinate_engine_through_alignment`）——いずれも`cargo-llvm-cov`が未到達と検出したことを契機に追加
- 2種類の予算超過が正しく`Error::ColumnAlignmentCostTooHigh`を返すこと（行数加重コスト: `distinct_column_cost_over_the_limit_is_column_alignment_cost_too_high`、行数非依存のペア数: `column_pair_count_over_the_limit_is_column_alignment_cost_too_high_even_with_one_row`）
- `diff_workbooks`（デフォルトエンジン）が本機能の追加によって挙動を変えていないこと（`diff_workbooks_default_behavior_is_unaffected_by_alignment_existing`）

[`tests/diff.rs`](../../../tests/diff.rs): `column_insertion_does_not_cascade_when_aligned_end_to_end`が、実際の`.xlsx`相当バイト列を`parse_workbook_reader`でパースした上でカスケード回避を再検証する。

パフォーマンス（実測、release build、Apple Silicon。`diff_workbooks_aligned_columns`をエンドツーエンドで計測、内部ループのマイクロベンチマークではない）:

| distinct列数(base=target) | 行数 | コスト | 実測時間 |
|---:|---:|---:|---:|
| 50 | 500 | 1,250,000 | 8.7ms |
| 200 | 500 | 20,000,000 | 108ms |
| 100 | 5,000 | 50,000,000 | 408ms |
| 20 | 50,000 | 20,000,000 | 364ms |
| 100 | 50,000 | 500,000,000 | 11.9秒 |

コスト正規化した時間（ms/コスト）は形状によって完全には一定ではなく（約5.4e-6〜2.4e-5ms/単位）、行数が非常に多く列数が少ない形状ほど悪化する（`ColumnContent`構築・`has_enough_distinct_values`のO(cols×rows)前処理コストが、O(cols²×rows)の照合コストに対して無視できなくなるため）。`MAX_COLUMN_ALIGNMENT_COST`（デフォルト10,000,000）は、実測した最悪ケースの単価に余裕を見て、`MAX_MERGE_REGIONS`が狙う「数百ミリ秒」と同水準の予算（約240ms）に収まるよう設定した。

なお、`count_matching_rows`（列ペアごとの一致セル数計算）を当初`BTreeMap::get`によるルックアップで実装したところ、行数が多いケースで無視できないO(log rows)の追加コストが乗ることが実測で判明した。`Sheet::iter_cells()`が既に行昇順であることを利用し、`ColumnContent::cells`を`BTreeMap`ではなく行ソート済み`Vec`にした上で、`diff::engine::diff_cells`と同じマージジョイン方式に書き換えたところ、この対数係数が消え、実測時間が同一入力で最大約5倍高速化した——`MAX_COLUMN_ALIGNMENT_COST`の実測値は、この最適化後のコードに対するもの。

## 未決事項 / オープンクエスチョン

1. **行アライメントとの統合**（[Issue #4](https://github.com/MinamiyamaKotaro/exceldiff/issues/4)）: 上記「行は再整列しない」節の通り、統合ポイント自体は設計済みだが、Issue #4自体が未着手のため実装されていない。
2. **`old_col`のSQLite永続化**（[Issue #5](https://github.com/MinamiyamaKotaro/exceldiff/issues/5)）: [storage.md 未決事項6](storage.md)参照。
3. **超低カーディナリティ・ヘッダー無し列の扱い**: 現状は安全側（座標ベース差分へのフォールバック）に倒しているが、この場合でも呼び出し側に「この列は整列を試みたが低カーディナリティのため断念した」という情報を明示的に伝える手段（例: `SheetDiff`への専用フラグ追加）は無い。実用上必要になった場合に追加を検討する。
4. **結合セルの列アライメント対応**: 上記「結合セルは列アライメント非対応」節の通り、現状スコープ外。要求が生じた場合に別途検討する。
