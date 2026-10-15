# 扱うデータと形式の設計

## 単位と識別子

世界の基本単位は`cell`で、画像のピクセルや画面倍率とは独立する。整数cell座標は符号付き`i32`、中間の端点・距離・積は`i64`以上で検証する。世界は半開矩形、正方向は右と下。chunkは64×64cellで、負座標はEuclidean除算で変換する。

物体の位置は`f64`のcell、速度はcell/秒、角度はrad、時刻はtick、熱は整数のエンジンenergy単位、温度はmK。公開APIには`WorldPoint`、`Velocity`、`CellSize`、`TickSpan`などの型を使い、画面座標と混同しない。NaN/inf、範囲外の値、無効な寸法を入力境界で拒否する。

基準の物理backendへの変換は64cell/m、質量は1000エンジン質量単位/kgとする。adapterが位置・速度・力・慣性を一貫して変換し、公開APIのcell単位をbackendへ漏らさない。変換係数はWorld manifestと実行profileへ記録し、既存Worldの途中で変更しない。

実行時の定義IDは`u32`。0は空の定義として予約し、不明な参照を空へ変換しない。永続参照は`package:name`の安定キーを使う。各部分はASCII小文字、数字、`_`、`-`の1〜96文字とし、重複を拒否する。定義は安定キー順でruntime IDを確定する。

作者コードの短いキーは登録元packageで修飾してから検証する。登録中に返す`MaterialHandle`は安定キーを保持する型付き参照で、freeze前の仮の連番を公開しない。freeze時に対応表を確定し、World内の高速な配列参照には`MaterialId`を使う。定義fieldと永続recordの詳細は[共通schema](23_CONTENT_AND_STATE_SCHEMAS.md)に従う。

個体IDは`index:u32 + generation:u32`。Scene、World、Entity、BlockInstance、RigidBody、Jobなどの型を別にする。世代が尽きたslotは再利用しない。資産は型付きハンドルと保持leaseを分ける。IDのコピーだけでは個体の寿命は伸びない。

## 全データの分類

| 分類 | 主な型と内容 | 所有・永続化 |
| --- | --- | --- |
| 実行設定 | 設定schema、window/render/audio/assets/診断、能力の解決結果 | Engine。Worldに影響する解決済み項目はsave/replayにも記録 |
| 定義 | Material、Behavior、Reaction、Item、Recipe、Effect、EntityPrefab、Generator、Biome、UI theme | ContentRegistry。不変、安定キーとversionをmanifestへ |
| 資産 | Texture、SpriteSheet、Animation、Sound、Font、Shader、Prefab source、locale | AssetService。source/import hash、依存、保持leaseを管理 |
| 世界 | Cell、Layer、Chunk、BlockInstance、world clock、seed、生成履歴 | World。保存と正規hash |
| セルsidecar | energy、phase fraction、fuel、wet、速度残差、混合、instance参照 | chunkの型別配列。移動・変換・codecの対象 |
| 物体 | Entity components、RigidBody、local cells、shape revision、joint | World。allocatorとbackend正規状態も保存 |
| gameplay | item stack、装備、container、HP等の属性、effect、AI、装置、進行 | 型付きcomponent/resource。作者型も同じ登録経路 |
| 作業 | pending command、receipt位置、探索・生成・分割・反応の継続状態 | 正規JobStore。保存とhash |
| 入力・通信 | tick入力、action、接続権限、受信sequence、複製世代 | Runtime/World/Net。保存対象を仕様で区別 |
| 表示 | transform補間、描画snapshot、視覚粒子、light cache、audio voice | 表示サービス。正規hashへ含めない |
| 制作・診断 | importer version、manifest、source map、metrics、trace、crash bundle | tools/diagnostics。配布物と診断artifact |

任意の新しいデータ型はこの分類、所有者、寿命、容量、codec、hash、移動・削除時の扱いを登録時に宣言する。分類できない状態をグローバル変数へ追加しない。

標準cell/componentで表せない正規状態は、[25](25_OPEN_EXTENSION_AND_COLLISION_CONTRACTS.md)のGridChannelとStateStoreへ登録する。独自液量、別解像度、f32精度の運動state、graph、VM等を保持し、標準形式への毎tick変換で意味を失わせない。

## セルとchunkのメモリ

