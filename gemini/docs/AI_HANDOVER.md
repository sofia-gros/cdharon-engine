# Charon Engine AI 引継ぎ・完全仕様書 (`docs/AI_HANDOVER.md`)

> **本書の対象**: 本プロジェクト（Charon Engine）の開発・保守・機能拡張を引き継ぐすべての AI エージェント。  
> **本書の役割**: プロジェクトの背景、設計哲学、データ構造、物理数理モデル、コードベース構造、および絶対遵守ルールを一切の曖昧さなく完璧に伝達し、次の AI が 1 秒でコンテキストを完全同期して高品質な作業を継続できるようにするためのマスターハンドオーバードキュメントです。

---

## 1. プロジェクト概要 & エグゼクティブ・サマリー

* **プロジェクト名**: `Charon Engine`（カロン・エンジン）
* **ジャンル**: 2D ピクセル・セル・オートマトン物理シミュレーションゲームエンジン（Noita や Terraria のような完全な 2D サンドボックスゲームの実現を目指す）
* **言語 / 環境**: Rust 2021 Edition / Windows (pwsh)
* **主要クレート**: `winit 0.30` (ウィンドウ・イベント), `pixels 0.15` (wgpu レンダリング), `noise 0.9` (地形生成), `serde / serde_yaml` (設定管理)
* **ユーザーのプロファイル**:
  * TypeScript / JavaScript、Go の経験が豊富。
  * **Rust は初学者**（Rust の学習を兼ねてこのプロジェクトを立ち上げている）。
  * したがって、AI からの提案・解説は「TS/Go とのメンタルモデルの比較」「所有権や借用チェッカーを回避する設計」「ビット演算の直感的な説明」を徹底すること。
* **プロジェクトの究極目標**:
  AI を使わずにユーザー個人（人間）が自力で理解・開発・拡張できる、高信頼・高パフォーマンス（60 FPS 安定）なゲームエンジンの完成。

---

## 2. コア・アーキテクチャ & 数理設計仕様

### 2.1 セルの 32-bit（4バイト）固定メモリ設計 (`src/simulation/cell.rs`)
CPU の L1 キャッシュ効率（64 バイト・キャッシュラインにジャスト 16 個格納）を最大化するため、セル構造体は厳密に 4 バイト（32-bit）境界でアライメントされています。

```text
       Byte 0 & 1 (16-bit)               Byte 2            Byte 3
  ┌─────────────────────────────────┬─────────────────┬─────────────────┐
  │         material: u16           │   flags: u8     │    data: u8     │
  │   (物質固有 ID 65,536 種類)     │  (動的状態 8bit)│ (テクスチャ/値) │
  └─────────────────────────────────┴─────────────────┴─────────────────┘
```

* `material: u16`: 物質の種類（0 = AIR, 1 = DIRT, 2 = STONE, 3 = WATER, 4 = SAND 等）。**最大 65,535 種類**のブロックに対応（テラリア規模対応）。
* `flags: u8`: 動的物理状態の直交ビットマップ。
  * `FLAG_CLOCK = 1 << 0` (Bit 0): 今フレーム処理済みトグル（二重移動防止）。
  * `FLAG_FALLING = 1 << 1` (Bit 1): 自由落下・崩落中フラグ（重力落下・滑落）。
  * `FLAG_BURNING = 1 << 2` (Bit 2): 燃焼中フラグ（将来用）。
  * `FLAG_WET = 1 << 3` (Bit 3): 濡れフラグ（将来用）。
* `data: u8`: 8x8 テクスチャサンプリングインデックス（`0..64`）やコンテキストデータ。

### 2.2 直交ビットフラグ化による「組み合わせ爆発」の根絶
* **解決されたアンチパターン**:  
  かつて存在した `LOOSE_DIRT` や `LOOSE_STONE` のような「物質名に状態を混ぜた ID」は完全撤廃済み。
* **現行ロジック**:  
  ブロックが削られて体積不足になると、マテリアル ID はそのまま（`DIRT` のまま）で **`cell.set_falling(true)`** を付与。
  着地して直下に安定した地盤ができると、自動的に **`cell.set_falling(false)`** となり静止固体ブロックへ復帰。

### 2.3 16-bit 動的マテリアルレジストリ (`src/simulation/material.rs`)
* **データ構造**: `MaterialRegistry` は `Vec<MaterialDef>` によるフラットな連続メモリ配列。
* **計算量**: 各セルでの物性参照は `&self.materials[material_id as usize]` の **O(1) 配列引き**。
  マテリアルが 5 個でも 10,000 個でも、参照にかかる CPU サイクルは全く同じ（約 1〜3 ナノ秒）。計算量は絶対に爆発しない。
