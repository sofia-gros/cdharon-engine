# 検証と出荷判定

本書は設計上の合格条件です。現時点でテスト実装・性能測定が済んだことを意味しません。

## 契約と検証の対応

| ID | 対応契約 | 最低限の検証と合格条件 |
| --- | --- | --- |
| V01 | C01 | 負座標chunk往復、半開境界、固定tick、overflow。30/60/144Hz描画で同tickのhash一致 |
| V02 | C02/C04 | cell size、flag予約bit、sidecar交換、stamp。chunk境界でも二重移動なし |
| V03 | C03/C11 | 空レジストリ、上限直前/超過、重複、missing、stale EntityId。panicなし・ID衝突なし |
| V04 | C05 | 空間交換、小領域全列挙、ランダム場、満pool。物質トークン増減が変換台帳と一致 |
| V05 | C05/C08 | 断熱閉領域のenergy保存、熱の上限、phase往復、反応予約競合、燃料枯渇 |
| V06 | C06/C10 | checkpoint→復元→再開のtick hash一致。job、lease、属性、allocatorを含む |
| V07 | C08 | dense対chunk対sleep/Dirtyのdifferential test。各tickが完全一致 |
| V08 | C07/C09 | 画面外既定ON、遠隔lease、上限、IO順、load/unload、世界端。データ消失・partial activationなし |
| V09 | C09/C11 | malformed content/save、長さ偽装、容量枯渇、disk full。操作がatomicに失敗 |
| V10 | C12 | 固定scene/seed/inputによるCPU/GPU/memory計測。条件とpercentileを公開 |
| V11 | C01/C04 | 薄壁、角、斜面、step頭上、水中、埋まり、高速弾。壁抜け・無限解決なし |
| V12 | C03/C11/C12 | 作者が重さ・燃焼・独自属性を定義し遠隔worldを操作するsample。文書だけで再現可能 |

保存workerの各write/flush/公開段階へfaultを注入し、最新または旧有効世代に戻れることをV09で確認します。移行fixtureは変更前の版を固定します。

## テスト方法

最小のpure simulationをheadlessで動かします。単体テストは構造・境界、property-based testは保存則・atomicity、differential testは最適化同値性を確認します。fuzzはdecoder/content/API境界を優先します。テストの失敗seedとcommand列をfixtureへ保存します。

M1でlegacyの見た目をそのまま正解としてgolden化しません。二重移動や壁抜けの既知問題を残すことになるため、契約に基づく小sceneと再現ログを基準にします。挙動変更でhashが変わる場合は理由とsimulation versionを更新し、無説明で期待値を書き換えません。

## M1出口の実行証拠（2026-10-14）

| Gate項目 | fixture / source test | 確認内容 |
| --- | --- | --- |
| V01 | `core::timing::tests::world_and_player_state_match_at_30_60_and_144_hz`; `one_second_produces_sixty_ticks_without_drift`; `negative_world_coordinates_use_euclidean_chunk_mapping`; `tick_overflow_returns_error_without_mutating_world` | 同一60 tickのWorld/Player状態、tick換算、負chunk座標、overflow拒否 |
| V02 | `test_cell_size_and_alignment`; `moved_cell_is_stamped_and_cannot_move_twice_in_one_tick`; `resident_remote_chunks_advance_in_global_world_scan_order`; `thermal_energy_diffuses_conservatively_and_roundtrips_in_snapshot` | CellCore layout、同tick二重移動防止、remote走査順、sidecar保存/移動 |
| V03 | `empty_registry_freezes_and_material_id_capacity_is_checked`; `duplicate_and_invalid_names_fail_without_changing_registry`; `freeze_assigns_ids_by_name_and_blocks_registration`; `freeze_rejects_unknown_combustion_product`; entity allocator stale-ID fixture | 空気のみregistry、全65,534作者ID枠と超過、重複/不正名、安定ID、stale generation、容量拒否 |
| V04 | `transaction_is_atomic_on_overflow_or_bad_order`; `brush_and_mining_update_material_ledger_atomically`; `philox_seeded_small_field_preserves_material_population_over_ticks`; `water_sand_and_dig_use_validated_world_commands` | 変換台帳atomicity、採掘容量予約、小世界fieldで30 tick保存則、既存水/砂/掘削API |
| V06 | `snapshot_roundtrip_preserves_state_and_rejects_trailing_bytes`; `snapshot_roundtrip_preserves_entity_allocator_and_rejects_stale_ids`; `future_commands_are_sorted_bounded_and_survive_checkpoint`; `replay_records_domain_lease_commands_and_canonical_hash`; `bundle_generation_restores_world_game_and_player_together` | snapshot/load後のallocator・pending queue、再開/replayの状態一致、World/game/Player bundle復元 |
| C06視覚状態分離 | `core::replay::tests::replay_and_canonical_hash_ignore_presentation_particles` | 演出particleの有無がWorld fingerprint・canonical hash・Replay初期状態検証を変えない |

M1再監査では`cargo check --locked --jobs 1`、`cargo test --locked --jobs 1`（86 passed）、`cargo clippy --all-targets --locked --jobs 1`、`cargo fmt --all -- --check`を実行し、全てpassした。V05/V07〜V12とクロスtarget certificationはM1出口の対象外で、後続gateで検証します。M1を通過扱いにする根拠はこの表のfixture coverageと実行結果であり、全製品契約の完了宣言ではありません。

