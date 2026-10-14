# 判断記録と整合性監査

設計版0.1、監査日2026-10-14。ここでの監査は文書・source比較であり、実装試験の合格記録ではありません。

## 採用した判断

| 判断 | 理由 | 代替案・再検討条件 |
| --- | --- | --- |
| D01 CPU単一threadの正規参照 | 更新順・保存則を先に固定できる | 並列候補＋安定commit。E6合格とADRが必要 |
| D02 CellCore4byte＋sidecar | 外観と温度を競合させず機能拡張できる | 8byte cell/SoA。memory/cache実測で比較 |
| D03 整数・固定小数点state | replayとtarget差を扱いやすい | float。決定性試験と演算制約が必要 |
| D04 64chunkは測定候補 | 16KiBだけでL1常駐を保証できない | 32/128。E2結果とformat互換を確認 |
| D05 1cell=1格子占有 | 資源、移動、保存、衝突の意味を統一 | 部分質量/サブセルは別simulation version |
| D06 局所崩落、精密entity衝突 | 4×4低密度を通行可とすると薄壁が消える | 回転剛体/全域支持は別工程 |
| D07 有限世界、domain外freeze | 計算上限と明示境界が必要 | 全世界activeはcreator APIで容量を拡張 |
| D08 画面外既定ON＋lease | ユーザー要件。視野外施設等をEngineとして提供 | policy・上限・activityを作者が変更可能 |
| D09 typed property/attachment | 作者が物性と独自項目を安全に拡張する | arbitrary callbackは正規契約を壊すため不採用 |
| D10 重さから密度順位を導出 | 軽さ/重さの二重定義の矛盾を避ける | 部分充填モデル導入時のみdensity/volumeを分離 |
| D11 世代保存をM2から実装 | allocator/AI/leaseを後付けで漏らさない | COWはprofile後。停止snapshotをfallbackとして維持 |
| D12 GPUはまず表示のみ | 正規物理への転送費用・target差を避ける | 別backendは比較・版変更・出荷試験が必要 |
| D13 Codexの中核拡張はrepo規約＋1つのSkill | 常時指示は軽くし、繰り返す横断作業だけを遅延読込 | 反復workが増えた時にworkflowごと追加 |
| D14 GitHub CIをWindows・Rust固定版で開始 | 既存プロトタイプ/開発hostに合い、format/check/lint/testを共有 | M6の実機計画でOS matrixを広げる |
| D15 レジストリで空気/missingを予約し、登録を検証 | 未知IDの空気化と`u16`切り詰めを防ぎ、作者入力に明確な失敗を返す | 名前順ID・凍結・永続辞書はM1で実装し、そこまでは登録順の実行時ID |
| D16 壁時計とsimulation tickを分離し60Hz accumulatorを使う | イベントループ頻度で物理速度が変わる問題を解消。1回最大4tickで遅れを保持し、非focus時間を注入しない | backlogの診断/一時停止と入力edge量子化はM1後続で追加 |
| D17 `lib.rs`を再利用境界にし描画をWorldから分離 | GUI hostとheadless runnerが同じsimulation crateを使う。WorldにPlayer/pixels依存を持ち込まない | rendererは現状Worldを直接読む。immutable view/snapshotは後続で追加 |
| D18 二重移動stampをu64 sidecarへ移す | C02の4-byte cell flagsからclock bitを除き、移動元/先の双方を同tick処理済みにする | stampはscratchでsave/hash対象外。restore時は全domainをwake |
| D19 Philox test vectorとworld診断fingerprintを実装 | RNGの基礎写像と同一world回帰検出を固定する | FNV-1a fingerprintは非暗号・Worldのみ。network/save互換hashとして使わない |
| D20 有限セル格納を64×64 chunk storeへ移行し、初期resident上限512とする | 符号付きworld座標・負chunk境界・容量拒否をworld更新前に確定する | 512は初期実装上限で性能保証値ではない。dynamic streaming/lease/byte policyを提供するM2実装でcreator設定へ置き換える |
| D21 splash粒子の乱数をtick/座標/domain/sampleから導出 | thread-localな可変乱数状態が呼出順や環境で変わるのを避ける | float entity移動や粒子物理はまだ正規固定小数点へ移行していない。粒子は正規mass物質ではなく視覚効果の範囲でのみ使用 |
| D22 streaming policyは純粋planで容量を先に拒否 | worker完了順からsimulation domainが決まるのを防ぎ、active/residentとbyte budgetを分ける | request lifecycle・storage reservation・tick activation・lease/saveは未接続。初期byte予約64KiB/chunkは実使用量をprofileして置き換える |
| D23 非連続chunk追加を事前reserve後にatomic publish | capacity/allocation failureで一部だけresidentに見える状態を避ける | 保存済み状態のdecode/restoreとは未接続。chunkは既定値で初期化されるため永続データを持つload経路では使わない |
| D24 同tick command batchをtarget/source/sequenceでsortし、結果eventを返す | arrival orderや作者間競合を安定化し、失敗結果を明示する | future command queue・source sequence永続化・replayは未実装。batch全体ではなく各command単位でvalidate/commit |
| D25 creator metadataはtyped値とversion付きattachmentとして保持し、massを交換rankの単一値にする | 物理rankと作者拡張を保存/hash対象に載せ、可燃性など未実装機能を宣言とsolverで混同しない | Rust/TOML schema v1 loaderを追加。required/min/max等のPropertySchema、instance attachment stateは未実装。flammability宣言はinert dataで、M4まで物理効果なし |
| D26 recipe craftはstaged inventoryに適用してから一括commit | 入力不足・出力slot不足・allocation errorで材料だけ消えるのを防ぐ | item definitions、採掘/配置、UI、combat、saveは未接続。slot数256・stack 1,000,000を初期hard boundとする |
| D27 snapshotはregistry content fingerprintを要求し、IDとsidecarをversion codecで候補復元 | 永続名辞書が未実装の段階で異なるmaterial登録へ誤復元するのを防ぎ、外部content変更を早期に拒否する | World stop snapshotはactive set、lease、finite thermal energyを含み、GenerationStoreはWorld snapshotとgame/player bundleを世代保存する。stable dictionary、directory durability、migration、full replayは未実装 |
| D28 miningはinventory候補を確保してからmaterial一致を再検証しWorld cellを除去 | 出力容量不足時に地形だけ消費することと、事前確認後のworld changeを避ける | 現在は有限矩形のみ。stable item-to-material content mapping、drop table、tick command transactionは未実装 |
| D29 remote chunkにresident/prefetchとactive/simulateを分け、active領域を正規座標順に更新 | 画面外のresident領域が単なる未使用bufferにならず、prefetch済み領域は凍結可能にする | Engine論理viewportからの自動visible plan/P0適用、finite initial active set、freeze/snapshot/hash、stable lease ID/source/region/modeの同期request/releaseと保存を実装。非同期確定activation、finite/remote全ruleの共有、entity/thermal sidecar、sleep、differential chunk-edge fixtureは未実装。M2ゲートは未通過 |
| D30 finite World heat uses the pairwise conservative integer reference and a dedicated energy sidecar | energy does not compete with CellCore appearance bits and moves with material; overflow is rejected before tick mutation | equal heat capacity and global 1/8 conductance only; inactive/remote thermal boundary handling, phase change, ignition, fuel timer, attachment, and reaction reservation remain incomplete |
| D31 Player canonical motion uses checked Q16.16 and 1/16-cell collision substeps | fractional motion is preserved while collision traversal is deterministic and arithmetic failures are reported | substep size is an initial rule and belongs to simulation version 5; candidate movement is bounded by i64 arithmetic, but golden movement fixtures and cross-target replay certification remain required |
| D32 World/game/Player bundle uses CHRBNDL2 with fixed raw Player state | save restore must not silently reset entity transform or water/ground flags | World snapshot v4/simulation v5 stores active sets, synchronous chunk leases and finite thermal energy; GenerationStore persists the bundle as one generation. No migration, full command replay, or unified collision-resistant state hash |
| D33 replay begins from a verified stopped World/Player fingerprint and candidate commit | a partial replay failure must not publish earlier frame mutations | `ReplayLog` currently records quantized Player input and PaintBrush only; diagnostic FNV fingerprints are not collision-resistant and domain/policy/other commands remain unsupported |
| D34 TOML content uses the existing Serde boundary plus the current `toml` parser | creator files need schema parsing without a hand-written TOML grammar | added `toml` 1.1.8; document size is capped at 64 KiB, unknown keys are rejected, and in-memory pack registration uses a candidate registry. Per-property schema constraints, filesystem pack discovery, migration and canonical content hashing remain open |
| D35 bounded generation save uses one self-validating file per stopped World snapshot | same-directory rename keeps publication simple and the previous complete file can be selected after checksum failure | file sync is implemented but parent directory sync and OS-specific power-loss guarantees are not; GenerationStore is not represented as a fully durable production save system |

