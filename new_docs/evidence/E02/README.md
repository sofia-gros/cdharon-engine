# E02：RuntimeとSceneの実装・検証記録

## 対象と判定範囲

[工程表](../../19_IMPLEMENTATION_ROADMAP.md)のE02とP001のRuntime追加出口を対象とする。旧M工程の判定を引き継がない。基準revisionは`628b96ac9e8613bd386a55b05cc94ad39993596f`。E01の未コミット基盤に接続し、作業前から存在する旧ゲーム・素材・保存・inventory等の変更は保持する。

E02の起動、Scene遷移、pause/resume、scope cleanup、入力、固定更新分離、service登録、System順序とbarrier、SameTick/Deferred、独立時計の実装を以下の試験で検証した。商用v1、E03の資産とGPU描画、E04のchunkとRuntime保存、E07の物理providerが完成したという判定ではない。

## 公開入口とコード

| source | 実装した内容 |
| --- | --- |
| [mod.rs](../../../src/runtime/mod.rs) | `charon_engine::runtime`の公開入口とheadless共通Runtime |
| [engine.rs](../../../src/runtime/engine.rs) | EngineBuilder、Plugin、service slot、独立所有World、候補tickと一括公開、失敗時停止 |
| [scene.rs](../../../src/runtime/scene.rs) | lifecycle、Context、遷移queue、scope、停止時解除、modal入力 |
| [input.rs](../../../src/runtime/input.rs) | 世代付きActionId、button/axis/pointer、複数binding、対象tickとsequence、記録codec |
| [schedule.rs](../../../src/runtime/schedule.rs) | phase、依存、読書集合、競合検証、明示barrier、解決済みScheduleProfile |
| [clock.rs](../../../src/runtime/clock.rs) | 有理速度・余り・pauseを保持するClockDomainと保存codec |
| [config.rs](../../../src/runtime/config.rs) | YAMLとtyped patchの統合、相対path、未知キー・値域・容量・plugin設定の検証 |
| [host.rs](../../../src/runtime/host.rs) | winitのwindow/input host、OS時計からtickへの割当、終了処理 |
| [runtime_scenes.rs](../../../examples/runtime_scenes.rs) | 作者APIでWorld登録、タイトルのEnter入力、Scene遷移、3tick後の正常終了 |

依存追加・更新はない。winit、serde、serde_yamlは既存依存を使う。E01のエラー型へRuntime用codeと原因詳細を追加し、StateStoreの型検証済みencodeをcommand提出へ接続した。既存の保存file形式は変更しない。新Runtimeの入力recordは明示tag・little-endianを使い、Rustのメモリ配置を保存しない。

## 実行と所有の契約

1. 既定値→YAML→typed patchを統合し、設定file基準でasset/storageの相対pathを解決する。未知キーや指定fileの読込失敗を無視しない。1MiBを超える設定を拒否する。
2. pluginの依存順でconfigure/build、全登録後にreadyを実行する。service slotは排他的。依存不足・循環・重複・未提供backend・必須音響の欠落は起動失敗とし、準備済みpluginを逆順で解放する。
3. SceneはEngineへ所有権を渡す。load/startは新scopeで一度実行し、pause/resumeはstartを呼び直さない。stop/removeで入力登録と解除処理を回収する。古いActionIdは世代検証に失敗する。Worldの寿命はSceneから独立する。
4. tickの入力を検証してから取り込む。予約入力は成功まで消さない。入力を`source, sequence`順へ正規化し、重複、逆送、NaN、不正な型を拒否する。割当結果は`last_input`から記録できる。
5. World候補を作り、BeforeScenes→Scene更新一回→残りのphaseを実行する。Scene Viewは前回の公開状態、System Viewは先行barrierまでの候補。phase境界と明示barrierで提出commandを候補へ反映する。同phaseの競合は明示依存を要求する。
6. SameTickではcursorと候補を保持し、件数予算の次pollへ継続する。進捗しないSystemを永続pollしないよう、同tick継続には`same_tick_limit`の決定的な診断上限を設ける。Sceneや入力を再実行せず、表示は最後の公開Worldを読む。Deferredの継続stateは作者がWorldへ保存する。
7. 全Systemと遷移が成功した時だけWorldとtickを公開する。System失敗、書込失敗の無視、遷移先start失敗では候補を公開せず、Runtimeを停止してcleanupを試行する。作者のcallback内部や外部資源を逆実行できるとは保証しない。正規stateはWorldへ置く。
8. 整数nanosecond×step_hzで時間を累積し、未処理のbacklogは捨てない。frame回数は固定更新数を変えない。World pauseとScene pauseは独立する。ClockDomainは分数余りを保持し、作者RandomSourceは登録済みstateとして独立streamを使える。

