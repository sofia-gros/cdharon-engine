# Rustでゲームを作る流れ

## 利用例の位置付け

この文書は、完成時に作者へ提供するRust APIの利用例です。ここに登場する型とメソッドは現在のSDKに実装済みという意味ではありません。APIの契約は[16](16_PUBLIC_API_CONTRACTS.md)を基準とし、例では周辺の登録やimportを省略しています。

作者はSceneや独自処理をRustの構造体とトレイトで作り、生成時の設定をビルダーのメソッドチェーンで記述します。実行中は入力を読み、オブジェクトや世界のメソッドを呼び出します。

## 1. エンジンとゲームを起動する

プロジェクトにはCargoの設定、エンジンの実行設定、Rustコード、画像や音を置きます。以下は制作を説明するための構成例です。

```text
my_game/
  Cargo.toml
  conf.yml
  assets/
    ui/
    textures/
    sounds/
  src/
    main.rs
    scenes/
    materials/
    systems/
```

Cargoで必要なエンジンと拡張の実装をビルドへ含め、YAMLで実行環境を設定し、RustコードでSceneとプラグインを登録します。設定の分類は[設定と拡張機能](05_ENGINE_CONFIGURATION.md)で説明します。

```rust
fn main() -> EngineResult<()> {
    // 設定ファイルの検証と、作者が登録した機能の準備を完了してから実行します。
    Engine::builder()
        .config_file("conf.yml")
        .plugin(SandboxPlugin::default())
        .scene("title", TitleScene::default())
        .scene("game", GameScene::new())
        .start_scene("title")
        .build()?
        .run()
}
```

この例の`GameScene`は作者が別途定義するSceneです。`Engine`、Scene、プラグインなどの型はエンジンの公開APIとして提供する想定です。上記は起動構成を示す部分であり、既存プロジェクトへ貼り付けて実行できる完成コードではありません。

`.scene(...)`は作者のSceneをエンジンへ渡し、`.plugin(...)`は拡張を導入します。Cargoに依存を追加したことと、ゲームで拡張を実際に使用することを、別の操作として扱います。

## 2. Sceneトレイトを実装する

Rustでは、作者の状態を構造体へ保持し、Sceneに必要な処理をトレイトのメソッドとして実装します。エンジンはそれらを管理し、必要なタイミングで呼び出します。

```rust
#[derive(Default)]
struct TitleScene;

impl Scene for TitleScene {
    fn load(&mut self, ctx: &mut LoadContext<'_>) -> EngineResult<()> {
        // 資産loaderの接続はE03の設計例です。E02の実行例はruntime_scenes.rsを参照してください。
        let assets = ctx.assets();
        // パスはYAMLで指定した資産ルートを基準に解決します。
        assets.image("title-back", "ui/title-back.png")?;
        assets.image("start-button", "ui/start-button.png")?;
        Ok(())
    }

    fn start(&mut self, ctx: &mut SceneContext) -> EngineResult<()> {
        // キーは読み込み境界で解決し、以降は画像用のハンドルを使います。
        let background_texture = ctx.assets().texture("title-back")?;
        let button_texture = ctx.assets().texture("start-button")?;
        let viewport = ctx.viewport().size();

        ctx.add()
            .image(background_texture)
            .size(viewport.width, viewport.height)
            .spawn()?;

        let button = ctx.add()
            .image(button_texture)
            .interactive(true)
            .spawn()?;

        // ハンドラーへは呼び出し時のContextが渡されます。
        // Scene自身への参照を保存せず、Sceneの終了時に購読も解除します。
        ctx.events().on_click(button, |_event, ctx| {
            ctx.scenes().start("game")
        })?;

        Ok(())
    }
}
```

`&mut self`は、自分のSceneの状態をこの処理中に変更できることを示します。`&mut SceneContext`は、この呼び出しの間だけエンジンの機能を借りて使う入口です。ContextをSceneへ保存せず、後で必要になる対象の識別子やハンドルを保存します。

`load`の読み込み要求が完了してから`start`へ進みます。読み込み失敗はScene開始前に扱い、診断と作者のエラー処理へつなぎます。ゲームの固定更新は`update`、表示frameは`render`、終了処理は`stop`で扱います。pause/resumeとscopeの契約は[16](16_PUBLIC_API_CONTRACTS.md)に従います。

## 3. マテリアルをビルダーで定義する

マテリアルは、基本定義、寸法、画像、動作、効果を型付きのメソッドで組み立てます。次は、読み込み済みの画像と音のハンドルを使う定義登録処理の一部です。

