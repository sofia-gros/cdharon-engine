//! 広域DDAの固定budget・save/restore・公開失効・全payload輸送の回帰試験。
use charon_engine::{
    foundation::{
        StableKey,
        codec::{Decoder, Encoder, StateCodec},
        grid::*,
        id::WorldId,
        schema::Schema,
    },
    world::*,
};
use std::sync::Arc;

fn key(s: &str) -> StableKey {
    StableKey::parse(s).unwrap()
}
fn point(x: i32, y: i32) -> CellPoint {
    CellPoint { x, y }
}
fn create() -> (World, LayerId) {
    let mut registry = ContentRegistry::new(ContentLimits::default()).unwrap();
    registry
        .layer(LayerDef {
            key: key("test:terrain"),
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    registry
        .register(MaterialBuilder::new("test:sand", MaterialBase::Powder).unwrap())
        .unwrap();
    registry
        .register(MaterialBuilder::new("test:wall", MaterialBase::Solid).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<MaterialMotion>(key("test:motion-schema"), 1, 24).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<MaterialMotionJob>(key("test:job-schema"), 1, 500_000).unwrap())
        .unwrap();
    registry
        .material_channel(
            key("test:terrain"),
            ChannelDefinition {
                key: key("test:motion"),
                version: 1,
                value_schema: key("test:motion-schema"),
                layout: GridLayout {
                    origin: point(0, 0),
                    x_scale: GridScale::new(1, 1).unwrap(),
                    y_scale: GridScale::new(1, 1).unwrap(),
                },
                default: MaterialMotion::default(),
                limits: GridLimits {
                    chunks: 8,
                    checkpoint_bytes: 1_000_000,
                },
            },
        )
        .unwrap();
    let content = registry.freeze().unwrap();
    let layer = content.layer_id(key("test:terrain")).unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        42,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.insert_empty_chunk(ChunkCoord::new(1, 0)?)?;
            edit.acquire_lease(RegionLease {
                source: key("test:active"),
                region: ChunkRegion::new(ChunkCoord::new(0, 0)?, 2, 1)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })
        })
        .unwrap();
    (world, layer)
}
fn place(world: &mut World, layer: LayerId, position: CellPoint, name: &str, speed: [i64; 2]) {
    let material = world
        .content()
        .material_id(MaterialHandle(key(name)))
        .unwrap();
    world
        .edit(|edit| {
            edit.set_cell(
                layer,
                position,
                CellCore {
                    material,
                    ..CellCore::AIR
                },
            )?;
            edit.set_channel(
                key("test:motion"),
                position,
                MaterialMotion {
                    velocity_q16: speed,
                    remainder_q16: [0, 0],
                },
            )
        })
        .unwrap();
}
fn material(world: &World, layer: LayerId, position: CellPoint) -> MaterialId {
    match world.cell(layer, position).unwrap() {
        CellQuery::Loaded(cell) => cell.core.material,
        _ => panic!("このテストではresidentである必要があります"),
    }
}
fn speed(world: &World, position: CellPoint) -> MaterialMotion {
    match world.channel_value(key("test:motion"), position).unwrap() {
        ChannelQuery::Loaded(value) => value,
        _ => panic!("速度channelが必要"),
    }
}
fn job(world: &World, layer: LayerId, start: CellPoint) -> MaterialMotionJob {
    MaterialMotionJob::new(world, layer, start, key("test:motion"), 128, 128, 2).unwrap()
}
fn save(world: &World) -> Vec<u8> {
    world.checkpoint(8_000_000).unwrap()
}
fn restore(world: &World) -> World {
    let data = save(world);
    World::restore(
        &data,
        Arc::clone(world.content()),
        WorldLimits::default(),
        8_000_000,
    )
    .unwrap()
}
fn roundtrip(input: &MaterialMotionJob) -> MaterialMotionJob {
    let mut out = Encoder::new(500_000);
    input.encode(&mut out).unwrap();
    let data = out.finish();
    let mut decoder = Decoder::new(&data, data.len()).unwrap();
    let job = MaterialMotionJob::decode(&mut decoder).unwrap();
    decoder.finish().unwrap();
    job
}

#[test]
fn wide_job_budget_one_and_save_restore_match_reference_transport() {
    let (mut world, layer) = create();
    let start = point(7, 1);
    let target = point(77, 1);
    place(&mut world, layer, start, "test:sand", [70 * 65536, 0]);
    let mut reference = restore(&world);
    let mut running = job(&world, layer, start);
    assert_eq!(running.status(), MaterialMotionJobStatus::Computing);
    assert_eq!(
        running.step(&world, 1).unwrap(),
        MaterialMotionJobStatus::Computing
    );
    assert_eq!(running.report().crossed_cells, 1);
    assert_eq!(
        material(&world, layer, start),
        material(&reference, layer, start)
    );
    for _ in 1..31 {
        assert_eq!(
            running.step(&world, 1).unwrap(),
            MaterialMotionJobStatus::Computing
        );
    }
    // 処理途中も同じWorldのStateStoreへcursorを保存でき、World復元後に続けられる。
    world
        .edit(|edit| edit.set_state(key("test:job"), key("test:job-schema"), &running))
        .unwrap();
    let mut world = restore(&world);
    running = world.state::<MaterialMotionJob>(key("test:job")).unwrap();
    assert_eq!(running.report().crossed_cells, 31);
    running = roundtrip(&running);
    while running.status() == MaterialMotionJobStatus::Computing {
        running.step(&world, 1).unwrap();
    }
    assert_eq!(running.report().checked_steps, 70);
    assert_eq!(running.report().crossed_cells, 70);
    assert_eq!(
        material(&world, layer, start),
        world
            .content()
            .material_id(MaterialHandle(key("test:sand")))
            .unwrap()
    );
    world.edit(|edit| running.publish(edit)).unwrap();
    reference
        .edit(|edit| {
            edit.transport_material_motion(
                layer,
                MaterialMotionConfig {
                    channel: key("test:motion"),
                    candidate_limit: 3,
                    max_distance: 128,
                    step_limit: 200,
                },
            )
        })
        .unwrap();
    assert_eq!(material(&world, layer, start), MaterialId::AIR);
    assert_eq!(
        material(&world, layer, target),
        material(&reference, layer, target)
    );
    assert_eq!(speed(&world, target), speed(&reference, target));
    assert_eq!(running.report().crossed_cells, 70);
}