* **クリエイター向け API**: `MaterialBuilder` による直感的な登録が可能。

### 2.4 4x4 半ブロック密度当たり判定 (`src/entities/player.rs`)
* **足元のつまずき完全解消**:  
  1 ピクセル単位の厳密判定を行わず、接触する 4x4 領域（計 16 セル）内の固体密度が過半数（8 セル以上）ある場合のみ衝突（壁・床）と判定。
  1〜7 ピクセルの散乱粒子は通り抜け可能。
* **落下粒子の除外**:  
  自由落下中（`cell.is_falling() == true`）のセルは足場から除外されるため、頭上からパラパラ落ちてくる土砂でプレイヤーがスタックしない。

### 2.5 8x8 テクスチャ適応的再タイリング
* **静止ブロック**: ワールド座標 `(x % 8, y % 8)` で自動タイリング描画。
* **落下中粒子**: ブロック構造から切り離されているため、セルが記憶しているピクセル固有色（`cell.shade()`）で描画。
* **水（WATER）**: 最表面ピクセルのみ水面ハイライト色（白波）を描画し、内部は適応的グラデーション。

---

## 3. コードベース構成 & ディレクトリマップ

```text
a:/Project/charon-engine/
├── Cargo.toml                  # 依存関係定義
├── conf.yml                    # ウィンドウ・ゲーム設定
├── docs/                       # 公式ドキュメント
│   ├── AI_HANDOVER.md          # 【本書】AI 向け完全引継ぎ書
│   ├── design2.md              # 確定版アーキテクチャ設計書（実在リンク約40件検証済み）
│   └── dir.md                  # ディレクトリ & モジュール責務仕様書
├── human_doc/                  # 人間向け完全開発ガイド（全 16 冊）
│   ├── 00_INDEX.md             # 総合目次 & ロードマップ & 日本語学習リンク集
│   ├── 01_ARCHITECTURE_FLAWS.md# 欠陥分析（組み合わせ爆発）
│   ├── 02_CELL_DATA_LAYOUT.md  # 32bit セル完全仕様
│   ├── 03_MATERIAL_SYSTEM.md   # データ駆動レジストリ仕様
│   ├── 04_PHYSICS_AND_COLLISION.md # 4x4 半ブロック物理 & 崩落数理
│   ├── 05_CHUNK_AND_SLEEP.md   # 64x64 チャンク & スリープ設計
│   ├── 06_IMPLEMENTATION_RECIPES.md # 型安全コードレシピ集
│   ├── 07_RUST_FOR_TS_GO_DEVS.md    # TS/Go 経験者向け Rust 入門
│   ├── 08_REFACTORING_WALKTHROUGH.md# 直交フラグ化 Before/After 手本
│   ├── 09_DYNAMIC_REGISTRY_16BIT.md # 16bit レジストリ完全仕様
│   ├── 10_CARGO_COMMANDS_AND_CLI.md # Cargo 必須コマンド & 6フェーズ手順
│   ├── 11_WORLDGEN_AND_STRUCTURES.md# パーリンワーム洞窟 & プレハブ遺跡
│   ├── 12_MINING_WEAPONS_AND_TOOLS.md# ツルハシ硬度 & 剣スイング円弧 & 弾道
│   ├── 13_MAGIC_EXPLOSIONS_AND_REACTIONS.md # 爆発クレーター & セル化学反応
│   ├── 14_ENEMY_AND_BOSS_AI.md # オートステップ歩行 AI & 多節ワーム & ボス
│   ├── 15_PARTICLES_AND_VFX.md # ゼロアロケーション VFX パーティクル
│   └── 16_UI_INVENTORY_AND_CRAFTING.md # ホットバー & インベントリ & 近傍クラフト
└── src/                        # Rust 実装ソースコード
    ├── main.rs                 # アプリケーション起動点
    ├── core/                   # エンジン基盤
    │   ├── config.rs           # conf.yml 読み込み
    │   ├── input.rs            # キー・マウス入力、座標正規化
    │   └── engine.rs           # winit 0.30 イベントループ、pixels レンダリング
    ├── simulation/             # 2D 物理シミュレーション
    │   ├── cell.rs             # 32bit セル（Cell, FLAG_*）
    │   ├── material.rs         # 16bit レジストリ（MaterialRegistry, MaterialBuilder）
    │   ├── particle.rs         # 水しぶき粒子（SplashParticle）
    │   └── world.rs            # ワールド物理（流体、粉体、構造崩壊、レンダリング）
    └── entities/               # 動的エンティティ
        └── player.rs           # プレイヤー（AABB、4x4 衝突、水中浮力、波立ち）
```

