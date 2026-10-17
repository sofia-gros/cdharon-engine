//! E05収支監査をRuntime Materials barrierへ接続する公開APIの統合試験。
use charon_engine::{
    foundation::{
        EngineError, EngineResult, ErrorCode, StableKey, grid::CellPoint, schema::Schema,
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
    registry.freeze().unwrap()
}

struct AuditedMaterialSystem {
    profile: Arc<MaterialInventoryProfile>,
    inject_unrecorded_source: bool,
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
        worlds.edit_terrain_world(id, move |world| {
            world.edit_material_accounted(
                &profile,
                &archive,
                key("test:balance"),
                key("test:balance-schema"),
                None,
                |edit| {
                    edit.transport_path(layer, &[p(2, 1), p(1, 1)])?;
                    if fail {
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
            )
        })?;
        Ok(WorkStatus::Complete)
    }
}

fn engine(fail: bool) -> (Engine, Arc<Content>, Arc<MaterialInventoryProfile>) {
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
        .in_world(key("game:world"))
        .barrier();
    let runtime = Engine::builder()
        .sandbox_world("world", builder)
        .system(
            descriptor,
            AuditedMaterialSystem {
                profile: Arc::clone(&profile),
                inject_unrecorded_source: fail,
            },
        )
        .build()
        .unwrap();
    (runtime, content, profile)
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
