# UI・インベントリ & クラフティング完全仕様書 (`16_UI_INVENTORY_AND_CRAFTING.md`)

> **本書の目的**  
> テラリアのようなピクセルアート UI（ホットバー、HP/マナバー、インベントリ）の描画と、作業台による近傍クラフト判定システムを完全実装するための設計書です。

---

## 1. ピクセルパーフェクト UI の描画パイプライン

UI はワールドの物理グリッドとは別に、**フレームバッファ（RGBA）の最前面に直接アルファブレンド描画** します。

```text
フレームバッファ描画順序:
1. ワールド全セル (World Cells)
2. 水しぶき・破片 (Particles)
3. プレイヤー・敵 (Entities)
4. 最前面 UI (Hotbar, Health, Inventory, Cursor) ★
```

### ホットバー（スロット枠）の描画コード
```rust
pub fn draw_hotbar(frame: &mut [u8], screen_w: u32, selected_slot: usize) {
    let slot_size = 20;
    let slot_gap = 2;
    let start_x = 10;
    let start_y = 10;

    for i in 0..10 {
        let sx = start_x + (i as i32 * (slot_size + slot_gap));
        let sy = start_y;
        let is_selected = i == selected_slot;

        // 枠線の色（選択中は金色、通常は灰色）
        let border_color = if is_selected { [255, 215, 0, 255] } else { [100, 100, 100, 200] };

        draw_rect_outline(frame, screen_w, sx, sy, slot_size, slot_size, border_color);
    }
}
```

---

## 2. インベントリデータモデル

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemStack {
    pub item_id: u16,
    pub count: u16,
    pub max_stack: u16, // 通常は 999
}

pub struct Inventory {
    /// 10 (ホットバー) + 40 (メインインベントリ) = 計 50 スロット
    pub slots: [Option<ItemStack>; 50],
    pub selected_hotbar_idx: usize,
}

impl Inventory {
    /// アイテムの自動スタック格納
    pub fn add_item(&mut self, item_id: u16, mut count: u16) -> bool {
        // 1. 既存スロットへのスタック加算
        for slot in self.slots.iter_mut().flatten() {
            if slot.item_id == item_id && slot.count < slot.max_stack {
                let space = slot.max_stack - slot.count;
                let add = space.min(count);
                slot.count += add;
                count -= add;
                if count == 0 {
                    return true;
                }
            }
        }

        // 2. 空きスロットへ新規配置
        for slot in self.slots.iter_mut() {
            if slot.is_none() {
                *slot = Some(ItemStack { item_id, count, max_stack: 999 });
                return true;
            }
        }

        false // インベントリ満杯
    }
}
```

---

## 3. テラリア型クラフト判定（近傍ステーション自動アンロック）

プレイヤーの近く（半径 40 ピクセル以内）に「作業台」や「炉」があるかを自動検知し、手持ち素材で作れるレシピを瞬時にアンロックします。

```rust
pub struct Recipe {
    pub result_item: u16,
    pub result_count: u16,
    pub required_materials: Vec<(u16, u16)>, // (item_id, count)
    pub required_station: Option<u16>,       // 作業台ブロックの ID
}

impl Recipe {
    /// レシピが今クラフト可能かを判定
    pub fn can_craft(&self, inv: &Inventory, nearby_stations: &[u16]) -> bool {
        // 1. 必要作業台が近くにあるか
        if let Some(req_st) = self.required_station {
            if !nearby_stations.contains(&req_st) {
                return false;
            }
        }

        // 2. 手持ち素材が足りているか
        for &(mat_id, needed) in &self.required_materials {
            let mut available = 0;
            for slot in inv.slots.iter().flatten() {
                if slot.item_id == mat_id {
                    available += slot.count;
                }
            }
            if available < needed {
                return false;
            }
        }

        true
    }
}
```
