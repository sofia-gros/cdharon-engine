# 研究・理論の適用と実験計画

調査日: 2026-10-14。2024〜2026年の一次資料に加え、基礎理論・開発者本人の技術報告を確認しました。網羅的な文献調査ではありません。arXivの公開を査読済みの証拠とは扱いません。

## 採用方針

論文の速度倍率を本エンジンへ転用せず、対象モデル、データ規模、GPU依存、精度、保存則、再現性を確認します。下表の「採用」は設計への反映であり、実装効果を測定済みという意味ではありません。

| ID / 資料 | 確認内容 | 本エンジンへの判断 |
| --- | --- | --- |
| R1: Wolnikほか、2025公開 | 数保存する2D binary CAを粒子移動規則として記述する枠組み | 交換演算、排他予約、小領域全列挙に反映。多物質CA全体の証明を得たとはしない |
| R2: Navarroほか、2024 | 近傍の重み付き和を使うCAをtensor coreへ写像 | weighted stencilの候補。分岐と交換を持つFalling Sandの正規物理には直結しない |
| R3: Catto、2024 | 並列順序やfloat演算で決定性が壊れる点、CI検証 | 安定順序・整数正規state・hash試験に反映 |
| R4: Catto、2026-06 | 初期内部stateとAPI操作列を記録するreplay設計 | checkpoint＋command＋hashを採用。ゲーム側の状態も記録する |
| R5: Catto、2026-07 | Box3Dの複雑凸形状collisionにwide SIMDを適用 | SoAとbatchingを実験。2DセルAABBに同じ改善を期待しない |
| R6: Kumarほか、2026、v2 | Cartesian grid上の移動境界・流体構造連成を大規模GPUへ実装 | 将来の圧力/連成研究。計算規模・モデルが違い1.0の依存にしない |
| R7: Salmonほか、SC11/2011 | counter-based RNGで独立した乱数参照を得る | tick/coord/domainで乱数を引く候補としてPhiloxを評価 |
| R8: Fiedler、2004 | 固定step、accumulator、描画補間、過負荷問題 | 60Hz固定tickと過負荷停止へ反映 |
| R9: Purho、GDC2019 | Noitaのfalling sandと大世界・破壊物理の技術概要 | chunk/活動領域の参考。動画概要だけから細部の正しさを断言しない |
| R10: Catto、2025-03 | dynamic AABB treeの挿入コスト評価 | 多数entityのbroad phase候補。地形格子には既存chunk indexを使う |

## 一次資料と適用限界

