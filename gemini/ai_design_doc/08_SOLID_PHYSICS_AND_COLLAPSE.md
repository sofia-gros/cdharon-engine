# 固体力学 & 構造崩壊アルゴリズム (`ai_design_doc/08_SOLID_PHYSICS_AND_COLLAPSE.md`)

> **本書の目的**  
> ピクセル世界における「土や石などのブロックが自立し、削られると体積不足で自然に雪崩のように崩落する」構造強度力学と、4x4 半ブロック密度崩落アルゴリズムの完全な数理を定義します。

---

## 1. 固体の自立限界と体積閾値（4x4 構造強度モデル）

現実世界の岩石や土壌粒子は、隣接粒子同士の凝集力（Cohesion）によって自立します。しかし、破壊されて支持構造を失うと粉砕・崩落します。

```text
[安定した 4x4 ブロック]      [半分以上削られた状態]     [自立限界崩壊！]
 ■ ■ ■ ■                     . . . .                    . . . .
 ■ ■ ■ ■       ──(掘削)──>   . . . .       ──(崩落)──>  . . . .
 ■ ■ ■ ■                     ■ ■ . .                    . . . .
 ■ ■ ■ ■                     ■ ■ ■ ■                    ░ ░ ░ ░ (落下粒子化)
 (16セル: 構造安定)          (6セル: 過半数未満)        (FLAG_FALLING 付与)
```

### 数理モデル
ある 4x4 領域 $B$ における固体セル数 $N_{\text{solid}}$ に対し、
$$ \text{StructuralIntegrity}(B) = \begin{cases} 
\text{Stable} & (N_{\text{solid}} \ge 8) \\
\text{Collapsing} & (1 \le N_{\text{solid}} < 8) 
\end{cases} $$
* $N_{\text{solid}} \ge 8$: 構造を維持（静止固体ブロックとして振る舞う）。
* $1 \le N_{\text{solid}} < 8$: 体積が半分未満。構造強度を喪失し、すべての残存セルに **`cell.set_falling(true)`** を付与して自由落下粒子へ転換。

---

## 2. 浮遊孤立セルの重力検知（Support Connectivity）

空中に 1 ピクセルだけ取り残された不自然な浮遊ピクセルを検知し、重力落下させます。

### 4 方向隣接支持判定
$$ \text{HasSupport}(x, y) = \text{Solid}(x, y+1) \lor \text{Solid}(x, y-1) \lor \text{Solid}(x-1, y) \lor \text{Solid}(x+1, y) $$
* 最底辺（$y = H-1$）を除く場所で、上下左右に静止固体（`is_falling == false`）が 1 つも接続していない場合、即座に `cell.set_falling(true)` を適用します。

---

## 3. 着地時の再結合安定化（Settling & Annealing）

落下中の土や石が床（静止した地盤）に着地し、直下に安定支持が得られた場合：
```rust
if curr_cell.is_falling() {
    let mat_def = self.registry.get(curr_cell.material);
    if mat_def.behavior == Behavior::Solid {
        if y + 1 >= self.height || self.is_solid_neighbor(x, y + 1) {
            // ★フラグを降ろして再び静止固体ブロックに復帰！
            curr_cell.set_falling(false);
        }
    }
}
```
この可逆的な状態遷移により、「土砂崩れが発生し、谷底に着地して再び地面として踏み固められる」自然な物理現象が完全に再現されます。