初期実装は安全性を確認できるscalar経路として、tick候補とbarrierでStateStoreを複製する。大規模Worldの性能認定は未実施で、E04/E13のページ共有・batch・負荷試験へ接続する。安価な即時setterとは扱わない。

## 試験とレビュー

[tests/runtime.rs](../../../tests/runtime.rs)の24件は公開APIを通す。

- タイトルからゲームへ遷移し、Worldが残る。pause中のrenderとWorld時計、resume時のstart回数を確認。
- scope容量1での停止・再開始、旧ActionId拒否、解除処理の実行。
- work_budget=1でSameTickを複数回再開し、候補が途中公開されずScene更新が一度だけであること。
- System失敗と遷移先start失敗でWorldが巻き戻ること。
- 未宣言アクセス、依存循環、競合する読書集合、service能力不一致の拒否。
- 30/60/144Hzで固定tick数が一致し、予定入力の押下tick1・解放tick6で同じWorld結果になること。
- backlog保持、button repeat、複数binding、同tick押下解放、focus reset、modalの実際の起動順。
- 設定の未知キー拒否と兄弟項目を保つpatch。plugin順序と失敗時の逆順cleanup。
- 時計の分数速度、停止、codec復元。入力codecの往復と全切断位置の拒否。
- 不正入力を再提出しても予約を失わないこと。無視した書込エラーで先行書込が確定しないこと。
- BeforeScenesの順序と一回実行、ScheduleProfileの順序、Deferredの正規進捗、進捗しないSameTickの診断停止。

レビューで見つけて修正した事項：

| 問題 | 修正と証拠 |
| --- | --- |
| 到着入力を単純に次処理tickへ入れるとcatch-up時の割当が変わる | 対象tickへの予約、OS到着時刻の割当、30/60/144Hz比較 |
| 不正入力で予約を先に消しRuntimeを停止する | 検証・取込成功後に予約を消去し、再提出試験 |
| 作者が書込エラーを無視すると部分成功する | command bufferに最初の失敗を保持、tick全体を公開拒否 |
| Scene登録順でmodal入力が変わる | 実際のlaunch順を保持して入力・描画・逆順停止へ使用 |
| 遷移先のstart失敗時にWorldだけ確定する | 公開候補を保留し、遷移失敗時は前Worldとtickへ復帰 |
| 無限SameTickが停止せず未完了tickを保持し続ける | poll予算と独立した継続上限、上限到達時のWorld非公開を検証 |
| 同phaseの競合をキー順で隠す | 推移的な依存を確認し、未順序の競合を起動時拒否 |

## 実行結果（2026-10-15、Windows x86_64 MSVC）

