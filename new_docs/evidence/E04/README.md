# E04：世界と保存基盤の実装記録

## 対象と状態

開始revisionは`91f173d`、実装ブランチは`codex/e04-world-and-persistence`です。E04の実装、出口レビュー、Windowsでの全体検査と分離tree検査を完了しました。初回実装の3 OS CIも成功しました。統合後の検証は末尾へ記録します。

[世界](../../09_WORLD_AND_STREAMING.md)、[データ](../../08_DATA_MODEL_AND_FORMATS.md)、[schema](../../23_CONTENT_AND_STATE_SCHEMAS.md)、[保存](../../14_PERSISTENCE_AND_REPLAY.md)、[活動域](../../24_AI_NAVIGATION_AND_SPATIAL_ACTIVITY.md)、[P001](../../25_OPEN_EXTENSION_AND_COLLISION_CONTRACTS.md)に基づきました。使用方法と実装契約は[29_WORLD_AND_PERSISTENCE.md](../../29_WORLD_AND_PERSISTENCE.md)を参照してください。

## 要件と証拠

| E04の出口 | 実装と検証 |
| --- | --- |
| Q02：大規模世界を部分生成 | i32 cell空間の疎な64×64 chunk。約43億cellの論理境界を設定しても初期配列0byte。作例で必要な5chunkだけ生成 |
| 多layer・素材寸法・個体 | 型付き素材builder、8byte CellCore、必要時sidecar、回転・跨chunk個体、部分/全体破壊、形状とanchor、独自block状態 |
| leaseと画面外活動 | LoadOnly/Simulate/Pinの和、容量失敗時の一括拒否、複数source、期限、camera観測と遠隔工場の独立性 |
| 生成と編集 | pass依存と安定順、絶対anchor seed、workerとtick公開の分離、不変baseと差分、生成版照合、取消・再開時の後着拒否 |
| Q09：保存・復帰 | scope別manifest、BLAKE3 blob、同期・不変世代公開、最低3有効世代、壊れたCURRENTから復旧、I/O全操作faultと部分write |
| residentの退避・再読込 | 保存再照合receipt、対象変更の拒否、跨chunk個体ID・component・channelの保持、RAM payload解放、空の別paging保存先への復元 |
| P001：別解像度・液量 | 有理数scaleと原点を持つ型付きchannel、負座標、部分量、chunk batch、地形coverageと独立byte予算 |
| P001：素材remap/epoch | owner/layer別の台帳、resident・後から生成・退避復帰の同一履歴、独自owner公開経路、失敗時のepoch rollback |
| P001：独自geometry | 登録providerの版とrecord検証、保存した占有maskの厳密な復元。任意角度bodyはE07 |
| 大きな編集と継続 | 正規cell Job、cursor・予約・入力revision・候補pageの保存、入力変更でInvalidated、復元候補の再照合 |
| 作者schemaとRuntime | WorldとblockのStateStore、明示移行と内容同一性検査、native World登録・barrier編集・時計の原子的確定 |
| 依存cache | revision差分と取り逃し検出、復元後FullRefreshRequired、境界・未読込を保持する隣接query |

公開APIの検査は[tests/world.rs](../../../tests/world.rs)の41件、統合作例は[examples/sandbox_world.rs](../../../examples/sandbox_world.rs)です。作例では16×16・3×2・1×1素材、多layer、4chunkを跨ぐ家具、作者の保管数、半解像度の部分液量、camera解除後の遠隔lease、退避を含む完全保存と復帰を一つの経路で実行します。

## 検証

Windows x86_64 MSVC、Rust 1.98.1。依存cacheを再利用し、`--target-dir target/e01-validation -j 1`で直列実行しています。

