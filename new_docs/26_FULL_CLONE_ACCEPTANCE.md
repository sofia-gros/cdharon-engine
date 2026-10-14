# Terraria・Noitaの完全クローン制作能力の判定

## 判定の意味

要求は、対象作品の全ゲーム内容・ルール・操作・表示・音・進行・保存後の継続を、通常の作者が公開SDKと作者コード・データで再現できることである。「同じジャンル」「代表機能が動く」「主要な仕組みがある」を合格条件にしない。対象作品の完成品をCharonへ同梱する要件と、完全クローンを制作できる基盤の要件は区別する。

以下は全機能を展開するための分類台帳であり、各行一つの作例を通せば原作の全内容を確認したことにはならない。対象buildのすべてのコンテンツと挙動を個別行へ展開し、抜けを検査する。作者コードで再現する項目にも、読取、更新位置、書込、描画、保存、通信の接続根拠が必要である。

## 比較対象と証拠

対象ごとにPC版のbuild、配布元、更新日、実行OS、難易度、ゲームmode、seed、入力機器、表示・音設定をTargetManifestへ固定する。版やmodeを未指定のまま、別々の仕様を混ぜて一致扱いにしない。初期の設計は両作品全体を対象とし、実装検証時に対象buildの具体的なfingerprintを記録する。都合のよい古い版へ変更して欠落を回避しない。

FeatureLedgerは`game/build, feature_id, parent_id, content_key, prerequisites, observations, initial_state, input_trace, expected, implementation_route, public_api, state_schema, save_scope, net_policy, test_id, result, evidence`を持つ。内容一覧、mode、環境条件、相互作用の一覧から未対応項目を抽出する。公開資料、実際の対象buildの観測、作者の推定を区別し、未観測を一致済みにしない。

比較するのは入力に対する外部挙動と必要な永続状態であり、原作のprivateなクラス名やコード構造を一致させる必要はない。原作と互換なseed、save形式、ネットワークprotocol等が対象の要件に含まれる場合もledgerへ独立に記載し、[25](25_OPEN_EXTENSION_AND_COLLISION_CONTRACTS.md)の乱数・codec・transportで実装経路を示す。知らない形式を推測で対応済みにしない。

## Terrariaの全内容を展開する分類

| ID | 展開する内容と境界 | 公開SDKでの制作経路 |
| --- | --- | --- |
| TC01 | title、全設定、キャラクター作成・選択、world作成・選択、削除・退出 | Scene、UI、input、PersistenceScope |
| TC02 | 難易度、character/world mode、死亡の差、研究・複製・時間等のmode固有操作 | 作者resource、policy、clock、取引 |
| TC03 | 全seed指定、特殊seed、world寸法、生成pass、構造物、初期loot | Generation provider、PRNG、複数解像度grid |
| TC04 | 全tile、wall、家具、背景、装飾、接続frame、paint、coating、形状変更 | Material/Block、author channel、独自framing、描画extractor |
| TC05 | tileのsolid/非solid切替、one-way、斜面、半ブロック、扉、足場、当たり判定例外 | CollisionQueries、channel、filter、CharacterMotion |
| TC06 | 全液体の量・流速・混合・接触・変換・ポンプと同位置tile | 液量channelとMaterialTransport、ThermalReactions |
| TC07 | 全移動、加減速、jump、重力反転、水中、飛行、dash、teleport、圧死 | 置換controller、SubstepPolicy、HitResolution |
| TC08 | hook、rope、minecart、mount、足場、乗車・降車、pet/minion追従 | Entity、custom motion/body、joint、作者AI |
| TC09 | 全採掘tool、採掘蓄積、reach、配置条件、支持、連鎖更新 | TransactionView、barrier、block state、tool policy |
| TC10 | 全item、stack、修飾、耐久等の個体状態、拾得・投棄・消滅、通貨 | ItemInstance、Container、独自StateStore、取引 |
| TC11 | 全recipe、作業台、近傍条件、素材group、変換・分解・再抽選 | Recipeと作者評価、予約、独自rule |
| TC12 | 全防具、装備、accessory、loadout、dye、set効果と適用順 | component、属性stack、順序付きSystem |
| TC13 | 全近接武器・toolの使用、use time、animation、hitbox・hurtbox | 独自HitResolution、tick animation、shape |
| TC14 | 全projectile、beam、whip、召喚、反射・貫通・追加更新・免疫 | custom collider、SubstepPolicy、攻撃免疫provider |
| TC15 | 全buff/debuff、food、potion、耐性、回復、knockbackと死亡処理 | 属性、clock、作者effect、取引 |
| TC16 | 全enemy、spawn、AI、地形無視、部位、drop、despawn例外 | 作者System/AI、独自query、activity policy |
| TC17 | 全boss、phase、部位共有、条件付き攻撃、出現・退場、報酬 | Entity群、HitResolution、進行state |
| TC18 | 全NPC、会話、shop、居住、条件判定、価格、移動・帰還 | 作者graph/resource、UI、inventory、region lease |
| TC19 | 全biome、感染・拡散、昼夜、天候、植生・作物・自然更新 | 作者clock、領域channel、tick schedule、生成・変換Job |
| TC20 | 全event、侵攻、季節、進行flag、解禁、achievement | StateStore、進行System、profile保存 |
| TC21 | 全wire色・交差、actuator、switch、timer、logic、trap、pump、teleporter | Signals置換、同tick queue、port/channel、取引 |
| TC22 | fishing、quest、捕獲・解放、golf等の独立した遊びと報酬 | 作者state machine、custom collision/body、UI、取引 |
| TC23 | bestiary、map/minimap、探索履歴、ping、marker、情報表示 | 作者channel/resource、UI canvas、独自描画 |
| TC24 | smart操作、mouse/gamepad、hotbar、inventory、craft、chat、全UI | input、UI canvas、focus、IME、作者layout |
| TC25 | 全sprite、tile blend、染色、背景、light mode、影、water、particle、camera | render graph置換、shader、extractor、visual particle |
| TC26 | 全効果音、環境音、music、優先順位、距離と切替 | audio bus、作者mix、stateに基づくevent |
| TC27 | characterとworldと探索mapの独立保存、移動、復帰、mode差 | PersistenceScope、transfer transaction、codec |
| TC28 | 原作対象modeの参加人数、host/server、PvP/team、共有・個別loot、chat | OwnershipPolicy、Replication/Transport、author validators |
| TC29 | 全content量と最大world、長時間生成・更新、遠隔領域、負荷時の挙動 | profile予算、author grid、streaming、実機fixture |
| TC30 | 対象buildにある上記以外の内容、複数機能の組合せ、観測可能な例外 | FeatureLedgerへ展開しprovider/channelの経路を追加 |

