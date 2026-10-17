//! E05収支監査をRuntime Materials barrierへ接続する公開APIの統合試験。
use charon_engine::{
    foundation::{
        EngineError, EngineResult, ErrorCode, StableKey,
        grid::{CellPoint, GridLayout, GridLimits, GridScale},
        schema::Schema,
    },
    runtime::{
        Engine, StepResult,
        input::TickInput,
        schedule::{Phase, System, SystemContext, SystemDescriptor, WorkStatus},
    },
    world::*,
};
use std::sync::Arc;

fn key(s: &str) -> StableKey {
    StableKey::parse(s).unwrap()
}
fn p(x: i32, y: i32) -> CellPoint {
    CellPoint { x, y }
}
fn profile() -> MaterialInventoryProfile {
    MaterialInventoryProfile::new(
        key("test:inventory"),
        1,
        Vec::new(),
        Vec::new(),
        MaterialInventoryLimits {
            atoms: 128,
            components: 8,
        },
    )
    .unwrap()
}
struct NoArchives;
impl ArchiveStorage for NoArchives {
    fn put(&mut self, _hash: [u8; 32], _bytes: &[u8]) -> EngineResult<SaveDurability> {
        Err(EngineError::new(
            ErrorCode::InvalidState,
            "テストにarchiveなし",
        ))
    }
    fn read(&mut self, _hash: [u8; 32], _limit: usize) -> EngineResult<Vec<u8>> {
        Err(EngineError::new(
            ErrorCode::InvalidState,
            "テストにarchiveなし",
        ))
    }
}
fn build_content() -> Arc<Content> {
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
        .register(MaterialBuilder::new("test:sand", MaterialBase::Powder).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<MaterialBalanceLog>(key("test:balance-schema"), 1, 1_000_000).unwrap())
        .unwrap();
    registry
        .schema(Schema::of::<u64>(key("test:debug-schema"), 1, 8).unwrap())
        .unwrap();
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

struct AuditedMaterialSystem {
    profile: Arc<MaterialInventoryProfile>,
    inject_unrecorded_source: bool,
    swallow_nested_error: bool,
    unaccounted_raw: bool,
    unaccounted_raw_state: bool,
    recorded_source: bool,
    raw_then_audited: bool,
    apply_pipeline: bool,
}
impl System for AuditedMaterialSystem {
    fn run(&mut self, ctx: &mut SystemContext<'_>) -> EngineResult<WorkStatus> {
        let mut worlds = ctx.worlds();
        let id = worlds.view().id("world")?;
        let (archive, layer, material) = {
            let view = worlds.view();
            let terrain = view.terrain(id)?;
            (
                terrain.load_material_archive_inventory(
                    &self.profile,
                    &mut NoArchives,
                    1_000_000,
                )?,
                terrain.content().layer_id(key("test:layer"))?,
                terrain
                    .content()
                    .material_id(MaterialHandle(key("test:sand")))?,
            )
        };
        let profile = Arc::clone(&self.profile);
        let fail = self.inject_unrecorded_source;
        let swallow = self.swallow_nested_error;
        let recorded_source = self.recorded_source;
        let apply_pipeline = self.apply_pipeline;
        if self.raw_then_audited {
            // 同tickの先行barrierが物質を未記録で発行する。後続の監査付き
            // 原子取引はその改竄を見逃さず、Runtime全体の候補を破棄する。
            worlds.edit_terrain(id, move |edit| {
                edit.set_cell(
                    layer,
                    p(3, 1),
                    CellCore {
                        material,
                        ..CellCore::AIR
                    },
                )
            })?;
        }
        if self.unaccounted_raw_state {
            worlds.set(
                id,
                key("test:debug-record"),
                key("test:debug-schema"),
                &7u64,
            )?;
            return Ok(WorkStatus::Complete);
        }
        if self.unaccounted_raw {
            worlds.edit_terrain(id, move |edit| {
                edit.set_cell(
                    layer,
                    p(3, 1),
                    CellCore {
                        material,
                        ..CellCore::AIR
                    },
                )
            })?;
            return Ok(WorkStatus::Complete);
        }
        worlds.edit_material_accounted(
            id,
            charon_engine::runtime::MaterialAccountingSubmission {
                profile,
                archives: archive,
                key: key("test:balance"),
                schema: key("test:balance-schema"),
                reason: recorded_source.then_some(MaterialChangeReason {
                    key: key("test:explicit-source"),
                    version: 1,
                    kind: MaterialReceiptKind::Source,
                }),
            },
            move |edit| {
                if apply_pipeline {
                    edit.step_material_phase(
                        MaterialPhaseConfig {
                            layer,
                            pressure: None,
                            motion: Some((
                                MaterialForceConfig {
                                    channel: key("test:motion"),
                                    gravity_q16: [0, 0],
                                    candidate_limit: 16,
                                    max_velocity_q16: 65536,
                                },
                                MaterialMotionConfig {
                                    channel: key("test:motion"),
                                    candidate_limit: 16,
                                    max_distance: 8,
                                    step_limit: 128,
                                },
                            )),
                            scalar_candidate_limit: Some(16),
                            particle: None,
                        },
                        &StandardMaterialForces,
                    )?;
                    return Ok(());
                }
                edit.transport_path(layer, &[p(2, 1), p(1, 1)])?;
                if swallow {
                    // 意図的にErrを無視しても、WorldEditは失敗を覚えており
                    // 先行の正常な物質移動をcommitしてはならない。
                    let _ = edit.set_cell(
                        layer,
                        p(1024, 1024),
                        CellCore {
                            material,
                            ..CellCore::AIR
                        },
                    );
                }
                if fail || recorded_source {
                    edit.set_cell(
                        layer,
                        p(3, 1),
                        CellCore {
                            material,
                            ..CellCore::AIR
                        },
                    )?;
                }
                Ok(())
            },
        )?;
        Ok(WorkStatus::Complete)
    }
}

fn engine_with_failure(
    fail: bool,
    swallow: bool,
    raw: bool,
    raw_state: bool,
    recorded_source: bool,
    raw_then_audited: bool,
    apply_pipeline: bool,
) -> (Engine, Arc<Content>, Arc<MaterialInventoryProfile>) {
    let content = build_content();
    let profile = Arc::new(profile());
    let init_profile = Arc::clone(&profile);
    let initial_material = content
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    let builder = WorldBuilder::new(Arc::clone(&content)).initialize(move |world| {
        let layer = world.content().layer_id(key("test:layer"))?;
        world.edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.acquire_lease(RegionLease {
                source: key("test:active"),
                region: ChunkRegion::single(ChunkCoord::new(0, 0)?)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })?;
            edit.set_cell(
                layer,
                p(1, 1),
                CellCore {
                    material: initial_material,
                    ..CellCore::AIR
                },
            )
        })?;
        let archive =
            world.load_material_archive_inventory(&init_profile, &mut NoArchives, 1_000_000)?;
        world.initialize_material_accounting(
            &init_profile,
            &archive,
            key("test:balance"),
            key("test:balance-schema"),
            8,
        )
    });
    let descriptor = SystemDescriptor::new(key("test:audited-materials"), Phase::Materials)
        .write(key("engine:terrain"))
        .write(key("test:debug-record"))
        .in_world(key("game:world"))
        .barrier();
    let runtime = Engine::builder()
        .sandbox_world("world", builder)
        .system(
            descriptor,
            AuditedMaterialSystem {
                profile: Arc::clone(&profile),
                inject_unrecorded_source: fail,
                swallow_nested_error: swallow,
                unaccounted_raw: raw,
                unaccounted_raw_state: raw_state,
                recorded_source,
                raw_then_audited,
                apply_pipeline,
            },
        )
        .build()
        .unwrap();
    (runtime, content, profile)
}
fn engine(fail: bool) -> (Engine, Arc<Content>, Arc<MaterialInventoryProfile>) {
    engine_with_failure(fail, false, false, false, false, false, false)
}
fn at(world: &World, layer: LayerId, point: CellPoint) -> MaterialId {
    match world.cell(layer, point).unwrap() {
        CellQuery::Loaded(value) => value.core.material,
        _ => panic!("この試験のresidentが失われた"),
    }
}