## 旧資料・現コードから是正する点

| 観測した問題 | 製品設計での処置 |
| --- | --- |
| README/引継ぎのRust editionやpixels版がCargoと違う | 現状READMEはCargoを基準。将来versionは実測して固定 |
| 4byteセルに温度・密度・submaskを同時に要求する章がある | C02で1セルの意味を統一し、追加stateはsidecar |
| 赤黒chunk更新だけで並列安全を主張する | 対角/halo/read-write範囲を考慮。正規serialから開始 |
| `materials.len() as u16`の上限未検証 | reserved ID・上限・重複を登録前に検証 |
| 未知IDが空気へフォールバックし、`new()`直後の`get()`がpanicし得る | index 0を必ず空気として作り、未知/65535を不透明なmissing物質にする |
| 空registryのfallbackでindex0参照が起こり得る | empty/missing必須をfreezeで検証 |
| event loop更新を1/60秒と説明している | runtimeの固定accumulatorと正規tickを実装 |
| playerがceil速度で移動、4×4密度で衝突 | Q16.16 fractional stateと1/16-cell collision substepsを実装。精密swept query/summary broad phaseおよびV11 fixtureは未完了 |
| worldがplayerと描画処理に直接依存 | `render` moduleへ処理を移動。rendererからWorldを隠すview/snapshotは未実装 |
| 保存・allocator・pending状態の列挙不足 | C10と保存分類に正規stateを列挙 |
| 配列参照O(1)から全体性能を断言 | hot/cold分離と固定負荷測定。反応候補数も費用に含める |
| 一括flat Vecではchunk境界・符号付き座標・resident上限を扱えない | 64×64 `ChunkedCells`をWorldへ接続。範囲はまだ有限矩形を事前確保し、dynamic loadはM2残作業 |

