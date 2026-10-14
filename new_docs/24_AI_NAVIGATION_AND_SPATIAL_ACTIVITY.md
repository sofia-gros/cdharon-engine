# AI、経路探索、空間判定、画面外の活動

## 分けて扱う五つの状態

| 問い | 担当 | 意味 |
| --- | --- | --- |
| どのchunkに属し、World境界内か | Spatial/World | 座標と論理的な所属 |
| データが読み込まれているか | Streaming | queryと操作に必要な状態がresidentか |
| ゲームと物理を更新するか | Activity | active domainと個体の更新policy |
| cameraの表示範囲へ入るか | Camera/Visibility | viewportと対象boundsの幾何学的な重なり |
| AIが何を認識して、どう行動するか | Perception/AI/Navigation | 近傍、視線、状態、目的、経路 |

画面に映っていないという理由だけで、読み込んだ状態を破棄したり、敵を消したりしない。逆に、読み込まれたすべての区域を無条件に常時更新する設計にもしない。それぞれの状態と方針をAPIで取得・設定できるようにする。

## 空間query

`chunk_at(position)`、`world_contains(region)`、`region_state(region)`、`entities_in_aabb`、`entities_in_radius`、`raycast`、`shape_cast`を提供する。候補は空間索引で絞り、必要な精密判定を行う。

regionのcoverageはLoaded/Unavailable/WorldBoundary、resident/active/sleep、revisionを返す。複数chunkの一部だけが準備済みの状態を一つのtrueへ丸めず、all/anyの取得や未準備regionの列挙を提供する。

画面内判定はcameraとviewportの現時点の変換から計算する。回転・zoom・複数cameraを扱い、完全内側、交差、外側、camera未準備を区別する。描画のocclusion、レンダリングされなかったこと、AIの視線が遮られたことは別の判定とする。

## AIの共通基盤

知覚、判断、経路、移動を別の処理にする。知覚は近傍検索、faction、距離、視野、rayによる遮蔽、確定した音やdamage等の刺激を取得する。記憶はtarget、最後の確認位置・tick、刺激とconfidenceをblackboardへ保存する。

判断はstate machine、behavior tree、作者のSystemのいずれでも実装できる。結果は移動意図・攻撃・操作commandとし、AIが物理配列を書き換えない。単純な追跡に必ず経路探索を要求せず、steeringや局所回避も部品として提供する。

更新はEntityId順と固定のnode予算で行う。近い敵は毎tick、遠い個体は指定tick間隔というpolicyを設定できる。判断を間引いても、active物体の物理やtimerまで暗黙に間引かない。damage、視認、target変更などのイベントは次の判断を前倒しできる。

## 移動profileと経路探索

AgentProfileは形状、寸法、移動速度、step、jump、落下、泳ぎ、飛行、通過group、危険地形のcostを持つ。異なるprofileへ同じ通行判定を使い回さない。

ground経路はstandableな地点とwalk/jump/fall/swim等のaction edgeで構成する。jump edgeはcontrollerの軌道を精密shape queryへ通して検証する。飛行はgridまたはregion graphを使い、対角のcorner切り抜けを禁止する。動く足場は作者が登録するtimed linkや局所の接触追従へ接続する。

coarse graphでregionの接続を調べ、local A*で詳細経路を作る。costは整数の移動時間を基準にし、heuristicはその下界にする。同点はf、h、安定node keyの順で決める。結果の後処理も、精密sweepに通った区間だけ簡略化する。

返す状態はReady、Searching、WaitingForRegion、NoPath、Invalidated、CapacityExceeded。未読み込み区域があることをNoPathと同一にしない。Goalが動いた場合、経路上の地形revisionが変わった場合、agent形状が変わった場合は再検証・再探索する。

frontier、visited、親、cursor、入力revision、agent profile、要求したleaseを保存する。探索の途中でsave/loadしても二重の要求や無制限なやり直しを発生させない。大きな群れの同一目的には共有するregion経路やflow fieldを用意し、個々に同じ全探索を実行しない。

