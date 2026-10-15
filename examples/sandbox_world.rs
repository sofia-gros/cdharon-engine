//! E04の公開APIで、大きな論理世界を部分生成・編集・保存・退避・復帰します。
//!
//! `cargo run --locked --example sandbox_world`で実行します。生成器と配置内容は作例側の
//! Rustコードです。エンジン本体へ素材名・地形規則・ゲーム進行を固定しません。

use charon_engine::{
    foundation::{
        EngineError, EngineResult, ErrorCode, StableKey,
        grid::{CellPoint, GridLayout, GridLimits, GridScale},
        id::WorldId,
        schema::Schema,
    },
    presentation::Camera2d,
    world::*,
};
use std::sync::Arc;

fn key(value: &str) -> StableKey {
    StableKey::parse(value).expect("作例で固定した安定キー")
}
fn point(x: i32, y: i32) -> CellPoint {
    CellPoint { x, y }
}
struct Terrain;
impl GenerationPass for Terrain {
    fn key(&self) -> StableKey {
        key("sample:terrain")
    }
    fn version(&self) -> u32 {
        1
    }
    fn dependencies(&self) -> &[StableKey] {
        &[]
    }
    fn configuration(&self) -> &[u8] {
        b"surface=96;anchor-grid=128;schema=1"
    }
    fn generate(
        &self,
        input: &GenerationInput<'_>,
        output: &mut ChunkDraft<'_>,
    ) -> EngineResult<()> {
        let front = input.content.layer_id(key("sample:foreground"))?;
        let back = input.content.layer_id(key("sample:background"))?;
        let rock = input
            .content
            .material_id(MaterialHandle(key("sample:rock")))?;
        let origin = input.coordinate.origin()?;
        // region anchorを絶対座標から決め、隣接chunkの生成完了順から独立させます。
        // この作例のstructureは背景の柱です。実体を持つ家具は後でblock APIから配置します。
        for dy in 0..64 {
            for dx in 0..64 {
                let point = point(origin.x + dx, origin.y + dy);
                if !input.bounds.contains(point) {
                    continue;
                }
                if point.y >= 96 {
                    output.set_cell(
                        front,
                        point,
                        CellCore {
                            material: rock,
                            flags: 0,
                            variation: (input.anchor_seed(key("sample:variation"), point) & 3)
                                as u16,
                        },
                    )?;
                }
                if point.x.rem_euclid(128) < 3 {
                    output.set_cell(
                        back,
                        point,
                        CellCore {
                            material: rock,
                            ..CellCore::AIR
                        },
                    )?;
                }
            }
        }
        Ok(())
    }
}
fn main() -> EngineResult<()> {
    let mut registry = ContentRegistry::new(ContentLimits::default())?;
    for (name, role, order) in [
        ("sample:foreground", LayerRole::Terrain, 1),
        ("sample:background", LayerRole::Background, 0),
    ] {
        registry.layer(LayerDef {
            key: key(name),
            role,
            render_order: order,
            collision_group: if role == LayerRole::Terrain { 1 } else { 0 },
            simulated: role == LayerRole::Terrain,
        })?;
    }
    for (name, base, size) in [
        ("sample:rock", MaterialBase::Solid, [16, 16]),
        ("sample:wood", MaterialBase::Solid, [3, 2]),
        ("sample:sand", MaterialBase::Powder, [1, 1]),
    ] {
        registry.register(MaterialBuilder::new(name, base)?.block_size(size[0], size[1]))?;
    }
    registry.schema(Schema::of::<u8>(key("sample:amount"), 1, 1)?)?;
    registry.schema(Schema::of::<u32>(key("sample:count"), 1, 4)?)?;
    registry.channel(ChannelDefinition {
        key: key("sample:water"),
        version: 1,
        value_schema: key("sample:amount"),
        layout: GridLayout {
            origin: point(0, 0),
            x_scale: GridScale::new(1, 2)?,
            y_scale: GridScale::new(1, 2)?,
        },
        default: 0u8,
        limits: GridLimits {
            chunks: 64,
            checkpoint_bytes: 4 * 1024 * 1024,
        },
    })?;
    let content = registry.freeze()?;
    // B01の論理寸法。全体では約43億cellですが、この時点で地形配列は0byteです。
    let bounds = WorldBounds::new(point(0, 0), 8192 * 16, 2048 * 16)?;
    let limits = WorldLimits {
        resident_chunks: 64,
        active_chunks: 32,
        blocks: 1024,
        ..WorldLimits::default()
    };
    let mut world = World::new(
        WorldId::from_parts(0, 1)?,
        2026,
        bounds,
        Arc::clone(&content),
        limits,
    )?;
    assert_eq!(world.terrain_bytes(), 0);
    let pipeline =
        GenerationPipeline::new(key("sample:generator"), 1, vec![Arc::new(Terrain)], 65536)?;
    let near = ChunkRegion::new(ChunkCoord::new(0, 0)?, 2, 2)?;
    let remote = ChunkRegion::single(ChunkCoord::new(1000, 100)?)?;
    for region in [near, remote] {
        let request = world.edit(|edit| {
            edit.request_generation(key("sample:exploration"), region, &pipeline, 0)
        })?;
        let task = world.edit(|edit| edit.begin_generation(request))?;
        // 別threadへ渡してもWorldを共有しません。採用は次の明示操作で行います。
        let worker = Arc::clone(&pipeline);
        let result = std::thread::spawn(move || task.run(&worker))
            .join()
            .map_err(|_| EngineError::new(ErrorCode::InvalidState, "作例の生成worker失敗"))?;
        world.edit(|edit| edit.submit_generation(result))?;
        world.edit(|edit| edit.publish_generation(request))?;
    }
    let front = content.layer_id(key("sample:foreground"))?;
    let chest = world.edit(|edit| {
        edit.place_block(
            front,
            point(63, 63),
            MaterialHandle(key("sample:wood")),
            Orientation::Right,
        )
    })?;
    world.edit(|edit| {
        edit.set_block_state(
            chest,
            key("sample:stored-items"),
            key("sample:count"),
            &17u32,
        )?;
        edit.set_channel(key("sample:water"), point(10, 10), 127u8)
    })?;
    let factory = world.edit(|edit| {
        edit.acquire_lease(RegionLease {
            source: key("sample:remote-factory"),
            region: remote,
            mode: LeaseMode::Simulate,
            expires: None,
        })
    })?;
    let mut camera = Camera2d::screen(64, 64);
    camera.position = [64.0, 64.0];
    let observation = CameraObservation::capture(
        key("sample:camera"),
        &camera,
        &world,
        ObserverMargins {
            simulation: 0,
            resident: 0,
        },
    )?;
    assert!(observation.missing_chunks(&world, 64)?.is_empty());
    world.edit(|edit| edit.apply_camera_observation(observation))?;
    assert_eq!(world.active_chunks().count(), 5);
    world.edit(|edit| {
        edit.apply_camera_observation(CameraObservation::disabled(key("sample:camera")))
    })?;
    // カメラの外でも、別sourceの工場はSimulate leaseで動作対象に残ります。
    assert_eq!(world.active_chunks().count(), 1);
    let mut archives = DirectoryArchives::new("target/e04-artifacts/regions")?;
    let stored = world
        .prepare_eviction(near, 8 * 1024 * 1024)?
        .store(&mut archives)?;
    world.edit(|edit| edit.finish_eviction(stored))?;
    assert_eq!(world.resident_chunks(), 1);
    let mut saves = SaveRepository::new(
        DirectoryStorage::new("target/e04-artifacts/save")?,
        SaveLimits::default(),
        3,
    )?;
    let profile = WorldSaveProfile {
        scope: key("sample:world"),
        profile: key("sample:scalar"),
        plugins: vec![],
    };
    let receipt = world.save(&mut saves, &mut archives, &profile, 16 * 1024 * 1024)?;
    let mut restored = World::load(
        &mut saves,
        &mut archives,
        Arc::clone(&content),
        &profile,
        WorldLimits::default(),
        16 * 1024 * 1024,
    )?;
    let loaded = restored
        .region_load_task(ChunkCoord::new(0, 0)?, 8 * 1024 * 1024)?
        .load(&mut archives)?;
    restored.edit(|edit| edit.restore_region(loaded))?;
    assert_eq!(
        restored.block_state::<u32>(chest, key("sample:stored-items"))?,
        17
    );
    assert!(matches!(
        restored.channel_value::<u8>(key("sample:water"), point(10, 10))?,
        ChannelQuery::Loaded(127)
    ));
    assert!(restored.lease(factory).is_ok());
    println!(
        "論理世界：131072 × 32768 cell、resident：{} chunk、地形配列：{} byte、保存世代：{}",
        restored.resident_chunks(),
        restored.terrain_bytes(),
        receipt.generation
    );
    println!("跨chunk家具の個体ID・作者状態、部分液量、遠隔lease、退避区域を復元しました。");
    Ok(())
}
