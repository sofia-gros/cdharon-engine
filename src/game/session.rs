//! Deterministic small game-state slice: mining, crafting, combat, and checkpoints.

use crate::entities::player::Player;
use crate::game::inventory::{Inventory, InventoryError, ItemStack, Recipe};
use crate::simulation::material::MaterialRegistry;
use crate::simulation::world::World;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Enemy {
    pub id: u32,
    pub x: i32,
    pub y: i32,
    pub hp: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GameError {
    Inventory(InventoryError),
    UnknownEnemy(u32),
    InvalidDamage,
    InvalidState,
    EmptyTarget,
    WorldChanged,
}

impl std::fmt::Display for GameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for GameError {}

impl From<InventoryError> for GameError {
    fn from(value: InventoryError) -> Self {
        Self::Inventory(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameSession {
    pub tick: u64,
    pub player_hp: u16,
    pub inventory: Inventory,
    pub enemies: Vec<Enemy>,
    pub ore_material: u16,
    pub ore_item: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameCheckpoint(GameSession);

const GAME_SNAPSHOT_MAGIC: &[u8; 8] = b"CHARGAME";
const GAME_SNAPSHOT_VERSION: u16 = 1;
const GAME_BUNDLE_MAGIC: &[u8; 8] = b"CHRBNDL2";

impl GameSession {
    pub fn new(
        inventory_slots: usize,
        ore_material: u16,
        ore_item: &str,
    ) -> Result<Self, GameError> {
        if ore_material == 0 || ore_item.is_empty() {
            return Err(GameError::InvalidState);
        }
        let mut enemies = Vec::new();
        enemies
            .try_reserve_exact(2)
            .map_err(|_| GameError::InvalidState)?;
        enemies.push(Enemy {
            id: 1,
            x: 16,
            y: 0,
            hp: 12,
        });
        enemies.push(Enemy {
            id: 2,
            x: 24,
            y: 0,
            hp: 20,
        });
        Ok(Self {
            tick: 0,
            player_hp: 100,
            inventory: Inventory::try_new(inventory_slots, 99)?,
            enemies,
            ore_material,
            ore_item: ore_item.to_owned(),
        })
    }

    /// Commit a mined block's item reward atomically before removing its world cell.
    pub fn reserve_mining_reward(&mut self, count: u32) -> Result<(), GameError> {
        self.inventory.add(ItemStack {
            key: self.ore_item.clone(),
            count,
        })?;
        Ok(())
    }

    /// Stage inventory capacity before removing a cell; a failed world compare leaves inventory untouched.
    pub fn mine_cell(
        &mut self,
        world: &mut World,
        x: i32,
        y: i32,
        item_key: &str,
    ) -> Result<u16, GameError> {
        let material = world.get_cell(x, y).material;
        if material == 0 || !world.registry.is_registered(material) {
            return Err(GameError::EmptyTarget);
        }
        let staged = self.inventory.staged_add(ItemStack {
            key: item_key.to_owned(),
            count: 1,
        })?;
        if !world.remove_cell_if_material(x, y, material) {
            return Err(GameError::WorldChanged);
        }
        self.inventory = staged;
        Ok(material)
    }

    pub fn craft(&mut self, recipe: &Recipe) -> Result<(), GameError> {
        self.inventory.craft(recipe)?;
        Ok(())
    }

    pub fn attack(&mut self, enemy_id: u32, damage: u16) -> Result<bool, GameError> {
        if damage == 0 {
            return Err(GameError::InvalidDamage);
        }
        let enemy = self
            .enemies
            .iter_mut()
            .find(|enemy| enemy.id == enemy_id)
            .ok_or(GameError::UnknownEnemy(enemy_id))?;
        enemy.hp = enemy.hp.saturating_sub(damage);
        let defeated = enemy.hp == 0;
        if defeated {
            self.enemies.retain(|enemy| enemy.id != enemy_id);
        }
        Ok(defeated)
    }

    pub fn advance_tick(&mut self) -> Result<(), GameError> {
        self.tick = self.tick.checked_add(1).ok_or(GameError::InvalidState)?;
        Ok(())
    }

    pub fn advance_with_world(&mut self, world: &mut World) -> Result<(), GameError> {
        if self.tick != world.tick_count || self.tick == u64::MAX {
            return Err(GameError::InvalidState);
        }
        world
            .ensure_tick_available()
            .map_err(|_| GameError::InvalidState)?;
        world.update().map_err(|_| GameError::InvalidState)?;
        self.tick += 1;
        Ok(())
    }

    pub fn checkpoint(&self) -> GameCheckpoint {
        GameCheckpoint(self.clone())
    }

    pub fn restore(checkpoint: &GameCheckpoint) -> Self {
        checkpoint.0.clone()
    }

    pub fn encode_snapshot(&self) -> Result<Vec<u8>, GameError> {
        if self.enemies.len() > 256 || self.ore_item.len() > 128 {
            return Err(GameError::InvalidState);
        }
        let mut out = Vec::new();
        out.try_reserve(1024).map_err(|_| GameError::InvalidState)?;
        out.extend_from_slice(GAME_SNAPSHOT_MAGIC);
        put_u16(&mut out, GAME_SNAPSHOT_VERSION);
        put_u64(&mut out, self.tick);
        put_u16(&mut out, self.player_hp);
        put_u16(&mut out, self.ore_material);
        put_string(&mut out, &self.ore_item)?;
        put_u16(&mut out, self.inventory.slots().len() as u16);
        put_u32(&mut out, self.inventory.max_stack());
        for slot in self.inventory.slots() {
            match slot {
                None => out.push(0),
                Some(stack) => {
                    out.push(1);
                    put_u32(&mut out, stack.count);
                    put_string(&mut out, &stack.key)?;
                }
            }
        }
        put_u16(&mut out, self.enemies.len() as u16);
        for enemy in &self.enemies {
            put_u32(&mut out, enemy.id);
            put_i32(&mut out, enemy.x);
            put_i32(&mut out, enemy.y);
            put_u16(&mut out, enemy.hp);
        }
        if out.len() > 1_000_000 {
            return Err(GameError::InvalidState);
        }
        Ok(out)
    }

    pub fn decode_snapshot(bytes: &[u8]) -> Result<Self, GameError> {
        if bytes.len() > 1_000_000 {
            return Err(GameError::InvalidState);
        }
        let mut r = GameReader { bytes, offset: 0 };
        if r.take(8)? != GAME_SNAPSHOT_MAGIC || r.u16()? != GAME_SNAPSHOT_VERSION {
            return Err(GameError::InvalidState);
        }
        let tick = r.u64()?;
        let player_hp = r.u16()?;
        let ore_material = r.u16()?;
        let ore_item = r.string(128)?;
        if player_hp == 0 || ore_material == 0 {
            return Err(GameError::InvalidState);
        }
        let slot_count = r.u16()? as usize;
        let max_stack = r.u32()?;
        if slot_count == 0 || slot_count > 256 {
            return Err(GameError::InvalidState);
        }
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(slot_count)
            .map_err(|_| GameError::InvalidState)?;
        for _ in 0..slot_count {
            match r.u8()? {
                0 => slots.push(None),
                1 => {
                    let count = r.u32()?;
                    let key = r.string(129)?;
                    slots.push(Some(ItemStack { key, count }));
                }
                _ => return Err(GameError::InvalidState),
            }
        }
        let inventory = Inventory::from_snapshot(max_stack, slots)?;
        let enemy_count = r.u16()? as usize;
        if enemy_count > 256 {
            return Err(GameError::InvalidState);
        }
        let mut enemies = Vec::new();
        enemies
            .try_reserve_exact(enemy_count)
            .map_err(|_| GameError::InvalidState)?;
        for _ in 0..enemy_count {
            let enemy = Enemy {
                id: r.u32()?,
                x: r.i32()?,
                y: r.i32()?,
                hp: r.u16()?,
            };
            if enemy.id == 0
                || enemy.hp == 0
                || enemies.iter().any(|known: &Enemy| known.id == enemy.id)
            {
                return Err(GameError::InvalidState);
            }
            enemies.push(enemy);
        }
        if r.offset != bytes.len() {
            return Err(GameError::InvalidState);
        }
        Ok(Self {
            tick,
            player_hp,
            inventory,
            enemies,
            ore_material,
            ore_item,
        })
    }

    pub fn encode_bundle(&self, world: &World, player: &Player) -> Result<Vec<u8>, GameError> {
        if self.tick != world.tick_count {
            return Err(GameError::InvalidState);
        }
        let world_bytes = world
            .encode_snapshot()
            .map_err(|_| GameError::InvalidState)?;
        let game_bytes = self.encode_snapshot()?;
        let world_len = u32::try_from(world_bytes.len()).map_err(|_| GameError::InvalidState)?;
        let game_len = u32::try_from(game_bytes.len()).map_err(|_| GameError::InvalidState)?;
        let total = 73_usize
            .checked_add(world_bytes.len())
            .and_then(|n| n.checked_add(game_bytes.len()))
            .ok_or(GameError::InvalidState)?;
        if total > 32_000_000 {
            return Err(GameError::InvalidState);
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(total)
            .map_err(|_| GameError::InvalidState)?;
        bytes.extend_from_slice(GAME_BUNDLE_MAGIC);
        put_u32(&mut bytes, world_len);
        put_u32(&mut bytes, game_len);
        player.encode_state(&mut bytes);
        bytes.extend_from_slice(&world_bytes);
        bytes.extend_from_slice(&game_bytes);
        Ok(bytes)
    }

    pub fn decode_bundle(
        bytes: &[u8],
        registry: MaterialRegistry,
    ) -> Result<(World, Self, Player), GameError> {
        if bytes.len() > 32_000_000 {
            return Err(GameError::InvalidState);
        }
        let mut r = GameReader { bytes, offset: 0 };
        if r.take(8)? != GAME_BUNDLE_MAGIC {
            return Err(GameError::InvalidState);
        }
        let world_len = r.u32()? as usize;
        let game_len = r.u32()? as usize;
        if world_len
            .checked_add(game_len)
            .and_then(|n| n.checked_add(73))
            != Some(bytes.len())
        {
            return Err(GameError::InvalidState);
        }
        let player =
            Player::decode_state(bytes, &mut r.offset).map_err(|_| GameError::InvalidState)?;
        let payload = r.take(
            world_len
                .checked_add(game_len)
                .ok_or(GameError::InvalidState)?,
        )?;
        let (world_bytes, game_bytes) = payload.split_at(world_len);
        let world =
            World::decode_snapshot(world_bytes, registry).map_err(|_| GameError::InvalidState)?;
        let game = Self::decode_snapshot(game_bytes)?;
        if world.tick_count != game.tick || !world.registry.is_registered(game.ore_material) {
            return Err(GameError::InvalidState);
        }
        game.inventory.staged_add(ItemStack {
            key: game.ore_item.clone(),
            count: 1,
        })?;
        Ok((world, game, player))
    }
}

fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_i32(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_string(out: &mut Vec<u8>, v: &str) -> Result<(), GameError> {
    let len = u16::try_from(v.len()).map_err(|_| GameError::InvalidState)?;
    put_u16(out, len);
    out.extend_from_slice(v.as_bytes());
    Ok(())
}

struct GameReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl GameReader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], GameError> {
        let end = self.offset.checked_add(n).ok_or(GameError::InvalidState)?;
        let s = self
            .bytes
            .get(self.offset..end)
            .ok_or(GameError::InvalidState)?;
        self.offset = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, GameError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, GameError> {
        Ok(u16::from_le_bytes(
            self.take(2)?
                .try_into()
                .map_err(|_| GameError::InvalidState)?,
        ))
    }
    fn u32(&mut self) -> Result<u32, GameError> {
        Ok(u32::from_le_bytes(
            self.take(4)?
                .try_into()
                .map_err(|_| GameError::InvalidState)?,
        ))
    }
    fn i32(&mut self) -> Result<i32, GameError> {
        Ok(i32::from_le_bytes(
            self.take(4)?
                .try_into()
                .map_err(|_| GameError::InvalidState)?,
        ))
    }
    fn u64(&mut self) -> Result<u64, GameError> {
        Ok(u64::from_le_bytes(
            self.take(8)?
                .try_into()
                .map_err(|_| GameError::InvalidState)?,
        ))
    }
    fn string(&mut self, limit: usize) -> Result<String, GameError> {
        let len = self.u16()? as usize;
        if len > limit {
            return Err(GameError::InvalidState);
        }
        let source = self.take(len)?;
        let mut value = String::new();
        value
            .try_reserve_exact(len)
            .map_err(|_| GameError::InvalidState)?;
        value.push_str(std::str::from_utf8(source).map_err(|_| GameError::InvalidState)?);
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::fixed::Fixed;
    use crate::simulation::command::WorldCommand;
    use crate::simulation::material::STONE;

    #[test]
    fn mining_crafting_combat_and_checkpoint_form_a_consistent_loop() {
        let mut game = GameSession::new(8, 4, "sample:ore").unwrap();
        game.reserve_mining_reward(3).unwrap();
        let recipe = Recipe {
            key: "sample:bar_recipe".into(),
            inputs: vec![ItemStack {
                key: "sample:ore".into(),
                count: 2,
            }],
            output: ItemStack {
                key: "sample:bar".into(),
                count: 1,
            },
        };
        game.craft(&recipe).unwrap();
        assert_eq!(game.inventory.count("sample:ore"), 1);
        assert_eq!(game.inventory.count("sample:bar"), 1);
        assert!(game.attack(1, 12).unwrap());
        let mut world = crate::simulation::world::World::new(8, 8).unwrap();
        game.advance_with_world(&mut world).unwrap();
        let saved = game.checkpoint();
        game.attack(2, 20).unwrap();
        let restored = GameSession::restore(&saved);
        assert_eq!(restored.tick, 1);
        assert_eq!(restored.enemies.len(), 1);
        assert_eq!(restored.inventory, game.inventory);
        let bytes = saved.0.encode_snapshot().unwrap();
        assert_eq!(GameSession::decode_snapshot(&bytes).unwrap(), saved.0);
        let registry = crate::simulation::material::MaterialRegistry::with_defaults();
        let fingerprint = world.state_fingerprint();
        let player = Player::new(
            Fixed::from_integer(3).unwrap(),
            Fixed::from_integer(4).unwrap(),
        );
        let bundle = saved.0.encode_bundle(&world, &player).unwrap();
        let (restored_world, restored_game, restored_player) =
            GameSession::decode_bundle(&bundle, registry).unwrap();
        assert_eq!(restored_game, saved.0);
        assert_eq!(restored_player, player);
        assert_eq!(restored_world.state_fingerprint(), fingerprint);
    }

    #[test]
    fn failed_mining_reward_keeps_inventory_unchanged() {
        let mut game = GameSession::new(1, 5, "sample:ore").unwrap();
        game.inventory
            .add(ItemStack {
                key: "sample:stone".into(),
                count: 99,
            })
            .unwrap();
        let before = game.inventory.clone();
        assert!(game.reserve_mining_reward(1).is_err());
        assert_eq!(game.inventory, before);
    }

    #[test]
    fn mining_removes_world_cell_only_after_inventory_capacity_is_reserved() {
        let mut game = GameSession::new(1, 5, "sample:ore").unwrap();
        let mut world = World::new(8, 8).unwrap();
        world
            .apply_command(WorldCommand::PaintBrush {
                center_x: 3,
                center_y: 3,
                radius: 0,
                material: STONE,
            })
            .unwrap();
        game.inventory
            .add(ItemStack {
                key: "sample:full".into(),
                count: 99,
            })
            .unwrap();
        assert!(game.mine_cell(&mut world, 3, 3, "sample:stone").is_err());
        assert_eq!(world.get_cell(3, 3).material, STONE);
        assert_eq!(game.inventory.count("sample:full"), 99);
    }
}
