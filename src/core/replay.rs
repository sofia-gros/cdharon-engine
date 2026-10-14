//! Versioned deterministic input and world-command recording.

use crate::entities::player::{Player, PlayerError};
use crate::simulation::command::{CommandEnvelope, dispatch_command_batch};
use crate::simulation::world::{SnapshotError, World};

const MAGIC: &[u8; 8] = b"CHARPLY1";
const REPLAY_VERSION: u16 = 1;
const MAX_FRAMES: usize = 1_000_000;
const MAX_COMMANDS_PER_FRAME: usize = 1024;
const MAX_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TickInput {
    pub horizontal_axis: i8,
    pub jump_pressed: bool,
    pub jump_held: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayFrame {
    /// World tick produced by this input frame (first recorded tick is 1).
    pub tick: u64,
    pub input: TickInput,
    pub commands: Vec<CommandEnvelope>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayLog {
    pub simulation_version: u32,
    pub content_fingerprint: u64,
    pub initial_tick: u64,
    pub initial_world_fingerprint: u64,
    pub initial_player_fingerprint: u64,
    frames: Vec<ReplayFrame>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayError {
    InvalidFormat,
    UnsupportedVersion,
    ContentMismatch,
    InitialStateMismatch,
    TickMismatch,
    InvalidInput,
    CapacityExceeded,
    AllocationFailed,
    WorldSnapshot(SnapshotError),
    Player(PlayerError),
}

impl std::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "replay failed: {self:?}")
    }
}
impl std::error::Error for ReplayError {}

impl ReplayLog {
    pub fn new(world: &World, player: &Player) -> Self {
        Self {
            simulation_version: World::simulation_version(),
            content_fingerprint: world.content_fingerprint(),
            initial_tick: world.tick_count,
            initial_world_fingerprint: world.state_fingerprint(),
            initial_player_fingerprint: player.state_fingerprint(),
            frames: Vec::new(),
        }
    }

    pub fn frames(&self) -> &[ReplayFrame] {
        &self.frames
    }

    pub fn record(
        &mut self,
        tick: u64,
        input: TickInput,
        mut commands: Vec<CommandEnvelope>,
    ) -> Result<(), ReplayError> {
        if !(-1..=1).contains(&input.horizontal_axis) {
            return Err(ReplayError::InvalidInput);
        }
        let expected_tick = self
            .initial_tick
            .checked_add(self.frames.len() as u64)
            .and_then(|tick| tick.checked_add(1))
            .ok_or(ReplayError::TickMismatch)?;
        if tick != expected_tick
            || commands.len() > MAX_COMMANDS_PER_FRAME
            || commands.iter().any(|command| command.target_tick != tick)
        {
            return Err(ReplayError::TickMismatch);
        }
        commands.sort_unstable_by_key(|command| {
            (command.target_tick, command.source_id, command.sequence)
        });
        for pair in commands.windows(2) {
            if pair[0].source_id == pair[1].source_id && pair[0].sequence == pair[1].sequence {
                return Err(ReplayError::InvalidFormat);
            }
        }
        if self.frames.len() >= MAX_FRAMES {
            return Err(ReplayError::CapacityExceeded);
        }
        self.frames
            .try_reserve(1)
            .map_err(|_| ReplayError::AllocationFailed)?;
        self.frames.push(ReplayFrame {
            tick,
            input,
            commands,
        });
        Ok(())
    }