#[test]
fn runtime_material_barrier_audits_mass_and_publishes_on_same_tick() {
    let (mut runtime, content, profile) = engine(false);
    let id = runtime.worlds().id("world").unwrap();
    let layer = content.layer_id(key("test:layer")).unwrap();
    assert_eq!(
        runtime.step(TickInput::empty(1)).unwrap(),
        StepResult::Complete(1)
    );
    let terrain = runtime.worlds().terrain(id).unwrap();
    let sand = content
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    assert_eq!(at(terrain, layer, p(1, 1)), MaterialId::AIR);
    assert_eq!(at(terrain, layer, p(2, 1)), sand);
    assert_eq!(terrain.tick(), 1);
    let archive = terrain
        .load_material_archive_inventory(&profile, &mut NoArchives, 1_000_000)
        .unwrap();
    terrain
        .verify_material_accounting(&profile, &archive, key("test:balance"))
        .unwrap();
    let saved = runtime.worlds().checkpoint(id).unwrap();
    let restored = World::restore(&saved, content, WorldLimits::default(), saved.len()).unwrap();
    restored
        .verify_material_accounting(&profile, &archive, key("test:balance"))
        .unwrap();
}

#[test]
fn runtime_rejects_unaccounted_source_and_rolls_back_entire_tick() {
    let (mut runtime, content, profile) = engine(true);
    let id = runtime.worlds().id("world").unwrap();
    let before = runtime.worlds().checkpoint(id).unwrap();
    assert!(runtime.step(TickInput::empty(1)).is_err());
    assert_eq!(runtime.worlds().tick(id).unwrap(), 0);
    assert_eq!(runtime.worlds().checkpoint(id).unwrap(), before);
    let world = runtime.worlds().terrain(id).unwrap();
    let archive = world
        .load_material_archive_inventory(&profile, &mut NoArchives, 1_000_000)
        .unwrap();
    world
        .verify_material_accounting(&profile, &archive, key("test:balance"))
        .unwrap();
    let layer = content.layer_id(key("test:layer")).unwrap();
    assert_eq!(
        at(world, layer, p(1, 1)),
        content
            .material_id(MaterialHandle(key("test:sand")))
            .unwrap()
    );
}

