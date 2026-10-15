# 動く物体、破壊可能剛体、連成

## v1で提供するもの

静的・kinematic・dynamic body、回転、摩擦・反発、連続衝突、sensor、joint、controllerを提供する。さらに、物質のlocal cellを持つ破壊可能剛体、地形からの抽出、破砕、粒子化、再固定を実装する。見た目だけ回転するspriteではこの要件を満たさない。

剛体backendは`rapier2d-f64`を採用する。solverを独自に再実装する代わりに、エンジン側でセルとの連成と破壊を実装する。backendの型はadapter内へ隠し、公開APIはBodyId、ColliderDef、JointDef、PhysicsSettingsを使う。正確な依存版とfeatureは実装工程でlockし、更新は物理profile変更として検証する。

Rapierは標準providerであり、作者へ強制する唯一の物理ではない。他のsolverと一致が必要な場合は[25のBodyPhysics/Coupling契約](25_OPEN_EXTENSION_AND_COLLISION_CONTRACTS.md)で置換する。係数調整だけで原作の衝突・運動が一致すると判断しない。

## 正規状態

BodyはID、pose、速度、外力、質量、重心、慣性、sleep、collision group、joint、backend対応表を持つ。破壊可能bodyはlocal grid、materialと全sidecar、局所anchor、shape revisionを追加で持つ。物質はlocal gridとWorldのどちらか一方へ所属し、二重計上しない。

接触、island、jointの蓄積、sleep情報を含むbackendの必要状態をSnapshotへ含める。公開poseだけを保存して完全な再開を主張しない。backendのcodecは版付きの隔離recordとし、版の変更時は移行または認定された再構築手順を必要とする。

## local gridから衝突形状を作る

1. local occupancyから4近傍の境界辺を抽出し、向きを固定して輪郭を列挙する。
2. 穴と外周を識別し、決定的な順序で矩形分割または凸分割する。
3. 占有cellと同じ領域を覆う分割shapeをcompound colliderへ登録する。近似profileを別に選ぶ場合だけ誤差を宣言し、完全再現の判定に1cell未満という一律の許容値を使わない。
4. cellの換算massから重心と慣性を再計算する。任意の視覚textureのalphaをそのままmassとしない。

形状はrevisionごとに再構築し、小変更はaffected regionを更新する。変更maskとcolliderを同じbarrierで公開する。dirty領域のqueryだけを新しくして古いcolliderで力学を解決しない。準備が間に合わなければ候補stepを待機し、exact queryによるTOI/normal補正を使う実装では補正結果をpose/impulseへ反映して検証する。

## Worldと剛体を接続する

World地形の静止colliderと剛体colliderをbackendへ与える。bodyのlocal cellをWorldへ仮の物質として上書きする方式は採用せず、別の`BodyStencil`で占有と境界を表す。

body候補poseから、掃引した領域と次poseの占有cellを列挙する。液体・気体・粉体が押し退けられる場合は、近隣の空き先、粒子pool、bodyに所属する排除reservoirの順で出力を予約する。reservoirは追い出した物質とenergyを保持する正規状態で、近隣が空いたら取引として放出する。

排除領域の和をbody ID順で処理し、一つのWorld cellを複数bodyが重複して消費しない。接触による一時的な形状重なりは占有の和集合として扱い、重複する熱・浮力サンプルを除く。

## 一つの物理step

1. 速度・角速度・外力の上限から保守的なswept領域を計算し、必要chunkと排除出力の最大容量を予約する。
2. backendの候補状態を作り、現在確定状態を保持したままsubstepを実行する。
3. 並進距離と回転端点の移動が0.5cell以下になるsubstep数を選ぶ。設定上限を超える速度は入力時に拒否するか、明示した速度上限を適用する。
4. candidate poseのBodyStencilで地形との正確な接触と流体排除を検証する。Unavailable側への侵入は許可しない。
5. 排除・接触・pose・速度・backend状態をまとめて確定する。必要な予約が満たせない場合はその物理stepを確定しない。

候補backendは二重化した再利用bufferへコピーする基準実装とする。serializationを毎tickの候補作成に使わない。容量を準備し、copy量と更新時間をprofileする。island単位の候補化は、完全な状態の巻き戻しを証明してから採用できる最適化とする。

上の速度上限はこの標準profileの条件である。高速弾、長いray、teleportは25のquery/motion providerへ接続でき、全方式に同じclampを強制しない。

接触solverの内部エラーや数値不正が発生した場合は、直前の確定状態を保持して診断停止する。部分的なWorldの排除だけを残さない。

## 浮力・抗力・熱・反応

流体と重なるlocal cellの体積と相対速度をサンプルし、浮力と抗力を求める。得られた力はbackendへ蓄積し、対応する流体側には速度場または移動要求として反作用を提出する。計算値のclampと単位をPhysicsSettingsに含める。

local grid内部はWorldと同じ熱・反応solverを使う。body表面とWorldの辺には被覆率から重みを付け、同じ整数fluxを双方で加減算する。回転や丸めでenergyが発行されないよう端数accumulatorを保持する。

物質が溶解・燃焼・腐食して失われたら、対応するlocal cell、mass、慣性、shape revisionを更新する。出力がWorldへ戻る場合は着地点を予約する。予約失敗では元のlocal cellと燃料を保持する。

## 地形からの抽出と破砕

地形の接続はcell maskと結合条件から求める。連結成分を抽出するJobは入力revisionと探索frontierを保存する。対象と必要なbody・粒子容量を予約してから、World cellをlocal gridへ移す。

bodyが分断された場合は、連結成分ごとに新しいbodyを作る。mass、重心、角運動量の継承規則を定め、元bodyの速度場`v + ω×r`から各破片の初速度を求める。jointのanchorが残る成分へ接続を移し、残らないjointは明示イベントで破棄する。

小さな破片を物質粒子へ変える閾値は作者が指定する。pool不足時は未変換のbodyを保持する。bodyが静止しただけで自動的に地形へ戻さず、再固定を選んだ場合のみ、占有先を予約して一括変換する。

## controllerとの関係

プレイヤー等のcontrollerは地形、body、片方向床を同じcollision facadeから問い合わせる。動く足場の接触速度を取り込み、bodyのpose変化を描画だけに使わない。ロープ、フック、車輪、乗り物はjointとcontrollerの組み合わせで作れる。

## 合格に必要な証拠

回転する木片が液体へ入り、浮いて熱を受け、部分的に燃えて割れ、破片が保存・復元後も継続して動くsceneを作る。粒子と地形への相互変換、jointの分断、満杯reservoir、chunk境界、満body pool、薄壁と高速移動を検証する。

物質トークン・energyの収支、分断前後の慣性と速度、Snapshot再開、並列候補評価と基準実装の一致を確認する。画面にbodyが表示されたことだけでは合格しない。
