# 公開Rust APIの契約

## 分かりやすさと内部設計

通常の作者はScene、ビルダー、型付きハンドル、短いメソッドを使う。大量処理が必要な作者にはbatch、予約、query、Job、アクセス宣言を公開する。内部で全世界のコピーやthread同期を行う処理を、安価なsetterに見せない。

簡単なAPIと高速なAPIは同じ状態と検証を使う。別の実装を二重に保守しない。上級APIには費用、実行条件、所有権、失敗時の状態を説明する。エンジンの正しさと性能を保つための複雑さは許容し、説明不足を許容しない。

以下のシグネチャはv1へ実装する公開境界の基準で、現在のcrateに存在するという意味ではない。関数を追加・変更する場合も、この意味と到達要件を保つ。

## 統一する戻り値

`EngineResult<T>`は`Result<T, EngineError>`。エラーはcode、stage、対象key/ID、field path、原因、必要量と利用可能量、再試行可否を持つ。作者入力に対してpanicしない。

主なcodeはInvalidValue、UnknownKey、WrongAssetType、StaleHandle、WrongWorld、NotReady、Unavailable、CapacityExceeded、Conflict、UnsupportedCapability、SchemaMismatch、IO、DeviceLost、PluginFailure。messageだけを解析しなくても分岐できるようにする。

操作提出は`CommandId`、確定は`CommandReceipt`を返す。非同期ロードや大規模作業はRequestId/JobIdと状態を返し、`Result<()>`だけで完了したように見せない。

## Engineとプラグイン

| API | 意味 |
| --- | --- |
| `Engine::builder()` | 未起動の構成を作る |
| `.config_file(path)` | 設定の読込元を指定。未指定は型付き既定値 |
| `.settings(EngineSettingsPatch)` | 明示項目だけを上書きする |
| `.plugin(P: Plugin)` | 実装を所有して登録し、依存と設定schemaを解決する |
| `.scene(key, S: Scene)` | Sceneを所有して管理する |
| `.start_scene(key)` | 初期Sceneを指定する |
| `.build() -> EngineResult<Engine>` | 検証、能力解決、準備を行う |
| `Engine::run(self) -> EngineResult<()>` | OS event loopを実行する |
| `HeadlessRuntime::step(TickInput)` | 描画を伴わず同じWorldを進める |

Pluginは`build(&self, &mut AppBuilder)`、`ready(&self, &mut AppBuilder)`、終了時処理を持つ。buildで型・依存・loader・システムを登録し、readyで依存serviceを参照して共有定義を登録する。資産の検証と利用はE03でLoadContext内の型付きloaderへ接続し、未読込資産を準備済みとみなさない。循環依存は開始前に診断する。

## Sceneと時計

```rust
pub trait Scene: 'static {
    fn load(&mut self, _ctx: &mut LoadContext<'_>) -> EngineResult<()> {
        Ok(())
    }

    fn start(&mut self, _ctx: &mut SceneContext<'_>) -> EngineResult<()> {
        Ok(())
    }

    fn update(&mut self, _ctx: &mut SceneContext<'_>) -> EngineResult<()> {
        Ok(())
    }

    fn render(&mut self, _ctx: &mut FrameContext<'_>) -> EngineResult<()> {
        Ok(())
    }

    fn stop(&mut self, _ctx: &mut StopContext<'_>) -> EngineResult<()> {
        Ok(())
    }
}
```

Scene自身はmain threadに所属し、`Send + Sync`を一律に要求しない。並列systemは別の型契約を持つ。updateは固定tick、renderは可変frame。pause中はupdateを呼ばず、表示を残せる。resume時にstartを再実行しない。stop後の再startには新しいScene scopeを作る。

SceneManagerはstart、switch、launch、pause、resume、stop、removeを提供する。switchは現在scopeの終了と次scopeの開始、launchは並存を意味する。別Sceneへの直接の可変参照ではなく、型付きメッセージか共有resourceで連携する。

## Contextと操作の反映

SceneContextは資産、入力、WorldView、操作Facade、SceneManager、UI、確定イベントへアクセスする。内部のWorld配列へ無制限の`&mut`を返さない。

`ctx.worlds().get_mut(world)`は操作用のWorldAccessを返す。`.entities().get_mut(id)`はEntityEditを返し、setterはX2/X3で適用する検証付きcommandを作る。名称に`mut`があっても即座にセル配列を変更する意味ではないことを説明する。

Viewは最後の確定状態を読む。同じcallback内で提出済みcommandの結果を読む必要がある場合は、TransactionViewまたは明示した`flush_local`可能境界を使用する。通常のViewに未確定状態を混ぜない。

