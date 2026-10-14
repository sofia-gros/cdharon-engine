# Charon Engine 完全商用化設計書 マスターインデックス (`/ai_design_doc/00_MASTER_INDEX.md`)

> **本書の対象**: 本ゲームエンジンを有料商用製品（Commercial Product）として完成・保守・拡張するすべての AI 開発エージェント。  
> **本書の役割**: 妥協や推測を一切排除し、Noita × Terraria × Celeste の美点を融合した完全な 2D サンドボックス物理ゲームエンジンを商業販売可能な水準で構築するための、全 42 冊からなる完全設計書体系の総合目次・ロードマップです。

---

## 1. 商用化設計書 全 42 冊の体系一覧

```text
ai_design_doc/
├── 00_MASTER_INDEX.md                    # 【本書】商用設計書マスターインデックス
│
├── [Part 1: 商業化 & エンジン哲学]
│   ├── 01_PRODUCT_VISION_AND_REQUIREMENTS.md   # 商用要件定義、目標性能、コアバリュー
│   ├── 02_ARCHITECTURE_OVERVIEW.md            # 全体アーキテクチャ、データフロー、モジュール結合度
│   └── 03_API_DESIGN_AND_SDK_GUIDELINES.md    # クリエイター向け SDK 設計、C-ABI/FFI 境界
│
├── [Part 2: メモリ・キャッシュ & チャンク基盤]
│   ├── 04_CELL_MEMORY_LAYOUT.md               # 32-bit セル完全仕様、アライメント、ビットパック
│   ├── 05_CHUNK_SYSTEM_AND_L1D_OPTIMIZATION.md# 64x64 チャンク、16KiB L1D 局所性、境界パディング
│   ├── 06_SPATIAL_INDEXING_AND_SLEEP.md       # ゼロコスト・スリープ、Dirty 矩形、アクティブマスク
│   └── 07_MEMORY_ALLOCATOR_AND_POOLS.md       # ゼロアロケーション、アリーナ、オブジェクトプール
│
├── [Part 3: 物理シミュレーション & セルオートマトン]
│   ├── 08_SOLID_PHYSICS_AND_COLLAPSE.md       # 固体構造強度、4x4 半ブロック密度崩落、自立限界
│   ├── 09_LIQUID_SIMULATION_AND_PRESSURE.md   # 流体多段落下、水平スライド、圧力伝播、浮力
│   ├── 10_POWDER_PHYSICS_AND_EROSION.md       # 粉体安息角、斜面滑落、水流侵食・運動量ドラッグ
│   ├── 11_GAS_AND_AERODYNAMICS.md             # 気体浮力、気圧拡散、寿命減衰、換気システム
│   ├── 12_THERMODYNAMICS_AND_REACTIONS.md     # 温度伝播、相転移、燃焼、セル化学反応マトリクス
│   └── 13_EXPLOSION_AND_SHOCKWAVE_DYNAMICS.md # 3ゾーン爆発数理、衝撃波、クレーター形成
│
├── [Part 4: レンダリング & グラフィックス]
│   ├── 14_WGPU_RENDERING_PIPELINE.md          # WGPU レンダラー、フレームバッファ、ダブルバッファ
│   ├── 15_TEXTURE_ATLAS_AND_TILING.md         # 8x8 テクスチャアトラス、適応的再タイリング、法線
│   ├── 16_DYNAMIC_LIGHTING_AND_SHADOWS.md     # 2D 光線追跡（Raycasting）、影付き照明、発光セル
│   ├── 17_POST_PROCESSING_AND_SHADERS.md      # ブルーム、水中歪み、熱気、CRT フィルター
│   └── 18_CAMERA_AND_PIXEL_PERFECT.md         # ピクセルパーフェクト追従、サブピクセル補間、震動
│
├── [Part 5: データ駆動マテリアル & Modding]
│   ├── 19_MATERIAL_REGISTRY_16BIT.md          # 16-bit ID（65,535 種）レジストリ、O(1) 配列引き
│   ├── 20_TOML_DATA_SCHEMA.md                 # 外部 TOML/JSON スキーマ、バリデーション、ホットリロード
│   ├── 21_MODDING_API_AND_SCRIPTING.md        # Rhai/WASM スクリプト埋め込み、Mod フック機構
│   └── 22_ASSET_PIPELINE.md                   # アセット自動パッキング、最適化、暗号化パッケージ
│
├── [Part 6: エンティティ・キャラクター物理]
│   ├── 23_AABB_CHARACTER_CONTROLLER.md        # Celeste 方式整数 AABB 物理、サブピクセル移動
│   ├── 24_HALF_BLOCK_COLLISION_MATH.md        # 4x4 半ブロック密度判定数理、オートステップ段差登り
│   ├── 25_PLAYER_STATE_MACHINE.md             # 階層型ステートマシン（走行、ジャンプ、水泳、ダッシュ）
│   └── 26_ANIMATION_AND_SPRITES.md            # スプライトシート、フレーム補間、手足 IK 連動
│
├── [Part 7: 武器・採掘ツール & 戦闘システム]
│   ├── 27_TOOL_MINING_SYSTEM.md               # ツルハシ硬度力学（パワー vs 密度）、アイテムドロップ
│   ├── 28_MELEE_COMBAT_AND_SWING_ARCS.md      # 近接武器スイング円弧判定、慣性、ヒットストップ
│   ├── 29_RANGED_PROJECTILES_AND_BALLISTICS.md# ブレゼンハム弾道、放物線、跳弾、ピクセル突き刺さり
│   └── 30_MAGIC_SYSTEM_AND_MODIFIERS.md       # 魔法投射物、マナ消費、投射物修飾子（Modifiers）
│
├── [Part 8: 敵・ボス AI & パスファインディング]
│   ├── 31_PIXEL_GRID_PATHFINDING.md           # 破壊可能グリッドにおける階層型 A* / JPS
│   ├── 32_ENEMY_AI_BEHAVIOR_TREES.md          # ビヘイビアツリー（BT）、索敵、視界（LoS）判定
│   ├── 33_ENEMY_TYPES_AND_LOCOMOTION.md       # 歩行敵、スライム跳躍、飛行旋回、IK 多節ワーム
│   └── 34_BOSS_FIGHT_FRAMEWORK.md             # ボス戦フレームワーク、多段階フェーズ、地形破壊突進
│
├── [Part 9: プロシージャル世界生成 & 建造物]
│   ├── 35_VERTICAL_DEPTH_AND_BIOMES.md        # 5 段階垂直深度レイヤー、温度・湿度バイオーム合成
│   ├── 36_PERLIN_WORMS_AND_CAVES.md           # パーリンワーム洞窟掘削、セルラー空洞、連結性保証
│   ├── 37_PREFAB_STRUCTURE_STAMPING.md        # プレハブ建造物スタンプ（ダンジョン、遺跡、宝箱部屋）
│   └── 38_ORE_VEINS_AND_RESOURCES.md          # 鉱脈クラスタリング、水脈・溶岩湖、樹木プロシージャル
│
└── [Part 10: UI・インベントリ・オーディオ・商用リリース]
    ├── 39_PIXEL_PERFECT_UI_SYSTEM.md          # 最前面 UI 描画、ホットバー、HP/マナバー、カーソル
    ├── 40_INVENTORY_AND_CRAFTING.md           # 50 枠インベントリ、自動スタック、近傍作業台クラフト
    ├── 41_AUDIO_ENGINE_AND_ACOUSTICS.md       # 2D 空間音響、水中ローパス、ピッチ揺らぎ、BGM
    └── 42_SAVE_LOAD_AND_COMMERCIAL_RELEASE.md # RLE 圧縮セーブ、非同期ストリーム、Steam 統合
```

---

## 2. 商用エンジンとしての 5 大品質基準（Quality Criteria）

1. **ゼロ・ガベージコレクション & 確定論的メモリ**:
   毎フレームのヒープアロケーションを 0 バイトに抑え、L1/L2 キャッシュライン適合構造により GC レスの安定した 60 FPS / 144 FPS を保証する。
2. **完全な O(1) スケーラビリティ**:
   マテリアル数が 1 万個、エンティティ数が数千体になっても、空間分割とテーブル引きによりフレームタイムを 16.6ms 以内に収める。
3. **データ駆動モッディング性**:
   エンジン再コンパイルなしに TOML、PNG、スクリプトを追加するだけで完全な新要素を拡張可能とする。
4. **ミリ秒未満の物理・レンダリング同期**:
   物理演算とグラフィックスの完全な決定論的分離。
5. **人間・AI 相互開発親和性**:
   詳細な日本語 Doc コメント、型安全性、厳密な単体テスト完備。
