# 完全クローン制作能力を基準とする設計レビュー

## 対象と結論

2026-10-15、`new_docs/`の製品要件、実行、データ、世界、素材、物理、gameplay、表現、保存、通信、API、検証、工程をレビューした。基準はTerrariaとNoitaの完全クローンを、公開SDKと作者projectで制作できることである。以前の「型」「主要な仕組み」「代表作例」の基準では不合格と判断し、P001として設計を修正した。

**修正後の設計判断：完全クローンを制作する基盤として成立する。** 理由は、原作の個別の規則を標準機能へ合わせる必要がなく、独自の状態表現・処理順・衝突・運動・効果評価・描画を公開経路で実装し、保存・再現・通信へ接続できるためである。標準機能の一覧だけを根拠にした判断ではない。

この結論は設計上の制作可能性の判断であり、実装済みSDKや完成した完全クローンの一致証明ではない。対象buildの全内容台帳と比較実行はE00〜E14の成果物とする。非公開アルゴリズムを調査済み、全spell/itemを実機照合済み、性能を計測済みとは記録しない。実装時に公開経路が欠けた場合は、設計を満たしていないとして修正する。

## 第1回：再現を妨げる指摘と修正

| 指摘 | 修正前の問題 | 修正と根拠 | 判定 |
| --- | --- | --- | --- |
| R01 判定範囲 | 主要機構とT/Nの代表作例を完成条件にしていた | 06/20/26を全内容・全挙動のledgerへ変更。mode、コンテンツ、例外も個別展開 | 設計上解消 |
| R02 物理backend | Rapierの係数調整で異なる原作の物理も再現できる前提だった | 11/25にBodyPhysics/Couplingの置換契約。保存・接触・占有まで公開 | 設計上解消 |
| R03 形状誤差 | 1cell未満の誤差を全物体へ許していた | 11でexactな占有分割。26で原作に基づく比較精度を要求 | 設計上解消 |
| R04 古いcollider | 新しいmask queryと古い力学colliderが併存できた | 11で形状とcolliderの同時公開、TOI/normal/impulseの反映と待機を要求 | 設計上解消 |
| R05 衝突表現 | ground sweep中心で、beam/whip/custom shapeと別hitboxの契約が不足 | 25でshape登録、保守的候補、全hit、coverage、filter/responseを定義 | 設計上解消 |
| R06 命中免疫 | 単一のAttackId方式に固定される可能性 | 25で発射者別・対象別・種類共有等のHitResolution置換 | 設計上解消 |
| R07 同tick連鎖 | spell、wire、反応を一律に次tickへ繰越していた | 07/10/12/25でSameTick/barrier。未完了で時刻を進めない | 設計上解消 |
| R08 更新回数と時計 | 投射物の追加更新、timer、早送り等を60Hzの同一時計で処理していた | 25でSubstepPolicy、clock domainと保存fieldを定義 | 設計上解消 |
| R09 乱数と精度 | counter乱数、f64、整数熱が作者の表現を固定していた | 07/08/25で状態付きPRNG、f32等の独自正規state、変換を派生値に限定 | 設計上解消 |
| R10 液量とtile | 全液体を満杯cellの占有として表すため中間液量が失われる | 10/25/23で型付きGridChannel、重なるtile/wall/液量、独自solver | 設計上解消 |
| R11 拡張の実効性 | フックのReplaceが何を置換できるか、batchの書込権限が不明 | 16/25でservice slot、ReadContext/CandidateContext、WriteBatch、検証とcommit | 設計上解消 |
| R12 独自データ | component以外のsoft body、VM、別解像度grid等の接続が不足 | 08/23/25でStateStore/GridChannelとgeometry/render/save bridge | 設計上解消 |
| R13 世界規模の変換 | 未読込区域・body・containerを含む素材変換の一貫性が不足 | 25/23でMaterialRemapTable、epoch、所有領域と適用policy | 設計上解消 |
| R14 保存の分割 | player/world/恒久進行/runを別に保持する契約が不足 | 14/23/25でPersistenceScopeとtransfer/参照manifest | 設計上解消 |
| R15 通信の自由度 | server権威・QUIC・4clientを唯一の形式や規模と読めた | 15/25でownership/replication/transport置換、対象人数profile | 設計上解消 |
| R16 見た目と音 | 標準light/UI/spriteで全表現を構成できる保証がなかった | 13/25でextractor、buffer、pass/light置換、UI canvas、独自mix経路 | 設計上解消 |
| R17 全コンテンツ | 256素材等の負荷作例だけで原作の内容量を確認した扱いになり得た | 18/26で全定義・全mode・対象最大負荷の照合。固定個数は最小fixture | 設計上解消 |
| R18 作者実装の禁止 | 独自spell評価をsample関数へ書くことを一律不合格としていた | 20で作者Rust評価器を認め、private権限と保存漏れだけを禁止 | 設計上解消 |

## 第2回：修正による契約の再確認

