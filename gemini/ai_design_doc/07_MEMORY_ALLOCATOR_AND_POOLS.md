# ゼロアロケーション・メモリプール設計 (`ai_design_doc/07_MEMORY_ALLOCATOR_AND_POOLS.md`)

> **本書の目的**  
> ガベージコレクションのない Rust においても、実行中の頻繁なメモリアロケーション（ヒープ確保）は断片化とフレームスパイク（カクつき）を招きます。毎フレームのアロケーションを厳密にゼロにする商用アリーナ & プール設計を定義します。

---

## 1. ゼロアロケーション方針（Zero-Allocation in Game Loop）

商用エンジンにおいて、**「ゲームプレイ中の `malloc` / `free` は禁止」** を基本原則とします。

1. **起動時にすべてのバッファを事前確保（Pre-allocation）**:
   ワールドグリッド、パーティクルプール、エンティティスロット、UI レンダリングバッファは初期化時に一括確保する。
2. **一時的な計算はスタックメモリまたはフレームアリーナで完結**:
   中間バッファが必要な場合は、1 フレームごとにポインタを先頭に戻すだけの `FrameArena` を使用する。

---

## 2. フレームアリーナ・アロケータ（Frame Arena）

```rust
pub struct FrameArena {
    buffer: Vec<u8>,
    offset: usize,
}

impl FrameArena {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            buffer: vec![0; capacity],
            offset: 0,
        }
    }

    /// 高速メモリ切り出し（O(1)、ポインタ加算のみ）
    pub fn alloc<T: Copy>(&mut self, count: usize) -> Option<&mut [T]> {
        let size = std::mem::size_of::<T>() * count;
        let align = std::mem::align_of::<T>();
        let aligned_offset = (self.offset + align - 1) & !(align - 1);

        if aligned_offset + size > self.buffer.len() {
            return None; // アリーナ容量超過
        }

        self.offset = aligned_offset + size;
        let ptr = unsafe { self.buffer.as_mut_ptr().add(aligned_offset) as *mut T };
        Some(unsafe { std::slice::from_raw_parts_mut(ptr, count) })
    }

    /// フレーム終了時のゼロコスト全解放
    #[inline]
    pub fn reset(&mut self) {
        self.offset = 0; // ポインタを戻すだけ（解放処理は走らない）
    }
}
```

---

## 3. エンティティ & パーティクル・オブジェクトプール

エンティティやパーティクルの生成・破棄は、`Vec` の追加・削除ではなく、**「世代インデックス付きスロットマップ（SlotMap）」** で管理します。

```rust
pub struct Slot<T> {
    pub item: T,
    pub generation: u32,
    pub is_alive: bool,
}

pub struct ObjectPool<T, const N: usize> {
    pub slots: [Slot<T>; N],
    free_indices: Vec<usize>,
}
```
* 生死フラグと世代番号（Generation）を持たせることで、ポインタのダングリング参照を完全に防止しつつ、事前確保された固定配列内で O(1) スポーン・O(1) デスポーンを実現します。
