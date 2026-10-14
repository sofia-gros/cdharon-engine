# 空間インデックス & ゼロコスト・スリープ設計 (`ai_design_doc/06_SPATIAL_INDEXING_AND_SLEEP.md`)

> **本書の目的**  
> 画面外や地下の静止した岩盤・土ブロックの計算を完全にスキップし、動いている領域のみをピンポイントで走査することで、CPU 負荷を 90% 以上削減するゼロコスト・スリープシステムを定義します。

---

## 1. 静止セルのスリープ（Dirty Rectangle & Bitmask）

画面内のピクセルのうち、実際に動いているのは「流れる水」「落ちる砂」「崩落中の土」だけであり、90% 以上の土や岩盤は完全に静止しています。全セルを無条件走査するのは最大の無駄です。

```text
┌──────────────────────────────────────────────┐
│  Chunk (64x64)                               │
│  [静止した岩盤ブロック]  (計算スキップ！)     │
│                                              │
│        ┌──────────────────┐                  │
│        │ [動く水と砂]     │  <- Dirty Rect   │
│        │ (ここだけ更新！) │                  │
│        └──────────────────┘                  │
│                                              │
└──────────────────────────────────────────────┘
```

---

## 2. スリープ判定アルゴリズム

### ① 走査範囲の動的縮小（Dirty Rect）
各 tick でセルが移動・変化した際、そのセルのローカル座標 `(lx, ly)` を含めるようにチャンクの `Dirty Rect` を拡張します。

```rust
impl Chunk {
    #[inline]
    pub fn mark_dirty(&mut self, lx: u8, ly: u8) {
        self.min_active_x = self.min_active_x.min(lx.saturating_sub(1));
        self.max_active_x = self.max_active_x.max((lx + 1).min(63));
        self.min_active_y = self.min_active_y.min(ly.saturating_sub(1));
        self.max_active_y = self.max_active_y.max((ly + 1).min(63));
        self.is_sleeping = false;
        self.sleep_timer = 0;
    }
}
```

### ② チャンク・スリープへの移行
* あるチャンク内で 1 回もセルの移動・変化が起きなかった tick が **3 フレーム連続** した場合、そのチャンクは `is_sleeping = true` に設定されます。
* `is_sleeping == true` のチャンクは、ループ内で `continue` され、1 ナノ秒も CPU サイクルを消費しません。

### ③ スリープ解除（Wake-Up）の伝播
* プレイヤーが採掘した時、爆発が起きた時、隣のチャンクから水が流れ込んできた時、対象座標を含むチャンクおよび隣接チャンクの `is_sleeping` を即座に `false` にして起床させます。

---

## 3. アクティブチャンクのビットマスク管理

CPU の 64-bit 整数レジスタ（`u64`）1 つで、最大 64 個のチャンクのアクティブ状態を 1 サイクルで判定します。

```rust
pub struct ChunkBitmask {
    /// 1 ビットが 1 チャンクのアクティブ状態を表す
    pub active_masks: [u64; 16], // 最大 1024 チャンクを管理
}

impl ChunkBitmask {
    #[inline]
    pub fn is_chunk_active(&self, chunk_id: usize) -> bool {
        let word = chunk_id / 64;
        let bit = chunk_id % 64;
        (self.active_masks[word] & (1 << bit)) != 0
    }
}
```
これにより、休眠状態の広大な地下世界が存在しても、フレームレートは 144 FPS に張り付きます。
