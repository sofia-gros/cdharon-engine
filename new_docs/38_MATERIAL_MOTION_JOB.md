# E05：保存再開できる広域物質DDA Job

## 目的と範囲

通常の `MaterialMotionConfig::max_distance` の局所的な高速移動とは分けて、1cellの物質トークンが多数のchunkにわたる場合に、検査の仕事量を固定budgetずつ進める。部分経路をWorldへ公開せず、ReadyになったJobだけを全channel追従の原子的な輸送として確定する。

`MaterialMotionJob::new` はWorld、layer、始点、`MaterialMotion` の物質channel、軸移動距離上限、経路cell容量、活動chunk数上限を受ける。始点はactiveなresident非空cellであり、block個体は対象外とする。入力のWorldId・Content fingerprint・material epoch・所有layer・始点素材・速度、活動chunkのrevisionを採取し、途中で入力が変わったときは無効Jobとして失敗させる。

## 仕事単位と保存

`step(world, budget)` は一予算あたり一格子境界を検査する。整数DDAは既存速度輸送と同じ交差比較を使い、斜めの同時交差ではX・Y・斜め先をすべて検査する。障害物へ当たったら手前の空きcellまでの経路をReadyにして速度を既定値へ戻す。World端の手前でも同様。未読込・非活動域に当たったときは移動できた部分だけを候補とし、未実行の距離を負債化しない。

途中の状態は `StateCodec` の `CHMMJOB1` で表現する。作者は `Schema::of::<MaterialMotionJob>(...)` をContentに登録し、World StateStoreへ保存する。Jobには二軸の進捗、残差を含む移動状態、全経路、完了理由、検査件数、入力revisionを保持する。再開しても同じ保存した進捗から次の境界を調べる。保存した経路の重複・非隣接・進捗不整合、未来epochやWorld・Contentの不一致を拒否する。

## 全量の確定と費用

`publish(&mut WorldEdit)` はReadyのみを受理し、当初のWorld入力が変わっていないか再確認してから、全経路を逆順にして既存の `transport_path` に渡す。この既存APIがCellCoreとlayerへ登録した全material channelを一回のWorld編集候補で動かす。失敗時はWorldの変更をrollbackする。

保存形式のbyte容量に加え、活動chunkの入力snapshot数、検査経路最大cell数、公開時の `path.len() * (material channel数 + 1)` に相当するWorld編集予算を消費する。最終確定は一括なので、最終publish自体のCPU/メモリが1 stepの予算内に収まるとは主張しない。これを分割公開すると経路途中で物質を複製・欠損するため、原子性を優先する。

## 完成判定と残作業

復元データは保存経路の全ステップを開始位置から読み直し、各一歩が元の整数DDA交差順に一致するか、step件数と終了理由が整合するかを検査する。不正な経路が復元できてもpublishで必ず再検証する。一方、毎stepで全経路を再走査すると固定budget=1でも二乗時間になるため、経路の構造検証はnew/decodeと最終publishの境界に限定し、step中はWorldのchunk revision等の失効だけを検査する。

これは**単一の非blockトークンの広域DDA**。同tick複数候補の競合を同時解決する汎用高速solverや、圧力由来の多物質連成、局所候補の順序予約、dirty/sleep、Runtimeの標準Scheduleを代替しない。複数Jobのpublishでは、一方が先にWorld revisionを更新すると他方は失効して作者が新しく入力を採取する。

`tests/material_motion_job.rs` はbudget=1・多chunk・途中StateStore保存/World復元、通常速度DDAとの最終一致、障害物での部分移動、容量上限・World入力変更の原子的拒否を検査する。変更後の全ターゲットcheck、警告拒否Clippy、全OS CI、レビュー結果はE05 evidenceへ追記する。**この一実装単位だけでE05/Q04は完了しない**。
