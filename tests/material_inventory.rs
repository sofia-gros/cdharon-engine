//! E05のresident収支がcell・粒子の所有権移動と保存復帰で一致することを検証します。

use charon_engine::{
    foundation::{StableKey, grid::CellPoint, id::WorldId},
    world::*,
};
use std::sync::Arc;

fn key(value: &str) -> StableKey {
    StableKey::parse(value).unwrap()
}

fn point(x: i32, y: i32) -> CellPoint {
    CellPoint { x, y }
}

fn fixture() -> (World, LayerId, LayerId) {
    let mut registry = ContentRegistry::new(ContentLimits::default()).unwrap();
    for name in ["test:front", "test:back"] {
        registry
            .layer(LayerDef {
                key: key(name),
                role: LayerRole::Terrain,
                render_order: 0,
                collision_group: 1,
                simulated: true,
            })
            .unwrap();
    }
    registry
        .register(
            MaterialBuilder::new("test:sand", MaterialBase::Powder)
                .unwrap()
                .mass_per_cell(7),
        )
        .unwrap();
    registry
        .register(
            MaterialBuilder::new("test:water", MaterialBase::Liquid)
                .unwrap()
                .mass_per_cell(3),
        )
        .unwrap();
    let content = registry.freeze().unwrap();
    let front = content.layer_id(key("test:front")).unwrap();
    let back = content.layer_id(key("test:back")).unwrap();
    let mut world = World::new(
        WorldId::from_parts(0, 1).unwrap(),
        17,
        WorldBounds::expandable(),
        content,
        WorldLimits::default(),
    )
    .unwrap();
    world
        .edit(|editor| editor.insert_empty_chunk(ChunkCoord::new(0, 0)?))
        .unwrap();
    (world, front, back)
}

fn amount(inventory: &ResidentMaterialInventory, key_name: &str) -> ResidentMaterialAmount {
    *inventory
        .materials
        .iter()
        .find(|entry| entry.material == key(key_name))
        .unwrap()
}

#[test]
fn all_layers_and_inactive_cells_count_once() {
    let (mut world, front, back) = fixture();
    let sand = world
        .content()
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    let water = world
        .content()
        .material_id(MaterialHandle(key("test:water")))
        .unwrap();
    world
        .edit(|editor| {
            editor.set_cell(
                front,
                point(1, 1),
                CellCore { material: sand, ..CellCore::AIR },
            )?;
            editor.set_cell(
                back,
                point(1, 1),
                CellCore { material: water, ..CellCore::AIR },
            )?;
            editor.set_cell(
                front,
                point(2, 1),
                CellCore { material: water, ..CellCore::AIR },
            )
        })
        .unwrap();
    let inventory = world.resident_material_inventory().unwrap();
    assert_eq!(inventory.archived_chunks, 0);
    assert_eq!(inventory.archived_particles, 0);
    assert_eq!(inventory.materials.len(), 2);
    let sand = amount(&inventory, "test:sand");
    assert_eq!((sand.cell_tokens, sand.particle_tokens, sand.total_tokens, sand.mass), (1, 0, 1, 7));
    let water = amount(&inventory, "test:water");
    assert_eq!((water.cell_tokens, water.particle_tokens, water.total_tokens, water.mass), (2, 0, 2, 6));
}

#[test]
fn emission_landing_and_checkpoint_keep_resident_total() {
    let (mut world, front, _) = fixture();
    let sand = world
        .content()
        .material_id(MaterialHandle(key("test:sand")))
        .unwrap();
    world
        .edit(|editor| editor.set_cell(
            front,
            point(1, 1),
            CellCore { material: sand, ..CellCore::AIR },
        ))
        .unwrap();
    let before = world.resident_material_inventory().unwrap();
    let handle = world
        .edit(|editor| editor.emit_material_particle(front, point(1, 1), [0, 0]))
        .unwrap();
    let emitted = world.resident_material_inventory().unwrap();
    assert_eq!(amount(&emitted, "test:sand").cell_tokens, 0);
    assert_eq!(amount(&emitted, "test:sand").particle_tokens, 1);
    assert_eq!(amount(&before, "test:sand").mass, amount(&emitted, "test:sand").mass);

    let bytes = world.checkpoint(4_000_000).unwrap();
    let mut restored = World::restore(
        &bytes,
        Arc::clone(world.content()),
        WorldLimits::default(),
        4_000_000,
    )
    .unwrap();
    assert_eq!(restored.resident_material_inventory().unwrap(), emitted);
    restored
        .edit(|editor| editor.land_material_particle(handle))
        .unwrap();
    assert_eq!(restored.resident_material_inventory().unwrap(), before);
}
