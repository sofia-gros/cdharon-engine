//! Q16の物性・独自力学provider・DDA統合を公開APIから検証します。
use charon_engine::{
    foundation::{EngineResult, StableKey, grid::*, id::WorldId, schema::Schema},
    world::*,
};
use std::sync::Arc;

fn key(text: &str) -> StableKey {
    StableKey::parse(text).unwrap()
}
fn p(x: i32, y: i32) -> CellPoint {
    CellPoint { x, y }
}
fn physics(
    base: MaterialBase,
    gravity: u32,
    drag: u32,
    buoyancy: u32,
    friction: u32,
) -> MaterialMotionProperties {
    let _ = base;
    MaterialMotionProperties {
        gravity_scale_q16: gravity,
        drag_q16: drag,
        buoyancy_scale_q16: buoyancy,
        friction_q16: friction,
    }
}
fn content(sand_drag: u32) -> Arc<Content> {
    let mut registry = ContentRegistry::new(ContentLimits::default()).unwrap();
    registry
        .layer(LayerDef {
            key: key("test:layer"),
            role: LayerRole::Terrain,
            render_order: 0,
            collision_group: 1,
            simulated: true,
        })
        .unwrap();
    for (name, base, mass, properties) in [
        (
            "test:sand",
            MaterialBase::Powder,
            4,
            physics(MaterialBase::Powder, 65536, sand_drag, 0, 32768),
        ),
        (
            "test:liquid",
            MaterialBase::Liquid,
            2,
            physics(MaterialBase::Liquid, 65536, 0, 65536, 0),
        ),
        (
            "test:water",
            MaterialBase::Liquid,
            10,
            physics(MaterialBase::Liquid, 0, 0, 0, 0),
        ),
        (
            "test:wall",
            MaterialBase::Solid,
            100,
            physics(MaterialBase::Solid, 0, 0, 0, 0),
        ),
    ] {
        registry
            .register(
                MaterialBuilder::new(name, base)
                    .unwrap()
                    .mass_per_cell(mass)
                    .motion_properties(properties),
            )
            .unwrap();
    }
    registry
        .schema(Schema::of::<MaterialMotion>(key("test:motion-schema"), 1, 24).unwrap())
        .unwrap();
    registry
        .material_channel(
            key("test:layer"),
            ChannelDefinition {
                key: key("test:motion"),
                version: 1,
                value_schema: key("test:motion-schema"),
                layout: GridLayout {
                    origin: p(0, 0),
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
    registry.freeze().unwrap()
}
fn fixture() -> (World, LayerId) {
    let content = content(32768);
    let layer = content.layer_id(key("test:layer")).unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        7,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    world
        .edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.acquire_lease(RegionLease {
                source: key("test:active"),
                region: ChunkRegion::single(ChunkCoord::new(0, 0)?)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })
        })
        .unwrap();
    (world, layer)
}
fn place(world: &mut World, layer: LayerId, point: CellPoint, name: &str, velocity: [i64; 2]) {
    let material = world
        .content()
        .material_id(MaterialHandle(key(name)))
        .unwrap();
    world
        .edit(|edit| {
            edit.set_cell(
                layer,
                point,
                CellCore {
                    material,
                    ..CellCore::AIR
                },
            )?;
            edit.set_channel(
                key("test:motion"),
                point,
                MaterialMotion {
                    velocity_q16: velocity,
                    ..Default::default()
                },
            )
        })
        .unwrap();
}
fn motion(world: &World, point: CellPoint) -> MaterialMotion {
    match world.channel_value(key("test:motion"), point).unwrap() {
        ChannelQuery::Loaded(value) => value,
        _ => panic!("速度channelが必要"),
    }
}
fn force(limit: i64) -> MaterialForceConfig {
    MaterialForceConfig {
        channel: key("test:motion"),
        gravity_q16: [0, 65536],
        candidate_limit: 100,
        max_velocity_q16: limit,
    }
}
fn travel() -> MaterialMotionConfig {
    MaterialMotionConfig {
        channel: key("test:motion"),
        candidate_limit: 100,
        max_distance: 8,
        step_limit: 500,
    }
}

#[test]
fn gravity_drag_and_surface_friction_are_deterministic_q16() {
    let (mut world, layer) = fixture();
    place(&mut world, layer, p(8, 8), "test:sand", [65536, 0]);
    place(&mut world, layer, p(8, 9), "test:wall", [0, 0]);
    let report = world
        .edit(|edit| {
            edit.integrate_material_forces(layer, force(1_000_000), &StandardMaterialForces)
        })
        .unwrap();
    assert_eq!(report.candidates, 2);
    assert_eq!(report.changed, 1);
    assert_eq!(motion(&world, p(8, 8)).velocity_q16, [16384, 65536]);
    let checkpoint = world.checkpoint(4_000_000).unwrap();
    let restored = World::restore(
        &checkpoint,
        Arc::clone(world.content()),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    assert_eq!(motion(&restored, p(8, 8)).velocity_q16, [16384, 65536]);
}

#[test]
fn buoyancy_from_denser_liquid_reduces_gravity_without_rounding_to_zero() {
    let (mut world, layer) = fixture();
    place(&mut world, layer, p(8, 8), "test:liquid", [0, 0]);
    place(&mut world, layer, p(8, 9), "test:water", [0, 0]);
    world
        .edit(|edit| {
            edit.integrate_material_forces(layer, force(1_000_000), &StandardMaterialForces)
        })
        .unwrap();
    // 1 - (10 - 2)/10 = 1/5の重力が残る。正規の整数演算で切り捨てます。
    assert_eq!(motion(&world, p(8, 8)).velocity_q16, [0, 13108]);
}

#[test]
fn custom_provider_replaces_standard_gravity_and_atomic_failure_rolls_back() {
    struct NoGravity;
    impl MaterialForceProvider for NoGravity {
        fn key(&self) -> StableKey {
            key("test:no-gravity")
        }
        fn version(&self) -> u32 {
            1
        }
        fn integrate(&self, input: MaterialForceInput<'_>) -> EngineResult<MaterialMotion> {
            Ok(input.motion)
        }
    }
    let (mut world, layer) = fixture();
    place(&mut world, layer, p(8, 8), "test:sand", [0, 0]);
    world
        .edit(|edit| edit.integrate_material_forces(layer, force(1_000_000), &NoGravity))
        .unwrap();
    // 内容が不変でもWorld::editの成功はrevisionを進めます。
    assert_eq!(motion(&world, p(8, 8)).velocity_q16, [0, 0]);
    let unchanged = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .edit(|edit| edit.integrate_material_forces(layer, force(100), &StandardMaterialForces))
            .is_err()
    );
    assert!(world.checkpoint(4_000_000).unwrap() == unchanged);
}

#[test]
fn force_and_motion_publish_once_and_recover_from_checkpoint() {
    let (mut world, layer) = fixture();
    place(&mut world, layer, p(8, 8), "test:liquid", [0, 0]);
    let before = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .edit(|edit| {
                edit.step_material_force_motion(
                    layer,
                    force(100),
                    travel(),
                    &StandardMaterialForces,
                )
            })
            .is_err()
    );
    assert_eq!(world.checkpoint(4_000_000).unwrap(), before);
    let report = world
        .edit(|edit| {
            edit.step_material_force_motion(
                layer,
                force(1_000_000),
                travel(),
                &StandardMaterialForces,
            )
        })
        .unwrap();
    assert_eq!(report.force.changed, 1);
    assert_eq!(report.motion.moved, 1);
    assert!(matches!(
        world.cell(layer, p(8, 9)).unwrap(),
        CellQuery::Loaded(cell) if cell.core.material != MaterialId::AIR
    ));
    let checkpoint = world.checkpoint(4_000_000).unwrap();
    let restored = World::restore(
        &checkpoint,
        Arc::clone(world.content()),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    assert_eq!(restored.checkpoint(4_000_000).unwrap(), checkpoint);
}

#[test]
fn motion_properties_are_validated_and_affect_content_identity() {
    let default = {
        let mut registry = ContentRegistry::new(ContentLimits::default()).unwrap();
        registry
            .layer(LayerDef {
                key: key("test:layer"),
                role: LayerRole::Terrain,
                render_order: 0,
                collision_group: 1,
                simulated: true,
            })
            .unwrap();
        registry
            .register(MaterialBuilder::new("test:sample", MaterialBase::Powder).unwrap())
            .unwrap();
        registry.freeze().unwrap()
    };
    let customized = {
        let mut registry = ContentRegistry::new(ContentLimits::default()).unwrap();
        registry
            .layer(LayerDef {
                key: key("test:layer"),
                role: LayerRole::Terrain,
                render_order: 0,
                collision_group: 1,
                simulated: true,
            })
            .unwrap();
        assert!(
            registry
                .register(
                    MaterialBuilder::new("test:bad", MaterialBase::Powder)
                        .unwrap()
                        .motion_properties(physics(MaterialBase::Powder, 65536, 65537, 0, 0))
                )
                .is_err()
        );
        registry
            .register(
                MaterialBuilder::new("test:sample", MaterialBase::Powder)
                    .unwrap()
                    .motion_properties(physics(MaterialBase::Powder, 65536, 32768, 0, 0)),
            )
            .unwrap();
        registry.freeze().unwrap()
    };
    assert_ne!(default.fingerprint(), customized.fingerprint());
}
