//! Q16.16を正規状態として使い、tickと衝突走査順から再現可能なPlayer移動を計算します。

use crate::core::fixed::{Fixed, FixedError};
use crate::simulation::determinism::sample_u32;
use crate::simulation::material::{Behavior, WATER};
use crate::simulation::particle::SplashParticle;
use crate::simulation::world::World;

const STEP: i64 = Fixed::SCALE / 16;
const WIDTH: Fixed = Fixed::from_raw(10 * Fixed::SCALE);
const HEIGHT: Fixed = Fixed::from_raw(18 * Fixed::SCALE);
const GROUND_SPEED: Fixed = Fixed::from_raw(72_090);
const WATER_SPEED: Fixed = Fixed::from_raw(36_045);
const GROUND_GRAVITY: Fixed = Fixed::from_raw(13_107);
const WATER_GRAVITY: Fixed = Fixed::from_raw(3_277);
const GROUND_JUMP: Fixed = Fixed::from_raw(-249_037);
const WATER_JUMP: Fixed = Fixed::from_raw(-98_304);
const GROUND_FALL: Fixed = Fixed::from_raw(6 * Fixed::SCALE);
const WATER_FALL: Fixed = Fixed::from_raw(117_965);
const GROUND_FRICTION: Fixed = Fixed::from_raw(39_322);
const WATER_FRICTION: Fixed = Fixed::from_raw(49_152);
const STOP_SPEED: Fixed = Fixed::from_raw(3_277);
const MOVE_THRESHOLD_X: Fixed = Fixed::from_raw(19_661);
const MOVE_THRESHOLD_Y: Fixed = Fixed::from_raw(32_768);
const SPLASH_STEP: Fixed = Fixed::from_raw(3 * Fixed::SCALE);
const MAX_AXIS_SPEED_RAW: u64 = 6 * Fixed::SCALE as u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerError {
    Arithmetic(FixedError),
    CoordinateOutOfRange,
    VelocityOutOfRange,
}

impl From<FixedError> for PlayerError {
    fn from(value: FixedError) -> Self {
        Self::Arithmetic(value)
    }
}
impl std::fmt::Display for PlayerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "player update failed: {self:?}")
    }
}
impl std::error::Error for PlayerError {}

/// 正規状態として保存するPlayerです。位置・速度・寸法は固定小数点で保持し、浮動小数点は表示演出だけに使います。
/// この分離により表示頻度やGPU差が移動結果へ混入せず、snapshotとReplayで同じ衝突位置を再現できます。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Player {
    pub x: Fixed,
    pub y: Fixed,
    pub vx: Fixed,
    pub vy: Fixed,
    pub width: Fixed,
    pub height: Fixed,
    pub is_grounded: bool,
    pub is_in_water: bool,
    step_timer: Fixed,
}

impl Player {
    pub(crate) fn state_fingerprint(&self) -> u64 {
        let mut hash = crate::simulation::determinism::Fnv64::default();
        hash.write(b"charon-player-q16-v1");
        for value in [
            self.x,
            self.y,
            self.vx,
            self.vy,
            self.width,
            self.height,
            self.step_timer,
        ] {
            hash.write(&value.raw().to_le_bytes());
        }
        hash.write(&[u8::from(self.is_grounded) | (u8::from(self.is_in_water) << 1)]);
        hash.finish()
    }
    pub fn new(x: Fixed, y: Fixed) -> Self {
        Self {
            x,
            y,
            vx: Fixed::ZERO,
            vy: Fixed::ZERO,
            width: WIDTH,
            height: HEIGHT,
            is_grounded: false,
            is_in_water: false,
            step_timer: Fixed::ZERO,
        }
    }

    pub(crate) fn encode_state(&self, out: &mut Vec<u8>) {
        for value in [
            self.x,
            self.y,
            self.vx,
            self.vy,
            self.width,
            self.height,
            self.step_timer,
        ] {
            out.extend_from_slice(&value.raw().to_le_bytes());
        }
        out.push(u8::from(self.is_grounded) | (u8::from(self.is_in_water) << 1));
    }