| コマンド | 結果 |
| --- | --- |
| `cargo test --locked --target-dir target/e01-validation -j 2 --quiet` | 全155件成功：既存112、E01 19、E02 24。doc testも失敗なし |
| `cargo check --all-targets --locked --target-dir target/e01-validation -j 1` | 成功 |
| `cargo clippy --all-targets --locked --target-dir target/e01-validation -j 1 -- -D warnings` | 成功、警告なし |
| `cargo fmt --all -- --check` | 成功 |
| `cargo doc --lib --no-deps --locked --target-dir target/e01-validation -j 1` | 成功 |
| `cargo run --locked --example runtime_scenes --target-dir target/e01-validation -j 1` | タイトル→ゲーム→3tick後の終了を実行して成功 |
| 同コマンドの`-- --window-smoke` | 実際のOSウィンドウを作成し、固定更新3tick後に終了、exit code 0 |

初回の通常target検査では既存cacheのrmeta不整合が発生し、専用targetでの再構築中にはメモリ確保失敗も発生した。その実行を成功として数えず、ビルドを直列化して上記を再実行した。テスト専用の出力先は公開APIや依存の変更ではない。

## 後工程との境界

E03が資産VFS、PNG等のdecode、非同期完了の反映、実際のGPU/音響adapterを接続する。E02のLoadContextは設定とserviceの同期準備だけを行い、空のAssetLoaderを成功として返さない。Headlessでrender providerがない場合、画像を描画したとは扱わない。

gamepad/custom入力は型付きevent注入に対応し、実機gamepad driver、再割当profile永続化、IME/UI/音響機器復帰はE09で統合する。Native hostのキー・mouse・pointer・focusの経路を実装したが、対話的な全機器操作、全monitor、macOS/Linuxの実行認定は未実施。

E04でWorld/chunkとRuntime全体の保存・再開をつなぐ。現在のWorld record/入力/ClockDomain/PRNGのcodec検証を、未完了tick・Scene全体のディスク復元の保証へ拡張解釈しない。全body/material等のproviderは担当工程で実装し、serviceを登録できるだけで物理処理が完成したと扱わない。

## コミット対象の分離検査

コミット予定のindexから`git write-tree`と`git archive`で`target/e02-review-tree`へ別の作業コピーを展開した。旧M3等の既存未コミット変更を含めず、E01/E02と新設計だけで検査する。こちらの既存テストは101件であるため、作業ツリーの155件と分けて数える。

- 分離したコミット対象：既存101 + E01 19 + E02 24 = **144件**。
- 検査は元の作業ツリーを書き戻さず、依存cacheだけ共用する。基準revisionと差分の関係を保ち、他の作業をコミットへ混入させない。

分離したコミット対象について、144件のテストに加え、全targetのcheck、警告拒否Clippy、fmt、Rustdoc、headless作例、OS window smokeも成功した。作業コピー切替時には同じpackageの古いmetadataが使われる場合があったため、元の作業ツリーの最終検査は`cargo clean --package charon-engine --target-dir target/e01-validation`でこのpackageの生成物だけを消して再構築した。依存クレートやソースは削除していない。

## PR #1のレビュー指摘対応

- [4281991638](https://github.com/sofia-gros/charon-engine/pull/1#discussion_r4281991638)：phase末尾がWorld pauseまたは非Running Sceneでスキップされても、共通cursor前進処理でcommandを反映します。poll予算1/64の両方で次phaseが更新後の値を読む回帰テストを追加しました。
- [4281991653](https://github.com/sofia-gros/charon-engine/pull/1#discussion_r4281991653)：`in_world`は活動状態の所属でありアクセス先を強制しないため、異なるWorld所属を競合除外に使う処理を削除しました。World横断の操作は維持し、同じキーの競合には依存とbarrierを明示します。
- [4281991663](https://github.com/sofia-gros/charon-engine/pull/1#discussion_r4281991663)：configure成功後、build開始直前から解放対象に含めます。設定拒否したpluginのbuild/shutdownが呼ばれず、先行pluginが解放されることを検査しました。既存の部分build失敗時の逆順解放も継続して検査します。

公開Runtimeテストは24件から27件になり、27件が成功しました。今回のE02ブランチで、全147件のテスト、全target check、警告拒否Clippy、整形、Rustdocが成功しました。新しい依存や保存形式の変更はありません。
