//! Simulationへ提出できる、検証可能な世界編集コマンド。

/// 現在の有限worldに対するコマンド。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorldCommand {
    PaintBrush {
        center_x: i32,
        center_y: i32,
        radius: i32,
        material: u16,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandEnvelope {
    pub target_tick: u64,
    pub source_id: u32,
    pub sequence: u64,
    pub payload: WorldCommand,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandEvent {
    pub target_tick: u64,
    pub source_id: u32,
    pub sequence: u64,
    pub result: Result<CommandResult, CommandError>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandBatchError {
    WrongTick,
    DuplicateSourceSequence,
    AllocationFailed,
}

/// コマンドを拒否した理由。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandError {
    InvalidRadius,
    UnknownMaterial,
    WriteLimitExceeded,
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for CommandError {}

/// Worldの正規tickを進められない理由。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorldError {
    TickOverflow,
    InvalidWorldDimensions,
    WorldCapacityExceeded,
    ActiveChunkCapacityExceeded,
    PinnedChunkCapacityExceeded,
    LeaseCapacityExceeded,
    AllocationFailed,
    InvalidMaterialRegistry,
}

impl std::fmt::Display for WorldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TickOverflow => write!(f, "simulation tick counter overflow"),
            Self::InvalidWorldDimensions => write!(f, "invalid world dimensions"),
            Self::WorldCapacityExceeded => write!(f, "world exceeds resident chunk capacity"),
            Self::ActiveChunkCapacityExceeded => write!(f, "active chunk capacity exceeded"),
            Self::PinnedChunkCapacityExceeded => write!(f, "pinned chunk capacity exceeded"),
            Self::LeaseCapacityExceeded => write!(f, "chunk lease capacity exceeded"),
            Self::AllocationFailed => write!(f, "world memory allocation failed"),
            Self::InvalidMaterialRegistry => write!(f, "material registry validation failed"),
        }
    }
}

impl std::error::Error for WorldError {}

/// 確定したコマンドの結果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandResult {
    pub cells_written: u32,
}

/// 1コマンドの最大候補セル数。設定変更はsimulation versionへ結び付ける。
pub const MAX_COMMAND_CANDIDATE_CELLS: u32 = 16_641;
pub const MAX_BRUSH_RADIUS: i32 = 64;

/// Apply a same-tick command batch in stable target/source/sequence order.
pub fn dispatch_command_batch(
    world: &mut crate::simulation::world::World,
    mut commands: Vec<CommandEnvelope>,
) -> Result<Vec<CommandEvent>, CommandBatchError> {
    let next_tick = world
        .tick_count
        .checked_add(1)
        .ok_or(CommandBatchError::WrongTick)?;
    if commands
        .iter()
        .any(|command| command.target_tick != next_tick)
    {
        return Err(CommandBatchError::WrongTick);
    }
    commands
        .sort_unstable_by_key(|command| (command.target_tick, command.source_id, command.sequence));
    for pair in commands.windows(2) {
        if pair[0].source_id == pair[1].source_id && pair[0].sequence == pair[1].sequence {
            return Err(CommandBatchError::DuplicateSourceSequence);
        }
    }

    let mut events = Vec::new();
    events
        .try_reserve_exact(commands.len())
        .map_err(|_| CommandBatchError::AllocationFailed)?;
    for command in commands {
        let result = world.apply_command(command.payload);
        events.push(CommandEvent {
            target_tick: command.target_tick,
            source_id: command.source_id,
            sequence: command.sequence,
            result,
        });
    }
    Ok(events)
}

#[cfg(test)]
mod batch_tests {
    use super::*;
    use crate::simulation::material::{SAND, WATER};
    use crate::simulation::world::World;

    #[test]
    fn batch_orders_by_target_source_and_sequence() {
        let mut world = World::new(8, 8).unwrap();
        let paint = |source_id, sequence, material| CommandEnvelope {
            target_tick: 1,
            source_id,
            sequence,
            payload: WorldCommand::PaintBrush {
                center_x: source_id as i32,
                center_y: 0,
                radius: 0,
                material,
            },
        };
        let events = dispatch_command_batch(
            &mut world,
            vec![paint(2, 0, WATER), paint(1, 2, SAND), paint(1, 1, WATER)],
        )
        .unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| (event.source_id, event.sequence))
                .collect::<Vec<_>>(),
            vec![(1, 1), (1, 2), (2, 0)]
        );
    }

    #[test]
    fn invalid_batch_is_rejected_before_world_mutation() {
        let mut world = World::new(8, 8).unwrap();
        let command = CommandEnvelope {
            target_tick: 1,
            source_id: 1,
            sequence: 3,
            payload: WorldCommand::PaintBrush {
                center_x: 1,
                center_y: 1,
                radius: 0,
                material: WATER,
            },
        };
        assert_eq!(
            dispatch_command_batch(&mut world, vec![command, command]),
            Err(CommandBatchError::DuplicateSourceSequence)
        );
        assert_eq!(
            world.get_cell(1, 1).material,
            crate::simulation::material::AIR
        );
    }
}