#[test]
fn ignored_nested_edit_error_cannot_publish_a_partial_material_tick() {
    let (mut runtime, content, profile) =
        engine_with_failure(false, true, false, false, false, false, false);
    let id = runtime.worlds().id("world").unwrap();
    let before = runtime.worlds().checkpoint(id).unwrap();
    assert!(runtime.step(TickInput::empty(1)).is_err());
    assert_eq!(runtime.worlds().tick(id).unwrap(), 0);
    assert_eq!(runtime.worlds().checkpoint(id).unwrap(), before);
    let terrain = runtime.worlds().terrain(id).unwrap();
    let layer = content.layer_id(key("test:layer")).unwrap();
    assert_eq!(
        at(terrain, layer, p(1, 1)),
        content
            .material_id(MaterialHandle(key("test:sand")))
            .unwrap()
    );
    assert_eq!(at(terrain, layer, p(2, 1)), MaterialId::AIR);
    let archives = terrain
        .load_material_archive_inventory(&profile, &mut NoArchives, 1_000_000)
        .unwrap();
    terrain
        .verify_material_accounting(&profile, &archives, key("test:balance"))
        .unwrap();
}

#[test]
fn runtime_raw_material_edit_remains_available_but_is_detected_by_explicit_audit() {
    let (mut runtime, content, profile) =
        engine_with_failure(false, false, true, false, false, false, false);
    let id = runtime.worlds().id("world").unwrap();
    assert_eq!(
        runtime.step(TickInput::empty(1)).unwrap(),
        StepResult::Complete(1)
    );
    assert_eq!(runtime.worlds().tick(id).unwrap(), 1);
    let world = runtime.worlds().terrain(id).unwrap();
    let layer = content.layer_id(key("test:layer")).unwrap();
    assert_ne!(at(world, layer, p(3, 1)), MaterialId::AIR);
    let cache = world
        .load_material_archive_inventory(&profile, &mut NoArchives, 1_000_000)
        .unwrap();
    assert!(
        world
            .verify_material_accounting(&profile, &cache, key("test:balance"))
            .is_err()
    );
}

