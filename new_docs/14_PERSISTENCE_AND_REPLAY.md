# 保存、復元、移行、再現

## 保存の完全性

保存点はWorld tickのX7終了後とする。World、Entity、block個体、物質粒子、bodyとjoint、thermal/timer、装置、AI、effect、進行、allocator、未完了command、正規Job、lease、乱数の導出versionを含める。

GridChannel、StateStore、置換provider、状態付きPRNG、substep/clock、MaterialRemapTableも同じ保存・hash対象とする。PlayerProfile、WorldSave、AccountProgress、RunStateは別のPersistenceScopeにでき、関連する世代と参照をmanifestで追跡する。SameTickの候補を処理中なら、直前の確定tickを保存し、未完了候補を完了状態として書かない。必要な入力はjournalへ残し、再開時の再実行による二重出力を防ぐ。[拡張契約](25_OPEN_EXTENSION_AND_COLLISION_CONTRACTS.md)

Sceneの表示資源やコールバックをそのままserializeしない。作者が保存を指定したScene状態とWorld状態は、登録codecと安定キーで保存し、復元後に新しいハンドラーと表示を再接続する。

## ファイルとmanifest

saveは世代ごとに不変のchunk/blobとroot manifestを持つ。manifestにengine version、schema version、sim profile、content hash、generator version、plugin key/version、World設定、root tick、blob一覧、長さとchecksumを記録する。

blobの基本headerはmagic、record kind、schema major/minor、展開後長、圧縮方式、checksum。payloadはlittle-endianの明示codecと長さ付きrecordで構成する。BLAKE3でcontent/blob hashを作り、安定順の辞書でruntime IDを再マップする。

圧縮、実行時ID、メモリの並び、HashMapの順を保存の意味に含めない。作者recordはcomponent keyとversionを持ち、型の解決前にRustのmemory layoutへ復元しない。

## 書き込み手順

1. tick境界のSnapshotを取得する。基準実装は停止Snapshotを持ち、通常保存は参照count付きページのcopy-on-writeでworkerへ渡す。
2. 排他writer lockの下で中断した未公開temporary世代を安全に回収し、新世代用の容量を確保する。公開済み世代や未知fileは回収に含めない。dirty blobを同一保存volumeの新しいtemporary世代へ書き、長さとchecksumを検証する。
3. payloadをflush/syncし、必要なroot manifestを最後に書いてsyncする。
4. 世代をpublishし、CURRENT pointerを同一directoryで置き換える。対象OSのdirectory durability手順をadapterで実施する。
5. 新世代を検証できた後にのみ旧世代を整理し、最低3世代を保持する。

CURRENTだけを信じず、世代を列挙して最後の完全なrootを選べるようにする。ディスク不足、権限、部分write、rename失敗、OS中断時に旧有効世代が読めることをfault testする。ファイルシステムの保証と実測した保証をrelease記録へ分けて記載する。

## loadは候補から公開する

header、長さ、checksum、manifest、必要な型とplugin、ID辞書、参照、容量、トークン収支を順に検証する。すべてを候補Worldへ構築し、成功後に現在Worldと入れ替える。失敗した候補を稼働中Worldへ混ぜない。

未知の必須pluginや必須型は明示エラーとする。作者が定めた回復移行がある場合だけmissing recordを扱い、未知物質を空気へ変換しない。回復結果は別の保存先へ書き、元のsaveを保持する。

## 版と移行

v1ではengine API、save schema、sim profile、content、generatorの版を別々に管理する。patchは互換性を維持し、minorが追加する任意recordには旧readerの扱いを明示する。意味が変わる場合はmigrationと新しいprofileを必要とする。

component、素材、item、effect、generatorには安定キーのrename/remapとversion migrationを登録する。移行は旧入力を読んで新候補を生成し、検証後に新世代として公開する。元データへin-placeで書き換えない。

backend物理recordの変更は、backend版の保持、正式な変換、検証済み再構築のいずれかを用意する。未完了の動作や接触状態を失う移行を、完全な再開と同じ表示にしない。

## replayとhash

replayは初期Snapshot、tick入力、操作順、domain変更、外部workerの採用結果、content/profile情報を持つ。Worldを変える外部IOやnetwork結果は、採用したtickとpayload hashまたはpayloadを記録する。

正規hashは、安定キーへ正規化した定義と、座標・型・個体IDの安定順にした全正規状態から計算する。表示粒子、audio voice、GPU cache、diagnostic timestampを含めない。物理backend stateは認定profile内の正規recordとして含める。

checkpointを復元して継続したhashと、保存せず進めたhashを照合する。異なる描画頻度、worker数、IO遅延でも同じprofileの結果が一致することを検証する。認定外profileのhash不一致を自動的にsave破損とは判断しない。

## 作者の保存API

`StateCodec`にstable key、schema version、encode、bounded decode、validate、migrateを登録する。Entity参照、資産参照、独自Jobの再接続はID辞書を通す。クロージャやOS handleの保存を作者に要求しない。

保存進行、失敗理由、必要容量、保持世代、移行の結果をquery/eventで取得できるようにする。自動保存と手動保存を同じ検証・世代方式へ接続する。
