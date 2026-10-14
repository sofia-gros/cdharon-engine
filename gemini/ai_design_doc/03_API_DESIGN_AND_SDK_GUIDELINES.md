# クリエイター向け SDK & API 設計指針 (`ai_design_doc/03_API_DESIGN_AND_SDK_GUIDELINES.md`)

> **本書の目的**  
> サードパーティの開発者や MOD 作成者が、Charon Engine の内部コードを一切改変することなく、新しいブロック、ツール、魔法、敵、UI をシームレスに組み込める商用 SDK / API の公開仕様を定義します。

---

## 1. SDK API 設計の 3 大原則

1. **Rust イディオマティックな型安全性（Type Safety）**:
   ビルダーパターン（`MaterialBuilder`, `EnemyBuilder`）を活用し、不正な物性パラメータ（負の密度、極端な速度）をコンパイル時または初期化時に弾く。
2. **C-ABI / FFI 互換レイヤー（将来の C# / C++ 統合）**:
   主要なワールド操作関数は `#[no_mangle] pub extern "C"` としてエクスポート可能なフラットなデータ構造でラップできるように設計する。
3. **データ駆動優先（Data-Driven First）**:
   「コードを書かずに TOML と PNG だけで 90% の要素を追加できる」ことを最優先とする。

---

## 2. クリエイター向け公開 API シグネチャ仕様

### ① マテリアル拡張 API
```rust
// クリエイターがゲーム初期化時、または MOD ロード時に呼び出す
let mut engine = CharonEngine::new(config)?;

let custom_slime_block = engine.registry_mut().register_builder(
    MaterialBuilder::new("bouncy_slime", [100, 255, 100, 220])
        .behavior(Behavior::Solid)
        .density(120)
        .can_be_washed_away(true)
        .bounce_coefficient(0.85) // 反発係数
)?;
```

### ② ワールド相互作用 API
```rust
impl World {
    /// 矩形領域または円形ブラシでのマテリアル配置
    pub fn fill_circle(&mut self, cx: i32, cy: i32, radius: i32, material: u16);
    
    /// 指定された威力と半径で爆発を起こす（地形破壊 + 衝撃波）
    pub fn explode(&mut self, cx: i32, cy: i32, radius: i32, power: f32);
    
    /// 水や流体を指定ベクトルで押し退ける（移動波立ち）
    pub fn displace_fluid(&mut self, cx: i32, cy: i32, push_vector: [f32; 2]);
    
    /// 2 点間の直線レイキャスト（ピクセル壁との衝突判定）
    pub fn raycast(&self, start: [f32; 2], end: [f32; 2]) -> Option<RayHit>;
}
```

### ③ イベントフック機構（Modding Event Hooks）
```rust
pub trait EnginePlugin: Send + Sync {
    fn on_init(&mut self, engine: &mut CharonEngine) {}
    fn on_pre_physics_tick(&mut self, world: &mut World) {}
    fn on_post_physics_tick(&mut self, world: &mut World) {}
    fn on_cell_reaction(&mut self, cell_a: &Cell, cell_b: &Cell, coord: [i32; 2]) {}
}
```
クリエイターは `engine.add_plugin(MyCustomMod::new())` を呼び出すだけで、ライフサイクルイベントに介入できます。
