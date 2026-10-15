# 世界、ブロック、生成、画面外の設計

## セルとブロックを両立する

物理格子は均一なcellで構成する。作者がマテリアルの`.block_size(w, h)`で指定する値は、その素材をブロックとして配置する際のfootprintである。16×16cellのブロックと1×1cellの素材を同じ世界で扱える。格子の寸法を素材ごとに変える仕組みにはしない。

配置はanchorとfootprintから占有先を計算し、全cellと個体componentを予約して一括commitする。ブロック単位破壊では残存footprintを一括除去し、セル単位破壊では指定maskだけを除去する。両者は`BreakMode`で選ぶ。画像の寸法や表示倍率はこれらを変更しない。

Full、Half、Triangle、OccupancyMask、Platformの形状を用意する。形状は論理cellの占有maskと衝突形状へ変換し、前景と背景を区別する。textureのalphaと衝突maskは独立して指定でき、画像からmaskを作るimporterを利用する場合は派生maskをcontentへ含める。

## レイヤー

前景の物理地形、背景壁、装飾、配線・装置anchorを別のlayerとして扱う。layerは役割、描画順、衝突group、物理への参加を持つ。背景画像を物理cellへ詰め込まない。複数cellに跨る家具や施設はBlockInstanceとcomponentで扱う。

配置時のanchor条件、隣接による見た目の変化、接続規則、染色・paint、採掘損傷は定義またはcomponentで表現する。形状や接続のcacheを保存の正としない。

## 座標と世界の範囲

World作成時に有限境界、または生成するchunkを必要に応じて増やす方式を選ぶ。後者も座標型の範囲を持ち、端でエラーを返す。画面解像度とWorldの幅・高さを同じ値として初期化しない。

問い合わせは`Loaded`、`Unavailable`、`WorldBoundary`を返す。未読み込みを空気に置き換えない。WorldBoundaryは外部操作不可、Unavailableは読み込み待機や明示拒否の理由となる。作者は非同期ロード要求とWorldのtickを進められる条件を取得できる。

## chunkの状態機械

```text
Absent -> Requested -> Preparing -> Ready -> ResidentInactive -> Active
Active -> ResidentInactive -> Saving -> Evicted
Preparing/Ready -> Failed または Cancelled
```

RequestId、入力世代、要求source、対象World、region、purpose、確定希望tickを持つ。IOや生成workerはPreparingまでを担当する。Readyの候補はworld境界、content、依存、容量を検証してからWorldへcommitする。 確定希望tickへ前進する前に、そのtickまでの全必須要求を検査する。遷移先tick期限の候補が全てReadyなら時計と区域を同じ取引で公開し、未準備なら時計も区域も公開しない。

Activeだけが物理更新する。ResidentInactiveは状態を保持して凍結する。退避は保存または再生成可能な不変baseが確定してから行う。編集済みchunkを保存失敗のまま破棄しない。Entity、物質粒子、装置、AIは所属regionと一緒に退避する。

## activityとlease

空間所属、画面内外、resident、active、AI方針の区別と各queryは[24](24_AI_NAVIGATION_AND_SPATIAL_ACTIVITY.md)に従う。画面外という一つの状態へまとめない。

カメラ周囲のactivityを既定で提供するが、active範囲は描画とは独立する。遠隔工場、複数プレイヤー、投射物の予測範囲などが`RegionLease`を要求できる。leaseはLoadOnly、Simulate、Pinを選び、source、期間、region、予算を保存する。

有効な要求の和を安定順で解決し、resident/active/byte予算を一括予約する。必須leaseを確保できない場合は対象操作を待機または拒否し、部分activationしない。lease解放後も、保存や隣接する物体の安全条件を満たすまで退避を待つ。

domain境界の物理は、未準備側へ物質を送り出さず、熱を流さない。物体は衝突境界として扱う。境界を越える予定の物体は事前leaseを要求する。非active期間の経過物理は既定で進めず、作者が時間経過の別モデルを登録した場合のみ、保存されたJobとして処理する。

## 生成

生成入力はseed、generator key/version、content hash、world設定、絶対座標。base terrain、biome、cave、ore、structure、decorationsのpassを登録し、依存順を固定する。

構造物や鉱脈は安定したregion anchorから候補を列挙し、要求chunkへclipする。隣接chunkの生成完了順や乱数の共有消費回数に依存させない。浮動小数点ノイズを使う場合も生成profileと実装版を固定し、参照データで照合する。

未編集のbaseはseedから再生成でき、編集はchunk revisionと差分として保存する。生成器の変更で編集済み区域を上書きしない。未生成区域に旧版が必要なら旧生成器を保持するか明示移行を行う。generator不在時はその区域の生成を拒否する。

## 大きな操作と地形の依存

大規模prefab、支持の探索、連結成分の切り出し、広域の置換はJobで進める。対象revisionと予約領域を保持し、入力が変更されたら再検証する。計算途中の候補を完成した地形として見せない。

爆発で穴が開いた場合は、衝突、navigation、light、描画、block anchor、配線接続のrevisionを更新する。各cacheへ必要な範囲を通知し、古いcacheが正規判定へ使われないよう、世代照合と正確なfallback queryを用意する。

## sleepとdirty

chunkは動く物質、熱の差、timer、作業中のJob、独自systemの活動がない場合だけsleepする。独自systemは恒等性を宣言でき、宣言しない状態があるchunkは寝かせない。dirtyは派生情報であり、セル状態そのものに兼用しない。

変更の依存距離はruleの宣言から求め、隣接chunkもwakeする。sleepとdirty最適化は、同じ操作を全域参照solverへ適用したtickごとの比較に合格してから有効にする。

## 必須の確認

負座標、異なる寸法、chunkを跨ぐ家具、半ブロックと斜面、前景・背景、部分破壊、再配置、生成順を入れ替えた同一世界、複数カメラと遠隔lease、保存失敗時の退避拒否を検証する。最大の世界を最初から全cell確保する実装では、大規模世界の要件を満たさない。