基準の`CellCore`は`material:u32, flags:u16, variation:u16`の8byte。flagsの使用bitはschemaに定義し、外観、更新済み印、温度を同じbitへ押し込まない。未定義bitの設定は拒否する。移動stampと予約印は保存しないscratch配列とする。

熱が有効なchunkは`energy:i64`配列を持つ。燃料・濡れ・phase fraction・速度残差は必要なchunkへ型別の配列として追加する。個体参照や特殊な混合状態は対応するstoreのindexを配列から参照する。セルごとのHashMapやBoxを基本格納方式にしない。

均一な静止chunkは`Uniform`、活動中は`Dense`、退避時はpaletteとRLEを使った圧縮表現を選べる。表現が異なっても論理的に同じ状態を返す。hashと保存の意味をメモリ表現へ結び付けない。

## ブロックの個体

`BlockDef`はMaterialDefに含む配置footprint、形状、破壊mode、anchor条件、texture参照。`BlockInstance`はID、定義キー、world anchor、layer、orientation、残存cell mask、独自componentを持つ。寸法はマテリアルごとに整数cellで定義する。

占有cellからblock個体を引くchunk-local indexと、世代付き個体IDの対応表を持つ。indexだけを保存形式へ露出しない。破壊やchunkを跨ぐ移動で参照を同時に更新し、空になった個体は関連するcomponentと一緒に破棄する。

## エンティティと作者component

Entity allocatorと型別のsparse-set component storeを使う。型は`ComponentKey, schema_version, layout, access, codec, migration, replication`を登録する。Rustの`TypeId`は実行時の型検索にだけ使い、永続キーにしない。汎用ECSの内部型を公開APIへ固定しない。

組み込みcomponentにはTransform、Sprite、BodyLink、Collider、Health、Status、InventoryRef、Controller、AI、EffectState、WorldAnchorを用意する。組み合わせは作者が選ぶ。作者型も同じquery・保存・複製の仕組みを利用する。

定常queryは型単位のslice/batchで処理し、EntityIdの安定順を取得する経路も用意する。componentの構造変更はcommit境界に遅延し、queryの途中でstoreの再配置を行わない。

## 定義schema

独自propertyはbool、整数、固定小数点、文字列enum、安定参照、型付きrecordで定義する。required、既定値、範囲、enum集合、参照先の種類、最大長をschemaに含める。instance componentの初期値も同じschemaで検証する。

挙動はコードのアドレスではなくBehaviorKeyと設定recordを持つ。Rust実装はプラグインが登録する。コードが必要な定義をデータだけで復元できるかのように扱わず、必要なプラグインと版をmanifestに記録する。

## ファイル形式

| ファイル | v1での形式と方針 |
| --- | --- |
| エンジン設定 | schema_version付きYAML。未知キー、型違い、単位違いを診断 |
| 作者の定義 | Rust builderが基準。data packはversion付きJSON recordを同じ検証へ通す |
| 資産manifest | UTF-8 JSON。安定キー、型、source path、importer、hash、依存 |
| prefab/map source | UTF-8 JSON。stable keyとcomponent record。Gitで差分を見られる形 |
| インポートcache | header、format/importer version、source/dependency hash、長さ、payload |
| save/replay/network | version付きlittle-endianの明示codec。長さと型を検証し、memory dumpを禁止 |

文字列はUTF-8、長さはbyte数を記録し、配列長と展開後byte上限を読取前に検証する。未知の必須recordは拒否し、任意recordの無視はschemaで許可した場合だけ行う。圧縮後のサイズだけで上限を判定しない。

## 容量の基準

既定profileはactive 512chunk、resident 2,048chunk、Entity 65,536、RigidBody 2,048、物質粒子262,144、command 16,384/tick、job 4,096とする。これは予約・診断用の初期値で、実際のメモリ予算以下になるようWorld作成時に検証する。すべてのchunkへ最大sidecarを無条件確保しない。

通常取引の既定上限は65,536書き込みcell、構造物や爆発の大規模操作は分割Job APIを使う。上限は型の表現限界まで作者が設定でき、設定変更は正規manifestに含める。APIは必要容量を見積もる`estimate`と、予約失敗の必要量・利用可能量を返す。

1セルの作者instance recordは既定4KiB、定義recordは既定256KiB、配列の要素数も型ごとに制限する。上限を増やす際は総byte予算とsave/networkの上限を同時に検証する。
