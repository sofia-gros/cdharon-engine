# 定義record、個体状態、codecの項目

## 共通規則

各定義はstable key、schema version、依存key、タグ、作者schemaを持つ。データpackとRust builderは同じ内部recordへ変換し、同じ検証を通す。未知fieldを無条件で無視しない。作者の追加fieldは登録済みschemaの領域へ置く。

v1の基本プリセットはSolid、Powder、Liquid、Gasを基準とし、作者が寸法・見た目・物性を与える。空とmissingはレジストリの内部予約として用意する。用途別の素材を追加しても、物理solverへその素材名の特別分岐を追加しない。

## コンテンツ定義の必須項目

| 定義 | 必須の意味を持つfield | 主要な検証 |
| --- | --- | --- |
| MaterialDef | key、base、footprint、shape、BreakMode、mass、physics、thermal、behavior refs、reaction refs、texture、effect refs、instance schema | 正の寸法/mass、shape領域、物性範囲、参照と挙動の共存、変換rule |
| BehaviorDef | key/version、実装key、設定record、read/write radius、出力見積もり、state key、sleep契約 | 実装の存在、設定schema、最大到達範囲、codec |
| PhaseFamily | key、相のMaterialKey、温度境界、熱容量、潜熱、energy境界 | 温度順、正の容量、連続なH、相参照、整数overflow |
| ReactionDef | key、入力条件と数量、priority、熱条件、出力と位置rule、energy増減、timer消費、instance変換 | 入出力の必要容量、無限の同tick連鎖禁止、移行rule |
| ItemDef | key、stack上限、個体化条件、タグ、texture、使用/装備挙動、属性schema | stack範囲、利用する効果、個体状態の同一性規則 |
| RecipeDef | key、集約した入力、出力、施設/道具/状態条件、時間、作業state | 重複inputの集約、出力と中断時rule、参照 |
| EntityPrefab | key、component型と初期値、親子、body/collider、表示、activity/永続化方針 | 循環する親子、component schema、所属World、全参照 |
| EffectProgram | key/version、node列と接続、入力型、state layout、trigger、出力見積もり | 型、参照、budget、再開位置、各node実装 |
| AIProgram | key/version、state/behavior graph、blackboard schema、知覚profile、移動profile、更新policy | 状態参照、予算、component依存、継続codec |
| DeviceDef | key、port型、接続rule、処理key、work state、item/World取引 | signal型、SameTick/Deferredの終了規則、queue容量、入出力 |
| GeneratorDef | key/version、pass依存、seed domain、region/anchor rule、利用するcontent | 循環、順序、座標範囲、chunk順非依存 |
| BiomeDef | key、判定field、生成・環境・spawn・表示の参照、変化rule | 判定範囲、priority、広域Jobへの接続 |
| AnimationDef | key、frame/track、長さ、補間、marker、clock種別 | frame範囲、正規markerと表示markerの区別 |
| UI/locale | key、tree/theme、font refs、文字列とplaceholder、locale fallback | 親子循環、文字型、font欠落、placeholder一致 |
| AssetRecord | key、種類、source、importer/version、設定、hash、依存、license meta | source所在、展開量、循環依存、型、認定形式 |

プリセットで得られる値も、builderで明示した値も、解決後はすべて同じfieldとして検証する。定義に残る不正な参照を初回使用まで持ち越さない。

## 物性と表現の範囲

mass_per_cellはu32の正値、energyはi64、温度は0〜1,000,000,000mK、heat capacityとlatent heatは正の整数で計算中間値がi128へ収まることを検証する。conductivity、drag、friction、restitution等は用途別の型で範囲を決め、負数やNaNを受け付けない。

footprintは各軸1〜65,535cellの型を持つ。ただし作成可能かは積のoverflow、通常取引またはJobの予算、World範囲、総byte容量を見積もって判断する。小さな寸法のプリセットを全素材の固定寸法にしない。

flagsのv1定義はbit0=moving、bit1=burning、bit2=wet、残り予約。burning/wetはtimerに対応する派生状態として整合を検査する。作者の独自状態はcomponent/sidecarへ置き、予約bitを無断で使わない。

## 保存する個体record