---

## 4. 検証状況 & 健全性ステータス

* **`cargo test`**: **7/7 件すべて合格（0.02 秒、合格率 100%）**
  * `test_cell_size_and_alignment`（4 バイト固定、アライメント検証）
  * `test_clock_bit_toggle`（二重更新防止トグル）
  * `test_falling_flag`（落下フラグのセット・解除）
  * `test_player_initial_state`（プレイヤー初期化）
  * `test_default_registry`（基本 5 マテリアル）
  * `test_builder_and_custom_registration`（独自ブロック登録）
  * `test_large_scale_registration_perf`（**10,000 個のマテリアル動的登録 & O(1) 参照**）
* **`cargo build`**: **警告 0 件、エラー 0 件**。完全なクリーンビルド状態。

---

## 5. 次の AI が絶対に守るべき鉄則（Strict Rules）

1. **DOC コメントは必ず日本語で書くこと**:
   コード内のすべての構造体、関数、定数には `///` による詳細な日本語 Doc コメントを付与すること。
2. **ファイルの読み書きにシェルコマンドを使用しないこと**:
   PowerShell や bash、Node -e 等のシェルコマンドによるファイル読み書きを厳禁する。ネイティブツール（`view_file`, `write_to_file`, `replace_file_content` 等）を必ず使用すること。
3. **目的のないファイル探索（"Exploring files"）や pagination reading の禁止**:
   原因調査や確認時はファイルを細切れに何回も開かず、エラーログやシンボル名でピンポイント確認を行うこと。
4. **実在確認済みリンクのみを使用すること**:
   ドキュメントに記載する URL は、実際にアクセス可能な実在確認済みリンク（The Book 日本語版、Tour of Rust、Wikipedia 等）のみを使用し、推測や存在しないリンクを排除すること。
5. **クリック可能なマークダウンリンクを提供すること**:
   ファイルやシンボルを言及する際は、`[cell.rs](file:///a:/Project/charon-engine/src/simulation/cell.rs)` や [`FLAG_FALLING`](file:///a:/Project/charon-engine/src/simulation/cell.rs#L26-L27) のように、`file://` スキームを用いた github スタイルのマークダウンリンクを必ず付与すること。

---

## 6. 次のフェーズの推奨実装タスク（優先順位順）

次にユーザーから機能追加を求められた場合の、実装の優先順位とロードマップです：

1. **ツルハシ採掘 & アイテムドロップ化**:
   [12_MINING_WEAPONS_AND_TOOLS.md](file:///a:/Project/charon-engine/human_doc/12_MINING_WEAPONS_AND_TOOLS.md) を参照し、右クリック掘削時に壊れたブロックを浮遊アイテムエンティティとしてドロップさせる。
2. **ホットバー & インベントリ UI**:
   [16_UI_INVENTORY_AND_CRAFTING.md](file:///a:/Project/charon-engine/human_doc/16_UI_INVENTORY_AND_CRAFTING.md) を参照し、画面上部に 10 スロットのホットバーを描画し、拾ったアイテムを格納・選択可能にする。
3. **洞窟生成（パーリン・ワーム）**:
   [11_WORLDGEN_AND_STRUCTURES.md](file:///a:/Project/charon-engine/human_doc/11_WORLDGEN_AND_STRUCTURES.md) を参照し、地下に蛇行する洞窟網を自動生成する。
4. **爆発 & セル化学反応**:
   [13_MAGIC_EXPLOSIONS_AND_REACTIONS.md](file:///a:/Project/charon-engine/human_doc/13_MAGIC_EXPLOSIONS_AND_REACTIONS.md) を参照し、手榴弾や火球魔法によるクレーター形成と水＋溶岩＝黒曜石の反応を組み込む。
5. **敵キャラクター（スライム & ゾンビ）**:
   [14_ENEMY_AND_BOSS_AI.md](file:///a:/Project/charon-engine/human_doc/14_ENEMY_AND_BOSS_AI.md) を参照し、ピクセル段差を登る歩行敵やジャンプするスライムを追加する。