**R1** — B. Wolnikほか、*Conditional traffic-like rules for particle-flow simulation in cellular automata*。arXiv v1の公開日は2025-12-09。binary占有と特定近傍の数保存が対象です。本設計のmulti-material交換・熱・反応は独自拡張なので、保存則は別途試験します。[論文](https://arxiv.org/abs/2512.08727)、[本文](https://arxiv.org/html/2512.08727v1)。

**R2** — C. A. Navarroほか、*CAT: Cellular Automata on Tensor cores*、2024-06-25。近傍和型のCAと大きい半径を扱います。疎なゲーム世界の条件分岐・物質交換・CPU/GPU同期まで高速化した結果ではありません。[論文](https://arxiv.org/abs/2406.17284)。

**R3** — Erin Catto、*Determinism*、2024-08-27。thread完了順に依存する結果並びと、演算・compiler差の問題を扱う開発者報告です。floatで決定性を実現する条件も示されますが、こちらは基盤を単純にするため整数/固定小数点を採用します。[本人の報告](https://box2d.org/posts/2024/08/determinism/)。

**R4** — Erin Catto、*Replay*、2026-06-07。snapshotからAPI操作列を再生し、step時のhashで検証する報告です。内部stateの取得、サイズ費用、ゲーム側の決定性が必要である点を保存設計へ反映します。これはBox2Dを組み込む決定ではありません。[本人の報告](https://box2d.org/posts/2026/06/replay/)。

**R5** — Erin Catto、*SIMD for Collision*、2026-07-18。主な事例はBox3Dの凸形状edge処理です。データをSoAへ整える費用とbatch規模を考慮し、2Dの小さな処理へ無条件適用しません。[本人の報告](https://box2d.org/posts/2026/07/simd-for-collision/)。

**R6** — Sushrut Kumarほか、*GPU-Accelerated Simulations of Moving Boundary Problems and Fluid-Structure Interaction at Extreme Scales*。2026-05公開、v2は2026-06-24。sharp-interface immersed boundary法を大規模な3D流体へ適用しています。今回はabstract/書誌までの調査であり、solver導入前には本文・実装・ライセンス・境界処理を精査します。[論文](https://arxiv.org/abs/2605.04335)。

**R7** — John Salmonほか、*Parallel Random Numbers: As Easy as 1, 2, 3*、SC11。counterとkeyから値を得る構造を参照します。Rust移植やtuple mappingの正しさは、本エンジンのgolden vector試験で確認します。暗号用途には使いません。[著者公開ページ](https://random123.com/)、[CBRNG仕様](https://random123.com/releases/1.09/docs/CBRNG.html)。

**R8** — Glenn Fiedler、*Fix Your Timestep!*。物理stepと表示周期を分離する基礎資料です。本設計の過負荷停止、tickを飛ばさない制御、入力量子化は製品向けに明示した追加判断です。[原著](https://gafferongames.com/post/fix_your_timestep/)。

**R9** — Petri Purho、*Exploring the Tech and Design of Noita*、GDC2019。大きな連続世界へfalling sandを拡張する実例として参照します。Noitaと同じ構造・品質・速度であると宣伝しません。[GDC公式ページ](https://www.gdcvault.com/play/1025695/Exploring-the-Tech-and-Design)。

**R10** — Erin Catto、*Dynamic Tree Improvements*、2025-03。AABB treeの挿入選択を改善する本人の説明です。cell terrainは格子で既に索引化できるため、treeはentityの候補集合に限って比較します。[本人の報告](https://box2d.org/posts/2025/03/dynamic-tree-improvements/)。

## 実験バックログ

| 実験 | 比較 | 合格条件 | 実施工程 / 不採用時 |
| --- | --- | --- | --- |
| E1 保存する移動 | 交換参照と候補予約 | 小領域列挙でtoken損失0、double move0 | M1 / 単純交換を維持 |
| E2 cache/locality | flat、64chunk、32/128の実験branch | tick hash一致、B0〜B3のp95・memory改善 | M2/M6 / 64基準、実測値で判断 |
| E3 sleep/Dirty | 全走査と活動領域 | 各tick完全一致、稀なwakeも取りこぼさない | M2 / sleep OFFで出荷可能 |
| E4 thermal stability | 整数fluxと広精度参照 | energy drift0、係数範囲内で暴走なし | M4 / 係数・範囲を縮小して再検証 |
| E5 scalar/SIMD | occupancy scan、熱、batch query | hash一致、標準sceneのp95≥10%改善、下位targetもfallback可能 | M6 / scalar維持 |
| E6 parallel candidates | serialとsnapshot候補生成 | 異なるthread数・遅延で一致、copy込みで改善 | M6後 / single thread維持 |
| E7 entity index | spatial hashとdynamic tree | query集合一致、変動地形/多数entityで改善 | M5/M6 / spatial hash維持 |
| E8 GPU stencil | CPUとdisplay-only近似 | 見た目の許容差、同期費用込みの改善 | M6後 / 正規CPUへ影響させない |

各実験は、仮説・再現コマンド・toolchain・raw結果・state差・memory差・判断を保存します。10%基準は採用の目安であり、有意差がない測定や性能低下sceneを無視しません。高度な最適化が不採用でも製品化できる基盤を維持します。

研究更新は各milestone開始時に、保存則、決定性、sparse CA、数値安定性の関連一次資料を再確認します。本書だけで継続監視のautomationを設定したものではありません。