## 文書間の確認表

| 共通事項 | 定義元 | 利用先 | 文書上の確認 |
| --- | --- | --- | --- |
| 単位・境界・tick | C01 | architecture、simulation、controller、storage | 1cell=1px、64chunk、60Hz、半開矩形へ統一 |
| flags/sidecar | C02 | simulation、authoring、storage | clockを永続flagから除去、energy/timer移動を保存 |
| material key/mass | C03 | authoring、content、codec | 0/65535予約、mass由来順位、stable key再mapping |
| command順序 | C04 | game、streaming、replay | P0〜P8、stable source/sequenceで固定 |
| atomicity | C05 | 移動、燃焼、採掘、craft、particle | 出力予約してから入力消費 |
| 再現性 | C06 | RNG、save/replay、研究 | 版/content/設定/認定targetに限定 |
| 画面外domain | C07 | streaming、生成、storage | default ON、prefetchとactiveを区別、lease保存 |
| sleep | C08 | thermal、attachment、restore | timer/確率/独自systemを取りこぼさずcache再構築 |
| 上限 | C09 | creator policy、transaction、benchmark | count/bytes両方予約。緩和時FPS保証を流用しない |
| 保存対象 | C10 | entities、AI、lease、attachments | pendingとallocatorを含め復元比較する |
| 障害 | C11 | author input、IO、renderer | Result、旧セーブ保持、candidate load |
| 出荷 | C12 | product、research、roadmap | 測定とV01〜V12の証拠で判定 |

## 実装前に詰める事項

詳細未決定を隠して「完全設計」と呼びません。以下は工程の入口で決め、相互影響を検証します。

| 項目 | 固定工程 | 決める内容 / 不合格時 |
| --- | --- | --- |
| RNG mapping・codec | M1 | test vector、tuple/domain符号化、endianness。再現性不合格なら進まない |
| streaming request protocol | M2 | cancellation、activation tick、保存中要求、byte予約。partial publish禁止 |
| 物性schemaと独自state | M3 | behavior別対応、attachment store、burn field version。未対応fieldは拒否 |
| 局所支持係数 | M3 | anchor/cohesion/falling復帰。薄壁衝突と支持モデルを混同しない |
| 熱範囲・相転移table | M4 | 係数、潜熱区間、int bound、丸め安定性。熱保存試験が必要 |
| AI経路state | M5 | serializeまたは同値再構築。save→resume一致が必要 |
| 標準機と容量初期値 | M6 | active/resident/bytes/performance条件。未測定なら数値を確定しない |
| 公開API/ライセンス | M7 | SemVer、互換範囲、配布条件。出荷前の意思決定 |

## 継続監査

今回の文書確認では、14 Markdownファイル、47相対リンク、12契約、12検証項目、9工程の定義数、code fence、末尾空白を機械確認しました。研究文書には調査した一次資料へのリンクを付けています。外部リンクの継続到達性や実装試験の合格を、この件数で保証するものではありません。

意味の点検で、相転移時のenergy原点、潜熱区間、flux前の温度丸め、作者massと密度順位、collision mask、attachment移動、準備要求の保存、IO完了時刻とactivation tickを整理しました。特に熱fluxは表示用の整数温度を流用せず、有理数温度から最後に丸める仕様へ修正しています。

新systemを加えるたび、読むstate、書くstate、順序、到達距離、容量、sleep理由、snapshot/codec、hash、復元、失敗時のatomicityを列挙します。一つでも未定義なら正規systemとして有効化しません。

文書のリンク・code fence・契約IDを機械確認し、上の表で意味の整合性を再確認します。機械確認は数学的な無矛盾証明ではありません。設計や試験で反例が出たら、契約を保ってアルゴリズムを変更するか、影響する版と文書をまとめて更新します。
