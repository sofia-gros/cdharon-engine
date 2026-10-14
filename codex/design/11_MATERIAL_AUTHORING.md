# クリエイター向けブロック定義と属性アタッチ

本書はユーザー指定の必須要件です。C02/C03/C05に従い、標準物性と作者拡張を同じ検証・凍結手順に載せます。ブロックはセル世界で使うMaterialDef、配置itemは別ItemDefとして関連付けます。

## 定義するもの

| 分類 | 標準項目 | 意味と検証 |
| --- | --- | --- |
| 基本 | stable key、behavior、tags | IDではなく名前で参照。tagsは分類用 |
| 重さ | `mass_per_cell` | 1占有セルの正のゲーム質量。整数1〜1,000,000、初期water=1000 |
| 軽さ・浮きやすさ | `buoyancy_scale` | 流体中の浮力応答倍率。Q係数0〜4、実効浮力は密度差と組み合わせる |
| 重力応答 | `gravity_scale` | 重力応答倍率0〜4。質量から自由落下速度を直接増やさない |
| 流動 | fall/flow速度、粘性、摩擦、drag | 速度・探索はC08の8セル上限以下。drag/摩擦は0〜1 |
| 構造 | hardness、cohesion、anchor、collision mask | 採掘抵抗・局所支持。壁と背景材の衝突分類 |
| 可燃性 | ignition energy、fuel ticks、burn rate、products | fuel_ticks=0は不燃。発火条件、熱発生、燃え尽き生成物を独立定義 |
| 熱 | heat capacity、conductivity、phase family | C04ではなく熱phaseの専用計算。容量/係数は物理設計の制限内 |
| 化学 | reaction rules、corrosion resistance | 定義済みrule参照。入力/出力予約を必須化 |
| 表示 | texture、base color、emission、opacity | 物理物性と別のcoldデータ |
| 作者属性 | typed property、attachment | コアが意味を知らない項目も、型とversionと保存方針を宣言 |

全セルは同じ格子体積なので、相対密度順位は`mass_per_cell`から導きます。重さと軽さを独立した逆数フィールドとして二重登録しません。作者UIは軽い/重いのプリセットから同じmass値を編集できます。「軽いのに落下が速い」等のゲーム表現はgravity/drag側で明示設定します。

このゲーム質量は交換優先度・浮力・damage等に使用できます。異物質交換は各トークンのmassを維持しますが、生成/反応の実在化学量論は保証しません。変換ruleにはトークンおよびゲーム質量の増減を明示し、意図しない生成を検出します。1セル部分充填・混合物は1.0外です。

## 定義例

TOML schema v1とRust APIの両方で定義できます。実装済みTOMLの全例は[`light_wood.toml`](../../content/examples/light_wood.toml)を参照してください。現在のschemaはname、behavior、mass、color、移動基本係数、typed properties/attachments、flammability宣言を持ちます。heat capacity、conductance、hardness等の標準solver fieldはまだ読み込まず、未知fieldとして拒否します。

現在のRust APIでの定義例:

```rust
let mut materials = MaterialRegistry::with_default_materials_builder();
materials.register_builder(
    MaterialBuilder::new("sample:light_wood", [120, 80, 40, 255])
        .behavior(Behavior::Solid)
        .mass_per_cell(500)
        .property("sample:mana", PropertyValue::Integer(12))
        .attach(MaterialAttachment {
            kind: "sample:mana_receiver".into(),
            version: 1,
            values: vec![],
        })
        .flammability(Flammability {
            ignition_energy: 800,
            fuel_ticks: 600,
            heat_per_tick: 8,
            exhaust_material: Some("core:air".into()),
        }),
)?;
materials.freeze()?;
let world = World::with_registry(640, 360, materials)?;
```

Property keys・attachment kind・referenceはnamespace付きで指定します。現在は値の型と構文、重複、個数上限、mass範囲、exhaust参照を検査します。schemaが定義するrequired/min/maxやattachment payload形式の検証は次段階です。

```toml
schema_version = 1
name = "sample:light_wood"
behavior = "solid"
mass_per_cell = 500
color = [120, 80, 40, 255]
dispersion = 0
max_fall_speed = 0
flow_speed = 0
can_be_washed_away = false

[[properties]]
key = "sample:mana_capacity"
kind = "integer"
integer = 12

[flammability]
ignition_energy = 800
fuel_ticks = 600
heat_per_tick = 8
exhaust_material = "core:air"

[[attachments]]
kind = "sample:mana_receiver"
version = 1

[[attachments.values]]
key = "sample:channel"
kind = "enum"
string = "blue"
```