| 確認 | 設計上の条件 | 担当仕様 |
| --- | --- | --- |
| 標準providerの二重実行 | slotの排他、対象の所属、無効化後の隠れた重力等を禁止 | 25のservice slot |
| 候補失敗の巻戻し | 自己state・channel・backendも候補に保持。公開済みtickへ部分変更を漏らさない | 07/22/25 |
| 独自queryの候補漏れ | broad phaseの保守的bounds、unsupportedとincompleteの明示 | 25の当たり判定 |
| 攻撃と力学の分離 | hurtboxとbody shape、owner、部位、免疫、視線を別channelにする | 25/26 |
| 高速物体・teleport | 標準速度clampを全方式へ強制せず、移動経路と到着点policyを作者が指定 | 11/25 |
| 異なるgrid | originと有理数格子比、coverageとbridge、save/hashを宣言 | 08/23/25 |
| 同tick処理の予算 | 時刻を進めずwork queue継続、無限loopを成功扱いにしない | 07/12/25 |
| 独自乱数の再開 | 内部stateと呼出し順、profile、codecを保存 | 07/14/23/25 |
| global素材変換 | resident以外にもepoch適用、所有領域と例外を作者policyで指定 | 23/25 |
| 描画の独自性 | custom stateをsnapshotへ抽出し、正規処理が表示cacheに依存しない | 13/25 |
| 外部形式への対応 | import/export codecとtransport公開。未実装の既存形式を対応済みとはしない | 25/26 |
| ゲーム専用規則 | TC/NCをエンジンに固定せず、作者projectで実装・全ledgerへ照合 | 06/12/26 |
| 入力と音の差 | 統合前のdevice eventと入力policy、独自DSP/mix、buffer出力を公開 | 25 |
| 複数scopeの原子性 | 共通の世代manifestとRuntime commit所有者、transfer IDで二重反映防止 | 25 |

第2回の時点で、既知の再現機構を標準アルゴリズムに合わせて変更しなければならない設計上の障害は残していない。未知の原作仕様と未実装・未測定は実証の対象として残し、設計上解消した指摘の証拠と混ぜない。

## 具体的な制作経路での確認

### タイル液体とactuator

作者がtile/wall、solid有効状態、液体種とu8量、wireを登録する。Signalsが同tickの訪問順でactuatorを変更し、barrier後のCollisionQueriesが新しいsolidを読み、MaterialTransportが同じ場所の液量を処理する。移動はTransactionViewで更新し、描画extractorが中間量の表面を表示する。全channelとqueueが保存・複製される。標準の満杯cell液体や1tick遅延への変換を必要としない。

### 追加更新するbeamと特殊命中

作者がpolyline/custom shapeとSubstepPolicy、owner・種類共有の免疫stateを持つ。保守的なboundsから候補を列挙し、独自overlap/sweepでhitを決める。HitResolutionが部位と免疫を評価し、penetrationとtimerを指定順に更新する。beamのsprite寸法、ground controller、既定AttackId方式に縛られない。

### 杖とtriggerの評価

作者がdeck/discard、mana、draw位置、shuffle乱数、modifier stack、cast/recharge clockを持つ独自評価器をEffectExecutionへ登録する。SameTick queueがdraw/発射/triggerを指定順に処理する。各出力を予約してbarrierで公開し、未完了ではtickを進めない。確定時の評価state・PRNG・projectile stateが保存される。標準EffectProgramのnodeだけで表せない挙動を削る必要がない。

### 回転する破壊可能物体

作者のBodyPhysicsが前状態、local material、World geometry、力とdtから候補pose/接触を作る。Couplingが掃引域・液体排除・熱を予約し、破壊で変わるlocal mask/collider・joint・質量を同時に公開する。保存は作者backendの必要stateも含む。Rapierの接触結果や誤差許容へ原作を合わせる必要がない。

### 標準に専用APIがないゲーム規則

釣り、会話・価格、研究、変身、秘密のpuzzle等は作者StateStore/System、入力、query、clock、UI、取引を組み合わせる。変身前の状態や各worldへのplayer移動はPersistenceScopeとtransferへ接続する。独自VMが必要なら作者crateが解釈器とstate codecを登録し、公開Contextから同じ権限を使う。ゲーム内容をエンジン本体へ追加しなければ作れない構造にはしない。

## 実装レビューへ引き継ぐ条件

各R項目は「設計上解消」と記録する。工程で実装・検証した際にsource revisionと試験を加え、「実装上解消」へ更新する。特に、全slotを置換したSDK試験、exact衝突、SameTickの待機と巻戻し、全stateの往復、対象規模の計測を省略しない。

今後、完全再現できない挙動が発見されたら、作者の責任という説明だけで閉じず、必要な公開経路の欠落を調べる。能力が足りない場合はPと要件へ追加し、設計・実装・検証を更新する。完全クローンを「同系統の作品」に戻して合格させない。

## 文書の構造検査

2026-10-15に文書28件のcode fenceの対応、相対リンク118件の参照先、READMEからの全ページ参照、V1-01〜V1-32の要件と追跡表、TC30分類/NC26分類、R18指摘の存在を検査し、エラー0件だった。AGENTS.mdの`git diff --check`も終了code 0を確認した。これは文書構造の検査であり、原作の一致試験やエンジンの物理試験の実行結果ではない。今回の変更は設計と開発ルールであり、cargoによる実装試験は実行していない。