#[test]
fn runtime_explicit_material_source_writes_audited_receipt_and_restores() {
    let (mut runtime, content, profile) =
        engine_with_failure(false, false, false, false, true, false, false);
    let id = runtime.worlds().id("world").unwrap();
    assert_eq!(
        runtime.step(TickInput::empty(1)).unwrap(),
        StepResult::Complete(1)
    );
    let world = runtime.worlds().terrain(id).unwrap();
    let sand = content
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    let layer = content.layer_id(key("test:layer")).unwrap();
    assert_eq!(at(world, layer, p(2, 1)), sand);
    assert_eq!(at(world, layer, p(3, 1)), sand);
    let log: MaterialBalanceLog = world.state(key("test:balance")).unwrap();
    assert_eq!(log.receipts().len(), 1);
    assert_eq!(log.receipts()[0].reason().kind, MaterialReceiptKind::Source);
    // Materials barrierの確定時はtick0で、終了時にRuntimeがtick1へ進める。
    assert_eq!(log.receipts()[0].tick(), 0);
    let cache = world
        .load_material_archive_inventory(&profile, &mut NoArchives, 1_000_000)
        .unwrap();
    world
        .verify_material_accounting(&profile, &cache, key("test:balance"))
        .unwrap();
    let bytes = runtime.worlds().checkpoint(id).unwrap();
    let restored = World::restore(&bytes, content, WorldLimits::default(), bytes.len()).unwrap();
    restored
        .verify_material_accounting(&profile, &cache, key("test:balance"))
        .unwrap();
}

#[test]
fn runtime_unrelated_author_state_put_remains_available_with_accounting() {
    let (mut runtime, _content, profile) =
        engine_with_failure(false, false, false, true, false, false, false);
    let id = runtime.worlds().id("world").unwrap();
    assert_eq!(
        runtime.step(TickInput::empty(1)).unwrap(),
        StepResult::Complete(1)
    );
    assert_eq!(runtime.worlds().tick(id).unwrap(), 1);
    let terrain = runtime.worlds().terrain(id).unwrap();
    assert_eq!(terrain.state::<u64>(key("test:debug-record")).unwrap(), 7);
    let cache = terrain
        .load_material_archive_inventory(&profile, &mut NoArchives, 1_000_000)
        .unwrap();
    terrain
        .verify_material_accounting(&profile, &cache, key("test:balance"))
        .unwrap();
}

#[test]
fn raw_then_accounted_in_one_materials_barrier_rolls_back_entire_runtime_tick() {
    let (mut runtime, content, profile) =
        engine_with_failure(false, false, false, false, false, true, false);
    let id = runtime.worlds().id("world").unwrap();
    let before = runtime.worlds().checkpoint(id).unwrap();
    // raw編集の直後にaccounted編集を同じMaterials barrierへ予約。
    // 台帳との差を後続のpreflightで検知し、raw編集を含む全候補をrollback。
    assert!(runtime.step(TickInput::empty(1)).is_err());
    assert_eq!(runtime.worlds().checkpoint(id).unwrap(), before);
    assert_eq!(runtime.worlds().tick(id).unwrap(), 0);
    let terrain = runtime.worlds().terrain(id).unwrap();
    let layer = content.layer_id(key("test:layer")).unwrap();
    assert_eq!(at(terrain, layer, p(3, 1)), MaterialId::AIR);
    let cache = terrain
        .load_material_archive_inventory(&profile, &mut NoArchives, 1_000_000)
        .unwrap();
    terrain
        .verify_material_accounting(&profile, &cache, key("test:balance"))
        .unwrap();
}

