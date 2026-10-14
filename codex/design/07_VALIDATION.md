# 検証と出荷判定

本書は設計上の合格条件です。現時点でテスト実装・性能測定が済んだことを意味しません。

## 契約と検証の対応

| ID | 対応契約 | 最低限の検証と合格条件 |
| --- | --- | --- |
| V01 | C01 | 負座標chunk往復、半開境界、固定tick、overflow。30/60/144Hz描画で同tickのhash一致 |
| V02 | C02/C04 | cell size、flag予約bit、sidecar交換、stamp。chunk境界でも二重移動なし |
| V03 | C03/C11 | 空レジストリ、上限直前/超過、重複、missing、stale EntityId。panicなし・ID衝突なし |
| V04 | C05 | 空間交換、小領域全列挙、ランダム場、満pool。物質トークン増減が変換台帳と一致 |
| V05 | C05/C08 | 断熱閉領域のenergy保存、熱の上限、phase往復、反応予約競合、燃料枯渇 |
| V06 | C06/C10 | checkpoint→復元→再開のtick hash一致。job、lease、属性、allocatorを含む |
| V07 | C08 | dense対chunk対sleep/Dirtyのdifferential test。各tickが完全一致 |
| V08 | C07/C09 | 画面外既定ON、遠隔lease、上限、IO順、load/unload、世界端。データ消失・partial activationなし |
| V09 | C09/C11 | malformed content/save、長さ偽装、容量枯渇、disk full。操作がatomicに失敗 |
| V10 | C12 | 固定scene/seed/inputによるCPU/GPU/memory計測。条件とpercentileを公開 |
| V11 | C01/C04 | 薄壁、角、斜面、step頭上、水中、埋まり、高速弾。壁抜け・無限解決なし |
| V12 | C03/C11/C12 | 作者が重さ・燃焼・独自属性を定義し遠隔worldを操作するsample。文書だけで再現可能 |

保存workerの各write/flush/公開段階へfaultを注入し、最新または旧有効世代に戻れることをV09で確認します。移行fixtureは変更前の版を固定します。

## テスト方法

最小のpure simulationをheadlessで動かします。単体テストは構造・境界、property-based testは保存則・atomicity、differential testは最適化同値性を確認します。fuzzはdecoder/content/API境界を優先します。テストの失敗seedとcommand列をfixtureへ保存します。

M1でlegacyの見た目をそのまま正解としてgolden化しません。二重移動や壁抜けの既知問題を残すことになるため、契約に基づく小sceneと再現ログを基準にします。挙動変更でhashが変わる場合は理由とsimulation versionを更新し、無説明で期待値を書き換えません。

unsafe/SIMDを追加した場合はscalar対照、境界fuzz、targetごとの試験を追加します。並列を採用する場合はthread数1/2/4/8、worker遅延、異なる完了順でも同じ結果を確認します。

## 性能予算（暫定）

物理60tick/sの時間窓は16.67msです。標準負荷でsimulation p95≤8ms、p99≤12msを目標とし、OS・描画・IOの余裕を持たせます。画面表示はp95≤16.67ms、p99≤25ms、定常実行で増え続ける遅れがないことを目標とします。GPU計測とCPU計測を分け、VSync待機を演算時間に混ぜません。

参考測定機の型番、RAM、OS、driver、build flags、Rust versionをM0で採取します。標準機と下位機をM6で選定するまで、最低動作環境を製品保証しません。上限を緩和した作者世界のFPSは別結果として公開します。

| scene | 固定する負荷 | 目的 |
| --- | --- | --- |
| B0 静止 | 96 active chunks、全静止、640×360表示 | sleep効果と基礎overhead |
| B1 流体 | 同domain、30% liquid/powder、16 entities | 標準の局所更新 |
| B2 破壊 | 毎60tickで半径32の爆発、256正規粒子 | Dirty急増、予約、最悪tick |
| B3 熱・反応 | 96 chunks、20%燃焼/反応候補 | sleepできない場面 |
| B4 遠隔 | player域と遠隔域、計256 active/512 resident | 画面外APIの上限負荷 |
| B5 IO | 境界移動、遠隔lease、保存、容量不足 | 停止時間、旧セーブ保持 |

B0〜B3が標準性能目標、B4は上限ストレスで同FPS保証をしません。B5の準備待機はフレーム性能から別に報告し、見えない除外をしません。30秒warmup後60秒計測を5回。最良値のみ採用せず分布、活動セル数、bytes/tick、allocations、memory峰値を記録します。

512 chunksならCellCoreのみ8MiB、全chunk分stamp16MiB、energy8MiBです。timer、損傷、属性、copy snapshot、画像、entity、allocator余白を合計して測定します。chunk byte予算256MiBはrenderer資産まで含む全プロセス上限ではありません。標準負荷のprocess memory暫定目標は512MiB以内です。

定常tick内の追加allocation=0をinstrumentationで確認します。登録・生成・ロード・snapshot確保は別に計測します。性能が外れた場合は全域走査、物性参照、copy、GPU upload等のprofileから原因を選び、正規stateを削って隠しません。

## 出荷ゲート

全契約の試験証拠、保存fault試験、8時間soak、最悪負荷での安全な劣化、作者sample、SDK文書、ライセンス/SBOM、対応環境での実機試験を必須とします。P0/P1の不具合（データ消失、頻発crash、再現性契約違反）が残るRCは出荷しません。

source変更のたびにformat/check/testを実行し、clippy指摘を分類します。設計変更だけならリンク、ID参照、仕様の相互整合を確認します。コードを変更していない設計作業でruntime合格と表示しません。
