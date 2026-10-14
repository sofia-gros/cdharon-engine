# 直交ビットフラグ化リファクタリング完全解説書 (`08_REFACTORING_WALKTHROUGH.md`)

> **本書の目的**  
> `LOOSE_DIRT` や `LOOSE_STONE` のようにマテリアル ID で動的状態を表現していた「組み合わせ爆発の不備」を解消し、  
> `Cell.flags` のビットマップ（`FLAG_FALLING`）による「直交設計」へと刷新した実際のリファクタリング手順を、  
> **変更前（Before）と変更後（After）の完全対比** および **なぜこのコードにしたのか（設計理由）** と共に詳しく解説します。

---

## 1. なぜこのリファクタリングが必要だったのか？（根本課題）

### 変更前の重大な欠陥
変更前のコードでは、土や石が破壊されて体積が減ったとき、以下のように別マテリアルへ変換していました：
```
DIRT (土ブロック)  ──(体積不足)──> LOOSE_DIRT  (崩落土粒子)
STONE (石ブロック) ──(体積不足)──> LOOSE_STONE (崩落小石粒子)
```

一見動いているように見えますが、これはプログラミング設計において**「組み合わせ爆発（Combinatorial Explosion）」**と呼ばれる致命的なアンチパターンです。

1. **マテリアル ID（`u8` = 最大 256 個）の急激な枯渇**:  
   今後「木（WOOD）」「レンガ（BRICK）」「氷（ICE）」を追加するたびに、`LOOSE_WOOD`, `LOOSE_BRICK`, `LOOSE_ICE` が必要になります。  
   さらに「燃えている（BURNING）」「濡れている（WET）」などの状態が加わると、`DIRT_BURNING`, `DIRT_WET`, `DIRT_BURNING_FALLING`... と天文学的に ID が増え、256 個の枠は瞬時に枯渇します。
2. **`match` / `if` 分岐コードの爆発的増殖**:  
   ワールドの崩落処理や描画処理の至る所で `if mat == DIRT ... else if mat == STONE ...` という個別判定が必要になり、新しい物質を 1 つ追加するたびにエンジン中の全ファイルを修正しなければならなくなっていました。
3. **不可逆な状態変化**:  
   `LOOSE_DIRT` という別物質にしてしまったため、落ちて床に着地しても「元の土ブロック」に戻すことが極めて困難でした。

---

## 2. 変更前（Before）と変更後（After）のコード完全対比

### ① セルデータ構造 (`src/simulation/cell.rs`)

#### 【Before（変更前）】
フラグ用の定数がなく、マジックナンバー `1` でクロックビットを直接操作していました。
```rust
// 変更前: 落下状態を表すビットフラグが存在しなかった
impl Cell {
    pub fn is_updated(&self, clock_bit: u8) -> bool {
        (self.flags & 1) == clock_bit
    }
    pub fn set_updated(&mut self, clock_bit: u8) {
        if clock_bit == 1 {
            self.flags |= 1;
        } else {
            self.flags &= !1;
        }
    }
}
```

#### 【After（変更後）】
`flags`（8bit）の各桁を独立したスイッチとして定義し、型安全なメソッドを提供しました。
```rust
// 変更後: 直交ビット定数と専用メソッドを定義
pub const FLAG_CLOCK: u8   = 1 << 0; // Bit 0: 0b00000001 (更新クロック)
pub const FLAG_FALLING: u8 = 1 << 1; // Bit 1: 0b00000010 (自由落下・崩落中)
pub const FLAG_BURNING: u8 = 1 << 2; // Bit 2: 0b00000100 (燃焼中・将来用)
pub const FLAG_WET: u8     = 1 << 3; // Bit 3: 0b00001000 (濡れ・将来用)

impl Cell {
    #[inline]
    pub fn is_falling(&self) -> bool {
        (self.flags & FLAG_FALLING) != 0
    }

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

### ② マテリアル定義 & レンダリング (`src/simulation/material.rs`)

#### 【Before（変更前）】
架空の粒子マテリアル `LOOSE_DIRT` (5) と `LOOSE_STONE` (6) を定義し、描画関数でも個別分岐していました。
```rust
// 変更前: マテリアル ID が状態を含んでいた
pub const AIR: u8 = 0;
pub const DIRT: u8 = 1;
pub const STONE: u8 = 2;
pub const WATER: u8 = 3;
pub const SAND: u8 = 4;
pub const LOOSE_DIRT: u8 = 5;  // ❌ 状態の混入
pub const LOOSE_STONE: u8 = 6; // ❌ 状態の混入