## 画面外の読み込みと更新

Worldへ結び付いたcameraを標準のObserverとして使える。既定policyは表示範囲に1chunkのsimulation margin、2chunkのresident marginを加える。これは有限の周辺を扱う既定値であり、画面外の全世界を読み込む意味ではない。

作者は自動Observerの有効・無効、margin、region、予算を変更できる。複数player、遠隔工場、飛翔物、AIの移動先なども独立したObserver/leaseを登録できる。cameraを持たないserverも同じ仕組みでactive domainを作る。

表示フレームごとのcamera補間値を物理へ直接渡さない。自動Observerのregion変更は外部入力としてX0でtickに割り当て、正規状態とreplayへ記録する。serverでは権限を検証したplayer位置等から活動域を決め、clientの任意のcamera移動で無制限の読込を要求させない。凍結は個体のanchorだけで決めず、bodyの占有範囲、jointの接続、進行中の取引を含む安全なdomain境界で行う。

読み込みだけならLoadOnly、物理や装置を動かすならSimulate、退避を防ぐならPinのleaseを取得する。活動範囲の和を予約し、上限超過では必要量と競合するsourceを診断する。自動的に重要な別regionを消して成功扱いにしない。

## 画面外の個体をどう扱うか

| policy | 状態と動作 |
| --- | --- |
| DomainBound | active領域内で更新し、外へ出る際は保存して凍結する。既定の永続的な扱い |
| KeepSimulating | 必要regionのSimulate leaseを保持する。容量を確保できない場合は明示して待機 |
| CoarseUpdate | 作者が登録した近似モデルを固定tick間隔で進める。精密物理と混同しない |
| DespawnByRule | 距離・時刻・条件に基づく作者のruleで消滅させ、drop/状態/receiptを処理 |

rendererのcullingだけをDespawnByRuleとして扱わない。重要なNPC、boss、装置、item等は作者が保持方針を選ぶ。凍結中のtimerは進まず、CoarseUpdate等を選んだ場合はそのモデルと経過状態を保存する。

## APIの利用イメージ

```rust
// 画面の外にある装置も、作者が明示した範囲で更新します。
let lease = world.activity()
    .lease(factory_region)
    .simulate()
    .source("game:factory")
    .request()?;

// cameraの幾何判定とWorldの準備状態は別の結果を取得します。
let visibility = camera.visibility().test(enemy_bounds);
let coverage = world.regions().state(enemy_region);

// 探索は即時に完了する保証を持たず、状態と必要regionを返します。
let route = world.navigation()
    .path(agent_profile, start, goal)
    .budget_nodes(256)
    .request()?;
```

型とメソッドはv1のAPI設計例で、現在実装済みという意味ではない。leaseとrouteのhandleは保持するIDで、単にローカル変数が破棄されたことを理由に正規作業を取消しない。解放・取消は明示操作または登録されたscopeの終了規則で行う。

## 保存・通信・検証

AI state、次の判断tick、探索途中、activity policy、lease、despawnの残り条件を保存する。複製はサーバーの結果を正とし、clientの画面外状態だけでserverのAIを停止させない。

Q02/Q07/Q09/Q10へ次のfixtureを含める。

- camera外だがSimulate lease内の装置とAIが動く。
- residentだがinactiveなAIが凍結し、復帰後に同じ探索を再開する。
- 画面に見えていても遮蔽されたtargetを視認済みにしない。
- chunkを跨ぐ経路がロード待機から再開し、地形破壊で経路が無効になる。
- 大きさの違うagent、斜面、jump、泳ぎ、飛行、動く足場の経路を検証する。
- 複数camera、headless、遠隔player、packet lossでdomainが意図せず変わらない。
- 明示despawnのitemと状態が二重処理されず、永続NPCは画面外になっただけで消えない。

これらはAIや画面外機能が「API名だけ存在する」状態を合格させないための受入条件とする。