    /// Replay into candidates and publish only if every frame succeeds.
    pub fn play(&self, world: &mut World, player: &mut Player) -> Result<(), ReplayError> {
        if self.simulation_version != World::simulation_version() {
            return Err(ReplayError::UnsupportedVersion);
        }
        if world.content_fingerprint() != self.content_fingerprint {
            return Err(ReplayError::ContentMismatch);
        }
        if world.tick_count != self.initial_tick
            || world.state_fingerprint() != self.initial_world_fingerprint
            || player.state_fingerprint() != self.initial_player_fingerprint
        {
            return Err(ReplayError::InitialStateMismatch);
        }
        let mut candidate_world = world
            .try_clone_stopped()
            .map_err(ReplayError::WorldSnapshot)?;
        let mut candidate_player = player.clone();
        for (index, frame) in self.frames.iter().enumerate() {
            let expected = candidate_world
                .tick_count
                .checked_add(1)
                .ok_or(ReplayError::TickMismatch)?;
            if frame.tick != expected || frame.tick != self.initial_tick + index as u64 + 1 {
                return Err(ReplayError::TickMismatch);
            }
            if !frame.commands.is_empty() {
                let mut commands = Vec::new();
                commands
                    .try_reserve_exact(frame.commands.len())
                    .map_err(|_| ReplayError::AllocationFailed)?;
                commands.extend_from_slice(&frame.commands);
                let events = dispatch_command_batch(&mut candidate_world, commands)
                    .map_err(|_| ReplayError::InvalidFormat)?;
                if events.iter().any(|event| event.result.is_err()) {
                    return Err(ReplayError::InvalidFormat);
                }
            }
            candidate_player
                .update(
                    frame.input.horizontal_axis,
                    frame.input.jump_pressed,
                    frame.input.jump_held,
                    &mut candidate_world,
                )
                .map_err(ReplayError::Player)?;
            candidate_world
                .update()
                .map_err(|_| ReplayError::TickMismatch)?;
        }
        *world = candidate_world;
        *player = candidate_player;
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, ReplayError> {
        let total_size = self
            .frames
            .iter()
            .try_fold(42_usize, |size, frame| {
                let commands = frame.commands.len().checked_mul(35)?;
                size.checked_add(12)?.checked_add(commands)
            })
            .ok_or(ReplayError::CapacityExceeded)?;
        if total_size > MAX_BYTES {
            return Err(ReplayError::CapacityExceeded);
        }
        let mut out = Vec::new();
        out.try_reserve_exact(total_size)
            .map_err(|_| ReplayError::AllocationFailed)?;
        out.extend_from_slice(MAGIC);
        put_u16(&mut out, REPLAY_VERSION);
        put_u32(&mut out, self.simulation_version);
        put_u64(&mut out, self.content_fingerprint);
        put_u64(&mut out, self.initial_tick);
        put_u64(&mut out, self.initial_world_fingerprint);
        put_u64(&mut out, self.initial_player_fingerprint);
        put_u32(
            &mut out,
            u32::try_from(self.frames.len()).map_err(|_| ReplayError::CapacityExceeded)?,
        );
        for frame in &self.frames {
            put_u64(&mut out, frame.tick);
            out.push(frame.input.horizontal_axis as u8);
            out.push(u8::from(frame.input.jump_pressed) | (u8::from(frame.input.jump_held) << 1));
            put_u16(
                &mut out,
                u16::try_from(frame.commands.len()).map_err(|_| ReplayError::CapacityExceeded)?,
            );
            for command in &frame.commands {
                put_u64(&mut out, command.target_tick);
                put_u32(&mut out, command.source_id);
                put_u64(&mut out, command.sequence);
                match command.payload {
                    crate::simulation::command::WorldCommand::PaintBrush {
                        center_x,
                        center_y,
                        radius,
                        material,
                    } => {
                        out.push(0);
                        put_i32(&mut out, center_x);
                        put_i32(&mut out, center_y);
                        put_i32(&mut out, radius);
                        put_u16(&mut out, material);
                    }
                }
            }
            if out.len() > MAX_BYTES {
                return Err(ReplayError::CapacityExceeded);
            }
        }
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ReplayError> {
        if bytes.len() > MAX_BYTES {
            return Err(ReplayError::CapacityExceeded);
        }
        let mut reader = Reader { bytes, offset: 0 };
        if reader.take(8)? != MAGIC {
            return Err(ReplayError::InvalidFormat);
        }
        if reader.u16()? != REPLAY_VERSION {
            return Err(ReplayError::UnsupportedVersion);
        }
        let simulation_version = reader.u32()?;
        let content_fingerprint = reader.u64()?;
        let initial_tick = reader.u64()?;
        let initial_world_fingerprint = reader.u64()?;
        let initial_player_fingerprint = reader.u64()?;
        let count = reader.u32()? as usize;
        if count > MAX_FRAMES {
            return Err(ReplayError::CapacityExceeded);
        }
        let mut frames = Vec::new();
        frames
            .try_reserve_exact(count)
            .map_err(|_| ReplayError::AllocationFailed)?;
        for index in 0..count {
            let tick = reader.u64()?;
            let axis = reader.u8()? as i8;
            let flags = reader.u8()?;
            if tick
                != initial_tick
                    .checked_add(index as u64 + 1)
                    .ok_or(ReplayError::InvalidFormat)?
                || !(-1..=1).contains(&axis)
                || flags & !3 != 0
            {
                return Err(ReplayError::InvalidInput);
            }
            let command_count = reader.u16()? as usize;
            if command_count > MAX_COMMANDS_PER_FRAME {
                return Err(ReplayError::CapacityExceeded);
            }
            let mut commands = Vec::new();
            commands
                .try_reserve_exact(command_count)
                .map_err(|_| ReplayError::AllocationFailed)?;
            for _ in 0..command_count {
                let target_tick = reader.u64()?;
                let source_id = reader.u32()?;
                let sequence = reader.u64()?;
                if target_tick != tick || reader.u8()? != 0 {
                    return Err(ReplayError::InvalidFormat);
                }
                let payload = crate::simulation::command::WorldCommand::PaintBrush {
                    center_x: reader.i32()?,
                    center_y: reader.i32()?,
                    radius: reader.i32()?,
                    material: reader.u16()?,
                };
                commands.push(CommandEnvelope {
                    target_tick,
                    source_id,
                    sequence,
                    payload,
                });
            }
            frames.push(ReplayFrame {
                tick,
                input: TickInput {
                    horizontal_axis: axis,
                    jump_pressed: flags & 1 != 0,
                    jump_held: flags & 2 != 0,
                },
                commands,
            });
        }
        if reader.offset != bytes.len() {
            return Err(ReplayError::InvalidFormat);
        }
        Ok(Self {
            simulation_version,
            content_fingerprint,
            initial_tick,
            initial_world_fingerprint,
            initial_player_fingerprint,
            frames,
        })
    }
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}
fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}
fn put_i32(out: &mut Vec<u8>, value: i32) {
    out.extend_from_slice(&value.to_le_bytes());
}
fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl Reader<'_> {
    fn take(&mut self, size: usize) -> Result<&[u8], ReplayError> {
        let end = self
            .offset
            .checked_add(size)
            .ok_or(ReplayError::InvalidFormat)?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or(ReplayError::InvalidFormat)?;
        self.offset = end;
        Ok(bytes)
    }
    fn u8(&mut self) -> Result<u8, ReplayError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, ReplayError> {
        Ok(u16::from_le_bytes(
            self.take(2)?
                .try_into()
                .map_err(|_| ReplayError::InvalidFormat)?,
        ))
    }
    fn u32(&mut self) -> Result<u32, ReplayError> {
        Ok(u32::from_le_bytes(
            self.take(4)?
                .try_into()
                .map_err(|_| ReplayError::InvalidFormat)?,
        ))
    }
    fn i32(&mut self) -> Result<i32, ReplayError> {
        Ok(i32::from_le_bytes(
            self.take(4)?
                .try_into()
                .map_err(|_| ReplayError::InvalidFormat)?,
        ))
    }
    fn u64(&mut self) -> Result<u64, ReplayError> {
        Ok(u64::from_le_bytes(
            self.take(8)?
                .try_into()
                .map_err(|_| ReplayError::InvalidFormat)?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        core::fixed::Fixed,
        simulation::{command::WorldCommand, material::SAND},
    };

    #[test]
    fn replay_codec_and_candidate_playback_match_live_tick_sequence() {
        let mut source = World::new(64, 64).unwrap();
        let mut source_player = Player::new(
            Fixed::from_integer(8).unwrap(),
            Fixed::from_integer(8).unwrap(),
        );
        let mut log = ReplayLog::new(&source, &source_player);
        log.record(
            1,
            TickInput {
                horizontal_axis: 1,
                jump_pressed: false,
                jump_held: false,
            },
            vec![CommandEnvelope {
                target_tick: 1,
                source_id: 1,
                sequence: 0,
                payload: WorldCommand::PaintBrush {
                    center_x: 5,
                    center_y: 5,
                    radius: 0,
                    material: SAND,
                },
            }],
        )
        .unwrap();
        log.record(
            2,
            TickInput {
                horizontal_axis: 0,
                jump_pressed: false,
                jump_held: false,
            },
            vec![],
        )
        .unwrap();
        let bytes = log.encode().unwrap();
        let log = ReplayLog::decode(&bytes).unwrap();
        let mut expected = source.clone();
        let mut expected_player = source_player.clone();
        for frame in log.frames() {
            if !frame.commands.is_empty() {
                dispatch_command_batch(&mut expected, frame.commands.clone()).unwrap();
            }
            expected_player
                .update(
                    frame.input.horizontal_axis,
                    frame.input.jump_pressed,
                    frame.input.jump_held,
                    &mut expected,
                )
                .unwrap();
            expected.update().unwrap();
        }
        log.play(&mut source, &mut source_player).unwrap();
        assert_eq!(source.state_fingerprint(), expected.state_fingerprint());
        assert_eq!(source_player, expected_player);
    }

    #[test]
    fn replay_rejects_invalid_tick_and_content_without_mutating_candidates() {
        let world = World::new(16, 16).unwrap();
        let mut player = Player::new(Fixed::ZERO, Fixed::ZERO);
        let before = player.clone();
        let mut log = ReplayLog::new(&world, &player);
        assert_eq!(
            log.record(
                2,
                TickInput {
                    horizontal_axis: 0,
                    jump_pressed: false,
                    jump_held: false
                },
                vec![]
            ),
            Err(ReplayError::TickMismatch)
        );
        log.record(
            1,
            TickInput {
                horizontal_axis: 0,
                jump_pressed: false,
                jump_held: false,
            },
            vec![],
        )
        .unwrap();
        let mut other = World::new(16, 16).unwrap();
        other
            .apply_command(WorldCommand::PaintBrush {
                center_x: 1,
                center_y: 1,
                radius: 0,
                material: SAND,
            })
            .unwrap();
        assert_eq!(
            log.play(&mut other, &mut player),
            Err(ReplayError::InitialStateMismatch)
        );
        assert_eq!(player, before);
    }

    #[test]
    fn failed_frame_does_not_publish_partial_replay_state() {
        let mut world = World::new(16, 16).unwrap();
        let mut player = Player::new(Fixed::ZERO, Fixed::ZERO);
        let before_world = world.state_fingerprint();
        let before_player = player.clone();
        let mut log = ReplayLog::new(&world, &player);
        log.record(
            1,
            TickInput {
                horizontal_axis: 1,
                jump_pressed: false,
                jump_held: false,
            },
            vec![CommandEnvelope {
                target_tick: 1,
                source_id: 1,
                sequence: 0,
                payload: WorldCommand::PaintBrush {
                    center_x: 4,
                    center_y: 4,
                    radius: 0,
                    material: u16::MAX,
                },
            }],
        )
        .unwrap();
        assert_eq!(
            log.play(&mut world, &mut player),
            Err(ReplayError::InvalidFormat)
        );
        assert_eq!(world.state_fingerprint(), before_world);
        assert_eq!(player, before_player);
    }
}
