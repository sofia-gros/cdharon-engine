# VFX & 超高性能パーティクル完全仕様書 (`15_PARTICLES_AND_VFX.md`)

> **本書の目的**  
> 毎フレーム数千個の火花、煙、血飛沫、魔法エフェクトを発生させても 60 FPS を 1ms 以下で維持する、ヒープ確保ゼロの固定長パーティクルプールと、ピクセルグリッドとの相互変換ロジックを解説します。

---

## 1. ゼロアロケーション・パーティクルプール（リングバッファ）

ゲーム中にパーティクルを `Vec::push` するとヒープ割り当てで GC/メモリアロケータがボトルネックになります。  
**固定長配列（例: 2048 個）を循環利用（リングバッファ）** することで、CPU 負荷をゼロにします。

```rust
pub const MAX_PARTICLES: usize = 2048;

#[derive(Clone, Copy, Default)]
pub struct VfxParticle {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub life: f32,     // 残り寿命（0.0 〜 1.0）
    pub decay: f32,    // 1 tick あたりの減衰量
    pub color: [u8; 4],
    pub settles_as_cell: Option<u16>, // 寿命終了または着地時にセル化するマテリアルID
}

pub struct ParticleSystem {
    particles: [VfxParticle; MAX_PARTICLES],
    head: usize,
}

impl ParticleSystem {
    pub fn new() -> Self {
        Self {
            particles: [VfxParticle::default(); MAX_PARTICLES],
            head: 0,
        }
    }

    /// 新しいパーティクルをプールにスポーン（O(1)・アロケーションなし）
    pub fn spawn(&mut self, mut p: VfxParticle) {
        self.particles[self.head] = p;
        self.head = (self.head + 1) % MAX_PARTICLES;
    }

    /// 全パーティクルの更新と着地セル化判定
    pub fn update(&mut self, world: &mut World) {
        for p in self.particles.iter_mut() {
            if p.life <= 0.0 {
                continue;
            }

            p.x += p.vx;
            p.y += p.vy;
            p.vy += 0.08; // 重力
            p.life -= p.decay;

            // 地面に着地した際のセル化（例: 血飛沫が床に落ちて血セルになる）
            if let Some(mat_id) = p.settles_as_cell {
                let cell = world.get_cell(p.x as i32, p.y as i32);
                if world.registry.get(cell.material).behavior == Behavior::Solid {
                    // 直前の空中位置にセルを配置
                    let prev_x = (p.x - p.vx) as i32;
                    let prev_y = (p.y - p.vy) as i32;
                    if world.get_cell(prev_x, prev_y).material == 0 {
                        world.paint_brush(prev_x, prev_y, 1, mat_id);
                        p.life = 0.0; // 消滅
                    }
                }
            }
        }
    }
}
```

---

## 2. 代表的 VFX エフェクトプリセット

1. **爆発の火花**: 高速で全方位に散らばり、重力で弧を描きながらオレンジから赤へと減衰。
2. **煙（Smoke）**: 上昇気流（負の重力）でゆっくり浮上し、アルファ値がフェードアウト。
3. **血飛沫（Blood）**: 敵を攻撃した瞬間に飛び散り、床や壁に激突するとその場に付着・セル化。
4. **魔法オーラ（Magic Spark）**: サイン波でらせん状に回転しながら漂う。
