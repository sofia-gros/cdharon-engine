# 人間向け完全実装手順 & コードレシピ (`06_IMPLEMENTATION_RECIPES.md`)

> **本書の目的**  
> 設計理論を実際の Rust コードに落とし込むための、完全にコンパイル可能で型安全なコードスニペットと、具体的な実装手順を提供します。  
> 本書に記載されたコードは、Charon Engine の現行実装と 100% 整合しています。

---

## レシピ 1: 32bit セル構造体 (`src/simulation/cell.rs`)

CPU キャッシュライン（64 バイト）に 16 個が隙間なく並ぶ、厳密な 4 バイト・アライメント構造体です。

```rust
// src/simulation/cell.rs

/// 更新クロックビットマスク（二重更新防止用: Bit 0）
pub const FLAG_CLOCK: u8   = 1 << 0; // 0b00000001
/// 自由落下・崩落中フラグマスク（構造強度を失い落下中: Bit 1）
pub const FLAG_FALLING: u8 = 1 << 1; // 0b00000010
/// 燃焼中フラグマスク（将来拡張用: Bit 2）
#[allow(dead_code)]
pub const FLAG_BURNING: u8 = 1 << 2; // 0b00000100
/// 濡れフラグマスク（将来拡張用: Bit 3）
#[allow(dead_code)]
pub const FLAG_WET: u8     = 1 << 3; // 0b00001000

/// ワールド上の 1 ピクセルを表現するセル構造体（厳密に 4 バイト）
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
#[repr(C, align(4))]
pub struct Cell {
    /// マテリアル識別番号（16-bit: 0 〜 65,535。最大 6 万種対応）
    pub material: u16,
    /// シミュレーション用直交フラグ（8-bit）
    pub flags: u8,
    /// コンテキスト・テクスチャバリエーションデータ（8-bit）
    pub data: u8,
}

impl Cell {
    #[inline]
    pub const fn new(material: u16) -> Self {
        Self { material, flags: 0, data: 0 }
    }

    #[inline]
    pub const fn with_shade(material: u16, shade: u8) -> Self {
        Self { material, flags: 0, data: shade }
    }

    #[inline]
    pub const fn shade(&self) -> u8 {
        self.data
    }

    /// 今フレームで処理済みかを判定（二重移動防止）
    #[inline]
    pub fn is_updated(&self, clock_bit: u8) -> bool {
        (self.flags & FLAG_CLOCK) == clock_bit
    }

    /// 更新クロックビットを設定
    #[inline]
    pub fn set_updated(&mut self, clock_bit: u8) {
        if clock_bit == 1 {
            self.flags |= FLAG_CLOCK;
        } else {
            self.flags &= !FLAG_CLOCK;
        }
    }

    /// 自由落下・崩落中かどうかを判定
    #[inline]
    pub fn is_falling(&self) -> bool {
        (self.flags & FLAG_FALLING) != 0
    }

    /// 自由落下状態を設定
    #[inline]
    pub fn set_falling(&mut self, falling: bool) {
        if falling {
            self.flags |= FLAG_FALLING;
        } else {
            self.flags &= !FLAG_FALLING;
        }
    }
}
```

---

## レシピ 2: 4x4 半ブロック密度による当たり判定関数 (`src/entities/player.rs`)

1 ピクセルの散乱粒子に足を取られず、4x4 領域内の接触ピクセルが過半数（8 セル以上）のときのみ壁・床として衝突します。

```rust
// src/entities/player.rs 内のメソッド

/// 接触境界上の 4x4 領域サンプリングによる半ブロック判定
fn collides_with_half_block(&self, test_x: f32, test_y: f32, world: &World) -> bool {
    let x_start = test_x.floor() as i32;
    let x_end = (test_x + self.width).ceil() as i32;
    let y_start = test_y.floor() as i32;
    let y_end = (test_y + self.height).ceil() as i32;

    let grid_x_start = x_start / 4;
    let grid_x_end = (x_end + 3) / 4;
    let grid_y_start = y_start / 4;
    let grid_y_end = (y_end + 3) / 4;

    for gy in grid_y_start..grid_y_end {
        for gx in grid_x_start..grid_x_end {
            let bx = gx * 4;
            let by = gy * 4;

            let mut solid_count = 0;
            for py in 0..4 {
                for px in 0..4 {
                    let cx = bx + px;
                    let cy = by + py;

                    // プレイヤーの AABB と実際に重なっているピクセルのみを対象にする
                    if cx >= x_start && cx < x_end && cy >= y_start && cy < y_end {
                        let cell = world.get_cell(cx, cy);
                        let behavior = world.registry.get(cell.material).behavior;
                        // 静止している固体ブロック、または安定した砂のみを壁・床としてカウント
                        // 自由落下・崩落中のセル（is_falling = true）は通り抜ける
                        let is_obstacle = (behavior == Behavior::Solid && !cell.is_falling())
                            || behavior == Behavior::Powder;
                        if is_obstacle {
                            solid_count += 1;
                        }
                    }
                }
            }

            // 4x4 領域内の接触固体ピクセルが 8 個以上（過半数）あれば衝突
            if solid_count >= 8 {
                return true;
            }
        }
    }

    false
}
```

---

## レシピ 3: 4x4 構造崩壊システム (`src/simulation/world.rs`)

ブロックが削られて体積が半分未満（1〜7セル）になると、構造強度を失って重力崩落（`set_falling(true)`）します。

```rust
// src/simulation/world.rs 内のメソッド

fn check_structural_collapse(&mut self) {
    let width = self.width;
    let height = self.height;
    let offset = (self.tick_count % 4) as u32;
    let grid_w = width / 4;
    let grid_h = height / 4;

    for gy in 0..grid_h {
        for gx in 0..grid_w {
            if (gx + gy) % 4 != offset {
                continue;
            }

            let bx = gx * 4;
            let by = gy * 4;
            let mut solid_indices = Vec::with_capacity(16);

            for py in 0..4 {
                for px in 0..4 {
                    let x = bx + px;
                    let y = by + py;
                    let idx = (y * width + x) as usize;
                    let cell = self.cells[idx];

                    let behavior = self.registry.get(cell.material).behavior;
                    if behavior == Behavior::Solid && !cell.is_falling() {
                        solid_indices.push(idx);
                    }
                }
            }

            let total_solid = solid_indices.len();

            // 4x4 半ブロック（16セル）中、固体が 1〜7 個（体積が半分未満）なら崩壊！
            if total_solid > 0 && total_solid < 8 {
                for idx in solid_indices {
                    // ★マテリアルIDを変えることなく、フラグだけで崩落状態に移行！
                    self.cells[idx].set_falling(true);
                }
            }
        }
    }
}
```

---

## レシピ 4: クリエイター向け独自ブロックの追加コード

物理シミュレーションの内部コードを 1 行も修正することなく、直感的に新しいブロックを追加できます。

```rust
// main.rs またはゲーム初期化時
use charon_engine::simulation::material::{MaterialBuilder, Behavior, MaterialRegistry};
use charon_engine::simulation::world::World;

let mut registry = MaterialRegistry::with_defaults();

// 1. 新しいブロック（木材ブロック）を登録
let wood_id = registry.register_builder(
    MaterialBuilder::new("wood", [150, 100, 50, 255])
        .behavior(Behavior::Solid)
        .density(180)
        .can_be_washed_away(true) // 崩落した木片は水流に流される
);

// 2. カスタムレジストリでワールドを生成
let mut world = World::with_registry(640, 360, registry);

// 3. 配置（ブラシ描画）
world.paint_brush(300, 150, 8, wood_id);
```
