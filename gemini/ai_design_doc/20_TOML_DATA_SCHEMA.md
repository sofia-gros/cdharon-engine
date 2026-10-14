# 外部 TOML / JSON データ駆動スキーマ & ホットリロード (`ai_design_doc/20_TOML_DATA_SCHEMA.md`)

> **本書の目的**  
> ゲーム開発者や MOD 制作者が、Rust コードをコンパイルし直すことなく、外部テキスト設定ファイル（TOML / JSON）を編集・保存するだけでブロックやアイテムを即座に追加・調整できるスキーマ定義とホットリロード仕様を定義します。

---

## 1. マテリアル定義 TOML 完全スキーマ

```toml
# mods/core_expansion/materials.toml

[[materials]]
id_name = "mithril_ore"          # 一意な文字列識別子
display_name = "ミスリル鉱石"     # ゲーム内表示名
behavior = "Solid"               # Empty | Solid | Liquid | Powder | Gas
density = 240                    # 0..255 (重さ)
dispersion = 0                   # 水平拡散力 (液体用)
max_fall_speed = 4               # 崩落時の最大自由落下速度
flow_speed = 1                   # 流速
can_be_washed_away = false       # 水流で流されるか
hardness = 4.5                   # 採掘に必要な硬度
texture = "textures/mithril.png" # 8x8 PNG テクスチャ相対パス

[[materials]]
id_name = "acid"
display_name = "濃硫酸"
behavior = "Liquid"
density = 110                    # 水 (100) より重い
dispersion = 3
max_fall_speed = 4
flow_speed = 2
can_be_washed_away = false
texture = "textures/acid.png"

# セル化学反応の定義
[[reactions]]
reactant_a = "acid"
reactant_b = "Stone"
product_a = "toxic_gas"
product_b = "Air"
reaction_rate = 0.8              # 接触時に反応が起きる確率 (0.0〜1.0)
```

---

## 2. アセットの実行時ホットリロード（Runtime Hot-Reloading）

開発時および MOD 制作時、`notify` クレートによるファイル変更監視スレッドを動作させます。
1. `materials.toml` またはテクスチャ PNG がエディタで保存される。
2. エンジンがファイル変更イベントを受信。
3. 次のフレームの開始時に、ゲームを停止させることなくレジストリと GPU テクスチャアトラスを再生成・即時反映。
4. クリエイターは **「ゲームを動かしたまま、色や流速の調整結果をリアルタイムに確認」** できます。