#[test]
fn runtime_materials_phase_pipeline_moves_sand_and_keeps_accounting_over_ticks() {
    let (mut runtime, content, profile) =
        engine_with_failure(false, false, false, false, false, false, true);
    let id = runtime.worlds().id("world").unwrap();
    let layer = content.layer_id(key("test:layer")).unwrap();
    let sand = content
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    for tick in 1..=3 {
        assert_eq!(
            runtime.step(TickInput::empty(tick)).unwrap(),
            StepResult::Complete(tick)
        );
        let terrain = runtime.worlds().terrain(id).unwrap();
        assert_eq!(terrain.tick(), tick);
        assert_eq!(at(terrain, layer, p(1, tick as i32 + 1)), sand);
        let cache = terrain
            .load_material_archive_inventory(&profile, &mut NoArchives, 1_000_000)
            .unwrap();
        terrain
            .verify_material_accounting(&profile, &cache, key("test:balance"))
            .unwrap();
        let snapshot = runtime.worlds().checkpoint(id).unwrap();
        let restored = World::restore(
            &snapshot,
            Arc::clone(&content),
            WorldLimits::default(),
            snapshot.len(),
        )
        .unwrap();
        assert_eq!(restored.checkpoint(snapshot.len()).unwrap(), snapshot);
    }
}

/// 同じMaterials phaseで別Systemが別の監査receiptを出す。barrier間で
/// 前の結果を確認し、後段が失敗すると両Systemの編集が全て巻き戻る。
struct ChainedMaterialSystem {
    profile: Arc<MaterialInventoryProfile>,
    issue_source: bool,
    inject_failure: bool,
}
impl System for ChainedMaterialSystem {
    fn run(&mut self, ctx: &mut SystemContext<'_>) -> EngineResult<WorkStatus> {
        let mut access = ctx.worlds();
        let id = access.view().id("world")?;
        let terrain = access.view().terrain(id)?;
        let layer = terrain.content().layer_id(key("test:layer"))?;
        let sand = terrain
            .content()
            .material_id(MaterialHandle(key("test:sand")))?;
        let archive =
            terrain.load_material_archive_inventory(&self.profile, &mut NoArchives, 1_000_000)?;
        let source = self.issue_source;
        let fail = self.inject_failure;
        let profile = Arc::clone(&self.profile);
        access.edit_material_accounted(
            id,
            charon_engine::runtime::MaterialAccountingSubmission {
                profile,
                archives: archive,
                key: key("test:balance"),
                schema: key("test:balance-schema"),
                reason: Some(MaterialChangeReason {
                    key: key(if source {
                        "test:phase-source"
                    } else {
                        "test:phase-sink"
                    }),
                    version: 1,
                    kind: if source {
                        MaterialReceiptKind::Source
                    } else {
                        MaterialReceiptKind::Sink
                    },
                }),
            },
            move |edit| {
                edit.set_cell(
                    layer,
                    p(4, 4),
                    CellCore {
                        material: if source { sand } else { MaterialId::AIR },
                        ..CellCore::AIR
                    },
                )?;
                if fail {
                    edit.set_cell(
                        layer,
                        p(9999, 9999),
                        CellCore {
                            material: sand,
                            ..CellCore::AIR
                        },
                    )?;
                }
                Ok(())
            },
        )?;
        Ok(WorkStatus::Complete)
    }
}