TOMLの小数係数は検証時に指定固定小数点へ丸め、その結果を正規化content hashへ入れます。NaN/inf、負の質量、未知単位、invalid product、燃焼cycleの無制限生成を拒否します。燃料・反応を使わないbehaviorでもfieldを無視せず、許可する組合せか診断します。

残fuel/wetはC02のu16 timerなので定義は0〜65,535tick内、可燃材のburn_rateは1以上です。長時間timerを必要とする拡張は別schema/stateを登録します。候補属性として弾性・反発、電気伝導、腐食、魔力応答などもtyped schemaへ追加でき、未実装の標準solverの機能とは区別します。

燃焼状態は残fuel timerを持ち、熱とproductsのcommitが可能なtickだけfuelを消費します。副産物容量が不足すれば延期するため、そのtickで一方的に燃料だけ失いません。煙を物質として生成するか、視覚エフェクトとして生成するかはruleに明示します。

## 作成・定義・アタッチAPI

作者は起動前のContentBuilderでPropertySchemaとAttachmentSchemaを登録し、MaterialBuilderへ値をattachできます。freeze後はread-onlyです。schema登録のない属性、同名別型、未知attachment versionはロードエラーです。

```rust
// 責務の案。型名・関数名はM3で確定する。
builder.register_property_schema(mana_conductance_schema)?;
builder.register_attachment_schema(mana_receiver_schema)?;
builder.add_material(wood_definition
    .with_property("sample:mana_conductance", PropertyValue::Int(12))
    .attach(mana_receiver_definition))?;
let content = builder.freeze()?;
```

PropertyValueはbool、i64、固定小数点、列挙、stable reference、上限付きstring/listに限定します。schemaにdefault、min/max、required、最大byte数、simulationに影響するかを宣言します。正規属性・attachmentはcontent hashに含め、保存には参照するcontent versionを記録します。

作者独自属性は自動的に物理機能を生みません。Rust game systemがqueryで読み、通常commandを提出します。標準物性を上書きするcallbackや、独自属性から無検証でWorldを書き換える機能は設けません。

material単位のattachmentは共有定義です。「置いた特定ブロックの充電量」のようなinstance stateは別の型付きsidecar/componentです。状態schema、初期化、移動時transfer、反応時変換、freeze、snapshot/hash/codecを必須登録し、登録が不完全ならinstance attachmentを有効化しません。

任意instance stateのbyte数・件数にも容量制限を設けます。第一版では既知の密sidecarと、少数の機能ブロック向け世代付きcomponent storeに絞り、セルごとのHashMapを作りません。instance stateがあるchunkは、そのsystemが恒等性を証明できるまでsleep禁止です。

## 標準物理への接続

2026-10-14実装状態: `MaterialBuilder::mass_per_cell`と標準mass値はセル交換の優先順位で使用します。旧`density(u8)` builderはmassへ変換するdeprecated adapterです。Rust builderとTOML schema v1は`Bool`/`Integer`/`FixedQ16`/`Enum`/`Reference`のnamespaced property、version付きattachment、燃焼宣言を登録し、重複key・不正name・mass上限・未解決exhaust material・未知TOML fieldを検証します。入力文書は64KiBに制限され、作者値はdiagnostic fingerprintへ含みます。Property/attachment schemaによるrequired/min/max検証、instance attachment state、燃焼solverは未実装で、燃焼宣言は現在のsimulationで発火しません。

massは密度交換、buoyancy/dragは流体とentity応答、hardnessは採掘、cohesionはP4、combustion/thermalはP6で使用します。使うphaseが未実装のfieldを製品UIで「有効」と表示しません。

粉体・液体の現行CAは速度場を持たないため、gravity_scale等がすべてのbehaviorへ同じ意味で効くとは限りません。M3ではbehavior別に対応表を固定し、セル移動の確率・速度上限へのmappingを定義します。unsupported combinationは拒否します。entity/正規粒子では固定速度へ適用可能です。

V03/V05/V09/V12で、軽い木が水より浮きやすいこと、重い物質の交換方向、可燃/不燃、満容量での燃料保存、独自属性のround-tripを確認します。
