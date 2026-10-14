# 16bit Material ID & 動的データ駆動レジストリ完全仕様書 (`09_DYNAMIC_REGISTRY_16BIT.md`)

> **対象読者**: テラリアのように数千〜1万種類以上のブロックやマテリアル、MOD拡張に対応したサンドボックスゲームを開発したいクリエイター。  
> **本書の目的**: ハードコードされた固定マテリアル配列の限界を打破し、**「L1 キャッシュ効率（4バイトセル）を1ミリも犠牲にせず、計算量を O(1) のまま維持し、最大 65,535 種類のブロックを動的登録可能にする」** アーキテクチャの完全な設計と使用法を解説します。

---

## 1. なぜ「1万個のブロック」を追加しても破綻しないのか？（3大根拠）

### ① メモリ効率：厳密な 4 バイト（32-bit）固定の維持
`Cell` 構造体は、16-bit のマテリアル ID を格納しつつ、アライメントとパディングを極限まで詰めることで **厳密に 4 バイト** に収められています。

```rust
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
#[repr(C, align(4))]
pub struct Cell {
    /// マテリアル識別番号: 0 〜 65,535（16-bit）
    pub material: u16,
    /// シミュレーション用直交フラグ: 8-bit
    pub flags: u8,
    /// コンテキスト・テクスチャバリエーションデータ: 8-bit
    pub data: u8,
}
```

* **サイズ**: 2 バイト (`u16`) + 1 バイト (`u8`) + 1 バイト (`u8`) = **4 バイト（32-bit）**
* **アライメント**: 4 バイト境界
* **L1 キャッシュ適合**: 現代の CPU キャッシュライン（64 バイト）に **ジャスト 16 個** のセルが収まります。全画面（640x360 = 約 23 万セル）でもメモリ消費はわずか **約 900 KiB** に留まります。

---

### ② 計算量：マテリアル数に依存しない「完全な O(1) 定数時間」
セルオートマトンの毎フレームの更新時間は、**「登録されているマテリアルの数」とは一切無関係**です。

```
【初心者が誤解しがちな計算モデル】
マテリアルが 10,000 個ある → 毎 tick、どのマテリアルか探すために 10,000 回探索？ ── ❌ 完全に誤り！

【Charon Engine の実際の O(1) 配列引き】
セルの ID（例: 7777 番「オリハルコン鉱石」）を取得
  ↓
レジストリのメモリ配列をインデックスで直接参照: &self.materials[7777]
  ↓
わずか 1〜3 CPU サイクル（O(1) 定数時間）で物性（密度・流速・落下速度）を取得！
```

マテリアルが **5 種類でも、10,000 種類でも、参照にかかる時間はナノ秒単位で全く同じ** です。したがって、クリエイターがどれだけブロックを追加しても計算量が爆発することはありません。

---

### ③ 描画量：8x8 テクスチャの O(1) 適応的サンプリング
描画時も同様に、セルのマテリアル ID から対応する `MaterialDef` の 8x8 テクスチャ配列を O(1) で参照し、ピクセル色を決定します。

---

## 2. クリエイター向け マテリアル登録 API

### 基本的な登録コード（Rust）

クリエイターは、`MaterialBuilder` を用いて直感的に新しいブロックや流体を定義し、`world.registry` または起動時のレジストリへ登録できます。

```rust
use charon_engine::simulation::material::{MaterialBuilder, Behavior, MaterialRegistry};

let mut registry = MaterialRegistry::with_defaults();

// 1. 新しい鉱石ブロックの登録（例: オリハルコン鉱石）
let orichalcum_id = registry.register_builder(
    MaterialBuilder::new("orichalcum_ore", [180, 50, 220, 255])
        .behavior(Behavior::Solid)
        .density(255)
        .can_be_washed_away(false) // 水流に耐える
);

// 2. 新しい流体の登録（例: 溶岩）
let lava_id = registry.register_builder(
    MaterialBuilder::new("lava", [255, 100, 20, 255])
        .behavior(Behavior::Liquid)
        .density(180)            // 水（100）より重く、水の下に沈む
        .max_fall_speed(3)
        .flow_speed(1)           // 水より粘性が高くゆっくり広がる
        .dispersion(2)
);

// 3. 登録された ID でワールドを初期化
let world = World::with_registry(640, 360, registry);
```

---

## 3. 外部データ駆動（TOML / JSON 設定ファイルからの読み込み設計）

ゲーム製作者や MOD 開発者がプログラムコードを 1 行も書かずに、外部の設定ファイル（TOML）を置くだけでブロックを追加できるデータ駆動スキーマに対応可能です。

```toml
# mods/fantasy_pack/materials.toml

[[materials]]
name = "mithril_ore"
behavior = "Solid"
density = 240
max_fall_speed = 4
flow_speed = 1
can_be_washed_away = false
base_color = [100, 200, 255, 255]

[[materials]]
name = "poison_sludge"
behavior = "Liquid"
density = 110
dispersion = 3
max_fall_speed = 4
flow_speed = 2
can_be_washed_away = false
base_color = [80, 220, 50, 220]
```

### TOML 読み込みローダーの雛形
```rust
use serde::Deserialize;

#[derive(Deserialize)]
struct MaterialConfig {
    name: String,
    behavior: String,
    density: u8,
    base_color: [u8; 4],
    // ...
}

pub fn load_materials_from_toml(path: &str, registry: &mut MaterialRegistry) -> Result<(), Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)?;
    let configs: Vec<MaterialConfig> = toml::from_str(&content)?;

    for cfg in configs {
        let behavior = match cfg.behavior.as_str() {
            "Liquid" => Behavior::Liquid,
            "Powder" => Behavior::Powder,
            _ => Behavior::Solid,
        };

        registry.register_builder(
            MaterialBuilder::new(cfg.name, cfg.base_color)
                .behavior(behavior)
                .density(cfg.density)
        );
    }
    Ok(())
}
```

---

## 4. 有名サンドボックスゲームとの比較まとめ

| 比較項目 | Noita | Minecraft (Java) | Terraria | **Charon Engine (本設計)** |
|---|---|---|---|---|
| **マテリアル表現** | セルオートマトン | 3D ボクセル | 2D タイルグリッド | **2D セルオートマトン** |
| **ID サイズ** | 16-bit | 16-bit | 16-bit | **16-bit (u16)** |
| **最大種類数** | 65,536 | 65,536 | 65,536 | **65,536 (0..65535)** |
| **1セルサイズ** | 4〜8 バイト | チャンクパレット間接参照 | 4 バイト | **厳密に 4 バイト** |
| **拡張性** | XML 定義 | データパック / レジストリ | 内部定数 + MOD | **Rust API / TOML レジストリ** |
| **計算量** | O(1) | O(1) | O(1) | **O(1) 定数時間** |
