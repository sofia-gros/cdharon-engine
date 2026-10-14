# データ駆動マテリアルレジストリ & テクスチャ設計 (`03_MATERIAL_SYSTEM.md`)

> **本書の目的**  
> ハードコードされた固定長配列を廃止し、**最大 65,535 種類（16-bit）** のマテリアルを外部ファイル（TOML）やビルダー API から安全かつ動的に登録・管理するレジストリと、画像テクスチャのアーキテクチャを定義します。

---

## 1. マテリアルレジストリの完全設計

マテリアル情報は、**連続メモリ（`Vec<MaterialDef>`）** として保持し、マテリアル ID（`u16`）をそのまま配列インデックスとして用いることで、**マテリアル数が 1 万個あっても完全な O(1) 定数時間** で参照します。

```rust
use std::collections::HashMap;

/// セル・オートマトンにおける物質の基本挙動タイプ
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Behavior {
    /// 何もない（自由移動の宛先となる）
    Empty,
    /// 固定された固体（4x4 以上の密集構造を持ち、動かない壁・床）
    Solid,
    /// 液体（下へ高速落下し、左右へ勢いよく水平に広がる）
    Liquid,
    /// 粉体（下へ落下し、斜め下へ崩落して山を作る）
    Powder,
}

/// 物質 1 種類の静的物性データ
#[derive(Clone, Debug)]
pub struct MaterialDef {
    /// 識別名（例: "dirt", "mithril_ore"）
    pub name: String,
    /// 基本挙動タイプ
    pub behavior: Behavior,
    /// 密度（重い物質が軽い物質を押しのけて沈む）
    pub density: u8,
    /// 水平拡散力（液体の広がりやすさ）
    pub dispersion: u8,
    /// 1 tick あたりの最大自由落下速度
    pub max_fall_speed: u8,
    /// 1 tick あたりの横方向流速
    pub flow_speed: u8,
    /// 水流に押し流されるかどうか（流砂・土砂崩れ判定）
    pub can_be_washed_away: bool,
    /// 8x8 ピクセルのテクスチャ（計 64 個の RGBA 色配列）
    pub texture: [[u8; 4]; 64],
}

/// 最大 65,535 種類のマテリアルを高速管理する動的レジストリ
#[derive(Clone, Debug)]
pub struct MaterialRegistry {
    /// マテリアル ID（u16: 0 .. 65,535）をそのままインデックスとして O(1) 配列引き
    materials: Vec<MaterialDef>,
    /// 名前から ID への逆引きインデックス
    name_to_id: HashMap<String, u16>,
}
```

### レジストリへのアクセス設計（安全かつゼロコスト）
```rust
impl MaterialRegistry {
    /// 指定された ID のマテリアル物性を取得（未登録 ID は AIR を安全に返却）
    #[inline]
    pub fn get(&self, id: u8) -> &MaterialDef {
        match &self.table[id as usize] {
            Some(mat) => mat,
            None => self.table[0].as_ref().expect("AIR must be registered"),
        }
    }
}
```

---

## 2. 外部定義ファイル (`materials.toml`) の仕様

コード内に数値をハードコードせず、マテリアル定義を完全に分離します。

```toml
# materials.toml の定義例

[[material]]
id = 0
name = "air"
density = 0
flags = []

[[material]]
id = 1
name = "dirt"
density = 200
flags = ["COLLAPSIBLE"]
texture_coords = [0, 0] # テクスチャアトラス上のタイル座標 (x=0, y=0)

[[material]]
id = 2
name = "stone"
density = 250
flags = ["COLLAPSIBLE"]
texture_coords = [1, 0]

[[material]]
id = 3
name = "water"
density = 100
dispersion = 6
max_fall_speed = 5
flow_speed = 3
flags = ["LIQUID"]
texture_coords = [2, 0]

[[material]]
id = 4
name = "sand"
density = 150
max_fall_speed = 3
flow_speed = 1
flags = ["POWDER", "WASHABLE"]
texture_coords = [3, 0]

[[material]]
id = 5
name = "wood"
density = 180
flags = ["COLLAPSIBLE", "FLAMMABLE"]
texture_coords = [4, 0]
```

---

## 3. 8x8 テクスチャアトラスの画像ロード方式

数万行におよぶ RGBA 数値のベタ書きを排除し、**1 枚の画像ファイル（PNG）** から読み込みます。

```text
[textures/materials.png (128x128 ピクセル)]
  0       8       16      24      32
0 ┌───────┬───────┬───────┬───────┬───────┐
  │ DIRT  │ STONE │ WATER │ SAND  │ WOOD  │ ...
8 ├───────┼───────┼───────┼───────┼───────┤
  │ IRON  │ GOLD  │ LAVA  │ ACID  │ ICE   │ ...
  └───────┴───────┴───────┴───────┴───────┘
  1 タイル = 8x8 ピクセル
```

### ロードロジックの概要
1. 起動時に `image` クレートまたは軽量デコーダで `materials.png` をメモリに読み込む。
2. TOML の `texture_coords = [tile_x, tile_y]` に従い、$(8 \times \text{tile\_x}, 8 \times \text{tile\_y})$ から 8×8 ピクセルの RGBA を切り出して `MaterialDef::texture` に格納する。
3. これにより、グラフィックの変更は **画像を差し替えるだけ** で完了し、プログラムの再コンパイルが不要になる。

---

## 4. マテリアル間化学反応テーブル (`256x256`)

マテリアル同士の接触反応（水＋溶岩＝石、酸＋木＝煙など）は、**256×256 の平坦配列** で表現します。

```rust
/// 反応結果の定義
#[derive(Clone, Copy)]
pub struct Reaction {
    /// 反応後の自身のマテリアル
    pub result_self: u8,
    /// 反応後の相手のマテリアル
    pub result_other: u8,
    /// 反応確率（0 .. 255）
    pub probability: u8,
}

pub struct ReactionTable {
    /// (自マテリアル, 接触相手マテリアル) -> 反応結果
    table: [[Option<Reaction>; 256]; 256],
}

impl ReactionTable {
    /// O(1) で反応結果を即座に判定
    #[inline]
    pub fn get(&self, a: u8, b: u8) -> Option<Reaction> {
        self.table[a as usize][b as usize]
    }
}
```

接触判定時、ハッシュ検索や文字列比較は一切行わず、**2 回の配列インデックス参照（数ナノ秒）** で反応を処理できます。
