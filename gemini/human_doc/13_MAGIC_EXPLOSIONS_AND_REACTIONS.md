# 魔法・爆発・セル化学反応完全仕様書 (`13_MAGIC_EXPLOSIONS_AND_REACTIONS.md`)

> **本書の目的**  
> Noita のような「世界全体がシミュレーションされ、爆発で地形が吹き飛び、物質同士が化学反応を起こす」魔法と爆発の完全な数理アルゴリズムを提供します。

---

## 1. 爆発数理（衝撃波・クレーター形成・破片四散）

爆発が発生した際、爆心地からの距離 $r$ に応じて 3 つのゾーンで異なる物理処理を行います。

```text
       ┌───────────────────────┐
       │   ゾーン 3: 外周衝撃波 (ノックバック & 構造崩壊)
       │   ┌───────────────────┐
       │   │  ゾーン 2: 粉砕領域 (ブロックを粒子化して放射状に吹き飛ばす)
       │   │   ┌───────────────┐
       │   │   │ ゾーン 1: 蒸発 (完全消滅 AIR)
       │   │   │      (★) 爆心地
```

### 爆発エンジンの Rust 実装
```rust
pub fn create_explosion(world: &mut World, cx: i32, cy: i32, radius: i32, power: f32) {
    let r_sq = radius * radius;
    let inner_sq = (radius / 2) * (radius / 2);

    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let dist_sq = dx * dx + dy * dy;
            if dist_sq > r_sq {
                continue;
            }

            let x = cx + dx;
            let y = cy + dy;

            if x < 0 || x >= world.width as i32 || y < 0 || y >= world.height as i32 {
                continue;
            }

            let idx = (y as u32 * world.width + x as u32) as usize;
            let mut cell = world.cells[idx];
            if cell.material == 0 { // AIR
                continue;
            }

            if dist_sq <= inner_sq {
                // ゾーン 1: 爆心地近傍 → 蒸発して消滅
                world.cells[idx] = Cell::new(0); // AIR
            } else {
                // ゾーン 2: 外周 → 破砕され、衝撃波で放射状に吹き飛ぶ
                let dist = (dist_sq as f32).sqrt().max(1.0);
                let dir_x = dx as f32 / dist;
                let dir_y = dy as f32 / dist;

                // 自由落下フラグを立てて粒子化
                cell.set_falling(true);
                world.cells[idx] = cell;

                // 水しぶき・破片パーティクルとして空中に放出
                let blast_force = (1.0 - dist / radius as f32) * power;
                world.add_particle(SplashParticle::new(
                    x as f32,
                    y as f32,
                    dir_x * blast_force + rand_offset() * 2.0,
                    dir_y * blast_force - 1.5,
                    0.6,
                    [255, 120, 30, 255], // 火花・破片色
                ));
            }
        }
    }
}
```

---

## 2. セル間化学反応マトリクス（O(1) 反応テーブル）

隣接するセル同士が接触したときに起こる反応を定義します。

| 接触セル A | 接触セル B | 反応結果 | 現象 |
|---|---|---|---|
| **水 (WATER)** | **溶岩 (LAVA)** | **黒曜石 (OBSIDIAN) + 水蒸気 (STEAM)** | 溶岩が冷え固まり強固な岩盤になる |
| **火 (FIRE)** | **木 (WOOD)** | **火 (FIRE) + 灰 (ASH) + 煙 (SMOKE)** | 森林火災が延焼する |
| **酸 (ACID)** | **土/石 (DIRT/STONE)** | **空気 (AIR) + 有毒ガス (TOXIC_GAS)** | 地形を溶かして貫通する |
| **雷 (LIGHTNING)**| **水 (WATER)** | **帯電水 (CHARGED_WATER)** | 水たまり全体に電撃が拡散し大ダメージ |

### 高速 O(1) 反応評価のコード設計
```rust
#[inline]
pub fn check_reaction(cell_a: &mut Cell, cell_b: &mut Cell, registry: &MaterialRegistry) -> bool {
    let id_a = cell_a.material;
    let id_b = cell_b.material;

    // 水 + 溶岩の反応
    if (id_a == 3 && id_b == 5) || (id_a == 5 && id_b == 3) { // 3: WATER, 5: LAVA
        let obsidian_id = 6; // OBSIDIAN
        cell_a.material = obsidian_id;
        cell_a.set_falling(false);
        cell_b.material = 0; // STEAM または AIR
        return true;
    }

    false
}
```
