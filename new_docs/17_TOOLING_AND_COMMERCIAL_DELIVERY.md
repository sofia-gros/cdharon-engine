# 制作ツール、配布、商用利用の基盤

## v1で配布するもの

Rust SDK、API日本語資料、CLI、資産importer、デバッグinspector、map/prefabの編集・検査、検証作品、サーバー、packager、移行・復旧ツールを含める。code-firstの制作を基本とし、エンジン内部の編集なしでチームが作業できる状態を提供する。

独自の画像やフォント、音を読み、mapやprefabをテキスト形式または制作ツールで編集し、Gitへ保存できる。作例だけが使える非公開の編集機能を作らない。

## CLIの契約

コマンド名は実装時に確定するが、v1で次の操作を一つのCLIから実行できるようにする。

| 操作 | 出力と失敗時の扱い |
| --- | --- |
| new | Cargo project、設定、最小Scene、資産manifestを生成。既存ファイルを無断上書きしない |
| check | 設定、asset参照、定義、schema、依存、容量を検査し、file/fieldを報告 |
| import | source hashとimporter versionからcacheを生成。失敗時は旧cacheを保持 |
| run/server | profileと設定を選び、gameまたはheadless serverを起動 |
| inspect | save、content、World、asset依存、メモリ、job、network revisionを表示 |
| replay/verify | 再現データを再生し、最初の不一致tickとrecordを出力 |
| migrate/recover | dry-run、移行、別保存先への復旧、移行reportを生成 |
| package | 正確なtarget/profileで実行物、資産pack、設定、ライセンス情報を生成 |
| doctor | OS・GPU・driver・audio・toolchainの能力と認定profileとの差を表示 |

CI用の機械可読JSON出力、終了code、操作対象の明示を提供する。対話確認だけで自動実行できなくなるCLIにはしない。

## inspectorと制作補助

Scene tree、World/Entity/component、chunk state、熱・圧力・速度、body collider、navigation、light、input、資産参照、取引receiptを確認できるdebug UIを提供する。編集操作もcommandとして通し、保存・replayへ反映できる。

map/prefab編集は素材の配置寸法、layer、anchor、個体componentを扱い、実行と同じ検証を使う。imported imageからmaskやatlasを作る設定を保持し、アーティストがRustのソースを書き換えず資産を更新できる。

## チーム制作

資産の安定キーとmetaをsourceと一緒にGitへ保存し、cacheやplatform別packを再生成可能にする。ファイル移動時はキーを保持し、参照が壊れる変更を検出する。

定義schema、component版、importer版、generator版、save移行をレビュー対象へ含める。authoring dataとgenerated dataを区別し、生成物の手編集を正規の制作手順にしない。

## 配布と依存

現在のリポジトリにあるLICENSEはMIT表記である。エンジン、追加依存、作例コード、各資産のライセンスを混同せず、配布物ごとのmanifest、著作権表示、必要なNOTICE、SBOMを生成する。依存を追加する際は用途、代替、feature、保守状態を記録する。

SDKの依存はlock済みの認定構成と最小feature構成をCIで確認する。利用者はゲーム側Cargo.lockで版を固定できる。実行物が必要とするnative library、shader、資産pack、設定をpackagerが確認し、開発PCだけにあるファイルへ依存する配布を拒否する。

原作の画像や音をSDK作例の必須資産へ使わず、再配布条件を明示した独自・許諾済み資産を用意する。作品固有の資産をエンジンのライセンスに自動的に含める扱いにはしない。

## データと拡張の信頼境界

資産、map、save、network payloadは外部入力として上限付きdecodeを行う。パストラバーサル、圧縮展開量、巨大画像、ID不正、無制限のeffect連鎖、無制限のjob数を検査する。

Rustプラグインは信頼するnative codeとしてビルドへ含める。データpackと同等の隔離があるとは説明しない。プラグインのpanicは更新候補の不公開と診断停止へつなぎ、内部状態が壊れたまま継続させない。

通信に秘密鍵やtokenを含める場合は、ログとcrash bundleへ平文で出力しない。診断artifactの内容一覧を公開し、作者が送信や共有を選べる。SDKが自動で外部へ診断を送ることを必須にしない。

## v1の保守

公開API、schema、認定環境、既知の制限、移行手順、リリースノートを版ごとに公開する。P0/P1不具合と保存破損・脆弱性の報告経路、再現bundleの作り方、修正の検証手順を用意する。

サポート期間と更新方針は1.0の出荷manifestへ明記する。少なくとも1.xの互換なpatchで旧1.x saveを読み、移行が必要なら自動上書き前にbackupを作る。具体的な有償サポートやサービス契約を、OSSの出荷条件に勝手に追加しない。

## 作者向け資料

導入、Rustの最小限の所有権説明、Scene、資産、素材、controller、効果、AI、装置、保存、通信、最適化、配布、障害復旧を順に学べる資料を作る。API資料とtutorialが同じ公開版で実行できることをCIで確認する。

借用やbatchのために複雑になる箇所は、何を誰が所有するか、いつ変更が確定するか、単純な書き方との費用差を説明する。理解しにくいAPIを説明なしで「Rustだから」と処理しない。
