use charon_engine::core::fixed::Fixed;
use charon_engine::entities::player::Player;
use charon_engine::game::inventory::{ItemStack, Recipe};
use charon_engine::game::session::GameSession;
use charon_engine::simulation::command::WorldCommand;
use charon_engine::simulation::material::STONE;
use charon_engine::simulation::world::World;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut world = World::new(64, 64)?;
    let mut game = GameSession::new(16, STONE, "sample:stone")?;
    world.apply_command(WorldCommand::PaintBrush {
        center_x: 8,
        center_y: 8,
        radius: 1,
        material: STONE,
    })?;

    game.mine_cell(&mut world, 8, 8, "sample:stone")?;
    game.mine_cell(&mut world, 7, 8, "sample:stone")?;
    game.craft(&Recipe {
        key: "sample:stone_blade_recipe".into(),
        inputs: vec![ItemStack {
            key: "sample:stone".into(),
            count: 2,
        }],
        output: ItemStack {
            key: "sample:stone_blade".into(),
            count: 1,
        },
    })?;
    game.attack(1, 12)?;
    game.advance_with_world(&mut world)?;

    let player = Player::new(Fixed::from_integer(10)?, Fixed::from_integer(10)?);
    let bundle = game.encode_bundle(&world, &player)?;
    let (restored_world, restored_game, _restored_player) = GameSession::decode_bundle(
        &bundle,
        charon_engine::simulation::material::MaterialRegistry::with_defaults(),
    )?;
    println!(
        "sample complete: tick={}, inventory_blades={}, enemies={}, world_hash={:016x}, save_bytes={}",
        restored_game.tick,
        restored_game.inventory.count("sample:stone_blade"),
        restored_game.enemies.len(),
        restored_world.state_fingerprint(),
        bundle.len(),
    );
    Ok(())
}
