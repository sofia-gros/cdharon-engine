# エンジン全体アーキテクチャ & データフロー (`ai_design_doc/02_ARCHITECTURE_OVERVIEW.md`)

> **本書の目的**  
> Charon Engine を構成する主要サブシステム（コア、シミュレーション、グラフィックス、エンティティ、オーディオ、UI）の全体結合構造と、1 tick（1/60 秒）内の完全なデータフローを定義します。

---

## 1. サブシステム全体結合図

```mermaid
graph TD
    App["Application / OS (winit)"] --> Core["Core Engine (GameLoop, Config, Input)"]
    Core --> World["Simulation World (64x64 Chunks, Grid)"]
    Core --> Entities["Entity Manager (Player, Enemies, Projectiles)"]
    Core --> Audio["Audio Engine (Kira / Rodio)"]
    Core --> UI["UI & Inventory System"]
    
    World --> Physics["Physics Subsystem (Solid, Liquid, Gas, Powder)"]
    World --> Chemistry["Reaction & Thermodynamic Engine"]
    World --> Registry["MaterialRegistry (O(1) Data-Driven)"]
    
    Entities --> Collisions["4x4 Half-Block Collision Solver"]
    Collisions --> World
    
    Render["Rendering Pipeline (WGPU / Pixels)"]
    World --> Render
    Entities --> Render
    UI --> Render
```

---

## 2. 1 Tick（1/60 秒）の完全データフロー・シーケンス

```mermaid
sequenceDiagram
    autonumber
    participant Main as EventLoop
    participant In as InputState
    participant Ent as EntityManager
    participant Sim as SimulationWorld
    participant Ren as RenderPipeline

    Main->>In: OS イベント収集（キー、マウス、ゲームパッド）
    Main->>Ent: プレイヤー & 敵 AI 更新（入力と意思決定）
    Ent->>Sim: 4x4 半ブロック衝突判定 & 水流攪拌（displace_water）
    Main->>Sim: 物理セルオートマトン走査（下→上、左右交互）
    Sim->>Sim: 液体流動・粉体崩落・構造崩壊・化学反応の並列評価
    Main->>Ent: 自由飛翔パーティクル & 投射物（Projectile）移動
    Main->>Ren: フレームバッファへの全レイヤー描画
    Ren->>Ren: ワールドセル描画 -> パーティクル -> エンティティ -> 最前面 UI
    Ren->>Main: GPU へのプレゼンテーション（VSync 同期）
```

---

## 3. モジュール間依存性逆転と疎結合の原則

1. **シミュレーション層の自律性**:
   `SimulationWorld` はレンダリングバックエンド（wgpu 等）や入力デバイスの存在を知らない。純粋なメモリバッファと数値計算のみで完結する。
2. **エンティティ層のインターフェース結合**:
   エンティティ（プレイヤー、敵、弾丸）は `World` の直接の内部配列を直接書き換えず、`world.get_cell()`、`world.paint_brush()`、`world.create_explosion()` などの明確に定義されたパブリック API を経由してワールドへ影響を与える。
3. **データ駆動とレジストリの一方向依存**:
   物理処理コードは `MaterialDef` の静的構造体のみに依存し、ハードコードされたマテリアル ID（`if id == 1`）に依存しない。
