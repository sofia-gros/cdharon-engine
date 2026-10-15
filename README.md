# Charon Engine

Rust製のOSS汎用2Dサンドボックスゲームエンジンです。作者がRustでゲーム内容を定義し、エンジンがScene・入力・資源・実行管理とサンドボックスの基盤を提供します。

製品設計は[new_docs/README.md](new_docs/README.md)を参照してください。商用v1.0.0とTerraria・Noitaの完全再現を目標とし、このブランチで全機能を完成したとは扱いません。

## このブランチの実装

E01の型付きID・schema・独自状態・格子channel・取引と、E02のRuntime・Scene・plugin・入力・固定更新・System順序を実装しています。[E01の検証記録](new_docs/evidence/E01/README.md)、[E02の検証記録](new_docs/evidence/E02/README.md)に根拠と後工程を記載しています。

PR #1のレビューに対応し、非活動Systemのphase末尾でも先行commandを反映し、World所属による競合の見落としを修正しました。plugin設定拒否時には、build未開始の実装をshutdownしません。回帰テストは`tests/runtime.rs`です。

## 動かす

Rust 1.98.1を使用します。

```sh
cargo run --locked --example foundation_data
cargo run --locked --example runtime_scenes
cargo test --locked
```

ウィンドウの確認は`runtime_scenes -- --window-smoke`で実行できます。資産と描画、地形と保存などの後工程は[工程表](new_docs/19_IMPLEMENTATION_ROADMAP.md)を参照してください。

## 開発とライセンス

開発規約は[AGENTS.md](AGENTS.md)、ライセンスは[MIT](LICENSE)です。`codex/`と`gemini/`は履歴資料であり、現行の設計基準には使用しません。