    pub(crate) fn decode_state(bytes: &[u8], offset: &mut usize) -> Result<Self, PlayerError> {
        fn read_i64(bytes: &[u8], offset: &mut usize) -> Result<i64, PlayerError> {
            let end = offset
                .checked_add(8)
                .ok_or(PlayerError::CoordinateOutOfRange)?;
            let slice = bytes
                .get(*offset..end)
                .ok_or(PlayerError::CoordinateOutOfRange)?;
            *offset = end;
            Ok(i64::from_le_bytes(
                slice
                    .try_into()
                    .map_err(|_| PlayerError::CoordinateOutOfRange)?,
            ))
        }
        let x = Fixed::from_raw(read_i64(bytes, offset)?);
        let y = Fixed::from_raw(read_i64(bytes, offset)?);
        let vx = Fixed::from_raw(read_i64(bytes, offset)?);
        let vy = Fixed::from_raw(read_i64(bytes, offset)?);
        let width = Fixed::from_raw(read_i64(bytes, offset)?);
        let height = Fixed::from_raw(read_i64(bytes, offset)?);
        let step_timer = Fixed::from_raw(read_i64(bytes, offset)?);
        let flags = *bytes
            .get(*offset)
            .ok_or(PlayerError::CoordinateOutOfRange)?;
        *offset += 1;
        if flags & !3 != 0
            || width != WIDTH
            || height != HEIGHT
            || step_timer < Fixed::ZERO
            || step_timer > SPLASH_STEP
            || vx.raw().unsigned_abs() > MAX_AXIS_SPEED_RAW
            || vy.raw().unsigned_abs() > MAX_AXIS_SPEED_RAW
            || x.floor_i64() < i32::MIN as i64
            || x.floor_i64() > i32::MAX as i64
            || y.floor_i64() < i32::MIN as i64
            || y.floor_i64() > i32::MAX as i64
            || Self::checked_cell_bounds(x, width).is_err()
            || Self::checked_cell_bounds(y, height).is_err()
        {
            return Err(PlayerError::CoordinateOutOfRange);
        }
        Ok(Self {
            x,
            y,
            vx,
            vy,
            width,
            height,
            is_grounded: flags & 1 != 0,
            is_in_water: flags & 2 != 0,
            step_timer,
        })
    }

    /// Advance one canonical simulation tick. `input_x` is restricted to -1, 0, or 1.
    pub fn update(
        &mut self,
        input_x: i8,
        jump_pressed: bool,
        jump_held: bool,
        world: &mut World,
    ) -> Result<(), PlayerError> {
        if !(-1..=1).contains(&input_x) {
            return Err(PlayerError::CoordinateOutOfRange);
        }
        if self.vx.raw().unsigned_abs() > MAX_AXIS_SPEED_RAW
            || self.vy.raw().unsigned_abs() > MAX_AXIS_SPEED_RAW
        {
            return Err(PlayerError::VelocityOutOfRange);
        }
        let mut next = self.clone();
        next.update_inner(input_x, jump_pressed, jump_held, world)?;
        *self = next;
        Ok(())
    }

    fn update_inner(
        &mut self,
        input_x: i8,
        jump_pressed: bool,
        jump_held: bool,
        world: &mut World,
    ) -> Result<(), PlayerError> {
        self.check_water(world)?;
        let (speed, gravity, jump, max_fall, friction) = if self.is_in_water {
            (
                WATER_SPEED,
                WATER_GRAVITY,
                WATER_JUMP,
                WATER_FALL,
                WATER_FRICTION,
            )
        } else {
            (
                GROUND_SPEED,
                GROUND_GRAVITY,
                GROUND_JUMP,
                GROUND_FALL,
                GROUND_FRICTION,
            )
        };
        if input_x != 0 {
            self.vx = speed.checked_mul(Fixed::from_raw(i64::from(input_x) * Fixed::SCALE))?;
        } else {
            self.vx = self.vx.checked_mul(friction)?;
            if self.vx.raw().unsigned_abs() < STOP_SPEED.raw() as u64 {
                self.vx = Fixed::ZERO;
            }
        }
        if jump_pressed && self.is_grounded {
            self.vy = jump;
            self.is_grounded = false;
        } else if jump_held && self.is_in_water {
            self.vy = jump;
        }
        self.vy = self.vy.checked_add(gravity)?.min(max_fall);
        self.move_axis(true, world)?;
        self.move_axis(false, world)?;
        self.interact_with_water(world)?;
        Ok(())
    }

