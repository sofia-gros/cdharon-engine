//! 配布pack→Scene load→sprite/camera→CPU参照描画→wgpu表示までの作者向け実行例。
//!
//! `cargo run --example assets_scene`はheadlessでPNGを出力し、
//! `-- --window-smoke`は実際のwindowで120tick動かして終了します。
//! `-- --window`は終了するまで表示します。音声はdecodeのみで、再生機器は要求しません。

use charon_engine::{
    assets::{AssetHandle, AssetPack, AssetServer, AssetSettings, ImageAsset, Vfs, VfsLimits},
    foundation::{EngineError, EngineResult, ErrorCode, StableKey},
    presentation::{Camera2d, Color, Sprite, Viewport},
    runtime::{
        DesktopGpu, Engine, FrameContext, LoadContext, Scene, SceneContext,
        config::EngineSettingsPatch,
    },
};
use std::{cell::Cell, rc::Rc};

struct Gallery {
    image: Option<AssetHandle<ImageAsset>>,
    frames: Rc<Cell<u64>>,
    auto_quit: bool,
}
impl Scene for Gallery {
    fn load(&mut self, ctx: &mut LoadContext<'_>) -> EngineResult<()> {
        let mut assets = ctx.assets();
        self.image = Some(assets.spritesheet("tiles", "game:tiles.png", 1, 1)?);
        assets.atlas(
            "atlas",
            "game:atlas.json",
            self.image.expect("loadで設定した画像"),
        )?;
        assets.audio("wav", "game:tone.wav")?;
        assets.audio("ogg", "game:tone.ogg")?;
        assets.font("font", "game:Abel-Regular.ttf", 0)?;
        Ok(())
    }
    fn update(&mut self, ctx: &mut SceneContext<'_>) -> EngineResult<()> {
        if self.auto_quit && ctx.tick() >= 120 {
            ctx.scenes().quit()?;
        }
        Ok(())
    }
    fn render(&mut self, ctx: &mut FrameContext<'_>) -> EngineResult<()> {
        self.frames.set(self.frames.get() + 1);
        if self.frames.get() == 1
            && let Ok(gpu) = ctx
                .services()
                .get::<DesktopGpu>(StableKey::parse("engine:gpu-info")?)
        {
            println!("GPU表示：{} / {}", gpu.backend, gpu.name);
        }
        let image = self.image.expect("startより前に画像をload済み");
        ctx.draw().clear(Color([24, 28, 40, 255]));
        for index in 0..4 {
            // 表示寸法はRustで指定します。素材の物理寸法やPNGのpixel数は変更しません。
            ctx.draw().image(
                image,
                Sprite::new()
                    .frame(index)
                    .position(8.0 + index as f64 * 36.0, 16.0)
                    .size(28.0, 28.0),
            )?;
        }
        let mut camera = Camera2d::screen(80, 48);
        camera.viewport = Viewport {
            x: 80,
            y: 48,
            width: 80,
            height: 48,
        };
        camera.position = [0.0, 0.0];
        camera.zoom = 2.0;
        camera.rotation = 0.2;
        ctx.draw_with(camera).image(
            image,
            Sprite::new()
                .frame(1)
                .position(0.0, 0.0)
                .size(14.0, 14.0)
                .rotated(0.4),
        )?;
        Ok(())
    }
}
fn main() -> EngineResult<()> {
    let limits = VfsLimits::default();
    // 同じbyte列をWindows/Linux/macOSで読むfixtureです。実行時の作業フォルダを使いません。
    let pack = AssetPack::decode(
        include_bytes!("../tests/fixtures/e03/assets.chpack"),
        limits,
    )?;
    let mut vfs = Vfs::new(limits);
    vfs.mount_pack(pack)?;
    let mut assets = AssetServer::new(vfs, AssetSettings::default())?;
    assets.set_source_license("game:tiles.png", "CC0-1.0")?;
    assets.set_source_license(
        "game:Abel-Regular.ttf",
        "SIL Open Font License 1.1; game:Abel-OFL.txt",
    )?;
    let window = std::env::args().any(|arg| arg == "--window" || arg == "--window-smoke");
    let auto_quit = std::env::args().any(|arg| arg == "--window-smoke");
    let frames = Rc::new(Cell::new(0));
    let mut engine = Engine::builder()
        .asset_server(assets)
        .settings(EngineSettingsPatch::default().window_size(160, 96))
        .scene(
            "gallery",
            Gallery {
                image: None,
                frames: Rc::clone(&frames),
                auto_quit,
            },
        )
        .start_scene("gallery")
        .build()?;
    if window {
        engine.run()?;
        if frames.get() == 0 {
            return Err(EngineError::new(
                ErrorCode::InvalidState,
                "GPU frameが一度も表示されていない",
            ));
        }
        println!("表示frame数：{}", frames.get());
        return Ok(());
    }
    engine.frame(0.0)?;
    let mut rgba = vec![0; 160 * 96 * 4];
    engine.render_into(&mut rgba)?;
    std::fs::create_dir_all("target/e03-artifacts").map_err(io)?;
    let file = std::fs::File::create("target/e03-artifacts/gallery.png").map_err(io)?;
    let mut encoder = png::Encoder::new(file, 160, 96);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|cause| EngineError::new(ErrorCode::Io, "PNG出力").detail(cause.to_string()))?;
    writer
        .write_image_data(&rgba)
        .map_err(|cause| EngineError::new(ErrorCode::Io, "PNG出力").detail(cause.to_string()))?;
    writer
        .finish()
        .map_err(|cause| EngineError::new(ErrorCode::Io, "PNG出力").detail(cause.to_string()))?;
    println!("target/e03-artifacts/gallery.pngへ描画しました");
    engine.shutdown()
}
fn io(cause: std::io::Error) -> EngineError {
    EngineError::new(ErrorCode::Io, "作例の画像出力").detail(cause.to_string())
}