```rust
let soil = Material::builder("game:soil")
    .base(BasicMaterial::Solid)
    .block_size(16, 16)
    .texture(soil_texture)
    .behavior(Powder::default())
    .destroy_effects(
        DestroyEffects::builder()
            .sound(break_sound)
            .build()?,
    )
    .build()?;

let soil_id = materials.register(soil)?;
```

基本プリセットはSolid、Powder、Liquid、Gasを基準とします。`.behavior(...)`へ作者の独自実装を渡す入口も設け、基本動作を選ぶ書き方と同じ仕組みへ接続します。`soil_id`は準備段階のMaterialHandleで、確定したWorldのhot配列ではMaterialIdへ解決します。

`base`は既定値の出発点、各メソッドは作者の明示指定です。`build()`で組み合わせと値を検証し、`register()`で識別子や参照を登録します。基本定義と明示指定の優先順位は、メソッドの呼び出し順に依存させない方針です。

ブロックサイズは整数cellの占有寸法です。[世界設計](09_WORLD_AND_STREAMING.md)に従い、画像の解像度とは分けます。粒子化の方式と移動の方式など、異なる意味の設定は別の型とメソッドに整理します。

共有するマテリアル定義は、世界の作成前にコンテンツの初期化として登録します。Sceneの再開始で同じ共有定義を繰り返し登録する構成を避け、共有レジストリとSceneからの利用を分けます。

## 4. プレイヤーなどのオブジェクトを作る

作者は必要な画像と物理の構成を指定し、オブジェクトを生成します。次は、利用する世界のContextを得た後の生成処理を示します。

```rust
let player = world.entities()
    .sprite(player_texture)
    .position(32.0, 32.0)
    .size(16.0, 24.0)
    .body(Body::dynamic())
    .spawn()?;
```

`player`はエンジンの内部オブジェクトへの長期間の可変参照ではなく、後でその対象を取得する`EntityId`を想定します。作者はこの識別子と、所属する世界の`WorldId`をSceneに保持します。

`.size(...)`はWorld上の表示寸法をcellで指定します。衝突形状はBodyの設定で指定し、省略時はこの生成時の寸法から矩形を作ります。生成後の表示サイズ変更では衝突形状を暗黙に変更しません。画像を切り出すフレーム寸法は資産の設定です。

## 5. 入力と更新処理を書く

入力はアクションとして一度登録し、その識別子を保持します。次は、Scene開始時に行う登録例です。

```rust
let left = ctx.input()
    .action("move-left")
    .key(KeyCode::KeyA)
    .gamepad_button(GamepadButton::DPadLeft)
    .register()?;

let right = ctx.input()
    .action("move-right")
    .key(KeyCode::KeyD)
    .gamepad_button(GamepadButton::DPadRight)
    .register()?;
```

更新処理では、押下中なら`held`、押した瞬間なら`just_pressed`、離した瞬間なら`just_released`のように、取得する状態を明確に分けます。次は、作者の更新処理から呼ぶ移動処理の設計例です。

```rust
fn move_player(
    ctx: &mut SceneContext,
    world: WorldId,
    player: EntityId,
    left: ActionId,
    right: ActionId,
    speed: f64,
) -> EngineResult<()> {
    // 入力を読み終えてから、対象の世界とエンティティを一時的に借ります。
    let left_axis = if ctx.input().held(left) { 1.0 } else { 0.0 };
    let right_axis = if ctx.input().held(right) { 1.0 } else { 0.0 };
    let velocity = (right_axis - left_axis) * speed;

    ctx.worlds()
        .get_mut(world)?
        .entities()
        .get_mut(player)?
        .set_velocity_x(velocity)?;

    Ok(())
}
```

`speed`は作者のプレイヤー構成に置くcell/秒の値です。位置と速度は[データ設計](08_DATA_MODEL_AND_FORMATS.md)の`f64`を使います。速度変更は[公開API契約](16_PUBLIC_API_CONTRACTS.md)に従って更新境界で適用し、この呼び出し自体が物理を進めるわけではありません。

## 6. 独自機能、保存、配布へ進む

作者はトレイトを実装するプラグインやシステム、型付きのイベント、明示されたフックを使って独自機能を追加します。Sceneと同じ共通サービスへ接続し、必要な状態の登録と保存・復元も提供します。

ゲームコードと拡張はCargoでビルドし、実行設定と資産を配布物へまとめます。検証・インポート・パッケージ化と配布検査は[制作・配布仕様](17_TOOLING_AND_COMMERCIAL_DELIVERY.md)に従います。
