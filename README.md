# Charon Engine

> Rust製の、破壊可能な地形とセル単位の物理を備える2Dサンドボックスエンジン。

[![Rust CI](https://github.com/sofia-gros/cdharon-engine/actions/workflows/rust.yml/badge.svg)](https://github.com/sofia-gros/cdharon-engine/actions/workflows/rust.yml)
[![Rust](https://img.shields.io/badge/Rust-2024%20Edition-orange?logo=rust)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](./LICENSE)

---

## 概要

Charon Engineは、Falling Sand（セル・オートマトン）方式の物理シミュレーションと、探索・アクションゲーム向けの機能を組み合わせることを目指す2Dゲームエンジンです。水が流れ、砂が落ち、削られた地形が崩れる世界をRustで構築しています。

現在は開発中のプロトタイプです。READMEではソースコードで確認できる機能と、今後の設計目標を分けて記載しています。製品版設計とゲート状況は [`codex/README.md`](./codex/README.md) および[設計インデックス](./codex/design/00_INDEX.md)を参照してください。

### 主な特徴

- **セル単位の地形物理** — 水・砂・固体の移動と、支持を失った地形の崩落
- **固定シミュレーションtick** — 60Hzの整数tickと、順序を固定したコマンド処理
- **拡張可能なマテリアル** — Rust APIとTOMLから物質・ゲーム内質量・型付き属性を定義
- **画面外chunk API** — 制限付きの既定active領域、resident領域、遠隔chunk lease
- **保存・再現の基盤** — version付きWorld snapshot、世代保存、限定Replay API
- **headless実行** — ウィンドウなしでsimulationとgameplay sampleを実行

---

## 現在の実装状況

| 機能 | 状態 |
| --- | --- |
| 地形生成・編集 | 固定seedの地形生成、ブラシによる水・砂配置と掘削 |
| セル物理 | 水の流動、砂の落下、固体の局所崩落。気体・化学反応は未実装 |
| プレイヤー | Q16.16位置・速度、移動・ジャンプ、水との相互作用 |
| マテリアル | 安定名による登録、質量rank、型付きproperty/attachment、TOML schema v1 |
| チャンク | 64×64格納、有限領域とremote resident/active領域、同期lease API |
| 熱 | 有限Worldの等熱容量energy拡散。material別熱容量、remote sidecar、燃焼は未実装 |
| ゲーム機能 | 採掘、容量付きinventory、recipe craft、敵HP/攻撃のsample API |
| 保存 | World snapshotとWorld+GameSession+Player bundleの世代保存・復元 |
| Replay | Player入力とPaintBrush commandを対象とする限定API。全command対応は未実装 |
| GUI | `winit`と`pixels`によるウィンドウ表示、地形編集操作 |

> [!WARNING]
> M1〜M5の製品版ゲートは未通過です。未実装項目、契約、検証条件は[ロードマップ](./codex/design/09_ROADMAP.md)を参照してください。性能・対応環境・セーブ互換性は製品品質として保証していません。

---

## 必要環境

- Rust 2024 Edition対応のRust toolchain
- GUI実行にはウィンドウ表示環境
- 初回build時はCargo依存crateの取得が必要

GitHub ActionsではRust 1.98.1を使用します。ローカルでも同じtoolchainを使う場合は、rustupでRust 1.98.1、`rustfmt`、`clippy`を導入してください。

---

## Quick Start

リポジトリをcloneして、GUIアプリを起動します。

```bash
git clone https://github.com/sofia-gros/cdharon-engine.git
cd cdharon-engine
cargo run --locked
```

### 操作

| 入力 | 操作 |
| --- | --- |
| `A` / `←`、`D` / `→` | 左右に移動 |
| `W` / `Space` / `↑` | ジャンプ。水中では上方向へ移動 |
| 左クリック長押し | 水を配置 |
| `Shift` + 左クリック、または中クリック | 砂を配置 |
| 右クリック長押し | 地形を削除 |
| `F11` | フルスクリーン切替 |

### Headless実行

```bash
# 600 tick進める（引数を省略した場合も600 tick）
cargo run --locked --bin headless -- 600
```

### Gameplay sample

採掘、craft、戦闘、World/game/player bundleの保存・復元を確認できます。

```bash
cargo run --locked --bin gameplay_sample
```

---

## マテリアル定義

作者向けTOML schemaの例は [`content/examples/light_wood.toml`](./content/examples/light_wood.toml) にあります。物質名は`namespace:name`形式です。質量はセル間の密度rankに使われ、型付きproperty、version付きattachment、燃焼宣言を登録できます。宣言した燃焼性や独自属性がすべてsimulationで動作するわけではありません。

画面外chunkの既定policy、lease、作者による容量設定は[画面外chunk API設計](./codex/design/12_OFFSCREEN_CHUNK_API.md)を参照してください。

---

## アーキテクチャ

```text
charon-engine/
├── src/
│   ├── core/          # 固定tick、入力、設定、GUI host、Replay
│   ├── entities/      # プレイヤー状態と移動
│   ├── game/          # inventory、craft、採掘、戦闘sample
│   ├── simulation/    # World、chunk、material、熱、保存
│   ├── bin/           # headless / gameplay sample
│   └── render.rs      # 描画データの生成
├── content/examples/  # 作者向けmaterial TOML
├── codex/             # 現在の実装READMEと製品版設計
├── gemini/            # 背景・学習・旧設計資料
└── .github/workflows/ # CI
```

シミュレーションの正規状態は描画や壁時計から分離します。simulation、保存形式、マテリアル、画面外chunkの共通契約は [`codex/design/02_CONTRACTS.md`](./codex/design/02_CONTRACTS.md) にまとめています。

---

## 開発

リポジトリrootから品質チェックを実行します。

```bash
cargo fmt --all -- --check
cargo check --locked
cargo test --locked
cargo clippy --all-targets --locked
```

変更時は [`AGENTS.md`](./AGENTS.md) の規約を確認してください。simulation、material、chunk、collision、persistenceに影響する変更では [engine-system-change skill](./.agents/skills/engine-system-change/SKILL.md) の横断確認を使います。Pull Requestとpushでは[GitHub Actions](./.github/workflows/rust.yml)が同じ品質チェックを実行します。

設計文書の入口は[製品版設計インデックス](./codex/design/00_INDEX.md)です。現在の機能と将来仕様に差がある場合は、sourceを実装状況の基準、共通契約を将来設計の基準として扱います。

---

## Contributing

IssueやPull Requestでの報告・提案を歓迎します。simulation挙動、公開API、content schema、save formatの変更では、関連する契約・version・テスト・設計記録を合わせて更新してください。性能や決定性については測定・再現可能な検証結果を添えてください。

## License

[MIT License](./LICENSE)
