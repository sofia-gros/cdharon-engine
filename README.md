# Charon Engine

Rust製の、汎用2Dサンドボックスゲームエンジンです。

Fabric APIのような機能へのアクセスと拡張性、Phaserのようなシーン・入力・資産・実行管理を、2Dサンドボックスのシミュレーションと統合します。クリエーターがRustでゲームのルール、素材、キャラクター、操作、進行を定義し、エンジンが制作と実行の基盤を提供します。

**現在はv1.0.0へ向けて開発中です。** E04「世界と保存基盤」までの新基盤を実装・検証しています。商用v1.0.0の完成や、Terraria・Noitaの完全クローン制作能力を実証済みという意味ではありません。

## 目指すもの

- ブロックとセル、粉体・液体・気体、熱と反応、破壊可能な回転物体を組み合わせられる世界
- シーン、入力、資産、描画、音響、UIを統合したゲーム制作API
- AI・経路探索、空間検索、画面外の活動制御、保存・リプレイ・通信
- 標準処理では表せない挙動も、独自データと公開API、置換可能な処理から構築できる拡張性

これらは**v1.0.0の設計目標**です。特定ゲームのルールをエンジンへ固定せず、TerrariaとNoitaの完全クローンを公開APIで作れるかどうかを受け入れ基準の一つとします。詳細は[製品設計](new_docs/README.md)と[完全クローンの受け入れ基準](new_docs/26_FULL_CLONE_ACCEPTANCE.md)を参照してください。

## 現在の実装と検証

| 工程 | 実装した範囲 | 根拠 |
| --- | --- | --- |
| E01：Coreとデータ | 型付きID、schema、component、独自状態、格子channel、codec、乱数、取引 | [実装・検証記録](new_docs/evidence/E01/README.md) |
| E02：RuntimeとScene | 起動、plugin、Scene遷移、scope、入力、固定更新、System順序、時計、headless | [実装・検証記録](new_docs/evidence/E02/README.md) |
| E03：資産と基本表示 | VFSとpack、PNG・WAV/Ogg・fontのimport、共有cache、解放・世代差し替え、sprite、camera、描画adapter | [実装・検証記録](new_docs/evidence/E03/README.md) |
| E04：世界と保存基盤 | 多layer・素材寸法・個体・部分生成・lease・独自channel・区域退避・世代保存 | [実装・検証記録](new_docs/evidence/E04/README.md) |

E03のコードは全168件のテスト、整形、全ターゲットのコンパイル、警告拒否Clippy、Rustdocを通過しました。同じ資産packの読込とCPU参照描画はWindows・Ubuntu・macOSで、実ウィンドウのGPU表示はWindowsのVulkan / GeForce RTX 4060で確認しています。検証対象のコミットとCIへのリンクはE03の記録にあります。

E04では、素材別寸法・多layer・疎なchunk・個体・原子的編集・lease・生成baseと編集差分・独自channel・remap・Runtime・世代保存・区域の退避と復帰を接続しました。[公開APIと保存契約](new_docs/29_WORLD_AND_PERSISTENCE.md)、[検証記録](new_docs/evidence/E04/README.md)に、実行した検査と残る後工程を記載しています。新設計に沿った世界・マテリアル・物理・ゲーム共通サービス、完成した表現・音響、通信、制作ツールと商用認定は[工程表](new_docs/19_IMPLEMENTATION_ROADMAP.md)に従って実装・検証します。既存のシミュレーションやゲーム作例があることだけで、新工程を通過したとは判定しません。

## 動かす

CIではRust **1.98.1**を使用しています。初回ビルドには依存crateを取得するネットワーク接続が必要です。ウィンドウ表示にはOSの表示環境と対応GPUが必要ですが、headlessの作例はウィンドウなしで動作します。

```sh
git clone https://github.com/sofia-gros/charon-engine.git
cd charon-engine
rustup toolchain install 1.98.1 --profile minimal --component rustfmt --component clippy
cargo +1.98.1 run --locked --example assets_scene
```

資産packから画像・音声・fontを読み込み、`target/e03-artifacts/gallery.png`へ画像を出力します。音声の再生と文字組みはE09の対象であり、この作例ではimportを確認します。

ウィンドウでスプライトとカメラを確認する場合：

```sh
cargo +1.98.1 run --locked --example assets_scene -- --window
```

その他の公開APIの作例：

```sh
cargo +1.98.1 run --locked --example foundation_data
cargo +1.98.1 run --locked --example runtime_scenes
cargo +1.98.1 run --locked --example sandbox_world
```

| 作例 | 内容 |
| --- | --- |
| [foundation_data.rs](examples/foundation_data.rs) | 作者の状態・component・格子データの保存と取引 |
| [runtime_scenes.rs](examples/runtime_scenes.rs) | headlessで入力を注入し、Scene遷移と終了を実行 |
| [assets_scene.rs](examples/assets_scene.rs) | 共通packのimport、スプライト、複数カメラ、headless/GPU表示 |
| [sandbox_world.rs](examples/sandbox_world.rs) | 約43億cellの論理世界を部分生成し、家具・部分液量・遠隔lease・退避区域を保存復帰 |

## ゲームの作り方

作者はSceneや独自SystemをRustの構造体とトレイトで実装し、型付きビルダーで生成時の設定を組み立て、実行中はメソッドとContextを通して操作します。Cargoが組み込む実装と依存を選び、YAMLがエンジンの実行設定を管理し、Rustコードがゲーム内容を定義します。

- [クリエーターの制作手順](new_docs/02_CREATOR_WORKFLOW.md)
- [APIの設計方針](new_docs/03_CREATOR_API_MODEL.md)
- [エンジン設定の責務](new_docs/05_ENGINE_CONFIGURATION.md)
- [実装済みの資産・基本描画API](new_docs/28_ASSET_AND_BASIC_RENDERING.md)

制作手順とv1のAPI設計には今後の実装も含まれます。現在使える具体的な経路は、各工程の検証記録とコンパイル対象の作例を参照してください。

## 開発に参加する

製品設計の入口は[new_docs/README.md](new_docs/README.md)、開発規約は[AGENTS.md](AGENTS.md)です。このREADMEも実装と検証の進捗に合わせて更新します。`codex/`と`gemini/`は履歴資料として保存しており、現在の開発仕様には使用しません。

リポジトリ直下で実行する品質チェック：

```sh
cargo fmt --all -- --check
cargo check --locked
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cargo doc --lib --no-deps --locked
```

[全体Rust CI](.github/workflows/rust.yml)と[3 OSの資産・描画CI](.github/workflows/e03-assets.yml)、[3 OSの世界・保存CI](.github/workflows/e04-world.yml)を継続実行します。公開APIや保存形式の変更には、設計・利用例・テスト・互換性への影響を記録してください。

## ライセンス

エンジン本体は[MIT License](LICENSE)です。依存crateと同梱資産にはそれぞれのライセンスが適用されます。[E03の検査用資産](tests/fixtures/e03/README.md)には、生成した画像・音声と、OFLのfontの出典・使用許諾を記録しています。
