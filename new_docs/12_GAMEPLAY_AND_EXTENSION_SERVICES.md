# ゲームを構成する共通サービス

## 任意の機能として提供する

この文書の機能はv1で配布する共通プラグインとAPIである。作者が独自実装へ置換できるよう、状態、イベント、取引、世界queryを公開する。ゲーム起動時に特定のプレイヤーや敵、レシピ、進行を自動生成しない。

## Entityと移動

EntityはTransform、見た目、body、controller、属性、AIなどをcomponentとして組み合わせる。親子transformと装備の表示anchorを提供する。World所属とSceneの表示所属を別に保持し、UIの終了で世界の個体が消えない。

ground controllerはswept capsule/AABB、接地normal、slide、設定可能なstep、斜面角度、片方向床、移動足場を扱う。collision summaryは候補選別にだけ使用し、最終判定は実際のshapeとcell maskで行う。水中・飛行controllerも同じqueryへ接続する。

高速移動は最初の接触時刻を求め、残移動を接線方向へ処理する。既定の接触反復上限は8、超過した残移動は停止して診断を記録する。埋まりは固定順の近傍探索で解消し、見つからなければ`Crushed`を発行する。damageや復活は作者の規則へ渡す。

## Item、inventory、装備、craft

ItemDefは安定キー、stack上限、外観、タグ、使用挙動、装備slot、作者属性を持つ。個別の耐久・修飾・内容物があるitemはItemInstanceIdで管理し、同一キーだからといって状態の異なるitemをまとめない。

Containerはslot・byte・重量等の許容量を定義できる。追加、移動、分割、装備、採掘drop、配置、craftは取引として検証する。材料の重複指定は集約し、出力容量、作業台、道具、必要なworld revisionを先に予約する。

Recipeは入力条件、出力、作業条件、時間、作業状態を持つ。即時craftと継続作業を同じreceiptへ接続する。途中作業は残量と予約の扱いを保存し、load後の二重消費を防ぐ。保管UIとcraft検索は同じqueryから構築する。

採掘はdamageの蓄積、対象のBreakMode、tool条件、drop ruleを評価する。ブロック単位とセル単位の破壊を選び、採掘可能かどうかを固定の素材名で判断しない。

## Combatと状態

AttackId、source、hit group、shape、duration、対象条件、damage/effect recordを登録する。近接はtickで変化するhitbox、遠隔はCCD付きの物体またはray/sweep、継続効果はtimerとmodifierで表す。

同じ攻撃による重複命中は`AttackId × target × hit_window`で管理する。遮蔽query、penetration、反射、属性耐性、knockback、owner判定を独立した部品にする。HP・mana等は登録済みAttributeKeyとして扱い、作者が独自属性を追加できる。

死亡はdrop出力の予約、状態遷移、個体破棄、イベントを一つの順序で確定する。出力容量不足のときに黙ってdropを消さず、world drop containerへ待機させるか、作者が明示した消費ruleを使う。

## 効果と合成可能な処理

EffectProgramは生成、移動、属性変更、world操作、条件分岐、修飾、発射、命中時trigger、timer trigger、別programの呼び出しを組み合わせる。作者は新しいEffectNodeの実装とcodecを登録できる。

programを型付きの中間形式へcompileし、参照、値域、生成量、必要サービスを検証する。進行にはprogram key/version、命令位置、局所値、対象、source、乱数tuple、予約とreceiptを保持する。

ループや再帰的triggerは実装可能とし、命令数・spawn数・書き込みcell数予算で区切る。Deferredは継続Jobを保存して次tickへ送り、SameTickは候補tickを進めず継続する。選択は[25](25_OPEN_EXTENSION_AND_COLLISION_CONTRACTS.md)に従う。容量失敗と予算待機を区別し、再開でmanaやitemを二度消費しない。作者は独自評価器へ置き換え、deck/discard、draw、shuffle、modifierの適用順と副作用を独自stateへ保持できる。

呪文のcard順、同時発射、遅延、散布、修飾、triggerを、この汎用programと作者の評価規則で構成できるようにする。特定の武器や杖をエンジンの唯一の魔法方式にしない。

## AIとnavigation

知覚、経路探索、空間query、画面外policyの詳細は[24](24_AI_NAVIGATION_AND_SPATIAL_ACTIVITY.md)を実装基準とする。

作者はstate machine、behavior treeまたは独自systemを登録する。blackboard、target、timer、行動の進捗はcomponentとして保存する。AI更新は安定したEntityId順と件数予算を使う。

navigationはcoarseなregion接続とlocalな精密shape queryを組み合わせる。歩行、jump、fall、swim、fly、地形通過などの移動profileを登録できる。A*の優先順位は`f,h,node_key`で固定し、frontier、visited、入力revisionをJobへ保存する。

地形や動く物体で経路が変わったらaffected revisionを検証し、無効区間だけ再探索する。未読み込みのregionは`WaitingForRegion`としてleaseを要求する。敵の全探索を描画フレームの余り時間へ依存させない。

## 配線、装置、自動化

配線はWorld layer上の接続graph、信号は型付きのpayload、装置はportとcomponentを持つ。複数の接続種、交差、遅延、論理条件、作業queueを作れる。変更範囲のgraphをrevision付きで再構築する。

標準Deferredの信号はsourceとsequence順に処理し、新しい信号は次tickのqueueへ送る。同tickの伝播順が必要ならSignals providerとSameTick queueで処理する。ループ、訪問済み集合、信号の消費、進行状態を保存する。電気や液体等の追加相互作用は素材・効果のAPIへ接続する。

装置はSimulate leaseによってカメラから離れても動作できる。recipe作業、ポンプ、運搬、保管を通常の取引APIで構成し、sample専用のWorld直書きを使わない。

## 生成・進行・環境

時刻、天候、biome、spawn rule、住居や施設の条件、進行flag、world eventを型付きresourceとして登録する。世界の一部が別のbiomeへ変化する処理も、広域Jobとmaterial変換で作れる。

ボスのphase、攻撃予告、部位、NPCの会話や仕事は、Entity、属性、AI、UI、効果を組み合わせる作例として示す。一般の作者に公開されていない専用権限を与えない。

## 作者の独自実装

各サービスは、読み取りView、変更command、イベント、フック、保存型を持つ。必要ならbatch kernelを登録できる。任意のRust globalへ正規状態を隠しても保存可能と見なす設計にはしない。

状態・規則を切り替えた際に、セーブ、通信、再現性へ何が影響するかをAPI資料で示す。便利な標準部品が、作者の作品の規則を固定しないことを統合作例で確認する。

専用APIがない釣り、独立したmini game、会話・経済、研究・複製、変身、全域素材変換等も、25のStateStore、channel、provider、clock、scopeを通して作者が実装できる。列挙だけで対応済みとせず、[26の全内容台帳](26_FULL_CLONE_ACCEPTANCE.md)へ具体的な接続と比較fixtureを記録する。