FrameContextは描画・UI・読み取りと、次tickへのintent提出を持つ。正規の物理setterを直接提供しない。StopContextはscopeの終了処理に必要な権限に絞る。

## 資産と寿命

AssetLoaderは画像、音、sprite sheet、font、shader、定義packを読み込む。登録は`AssetKey<T>`、利用は`Handle<T>`、保持は`AssetLease<T>`。Scene内のloadはSceneAssetScopeが保持leaseを持つ。

短いキーは作者packageのnamespaceを補って正規化する。`title-back`は`game:title-back`の省略として扱える。保存とmanifestには省略しない安定キーを使う。

型付きAssetRefはbuilder内で未解決のキーを保持できる。コンテンツ準備後に解決し、プレイ開始前に未知参照を診断する。lookupを毎cellのhot loopへ持ち込まない。

## マテリアルAPI

MaterialBuilderはbase、block_size、shape、texture、mass、friction、hardness、cohesion、thermal、behavior、break_mode、destroy_effects、property、instance_componentを提供する。物性や動作が独立している項目を一つのbooleanへまとめない。

buildは値と組み合わせを検証してMaterialDefを返す。registerはMaterialHandleを返し、content確定後にWorld用のMaterialIdへ解決する。共有定義の登録順によって作者が保持するハンドルが別素材へ変わらない。hot loopはWorldViewの解決済みIDを利用できる。

基本定義より作者の明示指定を優先する。同じ項目の複数指定は後の明示指定を採用する。複数挙動を合成する場合は、それぞれの読み書き範囲と出力の競合を検証する。

## Entityとcomponent

EntityBuilderはWorld、transform、表示、body、collider、componentを受け、spawnはEntityIdを返す。生成に失敗した場合はID・component・bodyの一部だけを残さない。

EntityBuilderの`size(width, height)`はcell単位の表示寸法とする。Bodyを要求してcollider形状を省略した場合は、生成時の寸法から矩形を作る。生成後の表示寸法変更でcolliderを暗黙に変更せず、形状変更は専用commandで検証・予約する。sprite sheetのframe寸法は画像pixel単位の資産情報として別に扱う。

作者componentは`Component`、永続化するものは`StateCodec`、通信するものは`Replicate`へ登録する。derive macroは安定キーとschema versionを要求し、内部参照を自動で保存可能と見なさない。

queryは型付きの読み取りViewを基本とし、大量更新には`SystemContext`のbatch queryを使う。構造変更はCommandsへ提出する。unsafeなaliasを作者が解決しないと通常のゲームを作れないAPIにしない。

## 入力

ActionBuilderはbutton/axis/pointerの型、keyboard、gamepad、local player、deadzone等を持つ。registerはActionIdを返す。`held`、`just_pressed`、`just_released`、`axis`、`pointer`を型ごとに提供する。

ActionIdはSceneの入力scopeに所属する。共有するaction mapはEngineのInputProfileへ登録し、Sceneごとに有効化する。再割り当てはprofileのpatchとして保存し、プレイヤー速度などのゲーム規則はprofileに含めない。

## イベント、フック、独自システム

eventは型付きpayload、EventId、source、tickを持つ。ハンドラーにはEventContextを渡し、scopeが所有する。識別子を捨てただけでは購読を解除しない。明示unsubscribeかscope終了で解除する。

通常のcallbackは`'static`の所有データやIDをcaptureする。正規の継続状態は登録済みstoreへ置く。独自イベントにもcodecとreplicationの必要性を宣言する。

フックは`Continue`、`Modify`、`Replace`、`Reject`などの型付き結果を定義し、優先順位と競合規則を持つ。異なるフックの戻り値を共通の曖昧なbooleanへしない。

Systemはphase、dependencies、read/write set、state key、仕事量予算を登録する。ParallelSystemはSend/Syncとアクセス宣言を検証する。宣言に違反する書き込みは実行前または検証profileで診断する。

component以外のGridChannel/StateStore、候補へのWriteBatch、barrier、SameTick queue、service slotの置換を公開する。[25](25_OPEN_EXTENSION_AND_COLLISION_CONTRACTS.md)がその能力と意味を定める。`Replace`という戻り値があるだけでは、物理や素材solverを置換できたことにしない。標準providerを外しても読込・衝突・更新・保存・描画・通信が接続されることを検証する。

## 大量操作API

空間と活動には、座標からchunkを取得するquery、region coverage、camera visibility、activity lease、非同期navigation jobを提供する。各状態、待機理由、保存とscopeの契約は[24](24_AI_NAVIGATION_AND_SPATIAL_ACTIVITY.md)に従う。

