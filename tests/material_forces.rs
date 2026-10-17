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

fn particle_config(cap: i64) -> MaterialParticleForceConfig {
    MaterialParticleForceConfig {
        gravity_q16: [0, 32768],
        candidate_limit: 4,
        max_velocity_q16: cap,
    }
}

#[test]
fn material_particle_forces_keep_q16_position_and_survive_checkpoint_resume() {
    let (mut world, layer) = fixture();
    place(&mut world, layer, p(8, 8), "test:sand", [0, 0]);
    let handle = world
        .edit(|edit| edit.emit_material_particle(layer, p(8, 8), [0, 0]))
        .unwrap();
    let first = world
        .edit(|edit| {
            edit.step_material_particle_forces(
                particle_config(1_000_000),
                &StandardMaterialForces,
                100,
            )
        })
        .unwrap();
    assert_eq!(first.force.changed, 1);
    assert_eq!(first.particles.processed, 1);
    assert_eq!(
        world.material_particle(handle).unwrap().velocity_q16(),
        [0, 32768]
    );
    let checkpoint = world.checkpoint(4_000_000).unwrap();
    let mut resumed = World::restore(
        &checkpoint,
        Arc::clone(world.content()),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    world
        .edit(|edit| {
            edit.step_material_particle_forces(
                particle_config(1_000_000),
                &StandardMaterialForces,
                100,
            )
        })
        .unwrap();
    resumed
        .edit(|edit| {
            edit.step_material_particle_forces(
                particle_config(1_000_000),
                &StandardMaterialForces,
                100,
            )
        })
        .unwrap();
    assert!(world.checkpoint(4_000_000).unwrap() == resumed.checkpoint(4_000_000).unwrap());
    assert_eq!(world.material_particle(handle).unwrap().cell(), p(8, 9));
}

#[test]
fn particle_force_failure_rolls_back_velocity_and_material_ownership() {
    struct RejectRemainder;
    impl MaterialForceProvider for RejectRemainder {
        fn key(&self) -> StableKey {
            key("test:bad-particle-remainder")
        }
        fn version(&self) -> u32 {
            1
        }
        fn integrate(&self, mut input: MaterialForceInput<'_>) -> EngineResult<MaterialMotion> {
            assert!(input.particle.is_some());
            input.motion.remainder_q16 = [1, 0];
            Ok(input.motion)
        }
    }
    let (mut world, layer) = fixture();
    place(&mut world, layer, p(8, 8), "test:sand", [0, 0]);
    let handle = world
        .edit(|edit| edit.emit_material_particle(layer, p(8, 8), [0, 0]))
        .unwrap();
    let before = world.checkpoint(4_000_000).unwrap();
    assert!(
        world
            .edit(|edit| edit
                .integrate_material_particle_forces(particle_config(100), &StandardMaterialForces))
            .is_err()
    );
    assert!(world.checkpoint(4_000_000).unwrap() == before);
    assert!(
        world
            .edit(|edit| edit
                .integrate_material_particle_forces(particle_config(1_000_000), &RejectRemainder))
            .is_err()
    );
    assert!(world.checkpoint(4_000_000).unwrap() == before);
    assert_eq!(
        world.material_particle(handle).unwrap().velocity_q16(),
        [0, 0]
    );
}

fn zero_gravity() -> MaterialForceConfig {
    MaterialForceConfig {
        gravity_q16: [0, 0],
        ..force(1_000_000)
    }
}

#[test]
fn sleeping_standard_force_matches_scalar_reference_every_tick_and_wakes_on_edit() {
    let (mut reference, layer) = fixture();
    place(&mut reference, layer, p(8, 8), "test:sand", [0, 0]);
    let bytes = reference.checkpoint(4_000_000).unwrap();
    let mut optimized = World::restore(
        &bytes,
        Arc::clone(reference.content()),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    let mut tracker = MaterialSleepTracker::default();
    for index in 0..8 {
        let plain = reference
            .edit(|edit| {
                edit.step_material_force_motion(
                    layer,
                    zero_gravity(),
                    travel(),
                    &StandardMaterialForces,
                )
            })
            .unwrap();
        let cached = optimized
            .step_standard_material_force_motion_sleeping(
                &mut tracker,
                layer,
                zero_gravity(),
                travel(),
            )
            .unwrap();
        assert_eq!(plain, cached.reference);
        assert_eq!(cached.sleeping, index != 0);
        assert_eq!(
            reference.checkpoint(4_000_000).unwrap(),
            optimized.checkpoint(4_000_000).unwrap()
        );
        reference.edit(|edit| edit.advance_tick()).unwrap();
        optimized.edit(|edit| edit.advance_tick()).unwrap();
    }
    // channelを別取引で変更するとchunk revisionが変わるのでwakeします。
    for world in [&mut reference, &mut optimized] {
        world
            .edit(|edit| {
                edit.set_channel(
                    key("test:motion"),
                    p(8, 8),
                    MaterialMotion {
                        velocity_q16: [65536, 0],
                        ..Default::default()
                    },
                )
            })
            .unwrap();
    }
    let plain = reference
        .edit(|edit| {
            edit.step_material_force_motion(
                layer,
                zero_gravity(),
                travel(),
                &StandardMaterialForces,
            )
        })
        .unwrap();
    let cached = optimized
        .step_standard_material_force_motion_sleeping(&mut tracker, layer, zero_gravity(), travel())
        .unwrap();
    assert!(!cached.sleeping);
    assert!(cached.woke);
    assert_eq!(cached.reference, plain);
    assert_eq!(
        reference.checkpoint(4_000_000).unwrap(),
        optimized.checkpoint(4_000_000).unwrap()
    );
}

#[test]
fn sleeping_solid_is_woken_by_falling_material_and_never_skips_nonzero_gravity() {
    let (mut reference, layer) = fixture();
    place(&mut reference, layer, p(8, 8), "test:wall", [0, 0]);
    let bytes = reference.checkpoint(4_000_000).unwrap();
    let mut optimized = World::restore(
        &bytes,
        Arc::clone(reference.content()),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    let mut tracker = MaterialSleepTracker::default();
    let plain = reference
        .edit(|edit| {
            edit.step_material_force_motion(
                layer,
                force(1_000_000),
                travel(),
                &StandardMaterialForces,
            )
        })
        .unwrap();
    let first = optimized
        .step_standard_material_force_motion_sleeping(
            &mut tracker,
            layer,
            force(1_000_000),
            travel(),
        )
        .unwrap();
    assert_eq!(plain, first.reference);
    assert!(tracker.has_proof());
    let plain = reference
        .edit(|edit| {
            edit.step_material_force_motion(
                layer,
                force(1_000_000),
                travel(),
                &StandardMaterialForces,
            )
        })
        .unwrap();
    let slept = optimized
        .step_standard_material_force_motion_sleeping(
            &mut tracker,
            layer,
            force(1_000_000),
            travel(),
        )
        .unwrap();
    assert!(slept.sleeping);
    assert_eq!(plain, slept.reference);
    for world in [&mut reference, &mut optimized] {
        place(world, layer, p(10, 8), "test:sand", [0, 0]);
    }
    let plain = reference
        .edit(|edit| {
            edit.step_material_force_motion(
                layer,
                force(1_000_000),
                travel(),
                &StandardMaterialForces,
            )
        })
        .unwrap();
    let wake = optimized
        .step_standard_material_force_motion_sleeping(
            &mut tracker,
            layer,
            force(1_000_000),
            travel(),
        )
        .unwrap();
    assert!(!wake.sleeping);
    assert!(wake.woke);
    assert_eq!(plain, wake.reference);
    assert!(!tracker.has_proof());
    assert_eq!(
        reference.checkpoint(4_000_000).unwrap(),
        optimized.checkpoint(4_000_000).unwrap()
    );
}

#[test]
fn sleeping_proof_is_codec_safe_and_wrong_profile_falls_back_to_reference() {
    use charon_engine::foundation::codec::{Decoder, Encoder, StateCodec};
    let (mut reference, layer) = fixture();
    place(&mut reference, layer, p(8, 8), "test:sand", [0, 0]);
    let mut tracker = MaterialSleepTracker::default();
    reference
        .step_standard_material_force_motion_sleeping(&mut tracker, layer, zero_gravity(), travel())
        .unwrap();
    assert!(tracker.has_proof());
    let mut encoder = Encoder::new(1_000_000);
    tracker.encode(&mut encoder).unwrap();
    let bytes = encoder.finish();
    let mut reader = Decoder::new(&bytes, bytes.len()).unwrap();
    let mut restored_cache = MaterialSleepTracker::decode(&mut reader).unwrap();
    reader.finish().unwrap();
    let world_bytes = reference.checkpoint(4_000_000).unwrap();
    let mut restored_world = World::restore(
        &world_bytes,
        Arc::clone(reference.content()),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    // 保存復元後はrevision差分の通知がないため、危険なskipをせず再評価。
    let plain = reference
        .edit(|edit| {
            edit.step_material_force_motion(
                layer,
                zero_gravity(),
                travel(),
                &StandardMaterialForces,
            )
        })
        .unwrap();
    let result = restored_world
        .step_standard_material_force_motion_sleeping(
            &mut restored_cache,
            layer,
            zero_gravity(),
            travel(),
        )
        .unwrap();
    assert!(!result.sleeping);
    assert_eq!(result.reference, plain);
    assert_eq!(
        reference.checkpoint(4_000_000).unwrap(),
        restored_world.checkpoint(4_000_000).unwrap()
    );
    // 証明は候補予算にも紐づき、より小さい予算へ再利用して失敗を隠さない。
    let unchanged = restored_world.checkpoint(4_000_000).unwrap();
    let mut smaller = travel();
    smaller.candidate_limit = 0;
    assert!(
        restored_world
            .step_standard_material_force_motion_sleeping(
                &mut restored_cache,
                layer,
                zero_gravity(),
                smaller,
            )
            .is_err()
    );
    assert_eq!(restored_world.checkpoint(4_000_000).unwrap(), unchanged);
}

#[test]
fn unavailable_neighbor_does_not_turn_reference_wait_into_false_sleep() {
    let (mut reference, layer) = fixture();
    // 重力倍率が0の静止素材でも、参照forceは重力側の未読込を
    // waiting_for_regionへ記録します。sleepがこの結果を消してはいけません。
    place(&mut reference, layer, p(8, 63), "test:wall", [0, 0]);
    let snapshot = reference.checkpoint(4_000_000).unwrap();
    let mut optimized = World::restore(
        &snapshot,
        Arc::clone(reference.content()),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    let mut tracker = MaterialSleepTracker::default();
    for _ in 0..3 {
        let normal = reference
            .edit(|edit| {
                edit.step_material_force_motion(
                    layer,
                    force(1_000_000),
                    travel(),
                    &StandardMaterialForces,
                )
            })
            .unwrap();
        assert_eq!(normal.force.waiting_for_region, 1);
        let result = optimized
            .step_standard_material_force_motion_sleeping(
                &mut tracker,
                layer,
                force(1_000_000),
                travel(),
            )
            .unwrap();
        assert_eq!(result.reference, normal);
        assert!(!result.sleeping);
        assert!(!tracker.has_proof());
        assert_eq!(
            reference.checkpoint(4_000_000).unwrap(),
            optimized.checkpoint(4_000_000).unwrap(),
        );
    }
}

#[test]
fn force_proof_from_fork_with_same_world_and_chunk_revision_cannot_skip_motion() {
    let (mut source, layer) = fixture();
    place(&mut source, layer, p(8, 8), "test:wall", [0, 0]);
    let base = source.checkpoint(4_000_000).unwrap();
    let mut stationary = World::restore(
        &base,
        Arc::clone(source.content()),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    let mut falling = World::restore(
        &base,
        Arc::clone(source.content()),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    place(&mut stationary, layer, p(8, 8), "test:wall", [0, 0]);
    place(&mut falling, layer, p(8, 8), "test:sand", [0, 0]);
    let mut proven = MaterialSleepTracker::default();
    stationary
        .step_standard_material_force_motion_sleeping(
            &mut proven,
            layer,
            force(1_000_000),
            travel(),
        )
        .unwrap();
    assert!(proven.has_proof());
    falling.edit(|_| Ok(())).unwrap();
    assert_eq!(stationary.revision(), falling.revision());
    let mut foreign = proven.clone();
    let result = falling
        .step_standard_material_force_motion_sleeping(
            &mut foreign,
            layer,
            force(1_000_000),
            travel(),
        )
        .unwrap();
    assert!(!result.sleeping, "別枝の同revision証明を信頼しない");
    assert!(result.woke);
    let sand = source
        .content()
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    assert!(matches!(
        falling.cell(layer, p(8, 9)).unwrap(),
        CellQuery::Loaded(cell) if cell.core.material == sand
    ));
}