#[test]
fn wide_job_collision_returns_partial_path_and_never_jumps_wall() {
    let (mut world, layer) = create();
    place(&mut world, layer, point(8, 8), "test:sand", [70 * 65536, 0]);
    place(&mut world, layer, point(12, 8), "test:wall", [0, 0]);
    let running = job(&world, layer, point(8, 8));
    let mut running = roundtrip(&running);
    assert_eq!(
        running.step(&world, 3).unwrap(),
        MaterialMotionJobStatus::Computing
    );
    assert_eq!(
        running.step(&world, 1).unwrap(),
        MaterialMotionJobStatus::Ready
    );
    assert!(running.report().collision);
    assert_eq!(running.report().crossed_cells, 3);
    world.edit(|edit| running.publish(edit)).unwrap();
    let sand = world
        .content()
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    let wall = world
        .content()
        .material_id(MaterialHandle(key("test:wall")))
        .unwrap();
    assert_eq!(material(&world, layer, point(11, 8)), sand);
    assert_eq!(material(&world, layer, point(12, 8)), wall);
    assert_eq!(speed(&world, point(11, 8)), MaterialMotion::default());
}

#[test]
fn stale_input_unfinished_publish_and_capacity_fail_without_mutating_world() {
    let (mut world, layer) = create();
    place(&mut world, layer, point(8, 8), "test:sand", [5 * 65536, 0]);
    let mut running = job(&world, layer, point(8, 8));
    let baseline = save(&world);
    assert!(world.edit(|edit| running.publish(edit)).is_err());
    assert_eq!(save(&world), baseline);
    let mut too_small =
        MaterialMotionJob::new(&world, layer, point(8, 8), key("test:motion"), 64, 2, 2).unwrap();
    assert_eq!(
        too_small.step(&world, 1).unwrap(),
        MaterialMotionJobStatus::Computing
    );
    assert!(too_small.step(&world, 1).is_err());
    assert_eq!(save(&world), baseline);
    running.step(&world, 2).unwrap();
    place(&mut world, layer, point(16, 8), "test:wall", [0, 0]);
    let changed = save(&world);
    assert!(running.step(&world, 1).is_err());
    assert!(world.edit(|edit| running.publish(edit)).is_err());
    assert_eq!(save(&world), changed);
}

#[test]
fn diagonal_job_checks_both_corner_cells_and_cannot_slip_through_a_wall() {
    let (mut world, layer) = create();
    place(
        &mut world,
        layer,
        point(8, 8),
        "test:sand",
        [2 * 65536, 2 * 65536],
    );
    place(&mut world, layer, point(9, 8), "test:wall", [0, 0]);
    let before = save(&world);
    let mut running = job(&world, layer, point(8, 8));
    assert_eq!(
        running.step(&world, 1).unwrap(),
        MaterialMotionJobStatus::Ready
    );
    assert!(running.report().collision);
    assert_eq!(running.report().crossed_cells, 0);
    assert_eq!(save(&world), before);
    world.edit(|edit| running.publish(edit)).unwrap();
    let sand = world
        .content()
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    assert_eq!(material(&world, layer, point(8, 8)), sand);
    assert_eq!(speed(&world, point(8, 8)), MaterialMotion::default());
}

#[test]
fn restored_job_rejects_forged_path_that_disagrees_with_dda() {
    let (mut world, layer) = create();
    place(&mut world, layer, point(7, 1), "test:sand", [3 * 65536, 0]);
    let mut running = job(&world, layer, point(7, 1));
    running.step(&world, 2).unwrap();
    let mut encoder = Encoder::new(500_000);
    running.encode(&mut encoder).unwrap();
    let mut bytes = encoder.finish();
    // 進捗cursor自体は正しいまま、経路のx座標だけを変更したbyte列を作る。
    // 内部のDDA経路検査は、単なる「隣接した経路」以上を検証する。
    let target = [8u8, 0, 0, 0, 1, 0, 0, 0];
    let candidate = bytes
        .windows(target.len())
        .position(|window| window == target)
        .expect("最初のDDA経路の保存点");
    bytes[candidate] = 9;
    let mut input = Decoder::new(&bytes, bytes.len()).unwrap();
    assert!(MaterialMotionJob::decode(&mut input).is_err());
}
