# 設計判断と依存の採用

## 判断の記録

本書はCharon Engineで採用する設計理由を記録する。参考製品の内部調査や開発史は記載しない。各判断を変更するときはP番号で影響と代替を追記する。

| 判断 | 採用する方針 | 理由と確認するリスク |
| --- | --- | --- |
| D01 APIの優先 | 整合性・性能・保守性を保ち、人間に理解しやすい通常APIを提供 | 予約やbatchが必要なら公開する。便利さのための暗黙コピーや待機を検査 |
| D02 責務の層 | author facade、runtime、独立したWorld/サービス、driverを分離 | Sceneなしでserverとtoolsを動かせる。層間同期とSnapshotの費用を計測 |
| D03 Rustの表現 | builder、trait、typed handle、scopeを基準にする | 動的object APIの重複を避ける。実際の作者利用で借用と説明の負担を確認 |
| D04 ブロックと物理 | 均一cellと素材ごとのfootprint/shapeを分離 | 建築と粒子破壊を両立。cross-chunk編集の原子性と個体状態を検証 |
| D05 状態の所属 | 素材定義とcell/個体状態、正規粒子と視覚粒子を分離 | 保存漏れ・二重所有・表示品質による物理変更を防ぐ |
| D06 世界 | sparseなchunk、部分生成、layer、activity/lease | 大規模世界と遠隔装置を両立。IO待機・保存・memoryの上限を測定 |
| D07 セルsolver | 正しいscalar基準、明示した移動・熱・反応順 | 最適化で別結果にならないよう比較。固定予算の圧力・jobの遅れを公開 |
| D08 剛体solver | Rapier f64をadapterで採用し、cell連成と破壊を実装 | 接触・jointの再発明を避ける。候補bufferの費用とbackend保存の版依存を検証 |
| D09 保存 | 明示codec、不変blob、世代manifest、候補loadとmigration | allocator・途中Jobも復元。OSとvolumeごとのdurabilityを試験 |
| D10 通信 | server権威と領域delta、任意の移動予測 | 異なるtargetのbit一致に依存しない。帯域、再接続、権限を検証 |
| D11 拡張 | Rustの静的pluginを基準、型付きdata packも同じ検証へ | 自由な機能追加を支える。native codeにdata sandboxの保証を付けない |
| D12 完成判定 | 二つの検証作品と独自拡張、配布・性能・障害の証拠 | 見た目だけ、関数の存在だけ、旧工程の合格だけでは完成にしない |

P001でD07/D08/D10/D12を補足する。標準solverとserver権威は既定providerであり、作者の表現能力の上限ではない。[25](25_OPEN_EXTENSION_AND_COLLISION_CONTRACTS.md)の公開slot、channel、候補書込、処理順の置換を必須とする。完成判定は[26](26_FULL_CLONE_ACCEPTANCE.md)の完全再現能力へ強化し、代表作例への縮小を認めない。設計reviewの指摘と解消先は[27](27_FULL_CLONE_DESIGN_REVIEW.md)へ記録する。

## 採用する実装部品

ここでの採用は設計上の方針であり、現在のCargo.tomlへ追加済みという意味ではない。版とfeatureはE00で評価してlockし、認定構成と最小構成をCIへ登録する。

| 部品 | 役割 | 代替と採用理由 |
| --- | --- | --- |
| winit | window、OS event、入力とIMEの接続 | OS個別の実装を直接維持する負担を避ける |
| wgpu | renderer、GPU資産、compute表示処理 | pixelsだけではrender graph/lighting拡張が足りない。APIへbackend型を露出しない |
| rapier2d-f64 | 接触、body、joint、CCD | 独自剛体solverより既存のsolverを検証して使い、cell連成へ注力 |
| Kira | audio mixer、voice、stream、spatial処理 | raw device APIだけから音響全体を再実装する負担を避ける |
| Taffy / cosmic-text | UI layout / shapingとfont処理 | 日本語や複雑な文字組みを独自の文字幅計算で済ませない |
| Quinn | QUIC transport | reliabilityと暗号化を独自UDP protocolへ重ねて再発明しない |
| serdeとbounded codec | config/data record / 正規saveとnetwork | 動的なmemory dumpを避け、型と上限のある入口へ接続 |
| BLAKE3 / zstd | content/blob hash / chunkとpackの圧縮 | 非暗号の診断用hashと配布整合性を混同しない。展開量を制限 |

serde用のYAML parserはE00で保守状態と入力の上限検証を確認して選定する。既存の依存があることだけを採用理由にしない。これは構文parserの選択であり、設定schemaと検証・優先順位は[05](05_ENGINE_CONFIGURATION.md)に確定している。

## 再現性と依存の構成

セル側の整数演算、乱数、順序をsim profileへ固定する。Rapierの`enhanced-determinism`とserialization機能を有効にする構成を認定候補とし、SIMD幅、並列、target、数学関数、挿入順の条件を実際の依存版で確認する。feature名だけでエンジン全体の再現性を認定しない。

backend更新は物理fixtureとsave fixtureを通し、旧profileとの違いを記録する。高速化のために別profileを提供する場合も、その保存・通信・replayの関係を説明する。

## 実装前後で検証して確定するもの

依存の正確な版、認定OSの具体的な版、実機の型番、最適化方式、内部cacheの最良のサイズは、実装と測定で固定する。これらは機能を未設計のままにする項目ではない。出荷時は未決定のまま残さない。

本書の判断が性能・正しさを満たさない場合は、根拠を示してPを作り、要件を維持する代替を設計する。依存や採用方式を守るために、body破壊や保存などの必須機能を削らない。
