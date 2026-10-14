# 粉体力学 & 水流侵食・運動量ドラッグ (`ai_design_doc/10_POWDER_PHYSICS_AND_EROSION.md`)

> **本書の目的**  
> 砂や雪、崩落土などの粉体が「一定の安息角で山を作り、雪崩を起こし、水流の運動量によって押し流される（流砂・侵食）」粉体物理の完全な数理モデルを定義します。

---

## 1. 粉体の安息角（Angle of Repose）と斜面雪崩

粉体は直下が塞がっている場合、左右斜め下（$45^\circ$ 方向）の空間へ滑り落ちます。

```text
       [砂の落下]
           ▼
           ■
         ■ ■ ■        <- 斜面が 45度を超えると
       ■ ■ ■ ■ ■         左右に滑落して円錐形の山（安息角）を形成する
```

* **摩擦係数（Friction）**:
  乾いた砂は $45^\circ$ の山を作りますが、湿った土や氷の粉（雪）はより急峻、あるいはなだらかな斜面を形成します。これは斜め下探索時の確率パラメータ（ランダムジッター）で調整されます。

---

## 2. 水流侵食 & 運動量ドラッグ（Fluid Momentum Drag）

水が横方向に流れる際、進行方向に砂や崩落土が存在する場合、流体の運動量によって粒子を前方へ押し出します。

```text
  [水流] --->  [砂粒子]  [空間 AIR]
  (WATER)     (SAND)      (AIR)
     │           │          │
     ▼           ▼          ▼
  (AIR)  ──>  (WATER) ──> (SAND)  ★砂が1歩押し出され、水がその位置へ滑り込む！
```

### 侵食判定アルゴリズム
```rust
let is_washable = target_def.can_be_washed_away 
    && (target_def.behavior == Behavior::Powder || target_cell.is_falling());

if is_washable {
    let push_to_x = nx + dir;
    if push_to_x >= 0 && push_to_x < world_w {
        let push_idx = (y * width + push_to_x as u32) as usize;
        if self.cells[push_idx].material == AIR {
            // 粒子を押し出し、水がそこへ進む（侵食・運搬の成立）
            self.cells[push_idx] = target_cell;
            self.cells[target_idx] = curr_cell;
            self.cells[curr_idx] = Cell::new(AIR);
            return;
        }
    }
}
```

* **物理的帰結**:
  川の底に積もった砂が水流で下流へ流され、ダムが決壊した際に土砂が一気に押し流される「土石流」の自然現象が、この極めてシンプルな O(1) 規則から創発的に再現されます。