    fn checked_cell_bounds(pos: Fixed, extent: Fixed) -> Result<(i32, i32), PlayerError> {
        let end = pos.checked_add(extent)?;
        let start = pos.floor_i64();
        let end_raw = end.raw();
        let end_cell =
            end_raw.div_euclid(Fixed::SCALE) + i64::from(end_raw.rem_euclid(Fixed::SCALE) != 0);
        if start < i32::MIN as i64 || end_cell > i32::MAX as i64 {
            return Err(PlayerError::CoordinateOutOfRange);
        }
        Ok((start as i32, end_cell as i32))
    }

    fn check_water(&mut self, world: &World) -> Result<(), PlayerError> {
        let (xs, xe) = Self::checked_cell_bounds(self.x, self.width)?;
        let (ys, ye) = Self::checked_cell_bounds(self.y, self.height)?;
        let mut water = 0;
        let mut total = 0;
        for y in ys..ye {
            for x in xs..xe {
                total += 1;
                if world.get_cell(x, y).material == WATER {
                    water += 1;
                }
            }
        }
        self.is_in_water = total > 0 && water > 4;
        Ok(())
    }

    fn move_axis(&mut self, horizontal: bool, world: &World) -> Result<(), PlayerError> {
        let velocity = if horizontal { self.vx } else { self.vy };
        let raw = velocity.raw();
        let count = raw.unsigned_abs().div_ceil(STEP as u64);
        if !horizontal {
            self.is_grounded = false;
        }
        if count == 0 {
            return Ok(());
        }
        let sign = raw.signum();
        let mut progressed = 0_i64;
        for index in 0..count {
            let delta = (raw.unsigned_abs().min((index + 1) * STEP as u64) as i64) - progressed;
            progressed += delta;
            let signed = delta * sign;
            let candidate = if horizontal {
                self.x.checked_add(Fixed::from_raw(signed))?
            } else {
                self.y.checked_add(Fixed::from_raw(signed))?
            };
            let (test_x, test_y) = if horizontal {
                (candidate, self.y)
            } else {
                (self.x, candidate)
            };
            if self.collides(test_x, test_y, world)? {
                if !horizontal && sign > 0 {
                    self.is_grounded = true;
                }
                if horizontal {
                    self.vx = Fixed::ZERO;
                } else {
                    self.vy = Fixed::ZERO;
                }
                break;
            }
            if horizontal {
                self.x = candidate;
            } else {
                self.y = candidate;
            }
        }
        Ok(())
    }

