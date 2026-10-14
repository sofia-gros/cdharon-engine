# カメラシステム & ピクセルパーフェクト追従・画面振動 (`ai_design_doc/18_CAMERA_AND_PIXEL_PERFECT.md`)

> **本書の目的**  
> プレイヤーを滑らかに追従しながらも「1 ピクセル未満の端数でドット絵が歪んだりガタつかない」ピクセルパーフェクトカメラ追従と、爆発や打撃時のジッターフリー画面振動（Screen Shake）の仕様を定義します。

---

## 1. サブピクセル補間とピクセルグリッドスナップ

カメラの座標を浮動小数点数（`f32`）で滑らかに移動させつつ、最終描画時にピクセルグリッドへ整数スナップさせる「2 段階カメラ」を採用します。

```text
[プレイヤー物理座標] (x: 104.382, y: 55.719)
        │
        ▼ (指数イージング追従: lerp / damp)
[カメラ仮想浮動小数点座標] (cam_x: 103.114, cam_y: 54.882)
        │
        ▼ (画面レンダリング時の丸め: round())
[整数ピクセル描画オフセット] (draw_x: 103, draw_y: 55)
```
* **ガタつき（Jitter）の完全防止**:
  表示ディスプレイが 4K や 1080p の場合、整数倍率（例: 3 倍拡大）の物理ピクセル境界上でサブピクセル補間を行うことで、ドット絵の形状を保ちながらバターのように滑らかなスクロールを実現します。

---

## 2. 画面振動システム（Screen Shake & Trauma Model）

単なるランダムオフセットではなく、GDC 等で定評のある **「Trauma（トラウマ）モデル」** を採用します。

$$ \text{ShakeAmount} = \text{Trauma}^2 $$
$$ \Delta x = \text{ShakeAmount} \times \text{MaxOffset} \times \text{Noise}(\text{time} \times \text{Frequency}) $$

```rust
pub struct CameraShake {
    /// トラウマ値 (0.0 〜 1.0)
    pub trauma: f32,
    /// 1 秒あたりのトラウマ減衰速度
    pub decay: f32,
    /// 最大揺れピクセル幅
    pub max_offset: f32,
    /// 揺れの回転角度（ラジアン）
    pub max_angle: f32,
}

impl CameraShake {
    /// 爆発や被弾時に呼び出す
    pub fn add_trauma(&mut self, amount: f32) {
        self.trauma = (self.trauma + amount).min(1.0);
    }

    pub fn update(&mut self, dt: f32) -> (f32, f32, f32) {
        if self.trauma <= 0.0 {
            return (0.0, 0.0, 0.0);
        }

        let shake = self.trauma * self.trauma; // 2乗で急激に立ち上がり、自然に収束
        let offset_x = shake * self.max_offset * rand_range(-1.0, 1.0);
        let offset_y = shake * self.max_offset * rand_range(-1.0, 1.0);
        let angle = shake * self.max_angle * rand_range(-1.0, 1.0);

        self.trauma = (self.trauma - self.decay * dt).max(0.0);
        (offset_x, offset_y, angle)
    }
}
```
* 小さな銃撃では `trauma += 0.1`、至近距離のメガ爆弾では `trauma = 1.0` とすることで、直感的かつ強烈な画面フィードバックが得られます。
