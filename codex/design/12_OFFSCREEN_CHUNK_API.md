# 画面外チャンクの読み込みAPIと制限

ユーザー指定により、**Engineの既定は画面外チャンク読み込みON**です。可視領域だけをロードする実装にはしません。C07/C09/C10に従い、作者が範囲・制限・活動方式を変更できます。

## 状態を区別する

| 状態 | 意味 |
| --- | --- |
| Visible | rendererの表示対象。simulationとは独立 |
| Prefetched | residentメモリに読み込み済み。まだsimulation対象でない場合がある |
| Active | simulation domain内。画面外でも物理とゲームsystemを更新 |
| Frozen | 保存された正規状態。実時間で進めない |
| Requested | worker準備中。まだLoadedとして公開しない |

activeは必ずresident。画面外をprefetchしただけで「全て物理更新」と説明しません。作者APIは要求modeを明示します。

## 既定policy（初期案）

| 項目 | Engine初期値 |
| --- | --- |
| `offscreen.enabled` | `true` |
| `active_margin_chunks` | 可視chunk矩形から全方向1chunk追加 |
| `prefetch_margin_chunks` | 可視chunk矩形から全方向2chunk追加 |
| `max_active_chunks` | 256 |
| `max_resident_chunks` | 512 |
| `max_pinned_chunks` | 128（resident上限の内数） |
| `max_chunk_requests` | 同時準備64 |
| `chunk_memory_budget_mib` | 256（cell、sidecar、scratch、metadata、準備/保存snapshotを含む） |

画面が640×360なら通常は余白込みでも上限内です。表示解像度変更・大きなviewport・作者設定は事前に必要数を計算し、足りなければ明示診断します。最大個数だけでbyte予算を保証せず、両方で容量を予約します。

これらはM2/M6で測定する初期値で、出荷確定値ではありません。作者は設定/APIで緩和できますが、無制限値を受理しません。座標・整数・メモリ安全限界は変更できません。OS割当失敗も通常エラーです。

## APIの責務

```rust
// 将来公開する責務案。1.0のsignatureはM7で凍結する。
request_chunks(region, LoadMode::Prefetch) -> Result<RequestId, ChunkError>
request_chunks(region, LoadMode::Simulate) -> Result<RequestId, ChunkError>
pin_chunks(region, mode) -> Result<ChunkLeaseId, ChunkError>
release_chunks(lease) -> Result<(), ChunkError>
set_streaming_policy(policy) -> Result<(), PolicyError>
query_chunk_status(region) -> ChunkStatus
```

`region`はchunk座標の半開矩形。可視範囲から遠く離れた鉱山・工場・ボスarena等も要求可能です。LoadMode::Simulateは画面外をactive domainへ追加します。複数leaseは参照数ではなく安定lease IDで管理し、最後のlease解放後に自動policyへ戻します。

request提出時に範囲・必要chunk数・byte予算を予約します。対象全域を準備してP0のactivation commandで一括公開します。準備失敗・capacity不足では元domainを保持し、partial activationしません。取り消し・失敗でも予約を解放します。

activation tickは提出時に決定してcommandへ記録し、workerの完了速度から決めません。byte予約は登録state schemaから求めた最悪サイズを使い、IO途中で勝手に正規上限を変えません。queued commandや準備要求の件数にも上限を設けます。

request/enqueue成功とactivation成功は別です。作者はRequestIdの確定eventを確認してからquery/commandします。Unavailableへ先に配置を送ると明示エラーです。メインsimulation threadでIOを同期待ちし続けず、runtimeを準備状態にして画面・キャンセル操作を受け付けます。

leaseは正規stateで、所有source、region、mode、generationを保存します。復元時に有効leaseを再現。Rust handleのDrop時刻で物理を変えず、明示release commandをtickに記録します。

## policy変更とeviction

creatorは自動margin、ロード有効/無効、固定区域、prefetch/simulate mode、上限、保持方式を変更できます。policyを変えるコマンドは正規設定/リプレイに含めます。offscreen.enabled=falseでも明示leaseは有効とし、自動追加だけを無効にします。

上限を現在のlease以下へ下げる操作は拒否し、作者に先行releaseを求めます。世界状態を勝手に破棄しません。自動domain変更も全体容量予約後にcommitし、既存active集合から外れるchunk・entity・正規粒子は同tick境界でfreezeします。

evictionはunpinnedかつinactiveのchunkのみ。使用順の決定にはtick last-useと座標tie-breakを使い、壁時計LRUを使いません。dirty chunkは永続化成功まで捨てません。IO失敗で余裕がなくなれば新要求を拒否または待機し、既存世界をempty化しません。

遠隔active領域同士が非連結でも更新可能です。間の未ロード区域はUnavailable境界なので、液体・粒子・entityは空間を飛び越えません。全世界を同時にactive化する場合、creatorは世界数・メモリ・CPU費用を負担する設定にし、同じ60 FPS保証を流用しません。

## 検証

V08ではデフォルトの画面外prefetch/active、遠隔lease、重複lease、明示release、再ロード保存一致、IO順序差、予約失敗、容量縮小拒否を確認します。V10では標準domainと遠隔activeを別scenarioにし、制限緩和時の実測値を公開します。
