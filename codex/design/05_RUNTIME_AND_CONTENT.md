# 実行環境とゲームシステム

契約は[02_CONTRACTS.md](02_CONTRACTS.md)、更新phaseは[03_ARCHITECTURE.md](03_ARCHITECTURE.md)を参照してください。

## キャラクターと衝突

playerの正規位置・速度はC01の固定小数点です。現在の「ceilした速度回数だけ1px移動」は廃止し、小数位置を維持します。entityと地形の境界は半開区間で統一します。

4×4領域のoccupancy bitsetはbroad phaseのcacheとして使います。実際の衝突は占有セルとのswept AABBで確認し、1pxの薄壁を密度閾値で通り抜けません。collision maskで対象となる固体・粉体はfalling状態でも衝突候補です。既定ではどちらもplayerと衝突し、作者が明示した背景・非衝突材だけを除外します。粉体による埋まりはP7の補正対象になります。

移動は最初の接触時間で止め、残りを接線方向へ処理します。tick内の接触解決回数は固定上限を設け、上限で残移動を停止します。計算では有理数の比較を広い整数で行い、同時接触のtie-breakは軸、セル座標の固定順です。高速投射物はsegment/swept queryを使います。

接地中のauto-stepは最大3セル。上昇候補の全経路・頭上・着地点を検証し、成功時のみ移動します。薄い床を消す方式でつまずきを直しません。ジャンプbufferとcoyote timeはtick整数で設定します。

P7で地形が重なった場合、固定順の近傍探索で最小移動の空き位置へ退避します。探索上限で見つからなければ`Crushed`イベントと救済respawnを行います。探索半径・損傷・復活地点は正規設定/コンテンツです。壁の内側へ放置しません。

水中判定はAABB内液体の割合、液体の移動抵抗・浮力は固定係数です。entityは流体を消さず、近傍への交換要求だけをP3へ提出します。player体積を水が占めることは1.0で許容し、完全な排水体積・圧力連成は実装しません。

## 生成と読み込み

1.0は有限世界です。生成結果はseed、generator version、content hash、chunk座標で決まります。生成workerの完了順やthread数を使いません。

地形は整数hashノイズまたは固定演算の生成器を基準にし、現在のfloatノイズとは別generator versionにします。height、depth biome、cave、ore、prefabの順で適用します。構造物は世界座標の安定anchorから列挙し、chunkへclipします。境界を跨ぐ構造物・鉱脈は同じanchorから決まり、chunkを生成する順に依存しません。

生成済みchunkは初回永続化し、その後は編集後のchunkが正です。generator更新で既存chunkを再生成しません。旧generatorがない場合、まだ生成されていない区域へ進めることを拒否し、明示移行を必要とします。

player周辺にactive simulation domainと先読み領域を持ちます。画面外ロードはEngine既定ONです。作者は[画面外API](12_OFFSCREEN_CHUNK_API.md)で遠隔regionをロード・保持・更新し、既定の制限や動作を変更できます。余裕距離は最大移動・8セルのルール到達距離を覆う必要があります。entityが外へ進む前にactivationが必要ならP0で待機します。domainは容量上限以下に検証します。

domainを出るchunk・entity・正規粒子は凍結して保存します。境界で連結している流体が止まることは製品モデルの制約です。画面外すべてを動かすと宣伝しません。active domainはカメラ描画範囲とは別で、変更履歴を保存・リプレイに含めます。

## 採掘・戦闘・クラフト

| 機能 | 正規状態とatomic操作 |
| --- | --- |
| 採掘 | セル損傷sidecarまたは採掘対象ジョブ。破壊時にitem drop slotを先に予約 |
| 配置 | inventory減算とセル配置を同じトランザクションでcommit |
| 近接攻撃 | 固定tickのhitbox、同attack IDの重複命中防止、遮蔽raycast |
| 投射物 | EntityId、固定位置/速度、貫通上限、segment collision、着弾command |
| 爆発 | 固定半径内を安定座標順で処理。各効果は上限予約後commit |
| 魔法 | 有限のeffect graph。cycle禁止、深さ/生成数/書き込みセル数を検証 |
| inventory | item stable key、slot数、stack上限、overflowを拒否 |
| クラフト | 材料・出力容量・作業台queryを検証し、一括commit |

採掘等のP3処理はinventoryと世界をまとめてcommitします。P8は既に確定した取引のevent公開・death整理を行う場所であり、P3の予約を別tickで再消費しません。単なる掘削brushはsandbox用の明示消去で、資源獲得ルールと区別します。

巨大な爆発は1.0の許可半径・セル数以内でのみ実行します。複数tickへ広げる拡張は別effectとして定義し、途中状態・再入禁止・保存を設計してから追加します。damageは物質の視覚粒子に依存しません。

敵はまず明示state machine、歩行・飛行各1例。pathfindingは4×4の派生navigation gridと精密衝突queryを組み合わせます。地形versionが変わった経路は再検証。探索予算はノード数で固定し、open set/進捗を保存するか、保存後の最初のtickで同じ結果になる再構築方式を試験で選びます。M5で方式を固定するまで出荷不可です。

ボス例は状態遷移、攻撃予告、破壊上限、arena queryで構成し、特別なWorld直書き権限を持ちません。hit/death/dropも通常APIを経由します。

## 描画・UI・音響

rendererはimmutable snapshotから地形、物質粒子、entity、装飾、光、UIの順で描画します。CellCore/Material hot物性へtexture配列を埋め込みません。色、テクスチャ、光の変更で物理hashを変えません。

最初はpixelsを維持し、Dirty texture uploadやlight passが必要な場合にwgpu直接描画を比較します。GPU処理は表示側に限定。整数倍率とletterbox、UI倍率、入力の逆変換を同じviewportから導きます。

device lostは世界を保ったままrenderer再生成を試し、失敗時は保存可能な停止画面へ移ります。device再生成中に物理だけ進めません。

光は上限付きの表示用近似。遮蔽・熱源・damageに使うraycastとは分離します。画面揺れ・点滅・装飾の品質設定は正規状態に影響しません。

UIはフォーカス中に入力を消費し、inventoryを開いたまま背後へ採掘commandを送らないようにします。音は確定event IDを使い重複再生を防ぎます。音響遮蔽は近似可能で、audio unavailableでもsimulationは進行可能です。

## データ拡張とSDK

作者データはversion付きTOML、画像はPNGを第一候補とします。schemaは物性、反応、item、recipe、entity definitionを型別に検証。未知field、重複key、循環参照、巨大画像、パストラバーサル、範囲外係数を診断します。

設定はユーザー設定（表示・音・入力）と正規設定（物理係数・容量・仕事量予算）を分けます。旧`conf.yml`は移行入力として読み、元ファイルをバックアップしてから新形式へ変換します。使わない項目を有効と表示しません。

物性は重さ・浮力・可燃性等の標準項目に加え、[ブロック定義設計](11_MATERIAL_AUTHORING.md)の型付きproperty/attachmentを扱います。正規instance stateを持つ独自ブロックは、移動・反応・保存・sleepの全経路に登録する必要があります。

Rustのゲーム拡張は信頼するコードとしてビルド時に組み込み、viewとcommandだけを渡します。1.0は未知のnative binaryを動的ロードしません。データModは起動時に検証して凍結し、プレイ中のhot reloadは画像等の非正規データに限定します。

content hashは正規化した物性・反応・ゲーム定義・生成定義を含みます。画像・音は別asset hash。ただし画像から衝突形状を生成する場合、その派生形状は正規contentへ入ります。エンジン内部のHashMap iteration順をcontent hashやID順へ使用しません。
