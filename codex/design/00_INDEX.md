# 製品版設計の入口

設計版: **0.1 / 2026-10-14**。対象はCharon Engine 1.0。ここに記載する仕様は実装予定であり、実装済み機能は[README](../README.md)を参照してください。

## 読む順序

| 文書 | 役割 |
| --- | --- |
| [01_PRODUCT.md](01_PRODUCT.md) | 製品範囲、ユーザー体験、対象環境、非目標 |
| [02_CONTRACTS.md](02_CONTRACTS.md) | 全文書が共有する単位、識別子、不変条件 |
| [03_ARCHITECTURE.md](03_ARCHITECTURE.md) | 所有権、依存関係、更新順、API |
| [04_SIMULATION.md](04_SIMULATION.md) | セル移動、境界、スリープ、熱・反応、崩落 |
| [05_RUNTIME_AND_CONTENT.md](05_RUNTIME_AND_CONTENT.md) | キャラクター、描画、ゲーム機能、生成、拡張 |
| [06_PERSISTENCE.md](06_PERSISTENCE.md) | 保存、復元、リプレイ、互換性、障害処理 |
| [07_VALIDATION.md](07_VALIDATION.md) | テスト、性能予算、出荷判定 |
| [08_RESEARCH.md](08_RESEARCH.md) | 一次資料、研究の適用条件、実験計画 |
| [09_ROADMAP.md](09_ROADMAP.md) | 依存関係付き工程、移行手順、完了条件 |
| [13_M0_BASELINE.md](13_M0_BASELINE.md) | 開発環境、check/test、固定scene測定、再現scene |
| [10_DECISIONS_AND_AUDIT.md](10_DECISIONS_AND_AUDIT.md) | 判断理由、文書間の整合性、未解決事項 |
| [11_MATERIAL_AUTHORING.md](11_MATERIAL_AUTHORING.md) | 重さ・浮力・可燃性、型付き属性の定義・アタッチ |
| [12_OFFSCREEN_CHUNK_API.md](12_OFFSCREEN_CHUNK_API.md) | 既定ONの画面外ロード、遠隔保持API、作者による制限変更 |

## 仕様の扱い

この文書群を今後の製品版設計の基準とします。`gemini/`は背景・学習資料として保持します。旧資料のコード例や性能断言を、そのまま実装要件として採用しません。

優先順位は、ユーザーの指示 → 共通契約 → 個別設計 → ロードマップです。矛盾が見つかった場合、都合のよい方を実装せず、該当契約と影響文書を同時に修正します。現在のソースは「現状」の証拠であり、将来仕様を満たしている証拠ではありません。

仕様変更は[判断記録](10_DECISIONS_AND_AUDIT.md)に、理由、互換性、代替案、検証方法を追記します。挙動変更にはsimulation version、保存構造の変更にはschema version、データの変更にはcontent hashを対応させます。

> [!IMPORTANT]
> 文書レビューだけでは、無欠陥・性能達成・数値安定性を保証できません。本設計は不変条件と段階的な検証ゲートを設け、未検証の最適化や互換性を製品仕様として約束しない方針です。

## 追跡規則

共通契約は`C01`〜`C12`、検証項目は`V01`〜`V12`、工程は`M0`〜`M8`で識別します。主要契約の実装には、対応する検証が必要です。実装時には実ファイル・テスト名・測定結果を工程の記録へ追加します。チェック済み表示は証拠が揃った項目だけに付けます。
