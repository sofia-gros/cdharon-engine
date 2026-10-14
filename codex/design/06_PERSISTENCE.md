# 保存・復元・リプレイ

前提はC03/C06/C07/C10/C11です。保存は後付けせず、M1から状態列挙、M2からcheckpointを実装します。

## 状態の分類

| 種類 | 保存・hash対象 |
| --- | --- |
| 正規状態 | tick、seed、domain、全生成済みchunk、CellCore、energy/timer/損傷等sidecar |
| 正規ゲーム状態 | entity全component、inventory、item drop、正規粒子、敵/ボス状態 |
| 正規制御状態 | pending command、保存可能なjob進捗、allocator世代/free順、入力held/edge消費状態、画面外policy/leaseと準備要求 |
| 再現性metadata | simulation/schema/generator version、content hash、正規設定、登録辞書 |
| 派生cache | stamp、Dirty、光、衝突summary、描画texture。保存せず再構築 |
| 非正規状態 | 装飾粒子、音再生、壁時計、描画補間、UI外観。世界hashから除外 |

凍結chunk/entityも正規保存対象です。AI経路などをcacheに分類できるのは、再構築が次状態へ影響しないことをV06で確認した場合だけ。影響するなら正規状態として保存します。

material/instanceの正規propertyとattachmentはschema付きで保存対象に入れます。準備中chunk要求はID、region、mode、予約、目標activation tickを保存し、worker内部bufferは保存しません。復元後に同じ要求を再準備し、必要tickを待機させます。prepared未commitデータの有無が世界hashを変えることはありません。

state hashは安定codecによる辞書順の内容から計算し、chunk/entity等の局所hashと全体rootを持ちます。scratchの有無・pool容量の余白・メモリaddressを含めません。allocator free順は将来EntityIdを変えるため含めます。

## 保存形式

明示的なlittle-endian codecを使います。`repr(C)`のraw dump、Rust enumのメモリ表現、usize、padding、floatのNaN表現に依存しません。

manifestにはmagic、schema version、simulation version、generator version、tick、seed、world bounds、正規設定、content/asset hash、stable-name辞書、chunk/object一覧を持ちます。各objectは型、version、非圧縮サイズ、checksumを持ちます。圧縮は交換可能で、semanticなデータ形式と分離します。

codecは長さ・サイズ・座標・ID・timer整合性・重複参照を検証します。圧縮爆弾を避けるため、宣言サイズと実出力の両方に上限を設けます。checksumは破損検出であり、悪意ある改変に対する認証とは称しません。

現行実装（2026-10-15）: `World::encode_snapshot/decode_snapshot`はlittle-endianのschema/simulation/generator version、registry fingerprint、有限矩形、stored/active remote chunks、有限初期region active set、lease ID/source/region/mode/next ID、有限/remote thermal energy sidecar、material issued/consumed台帳、固定容量EntityId allocator、未来tick command queue、視覚粒子を保持し、未知material、未対応flags、範囲超過lease、非finite particle値、不正allocator/queue、trailing bytesを拒否します。snapshot schema v9ではremote chunkの各セルenergyも保存し、hashへ含めます。`GameSession`は別codecを持ち、tick一致とcontent参照を検証したbundle APIでWorld・game状態・Q16.16 Player状態を一体として候補復元します。bundle magicはPlayer収録に伴いv2です。`ReplayLog` v2はPlayer入力、現行WorldCommand（brushとlease/visible-domain操作）をtick/source/sequence順に再生し、失敗時はcandidateをpublishしません。World+Player canonical state hashは保存codecから視覚particleを除いたsimulation projectionとPlayer codecを連結するFNV-1a 64-bitで、演出粒子はReplay初期照合にも含みません。暗号学的hashではありません。`GenerationStore`はWorld snapshotまたはWorld+GameSession+Player bundleを最大64MiBのpayloadとして保存し、generation番号・length・FNV診断checksumを持つファイルをflush後に同一directory内renameで公開します。最新3世代を保持し、新しい世代の検証失敗時は旧世代を返します。temp作成、header後のpartial write、flush、rename前の失敗注入を追加し、旧世代が残ることを検証しました。checksumは認証ではなく、directory fsync、OS別power-loss保証、stable-name dictionary、seed/config、AI、material別熱容量・timer/instance attachment state、migrationは含みません。現行World snapshot schema versionは9、simulation versionは10で、互換migrationを提供しません。

## 世代保存

1. P8完了時点でimmutable snapshotを確保する。メモリ予算不足なら停止checkpointへ切り替えるか保存を明示拒否する。
2. 未変更objectは前世代のimmutable objectを参照し、変更objectを同一保存volume内の新世代へ書く。
3. 各objectを書き込み・flushし、checksumを照合する。
4. manifestをtempへ書きflushし、新世代の確定manifestとして公開する。
5. すべて検証済みの世代だけを列挙可能にする。最新有効世代を選び、少なくとも直前2世代も保持する。
6. 未参照objectのGCは成功世代公開後の別処理で行う。進行中snapshot/保存/読込が参照するobjectは消さない。

`GenerationStore`はheader+payloadを単一ファイルへ書いてfile `sync_all`後に同一directory renameし、checksumを検証してからloadします。write/rename失敗ではtempを消し旧ファイルを保持し、temp作成・partial write・flush・rename前のfault injection、payload上限、checksum破損の各試験で旧世代fallbackを確認します。これはobject/manifest方式ではありません。実OSのdisk-full、directory sync、電源断、filesystemごとのrename耐久性を保証するfault試験は未実施です。Windows・外部drive等に同じ原子性を仮定せず、出荷条件に必要な耐久性試験は未完了です。

workerが書いている間に世界の可変メモリを読むことは禁止です。Copy-on-write導入はM6で測定後に判断し、M2の簡単な停止snapshotを削除しません。

## ロード

別の候補世界へ全検証・移行・容量予約を行い、成功してから実行世界を差し替えます。失敗したロードで現世界や元セーブを書き換えません。

名前辞書でMaterialIdを再マップし、entity参照とsidecarを復元します。unknown material/content欠落は通常ロードを拒否します。ユーザーが選ぶ回復モードだけmissing obstacleへ置換し、元セーブを保持して別slotへ保存します。回復モードの結果は元状態と同じリプレイと見なしません。

復元時、正規tick/command/job/allocatorを維持し、派生cacheを無効化します。全domain起床で開始し、保存なしで走らせた比較世界と以後のtick hashが一致しなければ不合格です。

## 互換性

schema migrationは連続する明示変換を用意し、旧fixtureから試験します。simulation versionが違うセーブは、安全な状態変換で新versionへ再開できる場合がありますが、旧リプレイと同じ未来は保証しません。

1.0出荷後はサポートするセーブschema一覧を維持します。content変更でstable keyが消えた場合のmappingは移行定義に明示し、自動でemptyへ置換しません。出荷前のプロトタイプには保存形式がないため、未存在の旧セーブ互換を約束しません。

## リプレイと障害再現

初期checkpoint、正規metadata、tick別command/input、domain activation履歴、定期hashを記録します。workerのIO完了時刻は記録せず、確定activation tickを記録します。再生時は必要データを準備して同tickに取り込みます。

リプレイの一致はsimulation version/content hash/正規設定/toolchain targetの認定範囲内です。入力だけ保存してRNGやjob進捗を捨てる方式にしません。異常時は最後のcheckpointと直近commandを保存した再現bundleを出力します。ユーザーのパスや個人情報は診断に必要な範囲だけにします。
