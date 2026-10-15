//! E03の可搬資産・scope・世代・参照描画を、作者公開APIで検証します。

use charon_engine::{
    assets::*,
    foundation::{EngineResult, ErrorCode, id::Arena},
    presentation::{
        Camera2d, Color, Filter, RenderAdapter, RenderFrame, ScalarRenderer, Sprite,
        SurfaceMapping, Viewport,
    },
    runtime::{
        Engine, FrameContext, LoadContext, Scene,
        input::{ScopeId, ScopeKind, TickInput},
    },
};
use std::{
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

const PNG: &[u8] = include_bytes!("fixtures/e03/tiles.png");
fn source() -> Vfs {
    let files = vec![
        ("game:tiles.png", PNG.to_vec()),
        ("game:bad.png", vec![1, 2, 3]),
        (
            "game:atlas.json",
            include_bytes!("fixtures/e03/atlas.json").to_vec(),
        ),
        (
            "game:tone.wav",
            include_bytes!("fixtures/e03/tone.wav").to_vec(),
        ),
        (
            "game:tone.ogg",
            include_bytes!("fixtures/e03/tone.ogg").to_vec(),
        ),
        (
            "game:font.ttf",
            include_bytes!("fixtures/e03/Abel-Regular.ttf").to_vec(),
        ),
    ];
    let mut vfs = Vfs::new(VfsLimits::default());
    vfs.mount_pack(
        AssetPack::new(
            files
                .into_iter()
                .map(|(key, bytes)| (VirtualPath::parse(key).unwrap(), bytes))
                .collect(),
            VfsLimits::default(),
        )
        .unwrap(),
    )
    .unwrap();
    vfs
}
fn server() -> AssetServer {
    AssetServer::new(source(), AssetSettings::default()).unwrap()
}
fn scopes() -> (Arena<u8, ScopeKind>, ScopeId, ScopeId) {
    let mut arena = Arena::new(4).unwrap();
    let a = arena.insert(0).unwrap();
    let b = arena.insert(0).unwrap();
    (arena, a, b)
}
fn wait<T: Asset>(server: &mut AssetServer, job: &mut AssetJob<T>) -> EngineResult<AssetHandle<T>> {
    let start = Instant::now();
    loop {
        if let Some(handle) = server.poll(job)? {
            return Ok(handle);
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "import worker完了の試験上限"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
#[test]
fn pack_is_canonical_and_every_truncation_is_rejected() {
    let files = vec![
        (VirtualPath::parse("game:b").unwrap(), vec![2]),
        (VirtualPath::parse("game:a").unwrap(), vec![1]),
    ];
    let mut reverse = files.clone();
    reverse.reverse();
    let pack = AssetPack::new(files, VfsLimits::default())
        .unwrap()
        .encode(VfsLimits::default())
        .unwrap();
    assert_eq!(
        pack,
        AssetPack::new(reverse, VfsLimits::default())
            .unwrap()
            .encode(VfsLimits::default())
            .unwrap()
    );
    for length in 0..pack.len() {
        assert!(AssetPack::decode(&pack[..length], VfsLimits::default()).is_err());
    }
    let mut corrupt = pack.clone();
    *corrupt.last_mut().unwrap() ^= 1;
    assert!(AssetPack::decode(&corrupt, VfsLimits::default()).is_err());
    assert!(
        AssetPack::decode(
            &pack,
            VfsLimits {
                files: 1,
                ..VfsLimits::default()
            }
        )
        .is_err()
    );
}
#[test]
fn portable_paths_and_case_collisions_are_checked_before_mount() {
    for path in [
        "game:../secret",
        "game:/absolute",
        "game:a\\b",
        "game:a//b",
        "game:CON.png",
        "game:x.",
        "game:LPT1/a",
        "game:a:b",
        "game:a/./b",
    ] {
        assert!(VirtualPath::parse(path).is_err(), "{path}");
    }
    let files = vec![
        (VirtualPath::parse("game:A.png").unwrap(), vec![]),
        (VirtualPath::parse("game:a.png").unwrap(), vec![]),
    ];
    assert!(AssetPack::new(files, VfsLimits::default()).is_err());
    let mut vfs = source();
    let before = vfs.paths().count();
    let duplicate = AssetPack::new(
        vec![(VirtualPath::parse("game:TILES.png").unwrap(), vec![])],
        VfsLimits::default(),
    )
    .unwrap();
    assert!(vfs.mount_pack(duplicate).is_err());
    assert_eq!(vfs.paths().count(), before);
}
#[test]
fn directory_and_pack_produce_identical_sources() {
    let mut directory = Vfs::new(VfsLimits::default());
    directory
        .mount_directory(
            "game",
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/e03"),
        )
        .unwrap();
    let packed = source();
    assert_eq!(
        directory
            .read(&VirtualPath::parse("game:tiles.png").unwrap())
            .unwrap(),
        packed
            .read(&VirtualPath::parse("game:tiles.png").unwrap())
            .unwrap()
    );
    let small = VfsLimits {
        file_bytes: 4,
        ..VfsLimits::default()
    };
    let mut directory = Vfs::new(small);
    directory
        .mount_directory(
            "game",
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/e03"),
        )
        .unwrap();
    assert_eq!(
        directory
            .read(&VirtualPath::parse("game:tiles.png").unwrap())
            .unwrap_err()
            .code,
        ErrorCode::CapacityExceeded
    );
}
#[test]
fn png_alpha_sheet_and_source_bounds_are_validated() {
    let mut enormous = PNG.to_vec();
    enormous[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
    enormous[20..24].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(
        PngImporter::default()
            .import(
                &enormous,
                ImportLimits {
                    image_dimension: u32::MAX,
                    ..ImportLimits::default()
                }
            )
            .is_err()
    );

    let limits = ImportLimits::default();
    let image = PngImporter::default().import(PNG, limits).unwrap();
    assert_eq!((image.width, image.height), (2, 2));
    assert_eq!(&image.rgba[8..12], &[0, 255, 0, 128]);
    assert_eq!(&image.rgba[12..], &[255, 255, 255, 0]);
    let sheet = PngImporter {
        frame_size: Some((1, 1)),
    }
    .import(PNG, limits)
    .unwrap();
    assert_eq!(sheet.frames.len(), 4);
    assert!(
        PngImporter {
            frame_size: Some((3, 1))
        }
        .import(PNG, limits)
        .is_err()
    );
    assert!(
        PngImporter::default()
            .import(
                PNG,
                ImportLimits {
                    decoded_bytes: 15,
                    ..limits
                }
            )
            .is_err()
    );
    for length in 0..PNG.len() {
        assert!(
            PngImporter::default()
                .import(&PNG[..length], limits)
                .is_err(),
            "切断位置={length}"
        );
    }
    let mut corrupt = PNG.to_vec();
    corrupt[45] ^= 1;
    assert!(PngImporter::default().import(&corrupt, limits).is_err());
}
#[test]
fn audio_and_font_import_are_device_independent() {
    let wav = AudioImporter
        .import(
            include_bytes!("fixtures/e03/tone.wav"),
            ImportLimits::default(),
        )
        .unwrap();
    assert_eq!((wav.sample_rate, wav.channels), (8000, 1));
    assert_eq!(wav.samples[2], -0.5);
    assert_eq!(wav.samples[4], -1.0);
    let ogg = AudioImporter
        .import(
            include_bytes!("fixtures/e03/tone.ogg"),
            ImportLimits::default(),
        )
        .unwrap();
    assert_eq!((ogg.sample_rate, ogg.channels), (8000, 1));
    assert!(!ogg.samples.is_empty());
    let font = FontImporter { face_index: 0 }
        .import(
            include_bytes!("fixtures/e03/Abel-Regular.ttf"),
            ImportLimits::default(),
        )
        .unwrap();
    assert!(font.face().unwrap().glyph_index('A').is_some());
    assert!(
        FontImporter { face_index: 1 }
            .import(
                include_bytes!("fixtures/e03/Abel-Regular.ttf"),
                ImportLimits::default()
            )
            .is_err()
    );
    assert!(
        AudioImporter
            .import(b"RIFF", ImportLimits::default())
            .is_err()
    );
    assert!(
        FontImporter { face_index: 0 }
            .import(b"bad", ImportLimits::default())
            .is_err()
    );
}
#[test]
fn shared_scope_release_and_explicit_lease_have_distinct_lifetimes() {
    let (_arena, a, b) = scopes();
    let mut server = server();
    server.open_scope(a).unwrap();
    server.open_scope(b).unwrap();
    let first = server
        .loader(a)
        .unwrap()
        .image("tiles", "game:tiles.png")
        .unwrap();
    let second = server
        .loader(b)
        .unwrap()
        .image("tiles", "game:tiles.png")
        .unwrap();
    let lease = server.lease(first).unwrap();
    let other = server.lease(second).unwrap();
    assert!(std::ptr::eq(&*lease, &*other));
    let bytes = server.resident_bytes();
    server.close_scope(a);
    server.collect();
    assert!(server.lease(first).is_ok());
    server.close_scope(b);
    server.collect();
    assert_eq!(server.resident_bytes(), bytes);
    drop(lease);
    drop(other);
    server.collect();
    assert!(server.lease(first).is_err());
    assert_eq!(server.resident_bytes(), 0);
    server.open_scope(b).unwrap();
    let new = server
        .loader(b)
        .unwrap()
        .image("tiles", "game:tiles.png")
        .unwrap();
    assert!(server.lease(first).is_err());
    assert_eq!(server.lease(new).unwrap().rgba.len(), 16);
}
#[test]
fn wrong_type_wrong_cache_and_conflicting_definition_are_rejected() {
    let (_arena, a, _) = scopes();
    let mut server = server();
    server.open_scope(a).unwrap();
    let image = server
        .loader(a)
        .unwrap()
        .image("tiles", "game:tiles.png")
        .unwrap();
    assert_eq!(
        server
            .resolve(AssetKey::<AudioAsset>::new("tiles").unwrap())
            .unwrap_err()
            .code,
        ErrorCode::SchemaMismatch
    );
    let other = AssetServer::new(source(), AssetSettings::default()).unwrap();
    assert!(other.lease(image).is_err());
    assert!(
        server
            .loader(a)
            .unwrap()
            .spritesheet("tiles", "game:tiles.png", 1, 1)
            .is_err()
    );
}
#[test]
fn reload_preserves_old_lease_and_rejects_outdated_completion() {
    let (_arena, a, _) = scopes();
    let mut server = server();
    server.open_scope(a).unwrap();
    let image = server
        .loader(a)
        .unwrap()
        .image("tiles", "game:tiles.png")
        .unwrap();
    let old = server.lease(image).unwrap();
    let mut first = server
        .loader(a)
        .unwrap()
        .request(
            AssetKey::<ImageAsset>::new("tiles").unwrap(),
            "game:tiles.png",
            PngImporter {
                frame_size: Some((1, 1)),
            },
        )
        .unwrap();
    let mut second = server
        .loader(a)
        .unwrap()
        .request(
            AssetKey::<ImageAsset>::new("tiles").unwrap(),
            "game:tiles.png",
            PngImporter {
                frame_size: Some((2, 1)),
            },
        )
        .unwrap();
    let new = wait(&mut server, &mut first).unwrap();
    assert_eq!(old.frames.len(), 1);
    assert_eq!(server.lease(new).unwrap().frames.len(), 4);
    assert!(server.lease(image).is_err());
    assert_eq!(
        wait(&mut server, &mut second).unwrap_err().code,
        ErrorCode::Conflict
    );
    let mut bad = server
        .loader(a)
        .unwrap()
        .request(
            AssetKey::<ImageAsset>::new("tiles").unwrap(),
            "game:bad.png",
            PngImporter::default(),
        )
        .unwrap();
    assert!(wait(&mut server, &mut bad).is_err());
    assert_eq!(server.lease(new).unwrap().frames.len(), 4);
}
#[test]
fn completed_unpolled_jobs_are_included_in_the_cache_budget() {
    let (_arena, a, _) = scopes();
    let mut server = AssetServer::new(
        source(),
        AssetSettings {
            cache_bytes: 64,
            ..AssetSettings::default()
        },
    )
    .unwrap();
    server.open_scope(a).unwrap();
    let first = server
        .loader(a)
        .unwrap()
        .image("tiles", "game:tiles.png")
        .unwrap();
    let held = server.lease(first).unwrap();
    let mut job = server
        .loader(a)
        .unwrap()
        .request(
            AssetKey::<ImageAsset>::new("new").unwrap(),
            "game:tiles.png",
            PngImporter::default(),
        )
        .unwrap();
    let other = wait(&mut server, &mut job).unwrap();
    assert!(server.resident_bytes() <= 64);
    let mut overflow = server
        .loader(a)
        .unwrap()
        .request(
            AssetKey::<ImageAsset>::new("overflow").unwrap(),
            "game:tiles.png",
            PngImporter::default(),
        )
        .unwrap();
    assert_eq!(
        wait(&mut server, &mut overflow).unwrap_err().code,
        ErrorCode::CapacityExceeded
    );
    assert!(server.lease(first).is_ok());
    assert!(server.lease(other).is_ok());
    drop(held);
}
struct BlockingImporter {
    started: mpsc::SyncSender<()>,
    resume: std::sync::Mutex<mpsc::Receiver<()>>,
}
impl Importer<ImageAsset> for BlockingImporter {
    fn identity(&self) -> String {
        "test:blocked-png-v1".into()
    }
    fn import(&self, bytes: &[u8], limits: ImportLimits) -> EngineResult<ImageAsset> {
        self.started.send(()).unwrap();
        self.resume.lock().unwrap().recv().unwrap();
        PngImporter::default().import(bytes, limits)
    }
}
#[test]
fn late_worker_cannot_publish_to_a_stopped_scene_scope() {
    let (_arena, a, _) = scopes();
    let mut server = server();
    server.open_scope(a).unwrap();
    let (started, ready) = mpsc::sync_channel(1);
    let (resume, gate) = mpsc::sync_channel(1);
    let mut job = server
        .loader(a)
        .unwrap()
        .request(
            AssetKey::new("tiles").unwrap(),
            "game:tiles.png",
            BlockingImporter {
                started,
                resume: std::sync::Mutex::new(gate),
            },
        )
        .unwrap();
    ready.recv_timeout(Duration::from_secs(10)).unwrap();
    server.close_scope(a);
    resume.send(()).unwrap();
    assert!(wait(&mut server, &mut job).is_err());
    assert!(
        server
            .resolve(AssetKey::<ImageAsset>::new("tiles").unwrap())
            .is_err()
    );
}
#[test]
fn cancellation_and_atlas_dependencies_preserve_valid_generations() {
    let (_arena, a, _) = scopes();
    let mut server = server();
    server.open_scope(a).unwrap();
    server
        .set_source_license("game:tiles.png", "CC0-1.0")
        .unwrap();
    let image = server
        .loader(a)
        .unwrap()
        .image("tiles", "game:tiles.png")
        .unwrap();
    assert_eq!(
        server.lease(image).unwrap().manifest().license.as_deref(),
        Some("CC0-1.0")
    );
    let atlas = server
        .loader(a)
        .unwrap()
        .atlas("atlas", "game:atlas.json", image)
        .unwrap();
    assert_eq!(server.lease(atlas).unwrap().frames.len(), 2);
    assert_eq!(
        server.lease(atlas).unwrap().manifest().dependency_hashes,
        vec![server.lease(image).unwrap().manifest().content_hash()]
    );
    let mut job = server
        .loader(a)
        .unwrap()
        .request(
            AssetKey::<ImageAsset>::new("cancelled").unwrap(),
            "game:tiles.png",
            PngImporter::default(),
        )
        .unwrap();
    job.cancel();
    assert!(server.poll(&mut job).is_err());
}
#[test]
fn camera_inverse_viewport_and_letterbox_share_one_coordinate_model() {
    let mut camera = Camera2d::screen(40, 20);
    camera.position = [-10.0, 5.0];
    camera.rotation = 0.4;
    camera.zoom = 2.5;
    let screen = camera.world_to_screen([-8.0, 6.0]).unwrap();
    let world = camera.screen_to_world(screen).unwrap().unwrap();
    assert!((world[0] + 8.0).abs() < 1e-10 && (world[1] - 6.0).abs() < 1e-10);
    assert!(camera.screen_to_world([40.0, 0.0]).unwrap().is_none());
    let mapping = SurfaceMapping::fit([20, 10], [100, 100], false).unwrap();
    assert!(mapping.to_logical([0.0, 0.0]).unwrap().is_none());
    assert_eq!(mapping.to_logical([50.0, 50.0]).unwrap(), Some([10.0, 5.0]));
    camera.zoom = f64::NAN;
    assert!(camera.world_to_screen([0.0, 0.0]).is_err());
    let clipped = SurfaceMapping::fit([20, 10], [10, 10], true).unwrap();
    assert_eq!(clipped.to_logical([0.0, 0.0]).unwrap(), Some([5.0, 0.0]));
    assert!(clipped.to_logical([-1.0, 0.0]).unwrap().is_none());
}
#[test]
fn reference_renderer_handles_alpha_z_order_clipping_and_budget_atomically() {
    assert!(RenderFrame::new(u32::MAX, u32::MAX, 1).is_err());
    let mut extreme = RenderFrame::new(1, 1, 1).unwrap();
    extreme.width = u32::MAX;
    extreme.height = u32::MAX;
    assert!(ScalarRenderer::default().render(&extreme, &mut []).is_err());

    let (_arena, a, _) = scopes();
    let mut server = server();
    server.open_scope(a).unwrap();
    let image = server
        .loader(a)
        .unwrap()
        .spritesheet("tiles", "game:tiles.png", 1, 1)
        .unwrap();
    let mut frame = RenderFrame::new(2, 2, 16).unwrap();
    frame.clear = Color::TRANSPARENT;
    frame
        .draw(&server, Camera2d::screen(2, 2))
        .image(image, Sprite::new().frame(2))
        .unwrap();
    let mut pixels = vec![0; 16];
    ScalarRenderer::default()
        .render(&frame, &mut pixels)
        .unwrap();
    assert_eq!(&pixels[..4], &[0, 255, 0, 128]);
    frame
        .draw(&server, Camera2d::screen(2, 2))
        .image(image, Sprite::new().frame(0).order(1, 0))
        .unwrap();
    ScalarRenderer::default()
        .render(&frame, &mut pixels)
        .unwrap();
    assert_eq!(&pixels[..4], &[255, 0, 0, 255]);
    let before = pixels.clone();
    assert!(
        ScalarRenderer { max_samples: 0 }
            .render(&frame, &mut pixels)
            .is_err()
    );
    assert_eq!(pixels, before);
    frame.reset();
    let mut camera = Camera2d::screen(1, 2);
    camera.viewport = Viewport {
        x: 1,
        y: 0,
        width: 1,
        height: 2,
    };
    frame
        .draw(&server, camera)
        .image(
            image,
            Sprite::new().frame(1).size(2.0, 2.0).filter(Filter::Linear),
        )
        .unwrap();
    ScalarRenderer::default()
        .render(&frame, &mut pixels)
        .unwrap();
    assert_eq!(&pixels[..4], &[0, 0, 0, 255]);
    assert_eq!(&pixels[4..8], &[0, 0, 255, 255]);
}
#[test]
fn draw_snapshot_keeps_assets_alive_after_scope_cleanup() {
    let (_arena, a, _) = scopes();
    let mut server = server();
    server.open_scope(a).unwrap();
    let image = server
        .loader(a)
        .unwrap()
        .image("tiles", "game:tiles.png")
        .unwrap();
    let mut frame = RenderFrame::new(2, 2, 4).unwrap();
    frame
        .draw(&server, Camera2d::screen(2, 2))
        .image(image, Sprite::new())
        .unwrap();
    server.close_scope(a);
    server.collect();
    assert!(server.resident_bytes() > 0);
    let mut pixels = vec![0; 16];
    ScalarRenderer::default()
        .render(&frame, &mut pixels)
        .unwrap();
    assert_eq!(&pixels[..4], &[255, 0, 0, 255]);
    frame.reset();
    server.collect();
    assert_eq!(server.resident_bytes(), 0);
}
struct AssetScene {
    image: Option<AssetHandle<ImageAsset>>,
    loads: Arc<std::sync::atomic::AtomicUsize>,
}
impl Scene for AssetScene {
    fn load(&mut self, ctx: &mut LoadContext<'_>) -> EngineResult<()> {
        self.image = Some(ctx.assets().image("tiles", "tiles.png")?);
        self.loads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
    fn render(&mut self, ctx: &mut FrameContext<'_>) -> EngineResult<()> {
        ctx.draw().image(self.image.unwrap(), Sprite::new())
    }
}
#[test]
fn scene_loader_and_headless_draw_use_the_real_shared_asset_server() {
    let loads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut engine = Engine::builder()
        .asset_server(server())
        .settings(charon_engine::runtime::config::EngineSettingsPatch::default().window_size(2, 2))
        .scene(
            "game",
            AssetScene {
                image: None,
                loads: Arc::clone(&loads),
            },
        )
        .start_scene("game")
        .build()
        .unwrap();
    engine.step(TickInput::empty(1)).unwrap();
    engine.frame(0.5).unwrap();
    let mut pixels = vec![0; 16];
    engine.render_into(&mut pixels).unwrap();
    assert_eq!(&pixels[..4], &[255, 0, 0, 255]);
    assert_eq!(loads.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(engine.tick(), 1);
    engine.shutdown().unwrap();
    assert_eq!(engine.tick(), 1);
    assert_eq!(engine.assets().resident_bytes(), 0);
}

#[test]
fn committed_pack_has_the_same_sources_and_import_results() {
    let pack = AssetPack::decode(
        include_bytes!("fixtures/e03/assets.chpack"),
        VfsLimits::default(),
    )
    .unwrap();
    let mut vfs = Vfs::new(VfsLimits::default());
    vfs.mount_pack(pack).unwrap();
    assert_eq!(
        vfs.read(&VirtualPath::parse("game:tiles.png").unwrap())
            .unwrap(),
        PNG
    );
    let (_arena, a, _) = scopes();
    let mut assets = AssetServer::new(vfs, AssetSettings::default()).unwrap();
    assets.open_scope(a).unwrap();
    let image = assets
        .loader(a)
        .unwrap()
        .spritesheet("tiles", "game:tiles.png", 1, 1)
        .unwrap();
    assert_eq!(assets.lease(image).unwrap().frames.len(), 4);
    let sound = assets
        .loader(a)
        .unwrap()
        .audio("sound", "game:tone.ogg")
        .unwrap();
    assert_eq!(assets.lease(sound).unwrap().sample_rate, 8000);
    let font = assets
        .loader(a)
        .unwrap()
        .font("font", "game:Abel-Regular.ttf", 0)
        .unwrap();
    assert!(
        assets
            .lease(font)
            .unwrap()
            .face()
            .unwrap()
            .glyph_index('A')
            .is_some()
    );
}
#[test]
fn unicode_normalization_does_not_create_platform_specific_asset_keys() {
    let first = VirtualPath::parse("game:caf\u{e9}.png").unwrap();
    let second = VirtualPath::parse("game:cafe\u{301}.png").unwrap();
    assert_eq!(first, second);
    assert!(
        AssetPack::new(
            vec![(first, vec![]), (second, vec![])],
            VfsLimits::default()
        )
        .is_err()
    );
}
#[test]
fn pending_job_count_is_bounded_until_the_result_is_consumed() {
    let (_arena, a, _) = scopes();
    let mut server = AssetServer::new(
        source(),
        AssetSettings {
            jobs: 1,
            ..AssetSettings::default()
        },
    )
    .unwrap();
    server.open_scope(a).unwrap();
    let mut first = server
        .loader(a)
        .unwrap()
        .request(
            AssetKey::<ImageAsset>::new("first").unwrap(),
            "game:tiles.png",
            PngImporter::default(),
        )
        .unwrap();
    assert!(
        server
            .loader(a)
            .unwrap()
            .request(
                AssetKey::<ImageAsset>::new("second").unwrap(),
                "game:tiles.png",
                PngImporter::default()
            )
            .is_err()
    );
    wait(&mut server, &mut first).unwrap();
    let mut second = server
        .loader(a)
        .unwrap()
        .request(
            AssetKey::<ImageAsset>::new("second").unwrap(),
            "game:tiles.png",
            PngImporter::default(),
        )
        .unwrap();
    wait(&mut server, &mut second).unwrap();
}
#[test]
fn duplicate_atlas_names_and_out_of_range_frames_are_rejected() {
    let duplicate=br#"{"schema_version":1,"frames":{"a":{"x":0,"y":0,"width":1,"height":1},"a":{"x":0,"y":0,"width":1,"height":1}}}"#;
    assert!(serde_json::from_slice::<AtlasDefinition>(duplicate).is_err());
    assert!(
        ImageRect {
            x: u32::MAX,
            y: 0,
            width: 2,
            height: 1
        }
        .validate(2, 2)
        .is_err()
    );
    let mut vfs = source();
    vfs.mount_pack(
        AssetPack::new(
            vec![(
                VirtualPath::parse("game:outside.json").unwrap(),
                br#"{"schema_version":1,"frames":{"a":{"x":3,"y":0,"width":1,"height":1}}}"#
                    .to_vec(),
            )],
            VfsLimits::default(),
        )
        .unwrap(),
    )
    .unwrap();
    let mut server = AssetServer::new(vfs, AssetSettings::default()).unwrap();
    let (_arena, a, _) = scopes();
    server.open_scope(a).unwrap();
    let image = server
        .loader(a)
        .unwrap()
        .image("tiles", "game:tiles.png")
        .unwrap();
    assert!(
        server
            .loader(a)
            .unwrap()
            .atlas("outside", "game:outside.json", image)
            .is_err()
    );
}
#[test]
fn palette_transparency_and_gamma_are_imported_before_rendering() {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
        encoder.set_color(png::ColorType::Indexed);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_palette(vec![255, 0, 0, 0, 0, 255]);
        encoder.set_trns(vec![255, 0]);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[0, 1]).unwrap();
        writer.finish().unwrap();
    }
    let image = PngImporter::default()
        .import(&bytes, ImportLimits::default())
        .unwrap();
    assert_eq!(image.rgba, vec![255, 0, 0, 255, 0, 0, 255, 0]);
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_source_gamma(png::ScaledFloat::new(1.0));
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[128, 128, 128, 128]).unwrap();
        writer.finish().unwrap();
    }
    let image = PngImporter::default()
        .import(&bytes, ImportLimits::default())
        .unwrap();
    assert_eq!(image.rgba, vec![188, 188, 188, 128]);
}
#[test]
fn invalid_draw_submission_poisoning_prevents_a_partial_frame() {
    let (_arena, a, _) = scopes();
    let mut server = server();
    server.open_scope(a).unwrap();
    let image = server
        .loader(a)
        .unwrap()
        .image("tiles", "game:tiles.png")
        .unwrap();
    let mut frame = RenderFrame::new(2, 2, 1).unwrap();
    let camera = Camera2d::screen(2, 2);
    frame
        .draw(&server, camera)
        .image(image, Sprite::new())
        .unwrap();
    assert!(
        frame
            .draw(&server, camera)
            .image(image, Sprite::new())
            .is_err()
    );
    let mut pixels = vec![7; 16];
    assert!(
        ScalarRenderer::default()
            .render(&frame, &mut pixels)
            .is_err()
    );
    assert_eq!(pixels, vec![7; 16]);
    frame.reset();
    assert!(
        frame
            .draw(&server, camera)
            .image(image, Sprite::new().position(f64::NAN, 0.0))
            .is_err()
    );
    assert!(frame.validate().is_err());
}

#[test]
fn linear_filter_uses_premultiplied_colors_at_transparent_edges() {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&[255, 0, 0, 255, 0, 0, 255, 0])
            .unwrap();
        writer.finish().unwrap();
    }
    let mut vfs = source();
    vfs.mount_pack(
        AssetPack::new(
            vec![(VirtualPath::parse("game:edge.png").unwrap(), bytes)],
            VfsLimits::default(),
        )
        .unwrap(),
    )
    .unwrap();
    let (_arena, a, _) = scopes();
    let mut server = AssetServer::new(vfs, AssetSettings::default()).unwrap();
    server.open_scope(a).unwrap();
    let image = server
        .loader(a)
        .unwrap()
        .image("edge", "game:edge.png")
        .unwrap();
    let mut frame = RenderFrame::new(3, 1, 1).unwrap();
    frame.clear = Color::TRANSPARENT;
    frame
        .draw(&server, Camera2d::screen(3, 1))
        .image(image, Sprite::new().size(3.0, 1.0).filter(Filter::Linear))
        .unwrap();
    let mut pixels = vec![0; 12];
    ScalarRenderer::default()
        .render(&frame, &mut pixels)
        .unwrap();
    // 透明な青を混ぜて紫にせず、赤の透明度だけを補間します。
    assert_eq!(&pixels[4..8], &[255, 0, 0, 128]);
}
