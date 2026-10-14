//! E02の実行例。Sceneは入力を解釈し、Worldの所有はEngineへ残します。
//!
//! `cargo run --example runtime_scenes`は入力を記録済みtickへ注入して自動終了します。
//! `-- --window`では実際のOS入力を使います。Enterで開始、3tick後に終了します。
//! 画像を表示する例ではありません。描画adapterと資産読込の接続はE03で扱います。

use charon_engine::{
    foundation::{
        EngineResult, StableKey,
        schema::{Schema, SchemaRegistry},
        state::{StateStore, StoreLimits},
    },
    runtime::{
        AppBuilder, Engine, Plugin, Scene, SceneContext, StepResult,
        input::{ActionId, InputBinding, InputEvent, InputValue, TickInput},
    },
};

fn key(value: &str) -> EngineResult<StableKey> {
    StableKey::parse(value)
}

struct GamePlugin;
impl Plugin for GamePlugin {
    fn key(&self) -> StableKey {
        StableKey::parse("sample:game").expect("固定された有効なキー")
    }
    fn build(&self, app: &mut AppBuilder) -> EngineResult<()> {
        let mut schemas = SchemaRegistry::default();
        schemas.register(Schema::of::<u64>(key("sample:counter")?, 1, 8)?)?;
        let mut state = StateStore::new(
            key("sample:world")?,
            schemas,
            StoreLimits {
                records: 16,
                payload_bytes: 128,
                checkpoint_bytes: 4096,
            },
        )?;
        let mut edit = state.transaction()?;
        edit.put(key("sample:elapsed")?, key("sample:counter")?, &0_u64)?;
        edit.commit()?;
        app.world(key("game:main")?, state)
    }
}

#[derive(Default)]
struct TitleScene {
    start: Option<ActionId>,
}
impl Scene for TitleScene {
    fn start(&mut self, ctx: &mut SceneContext<'_>) -> EngineResult<()> {
        // ActionIdはこの起動のscopeへ属します。停止時にEngineが登録を解除します。
        self.start = Some(ctx.input().action("start").key("enter").register()?);
        println!("タイトル開始：Enterでゲームへ進みます");
        Ok(())
    }
    fn update(&mut self, ctx: &mut SceneContext<'_>) -> EngineResult<()> {
        if let Some(action) = self.start
            && ctx.input().just_pressed(action)?
        {
            // 遷移はcallbackの途中で実行せず、tick確定境界で実行します。
            ctx.scenes().switch("game")?;
        }
        Ok(())
    }
}
struct GameScene;
impl Scene for GameScene {
    fn update(&mut self, ctx: &mut SceneContext<'_>) -> EngineResult<()> {
        let elapsed = {
            let mut worlds = ctx.worlds();
            let world = worlds.view().id("main")?;
            let elapsed: u64 = worlds.view().get(world, key("sample:elapsed")?)?;
            // Viewは公開済みの状態です。提出した書込はbarrierを経て一括公開します。
            worlds.set(
                world,
                key("sample:elapsed")?,
                key("sample:counter")?,
                &(elapsed + 1),
            )?;
            elapsed + 1
        };
        if elapsed == 3 {
            ctx.scenes().quit()?;
        }
        Ok(())
    }
}
fn main() -> EngineResult<()> {
    let window_smoke = std::env::args().any(|argument| argument == "--window-smoke");
    let mut engine = Engine::builder()
        .plugin(GamePlugin)
        .scene("title", TitleScene::default())
        .scene("game", GameScene)
        .start_scene(if window_smoke { "game" } else { "title" })
        .build()?;
    if window_smoke || std::env::args().any(|argument| argument == "--window") {
        return engine.run();
    }
    engine.queue_input_at(
        1,
        InputEvent {
            source: 0,
            sequence: 1,
            binding: InputBinding::Keyboard(key("keyboard:enter")?),
            value: InputValue::Button(true),
        },
    )?;
    loop {
        let mut result = engine.step(TickInput::empty(engine.tick() + 1))?;
        while matches!(result, StepResult::Pending(_)) {
            result = engine.resume()?;
        }
        if result == StepResult::Quit {
            break;
        }
    }
    let view = engine.worlds();
    let elapsed: u64 = view.get(view.id("main")?, key("sample:elapsed")?)?;
    println!("ゲームを{elapsed}tick実行して終了しました");
    engine.shutdown()
}
