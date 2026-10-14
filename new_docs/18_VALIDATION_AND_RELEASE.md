# 検証とv1.0.0の出荷判定

## 判定の原則

すべてのV1要件に、source、試験、実行結果、認定profile、残る問題を対応付ける。計画した試験を実行済みと表示しない。設計書と単体テストの存在だけで商用水準の完成を主張しない。

検証は単位、境界、property-based、differential、統合、配布物の再現、実機計測、長時間試験から構成する。失敗seed、入力、content manifest、最初の不一致tickをartifactへ残す。

## 正しさの試験群

| 試験群 | 最低限の対象 |
| --- | --- |
| Q01 APIと型 | build失敗、型違い、stale ID、WrongWorld、重複key、初期化失敗、購読解除 |
| Q02 World | 負座標、各layer、寸法とshape、cross-chunk、部分破壊、生成順、lease、退避 |
| Q03 取引 | 複数cell・item・粒子の容量不足、競合、途中エラー、台帳と確定状態の一致 |
| Q04 物質 | mass交換、粉体・液体・気体、圧力押し出し、混合、sidecar追従 |
| Q05 熱反応 | 断熱energy保存、熱の上限、相変化往復、燃料と出力容量、反応の競合 |
| Q06 body | 高速薄壁、回転、液体排除、reservoir、破砕、joint、熱、地形変換 |
| Q07 gameplay | inventory、craft、effect継続、AI探索、配線loop、world eventと独自状態 |
| Q08 表現入力 | alpha、座標変換、複数camera、UI focus、IME、gamepad切断、audio再送 |
| Q09 保存再現 | 全状態のroundtrip、checkpoint再開、IO fault、移行、世代復旧、worker順 |
| Q10 通信 | 権限、不正payload、packet loss、再接続、baseline、同時取引、4client一致 |
| Q11 制作配布 | newからpackageまで、cache再生成、依存不足、別PCでの配布物起動 |
| Q12 作者利用 | 初見の作者が独自Scene・素材・状態・効果を公開APIから制作 |

thin wall、diagonal corner、斜面、stepの頭上、片方向床、動く足場、bodyの回転端点を個別fixtureにする。UIが操作できたことを物理衝突の検証の代わりにしない。

Q01/Q03/Q06/Q07/Q09/Q10/Q12に[25のprovider置換試験](25_OPEN_EXTENSION_AND_COLLISION_CONTRACTS.md)を含める。Q06は見た目と異なるhitbox、beam/polyline、custom shape、開始時重なり、one-way、全hit/truncate、geometry revision、exactなTOI/normal/impulseまで検査する。Q07はSameTick/Deferred、追加更新、独自PRNG、f32 state、液量channel、全域material remapを区別する。

V1-32は[26の全内容・全挙動の照合](26_FULL_CLONE_ACCEPTANCE.md)で判定する。標準fixtureのB01〜B05、4client、content個数、8時間は最低限の基盤測定であり、原作の最大world・内容量・参加人数・極端な組合せの代わりにはしない。対象profileで再現に必要な負荷とメモリを追加計測し、未測定を性能保証としない。

## 参照実装との比較

セル・熱・反応・dirty/sleepに正確なscalar参照経路を保持する。最適化経路と同じ入力を適用し、各tickの正規状態を比較する。圧縮表現の違いは論理状態に正規化する。

並列systemはworker数1/2/4と遅延注入、streamingは生成・IOの完了順変更、表示は30/60/144Hzで検証する。同一認定profileのWorld結果を比較し、視覚粒子やaudio voiceの差を正規の不一致へ数えない。

## 性能の合格目標

基準機の目標は6core/12threadのdesktop CPU、RAM16GiB、4GiB VRAM相当のGPU、SSD。低負荷profileは4core/8threadと統合GPUを想定する。正確な型番、OS、driver、周波数、電源設定は測定前に固定し、後から都合のよい機材へ差し替えない。

既定のWorldは60tick/s。基準機の通常sceneではsimulation p95が12ms以下、p99が16.67ms以下、表示1080pの総frame p95が16.67ms以下を目標とする。2GiB以下のprocess resident memory、2GiB以下のVRAMを通常sceneの目標とする。資産量と世界の稼働内容を併記する。

| scene | 固定する負荷 |
| --- | --- |
| B01 建築探索 | 8,192×2,048blockの論理世界、block寸法16cell、可視960×540cell、active周辺を含む。2,000Entityと遠隔装置64 |
| B02 物質 | active 1,024×640cell、40%占有、移動する粉体・流体10万cell、thermal対象10万cell、反応1万候補/tick |
| B03 破壊連成 | dynamic body256、合計local cell20万、物質粒子2万、1回65,536cellの破壊、joint64 |
| B04 作者拡張 | 独自component型32、effect継続1,000、AI探索256、配線装置1,000、表示sprite10,000 |
| B05 IO通信 | 4client、遠隔lease、探索によるchunk入れ替え、保存、100ms RTT/2%loss |

B01/B02は通常性能の必須測定。B03〜B05は負荷を独立に測り、測定した同時稼働の上限と推奨profileを公開する。同時に全最大値を満たすと宣伝しない。極端なsceneでの劣化は明示したtick待機・件数予算・表示品質で処理し、itemや物質を勝手に消さない。

低負荷profileでは640×360cell相当のviewportと表示30FPSを許容し、物理の挙動を同じprofileの参照経路と一致させる。機能を未実装にして性能合格としない。

## メモリと長時間

chunk、sidecar、Entity、body候補、粒子、asset cache、GPU staging、save COW、network queueのbytesを計測する。reserveの見積もりと実使用量を照合し、容量拒否まで試す。tick内の再確保を計測し、hot pathの増大を回帰検知する。

各検証作品を8時間連続実行し、探索・生成・破壊・保存・load・Scene切替を反復する。初期warm-up後のメモリ増加が説明可能なcache上限へ収束すること、stale参照・台帳不一致・save破損がないことを確認する。

## 障害と外部入力

ファイル作成、partial write、sync、publish、CURRENT更新、世代整理へfaultを注入する。disk full、permission、破損、巨大長さ、圧縮bomb、未知record、欠落pluginを試す。GPU lost、audio切断、network切断、worker取消も試す。

decoder、importer、network、effect compile、generatorの境界へfuzzを実施する。入力byte数と計算予算を制限し、panic、無制限確保、無限loop、元save上書きを検出する。found seedを回帰fixtureへ固定する。

## 商用出荷の条件

- V1-01〜V1-32の証拠が揃い、[完全クローンの制作能力](26_FULL_CLONE_ACCEPTANCE.md)を全ledger行と公開APIの証拠で確認する。統合fixtureだけの通過では不足とする。
- 認定OSごとの配布物を別の環境で起動し、入力、音、表示、保存、移行、通信を確認する。
- 日本語API資料、導入、制作、最適化、配布、障害復旧の手順が実際の公開版と一致する。
- 公開API/保存形式の互換規約、依存と資産のlicense/SBOM、checksum、release notes、既知制限を配布する。
- 保存破損、再現不能な正規状態の消失、任意コード実行等のP0と、主要制作経路を塞ぐP1の未解決問題がない。
- バージョン、tag、source revision、toolchain、dependencies、認定profile、artifact hashを一つのrelease manifestにまとめる。

これらに未達があればv1.0.0とせず、未達項目を要件IDとともに示す。出荷日や既存テスト数に合わせて合格条件を縮めない。
