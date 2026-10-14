# ツルハシ・武器・採掘ツール完全仕様書 (`12_MINING_WEAPONS_AND_TOOLS.md`)

> **本書の目的**  
> テラリアのツルハシ掘削、剣のスイング近接攻撃、弓や銃の放物線弾道射撃、および破壊されたブロックのアイテムドロップ化を実現する物理・判定ロジックを解説します。

---

## 1. ツルハシ採掘システム（硬度 vs 採掘パワー）

マテリアルごとに「硬度（Hardness）」を定義し、ツルハシの「採掘パワー（Pickaxe Power）」でダメージを与えます。

### 採掘ダメージモデル
$$ \text{Damage} = \frac{\text{PickaxePower}}{\text{MaterialHardness}} \times \text{ToolSpeed} $$

```rust
// 採掘ツール定義
pub struct Tool {
    pub name: String,
    pub pickaxe_power: f32, // 銅=35%, 鉄=50%, 金=55%, ダイヤ=100%
    pub mining_radius: i32,  // 掘削半径（1〜4 ピクセル）
    pub reach_distance: f32, // 届く距離（ピクセル）
}

impl Tool {
    /// 指定座標のブロックを掘削
    pub fn mine_at(&self, world: &mut World, target_x: i32, target_y: i32) -> Option<u16> {
        let cell = world.get_cell(target_x, target_y);
        if cell.material == 0 { // AIR
            return None;
        }

        let mat_def = world.registry.get(cell.material);
        let hardness = mat_def.density as f32 / 50.0; // 密度から硬度を算出

        if self.pickaxe_power >= hardness {
            // 掘削成功: ブロックを空気（AIR）にし、ドロップしたマテリアルIDを返す
            let mined_mat = cell.material;
            world.paint_brush(target_x, target_y, self.mining_radius, 0); // AIR 配置
            // 周囲の体積が削られたことで、残存ブロックは自動的に 4x4 構造崩壊する！
            Some(mined_mat)
        } else {
            // 硬すぎて掘れない（火花エフェクト等を発生）
            None
        }
    }
}
```

---

## 2. 近接武器（剣のスイング円弧判定）

剣を振った際、プレイヤーの手元から扇状（円弧）の攻撃判定コライダーを毎フレーム回転させます。

```text
       Player
        (P) ───[Sword Blade]───> Hitbox Arc
         │   \
         │    \
         ▼     ▼ 扇状の範囲内の敵 AABB と矩形交差判定
```

### スイング弧の判定コード
```rust
pub struct MeleeSwing {
    pub center_x: f32,
    pub center_y: f32,
    pub radius: f32,
    pub current_angle: f32,
    pub arc_width: f32, // 扇形の角度幅（例: 0.8 ラジアン）
    pub damage: f32,
    pub knockback: f32,
}

impl MeleeSwing {
    /// 敵の AABB と接触しているか判定
    pub fn hits_enemy(&self, enemy_x: f32, enemy_y: f32, enemy_w: f32, enemy_h: f32) -> bool {
        let ex = enemy_x + enemy_w * 0.5;
        let ey = enemy_y + enemy_h * 0.5;

        let dx = ex - self.center_x;
        let dy = ey - self.center_y;
        let dist_sq = dx * dx + dy * dy;

        // 1. 半径内にあるか
        if dist_sq > self.radius * self.radius {
            return false;
        }

        // 2. 角度がスイング弧の範囲内にあるか
        let angle = dy.atan2(dx);
        let angle_diff = (angle - self.current_angle).abs();
        angle_diff <= self.arc_width * 0.5
    }
}
```

---

## 3. 遠隔投射物（弓矢・銃弾の弾道物理）

矢や銃弾は、ピクセル地形との高速衝突判定（レイキャスティング・ブレゼンハム）を行いながら移動します。

```rust
pub struct Projectile {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub gravity: f32,
    pub damage: f32,
    pub penetrates: u8, // 貫通回数
}

impl Projectile {
    pub fn update(&mut self, world: &mut World) -> bool {
        self.vy += self.gravity;

        let next_x = self.x + self.vx;
        let next_y = self.y + self.vy;

        // ブレゼンハムの直線補間で途中のピクセル壁との激突を検出（すり抜け防止）
        let hit_cell = world.get_cell(next_x as i32, next_y as i32);
        let behavior = world.registry.get(hit_cell.material).behavior;

        if behavior == Behavior::Solid {
            // 壁に激突！矢が刺さるか消滅
            return false; // 寿命終了
        }

        self.x = next_x;
        self.y = next_y;
        true // 生存
    }
}
```