pub static MATERIALS: [MaterialDef; 7] = [ /* ... 7 要素 ... */ ];

// 描画関数でもマテリアル ID で個別分岐
if material_id == LOOSE_DIRT {
    return get_material(DIRT).texture[stored_shade as usize % 64];
}
if material_id == LOOSE_STONE {
    return get_material(STONE).texture[stored_shade as usize % 64];
}
```

#### 【After（変更後）】
`LOOSE_*` を完全に撤廃。純粋な物質名 5 種に集約し、描画関数は `is_falling` フラグで統一処理しました。
```rust
// 変更後: 純粋な 5 種類の物質名のみ！
pub const AIR: u8 = 0;
pub const DIRT: u8 = 1;
pub const STONE: u8 = 2;
pub const WATER: u8 = 3;
pub const SAND: u8 = 4;

pub static MATERIALS: [MaterialDef; 5] = [
    // DIRT に「崩落時に水流に流される性質」や「落下速度」を付与
    MaterialDef {
        name: "Dirt",
        behavior: Behavior::Solid,
        density: 200,
        max_fall_speed: 3,
        flow_speed: 1,
        can_be_washed_away: true, // 崩落して粒子化した際は水流に流される
        /* ... テクスチャ ... */
    },
    // ...
];

// 描画関数: 物質の種類に関わらず「落下中かどうか」で一括判定
pub fn get_adaptive_color(
    material_id: u8,
    world_x: u32,
    world_y: u32,
    is_surface: bool,
    stored_shade: u8,
    is_falling: bool, // ★フラグを受け取る
) -> [u8; 4] {
    let mat = get_material(material_id);
    // ...
    // 自由落下中のセル、または粉体（砂）は、ブロックタイリングせず粒子固有色で描画
    if is_falling || mat.behavior == Behavior::Powder {
        let idx = (stored_shade as usize) % 64;
        return mat.texture[idx];
    }

    // 静止した固体はワールド座標で綺麗にタイリング
    if mat.behavior == Behavior::Solid {
        let tx = (world_x % 8) as usize;
        let ty = (world_y % 8) as usize;
        return mat.texture[ty * 8 + tx];
    }
    // ...
}
```

---

### ③ 物理崩落・更新処理 (`src/simulation/world.rs`)

#### 【Before（変更前）】
崩壊時にマテリアルを調べて、対応する `LOOSE_*` を代入するハードコード分岐がありました。
```rust
// 変更前: マテリアルごとに分岐が必要だった
if total_solid > 0 && total_solid < 8 {
    for (idx, mat) in solid_indices {
        let shade = self.cells[idx].shade;
        if mat == DIRT {
            self.cells[idx] = Cell::with_shade(LOOSE_DIRT, shade);
        } else if mat == STONE {
            self.cells[idx] = Cell::with_shade(LOOSE_STONE, shade);
        }
    }
}
```

#### 【After（変更後）】
マテリアル ID を一切気にせず、**`cell.set_falling(true)` を呼ぶだけ** で完結します。
```rust
// 変更後: どんなマテリアルであっても 1 行で汎用的に崩落！
if total_solid > 0 && total_solid < 8 {
    for idx in solid_indices {
        self.cells[idx].set_falling(true);
    }
}
```

さらに、**「地面に着地したら安定したブロックに戻る」** という物理復帰処理が自然に実現できました：
```rust
// 落下処理 update_powder_or_falling の末尾:
// 落下中の固体（土・石）が地面に着地し、直下に安定支持がある場合
if curr_cell.is_falling() {
    let mat_def = get_material(curr_cell.material);
    if mat_def.behavior == Behavior::Solid {
        if y + 1 >= self.height || self.is_solid_neighbor(x, y + 1) {
            // ★フラグを降ろすだけで、元のブロック構造に復帰！
            curr_cell.set_falling(false);
        }
    }
}
self.cells[curr_idx] = curr_cell;
```

---

### ④ プレイヤー当たり判定 (`src/entities/player.rs`)

#### 【Before（変更前）】
落下中のセルも静止セルも区別なく、単に `Solid` か `Powder` かだけで判定していました。

#### 【After（変更後）】
自由落下・崩落中のセル（`cell.is_falling()`）は浮遊している粒子であるため、プレイヤーの足場・障害物から除外しました。
```rust
let cell = world.get_cell(cx, cy);
let behavior = get_material(cell.material).behavior;