fn chained_runtime(fail_second: bool) -> (Engine, Arc<Content>, Arc<MaterialInventoryProfile>) {
    let content = build_content();
    let profile = Arc::new(profile());
    let init_profile = Arc::clone(&profile);
    let initial_material = content
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    let world = WorldBuilder::new(Arc::clone(&content)).initialize(move |world| {
        let layer = world.content().layer_id(key("test:layer"))?;
        world.edit(|edit| {
            edit.insert_empty_chunk(ChunkCoord::new(0, 0)?)?;
            edit.acquire_lease(RegionLease {
                source: key("test:active"),
                region: ChunkRegion::single(ChunkCoord::new(0, 0)?)?,
                mode: LeaseMode::Simulate,
                expires: None,
            })?;
            edit.set_cell(
                layer,
                p(1, 1),
                CellCore {
                    material: initial_material,
                    ..CellCore::AIR
                },
            )
        })?;
        let cache =
            world.load_material_archive_inventory(&init_profile, &mut NoArchives, 1_000_000)?;
        world.initialize_material_accounting(
            &init_profile,
            &cache,
            key("test:balance"),
            key("test:balance-schema"),
            8,
        )
    });
    let source = SystemDescriptor::new(key("test:material-source-system"), Phase::Materials)
        .write(key("engine:terrain"))
        .in_world(key("game:world"))
        .barrier();
    let sink = SystemDescriptor::new(key("test:material-sink-system"), Phase::Materials)
        .after(key("test:material-source-system"))
        .write(key("engine:terrain"))
        .in_world(key("game:world"))
        .barrier();
    let runtime = Engine::builder()
        .sandbox_world("world", world)
        .system(
            source,
            ChainedMaterialSystem {
                profile: Arc::clone(&profile),
                issue_source: true,
                inject_failure: false,
            },
        )
        .system(
            sink,
            ChainedMaterialSystem {
                profile: Arc::clone(&profile),
                issue_source: false,
                inject_failure: fail_second,
            },
        )
        .build()
        .unwrap();
    (runtime, content, profile)
}

#[test]
fn multiple_runtime_material_systems_commit_in_declared_order_with_two_receipts() {
    let (mut runtime, content, profile) = chained_runtime(false);
    let id = runtime.worlds().id("world").unwrap();
    assert_eq!(
        runtime.step(TickInput::empty(1)).unwrap(),
        StepResult::Complete(1)
    );
    let terrain = runtime.worlds().terrain(id).unwrap();
    let layer = content.layer_id(key("test:layer")).unwrap();
    assert_eq!(at(terrain, layer, p(4, 4)), MaterialId::AIR);
    let cache = terrain
        .load_material_archive_inventory(&profile, &mut NoArchives, 1_000_000)
        .unwrap();
    terrain
        .verify_material_accounting(&profile, &cache, key("test:balance"))
        .unwrap();
    let log: MaterialBalanceLog = terrain.state(key("test:balance")).unwrap();
    assert_eq!(log.receipts().len(), 2);
    assert_eq!(log.receipts()[0].reason().kind, MaterialReceiptKind::Source);
    assert_eq!(log.receipts()[1].reason().kind, MaterialReceiptKind::Sink);
    assert_eq!(terrain.tick(), 1);
}

#[test]
fn second_materials_system_error_rolls_back_first_system_and_clock() {
    let (mut runtime, content, profile) = chained_runtime(true);
    let id = runtime.worlds().id("world").unwrap();
    let before = runtime.worlds().checkpoint(id).unwrap();
    assert!(runtime.step(TickInput::empty(1)).is_err());
    let world = runtime.worlds().terrain(id).unwrap();
    assert_eq!(world.tick(), 0);
    assert_eq!(runtime.worlds().checkpoint(id).unwrap(), before);
    let layer = content.layer_id(key("test:layer")).unwrap();
    assert_eq!(at(world, layer, p(4, 4)), MaterialId::AIR);
    let cache = world
        .load_material_archive_inventory(&profile, &mut NoArchives, 1_000_000)
        .unwrap();
    world
        .verify_material_accounting(&profile, &cache, key("test:balance"))
        .unwrap();
}