| record | 保存する内容 |
| --- | --- |
| WorldState | ID、tick、seed、bounds、設定、manifest、allocator、layer、chunk index、lease、queue、Job |
| ChunkState | 座標、revision、logical cell、全有効sidecar、block参照、region所属個体、生成版と編集差分 |
| BlockInstance | ID、定義、anchor、向き、layer、残存mask、独自component、変換時の継承rule |
| EntityState | ID/generation、component record列、World/親子、activityと永続化方針 |
| BodyState | backend ID対応、solver正規状態、pose/速度、local grid、joint、shape revision、reservoir |
| ParticleState | ID、定義、token数、位置/速度、energy/timer、個体record、所有region |
| ContainerState | ID、slot内容、ItemInstance、stack数、予約、容量とrevision |
| EffectState | program/version、命令位置、local値、trigger、乱数、予約receipt、待機理由 |
| NavigationJob | agent profile、始点/終点、region/revision、frontier、visited、親、進捗、lease要求 |
| AIState | program/version、state、blackboard、target、知覚の記憶、次の判断tick、NavigationJobId |
| DeviceState | port/connection revision、signal queue、作業queue、継続state、必要lease |
| CanonicalJob | kind/key/version、入力revision、reservation、cursor、work buffer、次step、完了/取消状態 |
| CommandQueue | 対象tick、source、sequence、型付きpayload、権限と依存revision |

P001で以下の型も登録対象とする。標準の数値範囲は標準物性の制約であり、独自stateの値域・精度はschemaで宣言する。

| record | 保存する内容 |
| --- | --- |
| ProviderState | service slot、key/version、能力、profile、state record、依存provider、codec |
| GridChannelState | key/version、型schema、origin/格子比、domain、初期値、region payload、revision、dirty |
| StateStoreRecord | key/version、型付きpayload、ID/参照、容量、変換・移行rule |
| ScheduleState | profile/version、barrier、substep/分母、clock domain、SameTick/Deferred queue |
| RandomState | provider/version、stream key、seed、内部state、呼出し順 |
| MaterialRemapTable | epoch、stable keyの対応、対象となる所有領域、既適用世代 |
| PersistenceScope | kind/key、世代、参照scope、player/world/run等のroot、transfer履歴 |

候補tickのqueueとstateは公開済みtickと別に保持する。通常のsaveは確定状態を取り、途中候補を保存する追加checkpointを設ける場合は入力journal・候補版・rollback情報を含む別形式として認定する。

外部資源へのポインターやcallbackは保存しない。復元はキーとIDの辞書を作ってから参照を解決する。復元後の起動フックでcallbackや表示を接続し、同じ取引を再実行して出力を重複生成しない。

## binary codecの共通表現

整数は固定幅little-endian、boolとoptionは0/1、vectorとbyte列はu32長、文字列はUTF-8 byte長と内容。mapはkeyのbyte順で保存し、重複と順序不正を拒否する。field recordは`field_id:u16, flags:u16, length:u32, payload`を基準とする。

浮動小数点は定義の入口で有限性を検証し、定義の負の0は正の0へ正規化できる。正規実行状態とbackend保存では認定profileのbitを保持し、再開のための値を見た目の丸めで変更しない。

recordの必須/任意、追加field、既定値、旧versionからの変換をschema registryへ登録する。未知の必須field、未登録型、巨大なlengthは候補復元を中止する。連鎖参照は最大深度と合計件数で制限し、cycleを許す参照はID再接続として扱う。

## 作者の状態schema

作者のrecordはfieldごとに型、required、既定値、範囲、最大長、参照型を持つ。独自enumの候補集合と、削除した候補のmigrationを定義する。Rustのprivate field名やTypeIdから永続schemaを暗黙生成して固定しない。

deriveは登録の手間を減らすが、version、安定キー、保存対象、transient fieldを明示させる。transientの復元値と再接続処理も指定する。登録済み型がdecodeできても、参照先とWorld不変条件をvalidateするまで公開しない。

## 容量と形式を検証するデータ

最大長、空、最小/最大数値、同名別型、未知version、参照欠落、cycle、圧縮展開量、途中で切れたrecordをfixtureにする。各fieldの変更はsave、network、importer、hash、migrationへの影響をreviewする。