    fn collides(&self, x: Fixed, y: Fixed, world: &World) -> Result<bool, PlayerError> {
        let (xs, xe) = Self::checked_cell_bounds(x, self.width)?;
        let (ys, ye) = Self::checked_cell_bounds(y, self.height)?;
        let gx0 = xs.div_euclid(4);
        let gx1 = (xe + 3).div_euclid(4);
        let gy0 = ys.div_euclid(4);
        let gy1 = (ye + 3).div_euclid(4);
        for gy in gy0..gy1 {
            for gx in gx0..gx1 {
                let mut solid = 0;
                for py in 0..4 {
                    for px in 0..4 {
                        let cx = gx * 4 + px;
                        let cy = gy * 4 + py;
                        if cx >= xs && cx < xe && cy >= ys && cy < ye {
                            let cell = world.get_cell(cx, cy);
                            let behavior = world.registry.get(cell.material).behavior;
                            if (behavior == Behavior::Solid && !cell.is_falling())
                                || behavior == Behavior::Powder
                            {
                                solid += 1;
                            }
                        }
                    }
                }
                if solid >= 8 {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn interact_with_water(&mut self, world: &mut World) -> Result<(), PlayerError> {
        let moving = self.vx.raw().unsigned_abs() > MOVE_THRESHOLD_X.raw() as u64
            || self.vy.raw().unsigned_abs() > MOVE_THRESHOLD_Y.raw() as u64;
        if !moving {
            return Ok(());
        }
        let center_x = self
            .x
            .checked_add(self.width.checked_mul(Fixed::from_raw(Fixed::SCALE / 2))?)?;
        let foot_y = self.y.checked_add(self.height)?;
        world.displace_water(
            center_x.floor_i64() as i32,
            foot_y.floor_i64() as i32,
            self.vx.raw().signum() as i8,
        );
        self.step_timer = self.step_timer.checked_add(Fixed::from_raw(
            self.vx.raw().unsigned_abs().min(i64::MAX as u64) as i64,
        ))?;
        if self.step_timer > SPLASH_STEP {
            self.step_timer = Fixed::ZERO;
            let fx = center_x.floor_i64() as i32;
            let fy = foot_y.floor_i64() as i32;
            if world.get_cell(fx, fy).material == WATER
                || world.get_cell(fx, fy - 1).material == WATER
            {
                let px = center_x.to_f64() as f32;
                let py = fy as f32;
                for i in 0..3_u32 {
                    let key = (fx, fy, world.tick_count);
                    let offset_x = px + rand_offset(key, i * 3) * 6.0;
                    let offset_y = py - 2.0;
                    let p_vx = -(self.vx.to_f64() as f32) * 0.3 + rand_offset(key, i * 3 + 1) * 1.5;
                    let p_vy = -1.2 - rand_offset(key, i * 3 + 2).abs() * 1.5;
                    world.add_particle(SplashParticle::new(
                        offset_x,
                        offset_y,
                        p_vx,
                        p_vy,
                        0.4,
                        [120, 200, 255, 230],
                    ));
                }
            }
        }
        Ok(())
    }
}

fn rand_offset(key: (i32, i32, u64), sample: u32) -> f32 {
    let (x, y, tick) = key;
    let bits = sample_u32(1337, tick, x, y, 0x5350_4c53, sample) >> 8;
    (bits as f32 / 8_388_607.5) - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn player_state_uses_q16_16_and_fractional_motion_is_preserved() {
        let mut world = World::new(64, 64).unwrap();
        let mut player = Player::new(
            Fixed::from_integer(10).unwrap(),
            Fixed::from_integer(10).unwrap(),
        );
        player.update(1, false, false, &mut world).unwrap();
        assert_eq!(
            player.x,
            Fixed::from_integer(11)
                .unwrap()
                .checked_add(Fixed::from_raw(6_554))
                .unwrap()
        );
        assert_eq!(player.vx, GROUND_SPEED);
    }
    #[test]
    fn invalid_axis_is_rejected_without_mutation() {
        let mut world = World::new(64, 64).unwrap();
        let mut player = Player::new(Fixed::ZERO, Fixed::ZERO);
        let before = player.clone();
        assert!(player.update(2, false, false, &mut world).is_err());
        assert_eq!(player, before);
    }

    #[test]
    fn unbounded_external_velocity_is_rejected_before_iteration() {
        let mut world = World::new(16, 16).unwrap();
        let mut player = Player::new(Fixed::ZERO, Fixed::ZERO);
        player.vx = Fixed::from_raw(i64::MAX);
        let before = player.clone();
        assert_eq!(
            player.update(0, false, false, &mut world),
            Err(PlayerError::VelocityOutOfRange)
        );
        assert_eq!(player, before);
    }

    #[test]
    fn arithmetic_error_does_not_partially_commit_player_or_world() {
        let mut world = World::new(64, 64).unwrap();
        let mut player = Player::new(Fixed::from_raw(i64::MAX - 8), Fixed::ZERO);
        let before = player.clone();
        let world_hash = world.state_fingerprint();
        assert!(player.update(1, false, false, &mut world).is_err());
        assert_eq!(player, before);
        assert_eq!(world.state_fingerprint(), world_hash);
    }
    #[test]
    fn splash_randomness_is_keyed_by_tick_position_and_channel() {
        let key = (32, 48, 19);
        assert_eq!(rand_offset(key, 3), rand_offset(key, 3));
        assert_ne!(rand_offset(key, 3), rand_offset(key, 4));
        assert_ne!(rand_offset(key, 3), rand_offset((33, 48, 19), 3));
    }
}