TC30は不足を「任意の作者実装」とだけ書いて合格させる欄ではない。発見した各機能の接続と検証を具体化するまで、その機能は未確認とする。modeや全itemを一括で「同様に可能」と処理しない。

## Noitaの全内容を展開する分類

| ID | 展開する内容と境界 | 公開SDKでの制作経路 |
| --- | --- | --- |
| NC01 | 起動、全設定、通常・追加mode、seed、再挑戦、日次等の条件 | Scene、作者resource、seed provider、profile保存 |
| NC02 | 全world領域、接続、遠方、上下左右の拡張、別world、特殊構造 | Generation provider、World/region index、転送取引 |
| NC03 | 全material、density、粉体/液体/気体、静止・移動、特殊な移動規則 | 独自channel、MaterialTransport、作者物性schema |
| NC04 | 全material接触、燃焼、酸、凍結、蒸発、変換、条件付き反応 | ThermalReactions置換、同tick順序、反応state |
| NC05 | 電気、伝導、液体接続、火花、局所・全域の力場等の相互作用 | 作者field channel/System、接触query、力・damage command |
| NC06 | 爆発、掘削、地形変換、canonical particle、着地、飛散、遮蔽 | 爆発provider、shape/ray/sweep、台帳、予約 |
| NC07 | 切り出し、回転、摩擦、接触、液体排除、浮力、剛体の力学 | BodyPhysics/Coupling provider、GeometryView |
| NC08 | 全破壊可能prop、joint、ragdoll、rope、切断、燃焼・融解 | local grid、StateStore、custom physics、変換bridge |
| NC09 | 移動、levitation、kick、throw、swim、teleport、衝突例外 | CharacterMotion、shape filter、作者input、力と取引 |
| NC10 | HP等の属性、damage種、濡れ・stain、摂取、空気・窒息、耐性 | 作者component/channel、接触/摂取rule、clock |
| NC11 | bottle/pouchの量と混合、吸引・注入・投擲・破損 | 容量付き物質container、channel変換、body、取引 |
| NC12 | 全wand状態、slot/deck/discard、shuffle、draw、mana、cast/recharge | 独自EffectExecution、状態付きPRNG、同tickqueue |
| NC13 | 全spell/action、順序、副作用、modifier、multicast、trigger、timer、再帰 | 作者VMまたはRust evaluator、barrier、budget方針 |
| NC14 | 全projectile、trajectory、homing、beam、orbit、penetration、hit例外 | custom shape/motion/HitResolution、SubstepPolicy |
| NC15 | 全perk、stack、永久効果、組合せ、world変化、適用対象 | 作者属性・effect/resource、定義remap、状態移行 |
| NC16 | polymorph、possession、召喚、複製、所有者と元の状態の復元 | Entity state transfer、snapshot fragment、owner policy |
| NC17 | 全enemy、感覚、記憶、追跡、経路、特殊移動、faction、despawn | 作者AI、Navigation置換、activity、StateStore |
| NC18 | 全bossと部位、shield、特殊条件、攻撃pattern、報酬 | Entity群、custom damage/collision、進行resource |
| NC19 | 全item、gold、shop、選択、交換、inventoryとdrop | ItemInstance、Container、UI、取引 |
| NC20 | 全進行、秘密、puzzle、orb、quest、ending、worldの継続・変換 | 作者state/graph、region rule、world/profile保存 |
| NC21 | worldを跨ぐmaterial変換、遅延読込区域・body・containerへの適用 | MaterialRemapTable/epoch、変換policy、codec |
| NC22 | 全pixel/sprite描画、液体表現、照明、影、post effect、camera、UI | shader/render graph置換、extractor、UI canvas |
| NC23 | 全sound/music、ambient、音による刺激、reverbと混合 | audio adapter/作者mix、正規stimulus system |
| NC24 | 全実行状態のsave/load、wand途中、body/joint、world状態と恒久進行 | provider codec、PersistenceScope、replay、migration |
| NC25 | 原作の全content量、極端な組合せ、遠隔領域と長時間稼働 | capacity profile、SameTick、stress/soak、streaming |
| NC26 | 対象buildにある上記以外の内容、観測可能な例外と相互作用 | FeatureLedgerへ展開し必要な公開経路を具体化 |

