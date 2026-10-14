//! # パーティクル（Particle）モジュール
//!
//! 水しぶきや爆発破片など、グリッドセルから外れて自由軌道運動する微小粒子を管理します。
//! 連続的な浮動小数点座標を持ち、重力や初速に従って運動します。

/// 水しぶき等のエフェクト用粒子構造体
#[derive(Debug, Clone, Copy)]
pub struct SplashParticle {
    /// X 座標（浮動小数点、ワールドピクセル座標）
    pub x: f32,
    /// Y 座標（浮動小数点、ワールドピクセル座標）
    pub y: f32,
    /// X 方向速度（px/frame）
    pub vx: f32,
    /// Y 方向速度（px/frame）
    pub vy: f32,
    /// 現在の残り寿命（秒またはフレーム単位）
    pub life: f32,
    /// 最大寿命
    pub max_life: f32,
    /// 描画色（RGBA）
    pub color: [u8; 4],
}

impl SplashParticle {
    /// 新しい水しぶき粒子を生成します。
    ///
    /// # 引数
    /// * `x` - 初期 X 座標
    /// * `y` - 初期 Y 座標
    /// * `vx` - 初期 X 速度
    /// * `vy` - 初期 Y 速度
    /// * `life` - 生存時間（秒）
    /// * `color` - 描画色 (RGBA)
    pub fn new(x: f32, y: f32, vx: f32, vy: f32, life: f32, color: [u8; 4]) -> Self {
        Self {
            x,
            y,
            vx,
            vy,
            life,
            max_life: life,
            color,
        }
    }

    /// 粒子を 1 ステップ（1 tick）更新します。
    ///
    /// 重力を加算し、位置を更新し、寿命を減算します。
    ///
    /// # 引数
    /// * `gravity` - 重力加速度（例: 0.15）
    ///
    /// # 戻り値
    /// まだ生存している場合 `true`、寿命が尽きた場合 `false`
    pub fn update(&mut self, gravity: f32) -> bool {
        self.vy += gravity; // 重力落下
        self.vx *= 0.98; // 空気抵抗による減衰
        self.x += self.vx;
        self.y += self.vy;
        self.life -= 1.0 / 60.0;

        // フェードアウト効果（寿命に応じてアルファ値を減少）
        if self.max_life > 0.0 {
            let ratio = (self.life / self.max_life).clamp(0.0, 1.0);
            self.color[3] = (255.0 * ratio) as u8;
        }

        self.life > 0.0
    }
}
