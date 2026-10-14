# 世界生成 & バイオーム・建造物生成完全仕様書 (`11_WORLDGEN_AND_STRUCTURES.md`)

> **本書の目的**  
> 単純な地表ノイズだけでなく、テラリアや Noita のような「多層地下洞窟」「バイオーム（森林・砂漠・雪原・地獄）」「プレハブ建造物（ダンジョン・廃坑・宝箱部屋）」を自動生成する完全なアルゴリズムと実装コードを提供します。

---

## 1. 世界の階層構造（深度レイヤー & バイオーム）

世界を垂直方向（Y軸）に 5 つの深度レイヤーに分割し、水平方向（X軸）に温度・湿度ノイズでバイオームを決定します。

```text
Y = 0      ┌────────────────────────────────────────────────────────┐
           │ 宇宙・上空（浮遊島、ハーピー、低重力）                 │
Y = 100    ├────────────────────────────────────────────────────────┤
           │ 地表・森林・砂漠・雪原（太陽光、樹木、建物、水たまり） │
Y = 300    ├────────────────────────────────────────────────────────┤
           │ 地下浅層（土と石、小さな空洞、鉱脈、廃屋）             │
Y = 600    ├────────────────────────────────────────────────────────┤
           │ 洞窟深層（巨大空洞、溶岩溜まり、希少鉱石、ダンジョン） │
Y = 1000   ├────────────────────────────────────────────────────────┤
           │ 地獄・最底辺（黒曜石、溶岩海、レンガ遺跡、難関ボス）   │
           └────────────────────────────────────────────────────────┘
```

---

## 2. 洞窟生成アルゴリズム（Perlin Worms & Cellular Automata）

単純な密度ノイズだけでは「曲がりくねった探検したくなる洞窟」は作れません。**Perlin Worms（パーリン・ワーム）** を使用して自然なトンネルを掘削します。

### パーリン・ワーム掘削の Rust 実装例
```rust
use crate::simulation::cell::Cell;
use crate::simulation::material::AIR;
use crate::simulation::world::World;

/// パーリン・ワーム（掘削虫）による洞窟トンネルの自動生成
pub fn carve_perlin_worms(world: &mut World, start_x: f32, start_y: f32, length: u32, radius: f32) {
    let mut x = start_x;
    let mut y = start_y;
    let mut angle = 1.2f32; // 初期進行角度（ラジアン）

    for step in 0..length {
        // ノイズによって進行方向をなだらかに蛇行させる
        let noise_val = (step as f32 * 0.05).sin() * 0.4;
        angle += noise_val;

        x += angle.cos() * 2.0;
        y += angle.sin() * 2.0;

        // ワームの胴体（円形）を空気（AIR）で削る
        let r = radius + (step as f32 * 0.1).sin() * 1.5; // 太さを脈動させる
        world.paint_brush(x as i32, y as i32, r.max(2.0) as i32, AIR);
    }
}
```

---

## 3. プレハブ建造物（ダンジョン・宝箱部屋）スタンプ方式

建造物はプロシージャル（自動計算）とプレハブ（固定テンプレート）のハイブリッドで生成します。

### プレハブの文字列表現（マップスタンプ）
```rust
pub const TREASURE_ROOM_PREFAB: &str = "\
WWWWWWWWWW
W........W
W...C....W
W.PPPPP..W
W........W
WWWWDDWWWW";

// 凡例:
// W = STONE_BRICK (石レンガ)
// D = WOODEN_DOOR (木の扉)
// C = CHEST       (宝箱)
// P = PLATFORM    (足場)
// . = AIR         (空間)
```

### プレハブ配置エンジンの Rust 実装
```rust
pub fn stamp_prefab(world: &mut World, origin_x: i32, origin_y: i32, prefab: &str) {
    for (row_idx, line) in prefab.lines().enumerate() {
        for (col_idx, ch) in line.chars().enumerate() {
            let wx = origin_x + col_idx as i32;
            let wy = origin_y + row_idx as i32;

            let (mat_id, is_falling) = match ch {
                'W' => (world.registry.get_id_by_name("stone_brick").unwrap_or(2), false),
                'P' => (world.registry.get_id_by_name("wood").unwrap_or(1), false),
                'C' => (world.registry.get_id_by_name("chest").unwrap_or(2), false),
                _ => (0, false), // AIR
            };

            if wx >= 0 && wx < world.width as i32 && wy >= 0 && wy < world.height as i32 {
                let idx = (wy as u32 * world.width + wx as u32) as usize;
                let mut cell = Cell::new(mat_id);
                cell.set_falling(is_falling);
                world.cells[idx] = cell;
            }
        }
    }
}
```

---

## 4. 鉱脈の生成（セルラー・クラスター）

石の中に金、鉄、銅などの鉱石を塊（クラスター）として埋め込みます。

```rust
pub fn spawn_ore_vein(world: &mut World, cx: i32, cy: i32, ore_id: u16, size: u32) {
    let mut current_x = cx;
    let mut current_y = cy;

    for _ in 0..size {
        if let Some(idx) = world.get_index(current_x, current_y) {
            // 石ブロックの場所のみ鉱石に置換
            if world.cells[idx].material == 2 { // STONE
                world.cells[idx] = Cell::with_shade(ore_id, (current_x % 8 + current_y % 8) as u8);
            }
        }
        // ランダムウォークで隣接セルへ広げる
        current_x += (rand_int() % 3) - 1;
        current_y += (rand_int() % 3) - 1;
    }
}
```