- 公開APIテスト41件：成功。
- 作業tree全体の220件：成功。以前からの未コミット変更が含む11件と、新工程の検査を区別します。
- `cargo fmt --all -- --check`：成功。
- `cargo check --all-targets --locked`：成功。
- 警告拒否Clippy、Rustdoc、統合作例：成功。作例はresident 5chunk、地形配列655360byte、世代4を復帰しました。今回の変更だけをgit indexから切り出した分離treeでも、整形・全target check・209件のテスト・警告拒否Clippy・Rustdoc・統合作例が成功しました。
- [3 OSの世界・保存CI](../../../.github/workflows/e04-world.yml)を追加。未実行OSの成功は主張しません。

## レビューで修正した内容

- worker要求IDだけでは再開前の結果を拒否できないため、試行番号と元Content hashを保存・照合。
- 退避indexだけのsnapshotを完全保存と混同しないよう、全参照archiveをstreamでrootへ含め、別保存先で復元。
- 保存iteratorのpanicでもwriter lockを解放し、次回保存と直前の完全rootの読込を継続。
- schema移行が素材・layer・channel定義の変更を隠せないよう、物理定義の同一性を検証。
- 復元したJobのpageを入力とcursorから照合し、候補だけへの不正な編集混入を拒否。
- 生成baseと編集を分離し、単一cell編集は10byteの差分で保存。base保持分もterrain予算へ計上。
- 保存済みrevisionというIO付帯情報を正規Worldから除去。生成履歴・編集revisionを維持し、IO bookkeepingをhashへ混入させない。

## 依存、形式、制約

BLAKE3 1.xをcontent/blob照合へ追加しました。既存の資産packのSHA-256は維持します。標準ライブラリだけでBLAKE3を再実装する代替は保守・誤実装の費用が大きいため採用していません。

形式はCHWORLD1/CHREGN01/CHSAVE01/CHBLOB01の明示codecで、v1.0互換凍結前です。旧simulation形式の互換機能ではありません。「以前の保存」とは、この新基盤で直前に成功した完全保存を指します。

Windowsのfile同期とrename、Unixのdirectory同期を`SaveDurability`で区別します。単体・CIの成功だけでhardwareの電源断耐性を認定しません。媒体認定、長時間、性能最適化はE13の対象です。

COW pageに加え、独立候補の個体arena・管理辞書・全体検証にも費用があります。定数時間編集とは説明しません。圧縮方式noneを明示し、未知方式を拒否します。body、Entity、AI等の後工程のデータは各工程でこの保存・区域契約へ接続します。E04だけで完全クローンや商用v1を実証したとは扱いません。

開始時から存在する旧simulation/game等の変更は今回のcommitへ含めません。旧文書を実装仕様として使っていません。

## commit/push後のPR指摘対応と統合

E04の実装を`0b1adf1`でcommit/pushした後、ユーザー指定の順番でE02/E03のPR指摘に対応しました。

- PR #1：`d3afe3b`。phase末尾スキップ、World所属による競合除外、configure失敗前のplugin解放の3件を修正し、全147件と品質検査を通過してpushしました。
- PR #2：`f779707`で共通修正を取り込み、`58b649d`でpointerの物理→論理変換、余白/window外の無効化、resize再投影を修正。全174件と品質検査、実windowの120tick検査を通過してpushしました。
- E04：両方を`1ef310c`と`d6c2ced`へ取り込みました。旧実装の未コミット変更を含めないHEADの分離treeで、**215件**（既存・host 103、資産24、Core 19、Runtime 28、World 41）、整形、全target check、警告拒否Clippy、Rustdoc、sandbox_worldが全て成功しました。

E04初回実装`0b1adf1`のCI結果：

- [世界と保存：Windows・Ubuntu・macOS](https://github.com/sofia-gros/charon-engine/actions/runs/38753762911)：3 job成功。
- [Rust CI](https://github.com/sofia-gros/charon-engine/actions/runs/38753762530)：成功。
- [資産と参照描画の3 OS](https://github.com/sofia-gros/charon-engine/actions/runs/38753762515)：成功。
- [PR #1修正のRust CI](https://github.com/sofia-gros/charon-engine/actions/runs/38754310939)：成功。

未実行・実行中のCIを成功へ数えず、ソースrevisionと結果の対応を保ちます。PRコメントへの投稿やPRのmergeは行っていません。