// 静止している固体ブロック、または安定した砂のみを壁・床としてカウント
// 自由落下・崩落中のセル（is_falling = true）は通り抜ける
let is_obstacle = (behavior == Behavior::Solid && !cell.is_falling()) || behavior == Behavior::Powder;
if is_obstacle {
    solid_count += 1;
}
```
これにより、頭上からパラパラ落ちてくる崩落土砂でプレイヤーが引っかかる（スタックする）問題が完全に解消されました。

---

## 3. なぜこの設計にしたのか？ 4つの決定的メリット

### 1. マテリアル ID 枠（256個）が一切消費されない
「土」「石」という物質の種類（ID）と、「落下中」「燃焼中」「水没中」という動的状態（Flags）が直交しているため、状態がどれだけ増えてもマテリアル枠は 1 つも減りません。

### 2. 開放閉鎖原則（OCP: Open-Closed Principle）の達成
今後新しいブロック（例: `WOOD`（木材）、`GLASS`（ガラス））を追加する際、`world.rs` の崩落処理や落下処理コードを **1 行も書き換える必要がありません**。  
`Behavior::Solid` として定義するだけで、エンジンが自動的に「4x4 未満なら崩落」「孤立したら落下」「着地したら安定」を処理します。

### 3. 可逆的な状態遷移（着地でブロックに戻る）
`LOOSE_DIRT` 方式では「土→崩落土」への一方通行になりがちでしたが、ビットフラグ方式なら「`falling = true` で落下」「着地したら `falling = false` で再び土ブロック」という、現実世界の土砂崩れや堆積と同じ可逆物理が極めてシンプルに実現できます。

### 4. 高速な L1 キャッシュ適合（32bit 固定の維持）
ビットフラグ方式を採用したことで、`Cell` 構造体は厳密に 4 バイト（32-bit）のまま維持されています。
メモリ帯域の浪費がなく、1 つの CPU キャッシュライン（64 バイト）に 16 個のセルが隙間なく並びます。

---

## 4. 人間が新しいマテリアルを追加する際の実践手順（3ステップ）

この直交設計のおかげで、人間が新しいマテリアル（例えば `WOOD` = 木材）を追加する手順は極めてシンプルです。

### ステップ 1: `material.rs` に ID 定数を追加
```rust
pub const AIR: u8   = 0;
pub const DIRT: u8  = 1;
pub const STONE: u8 = 2;
pub const WATER: u8 = 3;
pub const SAND: u8  = 4;
pub const WOOD: u8  = 5; // ★新しく追加
```

### ステップ 2: `MATERIALS` 配列に物性と 8x8 テクスチャを追加
```rust
pub static MATERIALS: [MaterialDef; 6] = [ // 要素数を 6 に増やす
    // ...
    // WOOD (5)
    MaterialDef {
        name: "Wood",
        behavior: Behavior::Solid,
        density: 180,
        dispersion: 0,
        max_fall_speed: 2,
        flow_speed: 1,
        can_be_washed_away: true, // 木片は水流に流される
        texture: [
            // 8x8 ピクセルの木目テクスチャカラー RGBA
            [160, 110, 60, 255]; 64
        ],
    },
];
```

### ステップ 3: 完了！
物理シミュレーション（`world.rs`）や当たり判定（`player.rs`）は修正不要です。  
木材ブロックを削れば自動的に 4x4 体積不足で崩落し、水流があれば流され、着地すれば再び木片として堆積します。
