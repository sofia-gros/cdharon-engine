//! tick、source、sequenceで順序を固定し、検証してからSimulationへ提出する世界編集commandを定義します。

/// 現在の有限worldに対するコマンド。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorldCommand {
    PaintBrush {
        center_x: i32,
        center_y: i32,
        radius: i32,
        material: u16,
    },
    RequestChunkLease {
        source_id: u32,
        policy: crate::simulation::streaming::StreamingPolicy,
        region: crate::simulation::streaming::ChunkRect,
        mode: crate::simulation::streaming::LoadMode,
    },
    ReleaseChunkLease {
        lease_id: u64,
    },
    ApplyVisibleChunkPlan {
        plan: crate::simulation::streaming::VisibleChunkPlan,
        resident_limit: usize,
        active_limit: usize,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandQueueError {
    WrongTick,
    DuplicateSourceSequence,
    CapacityExceeded,
    AllocationFailed,
    InvalidPendingState,
}

pub const MAX_PENDING_WORLD_COMMANDS: usize = 4_096;

/// コマンドを拒否した理由。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandError {
    InvalidRadius,
    UnknownMaterial,
    WriteLimitExceeded,
    AccountingOverflow,
    AccountingMismatch,
    AllocationFailed,
    DomainRejected,
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
    ChunkIsActive,
    ChunkIsLeased,
    DomainRejected,
    AllocationFailed,
    InvalidMaterialRegistry,
    AccountingOverflow,
    AccountingMismatch,
    InvalidMaterialState,
    EntityCapacityExceeded,
    InvalidEntityIdState,
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
            Self::ChunkIsActive => write!(f, "active chunk cannot be removed"),
            Self::ChunkIsLeased => write!(f, "leased chunk cannot be removed"),
            Self::DomainRejected => write!(f, "world command targets a frozen domain"),
            Self::AllocationFailed => write!(f, "world memory allocation failed"),
            Self::InvalidMaterialRegistry => write!(f, "material registry validation failed"),
            Self::AccountingOverflow => write!(f, "material ledger counter overflow"),
            Self::AccountingMismatch => write!(f, "material ledger does not match world contents"),
            Self::InvalidMaterialState => write!(f, "world contains an invalid material ID"),
            Self::EntityCapacityExceeded => write!(f, "entity ID capacity exceeded"),
            Self::InvalidEntityIdState => write!(f, "entity ID allocator state is invalid"),
        }
    }
}

impl std::error::Error for WorldError {}

/// 確定したコマンドの結果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandResult {
    pub cells_written: u32,
    /// leaseなど安定domain handleを確保したcommandの成功時に設定されます。
    pub domain_id: Option<u64>,
}

/// 1コマンドの最大候補セル数。設定変更はsimulation versionへ結び付ける。
pub const MAX_COMMAND_CANDIDATE_CELLS: u32 = 16_641;
pub const MAX_BRUSH_RADIUS: i32 = 64;

/// 同じtickのcommand群をtarget/source/sequence順に検証・適用します。
/// tick不一致や重複sequenceなどbatch全体の形式不備は、最初のWorld変更前に拒否します。
/// 形式が妥当なら各commandを個別transactionとして適用し、拒否commandのerrorもeventに残します。
/// そのため同じbatchの別commandが成功しても、拒否commandの部分変更は各transaction内で起こりません。
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

    #[test]
    fn command_rejection_is_reported_without_rolling_back_valid_siblings() {
        let mut world = World::new(8, 8).unwrap();
        let commands = vec![
            CommandEnvelope {
                target_tick: 1,
                source_id: 1,
                sequence: 0,
                payload: WorldCommand::PaintBrush {
                    center_x: 1,
                    center_y: 1,
                    radius: 0,
                    material: WATER,
                },
            },
            CommandEnvelope {
                target_tick: 1,
                source_id: 1,
                sequence: 1,
                payload: WorldCommand::PaintBrush {
                    center_x: 2,
                    center_y: 2,
                    radius: 0,
                    material: u16::MAX,
                },
            },
        ];

        let events = dispatch_command_batch(&mut world, commands).unwrap();

        assert!(events[0].result.is_ok());
        assert_eq!(events[1].result, Err(CommandError::UnknownMaterial));
        assert_eq!(world.get_cell(1, 1).material, WATER);
        assert_eq!(
            world.get_cell(2, 2).material,
            crate::simulation::material::AIR
        );
    }
}