NC05は原作を再現する作者の内容であり、電子・光子等の特定の素材をエンジンの必須プリセットへ追加する指示ではない。NC12/NC13の実装に標準EffectProgramが合わなければ作者が別の評価器を使い、標準programの表現力へゲーム仕様を合わせない。

## 完全一致の照合方法

1. 原作buildのfeatureとcontentを列挙し、各項目へ入力、初期条件、出力と副作用を割り当てる。タイトルから各mode・全進行・各ending・終了後の状態まで遷移を辿る。
2. 全定義のfieldを比較可能なrecordへ展開し、未使用に見える定義と条件付き内容もledgerへ残す。個数だけの一致で済ませない。
3. 単独機能に加えて、全ruleが宣言する対象の組合せを生成する。すべてのmaterial pair、装備のstack順、spellの順序、hit免疫、tile/液体/actuator、保存境界などを検査する。履歴依存にはmodel-basedと長い入力列を使う。
4. 同じ入力traceを原作と再現実装へ適用し、位置・速度・接触tick、damage、液量、地形、乱数依存出力、timer、在庫、進行、表示・音の開始と終了を比較する。
5. 原作の内部状態を読めない項目は、初期条件を制御した観測列で同じ結果となるか比較する。内部状態を知らないことを「同じ」とする根拠にしない。非決定的な項目は観測した分布・相関・再現条件を記録する。
6. 時刻・離散出力は期待値へ一致させる。連続値や画像・音は比較計測の精度と原作自身のばらつきを先に固定する。差を消すために後から許容誤差を広げない。標準solverの誤差1cellを自動の許容値にしない。
7. 原作の挙動に観測可能な癖や不具合がある場合も、互換性対象から勝手に削らない。再現profileで同じ結果を選べるようにする。Charon自身の保存破損やalias違反を許す意味ではない。
8. 保存・再開、途中離脱、複数world、最大content、負荷時、対象modeの参加人数でも同じ経路を照合する。通常の短い操作だけで合格しない。

raw入力を原作の時刻単位へ割り当てるadapterと、Charonのtick/substepへの変換を明示する。原作と異なるtickへ移した効果を、動画の見た目が似ているだけで一致としない。

## エンジンと作者の実装分担

エンジン側の合格根拠は、全featureに対して公開経路が存在し、その経路の独立した実装試験があること。作者側のゲームデータと個別挙動をエンジンに全同梱する必要はない。ただし、具体的な再現projectを用意せず、任意のRustコードを書けることだけで全機能を検証済みにしない。

判定状態は「設計上の制作経路あり」「SDK経路を実装・検証済み」「対象buildとの一致を観測済み」に分ける。設計レビューの結論と完成した完全クローンの実証を混同しない。未確認の原作仕様があれば、未知と記録して一致証拠の対象へ残す。

## V1-32の出口

FeatureLedgerに未対応のエンジン能力、private API依存、標準部品に合わせるための挙動変更、未保存の正規状態がない。全分類を対象buildの内容へ展開した記録、独自providerの実装検証、対象buildとの比較記録を残す。T01〜T12/N01〜N12は導入用の統合fixtureであり、この出口の代わりにしない。

E00でledger schemaと対象manifestを準備し、各工程で該当行を検証し、E12で全体を照合する。新たな機能が見つかった場合は行を追加し、公開経路が欠けるならV1-31/V1-32の不足として解消する。有限の代表作例だけで、未知を含む全挙動の一致を証明したとは記録しない。