unsafe/SIMDを追加した場合はscalar対照、境界fuzz、targetごとの試験を追加します。並列を採用する場合はthread数1/2/4/8、worker遅延、異なる完了順でも同じ結果を確認します。


## M2出口の実行証拠（2026-10-15）

| Gate項目 | fixture / source test | 確認内容 |
| --- | --- | --- |
| V07 | `frozen_quiescent_chunk_matches_full_scan_and_rejoins_on_wake`; `thermal::tests::inactive_cells_keep_energy_and_block_flux_from_active_neighbors`; `inactive_finite_chunk_insulates_thermal_energy_until_reactivated` | 実sleep状態への遷移をassertし、全域scanと8 tick比較後、再active化して4 tick進めたCell/energyが一致。凍結側との熱fluxなし |
| V08 | `remote_chunks_continue_the_seeded_finite_terrain_and_snapshot_the_seed`; `queued_remote_activation_waits_for_its_tick_and_survives_save_restore`; `powder_crosses_a_finite_to_remote_chunk_edge_in_the_same_tick`; `powder_and_energy_cross_a_remote_to_finite_chunk_edge_together`; `liquid_push_transaction_can_span_a_chunk_boundary`; `rejected_chunk_lease_does_not_publish_chunks_or_consume_an_id` | 同一seedの絶対座標地形、target tickでの遠隔domain公開、両方向の継ぎ目移動、3セル押し出しとsidecar追従、容量拒否のatomicity |
| V09 | `injected_write_flush_and_publish_failures_keep_last_valid_generation`; `rejected_oversized_save_preserves_latest_generation`; `generations_roundtrip_retain_previous_and_fallback_after_corruption` | temp作成・header後partial write・flush・rename前の故障、容量超過、checksum破損で旧有効世代を保持 |

M2 gateでは`cargo test --locked --jobs 1`（99 passed）、`cargo check --locked --jobs 1`、`cargo clippy --all-targets --locked --jobs 1`、`cargo fmt --all -- --check`を実行し、全てpassしました。chunk data source/IO workerは未実装のため、要求済みchunkはactivation対象tickのP0で同期生成・確保します。世代保存ではtemp作成、partial write、flush、publish前、上限超過のfault injectionにより既存有効世代が残ることを確認しました。実OS上のdisk-full再現、directory fsync、電源断耐性はこのfixtureから保証しません。remote熱拡散とWorld座標を持つentity更新は後続工程の対象です。M2ではsleep候補経路を実装し、恒等条件を満たすchunkへの遷移と起床後の参照simulation一致を検証しました。



## 性能予算（暫定）

物理60tick/sの時間窓は16.67msです。標準負荷でsimulation p95≤8ms、p99≤12msを目標とし、OS・描画・IOの余裕を持たせます。画面表示はp95≤16.67ms、p99≤25ms、定常実行で増え続ける遅れがないことを目標とします。GPU計測とCPU計測を分け、VSync待機を演算時間に混ぜません。

参考測定機の型番、RAM、OS、driver、build flags、Rust versionをM0で採取します。標準機と下位機をM6で選定するまで、最低動作環境を製品保証しません。上限を緩和した作者世界のFPSは別結果として公開します。

| scene | 固定する負荷 | 目的 |
| --- | --- | --- |
| B0 静止 | 96 active chunks、全静止、640×360表示 | sleep効果と基礎overhead |
| B1 流体 | 同domain、30% liquid/powder、16 entities | 標準の局所更新 |
| B2 破壊 | 毎60tickで半径32の爆発、256正規粒子 | Dirty急増、予約、最悪tick |
| B3 熱・反応 | 96 chunks、20%燃焼/反応候補 | sleepできない場面 |
| B4 遠隔 | player域と遠隔域、計256 active/512 resident | 画面外APIの上限負荷 |
| B5 IO | 境界移動、遠隔lease、保存、容量不足 | 停止時間、旧セーブ保持 |

B0〜B3が標準性能目標、B4は上限ストレスで同FPS保証をしません。B5の準備待機はフレーム性能から別に報告し、見えない除外をしません。30秒warmup後60秒計測を5回。最良値のみ採用せず分布、活動セル数、bytes/tick、allocations、memory峰値を記録します。

512 chunksならCellCoreのみ8MiB、全chunk分stamp16MiB、energy8MiBです。timer、損傷、属性、copy snapshot、画像、entity、allocator余白を合計して測定します。chunk byte予算256MiBはrenderer資産まで含む全プロセス上限ではありません。標準負荷のprocess memory暫定目標は512MiB以内です。

定常tick内の追加allocation=0をinstrumentationで確認します。登録・生成・ロード・snapshot確保は別に計測します。性能が外れた場合は全域走査、物性参照、copy、GPU upload等のprofileから原因を選び、正規stateを削って隠しません。

## 出荷ゲート

全契約の試験証拠、保存fault試験、8時間soak、最悪負荷での安全な劣化、作者sample、SDK文書、ライセンス/SBOM、対応環境での実機試験を必須とします。P0/P1の不具合（データ消失、頻発crash、再現性契約違反）が残るRCは出荷しません。

source変更のたびにformat/check/testを実行し、clippy指摘を分類します。設計変更だけならリンク、ID参照、仕様の相互整合を確認します。コードを変更していない設計作業でruntime合格と表示しません。
