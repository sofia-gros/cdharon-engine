# Charon Engine

## 概要
Rustで作成したゲームエンジンです。
2D Falling Sand（セル・オートマトン）物理サンドボックスエンジンを目指しています。

```
## ディレクトリ構成
charon-engine/
├── Cargo.toml
├── conf.yml            # 設定ファイル
└── src/
    ├── main.rs
    ├── core/
    │   ├── mod.rs      # coreモジュールの定義 エンジンの心臓部。ウィンドウ管理、描画パイプライン、入力処理
    │   └── engine.rs   # CharonEngineのApplicationHandler実装とウィンドウ管理
    └── simulation/
        ├── mod.rs      # simulationモジュールの定義 サンドボックス物理演算。セル・オートマトン、チャンク管理
        └── world.rs    # 地形生成とノイズロジック（旧generate_terrainなど）
    ├── entities/       # 動的なキャラクター類。ECSで管理する場合はここ
    │   ├── player.rs
    │   └── enemies.rs
    └── items/          # 武器や防具のデータとロジック
        ├── mod.rs
        ├── weapon.rs
        └── armor.rs
```