Worldのbrush、prefab、爆発、region変換は`estimate -> reserve -> prepare -> commit`を明示できる。小さい操作は便利メソッドが同じ仕組みを包む。容量を超える操作はJobHandleで進捗、待機理由、取り消し条件を返す。

大きなbufferのコピー、Worldとの同期、GPU readbackが発生するAPIには資料へ費用を記載する。上級APIを隠すために性能を犠牲にせず、通常例と性能上の理由を伴う例の両方を示す。

## 説明と安定性

公開項目は、目的、単位、寿命、反映時点、失敗、容量、save/networkへの影響を日本語で記述する。型名だけを説明するコメントでは不十分とする。1.0の公開APIを破壊する変更はmajor、互換な追加はminor、修正はpatchとして管理する。

APIの使いやすさは、初見の作者が独自Scene・素材・操作・保存型を作る手順で検証する。所要時間、借用エラーの原因、説明が必要だった箇所を記録し、内部設計を壊さない改善へつなげる。

## E02で実装したRuntime境界

実装と実行結果は[evidence/E02](evidence/E02/README.md)を参照する。上記のv1全APIが実装されたことを意味しない。

- 公開入口は`charon_engine::runtime`。既存`core::engine::CharonEngine`の特定ゲーム用ループとは別の汎用入口で、依存を逆流させない。
- `Scene::load`は`LoadContext`を受け、現在は検証済み設定とserviceを参照する同期処理。資産loaderはE03でこのContextへ接続する。非同期asset完了待ちを成功値で代用しない。
- `SceneContext::worlds()`の`WorldAccess::view()`は最後の確定状態、`set(world, record, schema, &value)`はcodec検証済みcommand提出。書込失敗を無視してもそのtickのWorld候補は公開しない。Sceneの通常Viewへ未確定値を混ぜない。
- `SystemContext::worlds()`は先行barrierまでの候補を読み、宣言したrecordだけを書ける。`BeforeScenes`、`Intent`、`Transactions`、`Physics`、`Materials`、`Reactions`、`Resolve`のphase内で`after`依存を解決する。同phaseの競合に明示した依存がなければ起動を拒否する。Scene更新はBeforeScenesの完了後に一度だけ実行する。
- `SystemDescriptor::barrier()`はそのSystemの書込を次のSystemへ見せる。phase境界にもbarrierがある。`ContinueSameTick`はcursorを保持し、`Engine::resume()`で同tickを継続する。`same_tick_limit`はpoll予算と独立した診断上限を設定し、無限継続を成功扱いせず停止する。`Deferred`はこのtickを完了でき、保存する続きは作者のStateStoreへ置く。
- `schedule_profile()`は解決済み順序・権限・barrierの確認用。`ServiceRegistry::manifest()`の実装キーと合わせて実行構成を記録する。実行中のprofile差し替えとRuntime全体の保存復元はE02で保証せず、E04以降の保存接続で扱う。
- `TickInput`は対象tick・source・sequenceを持つ。`queue_input_at`で予約し、取り込んだ正規化入力を`last_input`から取得して`StateCodec`で記録できる。不正な外部入力は予約を失わず再提出できる。OSへの到着が遅れた入力は既に始めたtickを変えず、次の未開始tickへ割り当て、その結果を記録する。
- `frame`は公開Worldを読むだけ。pauseはScene updateを止め、renderとscopeを残す。World pauseは独立し、Worldに結び付けたSystemと時計だけを止める。
- `ClockDomain`は分数速度・余り・pauseをcodecで保存できる。`RandomSource`と作者の乱数stateはE01のStateStoreに格納し、Systemの順序で消費する。壁時計から乱数を作らない。
- callback内部で変更した作者structや外部資源そのものを巻き戻すことはできない。正規stateはWorldへ置く。callback失敗時はWorldを公開せずRuntimeを停止し、scopeとpluginのcleanupを試行する。

## E03で接続した資産と表示

[E03の具体的な契約と利用例](28_ASSET_AND_BASIC_RENDERING.md)は実装済みAPIを説明する。`LoadContext::assets`でscope付きload、`FrameContext::draw/draw_with`でsprite抽出、`FrameContext::loader/assets`で表示用の非同期要求と明示公開を行う。

`AssetHandle<T>`は保持せず、`AssetLease<T>`とScene scopeと提出済みframeが保持する。世代の差し替えは旧handleを新内容へ向け直さない。独自形式は`Asset`/`Importer<T>`、描画の置換は`RenderAdapter`を使い、CPUのsnapshotを独自hostへ取り出すこともできる。資産と描画から正規Worldへの可変参照は返さない。

E02時点で記載したLoadContextの設定・serviceだけという制約は、E03でasset loaderを接続したことで解消した。資産と基本表示以外のv1 APIを実装済みと扱わない。
