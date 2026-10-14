//! # ワールド（World）モジュール
//!
//! セル・オートマトンの物理世界全体を管理します。
//! ノイズによる地形生成、セルの高速更新（水の多段落下・流砂押し流し）、
//! 4x4 半ブロック体積不足時の構造崩落（粒子化）、および適応的レンダリングを担当します。

use noise::{Fbm, NoiseFn, SuperSimplex};
use std::collections::{HashMap, HashSet};

use crate::simulation::cell::Cell;
use crate::simulation::chunk::{CHUNK_AREA, CHUNK_EDGE, ChunkCoord};
use crate::simulation::chunk::{ChunkStoreError, ChunkedCells};
use crate::simulation::command::{
    CommandError, CommandResult, MAX_BRUSH_RADIUS, MAX_COMMAND_CANDIDATE_CELLS, WorldCommand,
    WorldError,
};
use crate::simulation::determinism::Fnv64;
use crate::simulation::material::{AIR, Behavior, DIRT, MaterialRegistry, SAND, STONE, WATER};
use crate::simulation::particle::SplashParticle;
use crate::simulation::streaming::{ChunkRect, ChunkRequestPlan, LoadMode, StreamingPolicy};

/// シミュレーションワールド構造体
#[derive(Clone)]
pub struct World {
    /// ワールドの幅（セル単位・論理ピクセル）
    pub width: u32,
    /// ワールドの高さ（セル単位・論理ピクセル）
    pub height: u32,
    /// 有限worldと追加resident chunkのセル格納。
    pub cells: ChunkedCells,
    /// 動的マテリアルレジストリ（最大 65,535 種のブロックを管理）
    pub registry: MaterialRegistry,
    /// 二重移動防止用のセル外scratch。保存対象ではなく復元時に再構築します。
    movement_stamps: Vec<u64>,
    /// Integer thermal energy sidecar for finite cells; never stored in Cell::data.
    thermal_energy: Vec<i32>,
    thermal_delta_scratch: Vec<i64>,
    thermal_next_scratch: Vec<i32>,
    remote_movement_stamps: HashMap<(i32, i32), u64>,
    active_remote_chunks: HashSet<ChunkCoord>,
    active_initial_chunks: Option<HashSet<ChunkCoord>>,
    chunk_leases: HashMap<u64, ChunkLease>,
    next_chunk_lease_id: u64,
    /// 現在の総 tick 数（走査方向の反転等に使用）
    pub tick_count: u64,
    /// 空間内を自由飛翔する水しぶき粒子リスト
    pub particles: Vec<SplashParticle>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotError {
    InvalidFormat,
    UnsupportedVersion,
    ContentMismatch,
    InvalidState,
    CapacityExceeded,
    AllocationFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellQuery {
    Available(Cell),
    Unavailable,
    WorldBoundary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkLease {
    pub id: u64,
    pub source_id: u32,
    pub region: ChunkRect,
    pub mode: LoadMode,
}

const SNAPSHOT_MAGIC: &[u8; 8] = b"CHARON\0\x01";
const SNAPSHOT_VERSION: u16 = 4;
const SIMULATION_VERSION: u32 = 5;
const GENERATOR_VERSION: u32 = 1;
const MAX_SNAPSHOT_BYTES: usize = 512 * CHUNK_AREA * 4 + 16_000_000;

impl World {
    pub const fn simulation_version() -> u32 {
        SIMULATION_VERSION
    }

    pub(crate) fn try_clone_stopped(&self) -> Result<Self, SnapshotError> {
        let bytes = self.encode_snapshot()?;
        Self::decode_snapshot(&bytes, self.registry.clone())
    }

    pub fn content_fingerprint(&self) -> u64 {
        registry_fingerprint(&self.registry)
    }
    /// デフォルトの解像度と基本レジストリで新しいワールドを初期化し、地形を自動生成します。
    pub fn new(width: u32, height: u32) -> Result<Self, WorldError> {
        Self::try_new(width, height)
    }

    pub fn try_new(width: u32, height: u32) -> Result<Self, WorldError> {
        Self::try_with_registry(width, height, MaterialRegistry::with_defaults())
    }

    /// 指定されたカスタムマテリアルレジストリを用いてワールドを初期化します。
    pub fn with_registry(
        width: u32,
        height: u32,
        registry: MaterialRegistry,
    ) -> Result<Self, WorldError> {
        Self::try_with_registry(width, height, registry)
    }

    pub fn try_with_registry(
        width: u32,
        height: u32,
        mut registry: MaterialRegistry,
    ) -> Result<Self, WorldError> {
        registry
            .try_freeze()
            .map_err(|_| WorldError::InvalidMaterialRegistry)?;
        let cells =
            ChunkedCells::try_new(0, 0, width, height, Cell::new(AIR)).map_err(map_chunk_error)?;
        let total = cells.len();
        let mut movement_stamps = Vec::new();
        movement_stamps
            .try_reserve_exact(total)
            .map_err(|_| WorldError::AllocationFailed)?;
        movement_stamps.resize(total, 0);
        let mut thermal_energy = Vec::new();
        thermal_energy
            .try_reserve_exact(total)
            .map_err(|_| WorldError::AllocationFailed)?;
        thermal_energy.resize(total, 0);
        let mut thermal_delta_scratch = Vec::new();
        thermal_delta_scratch
            .try_reserve_exact(total)
            .map_err(|_| WorldError::AllocationFailed)?;
        thermal_delta_scratch.resize(total, 0);
        let mut thermal_next_scratch = Vec::new();
        thermal_next_scratch
            .try_reserve_exact(total)
            .map_err(|_| WorldError::AllocationFailed)?;
        thermal_next_scratch.resize(total, 0);
        let mut world = Self {
            width,
            height,
            cells,
            registry,
            movement_stamps,
            thermal_energy,
            thermal_delta_scratch,
            thermal_next_scratch,
            remote_movement_stamps: HashMap::new(),
            active_remote_chunks: HashSet::new(),
            active_initial_chunks: None,
            chunk_leases: HashMap::new(),
            next_chunk_lease_id: 1,
            tick_count: 0,
            particles: Vec::new(),
        };

        world.generate_terrain();
        Ok(world)
    }

    /// ノイズアルゴリズム（Simplex FBM）を用いて地形を生成します。
    pub fn generate_terrain(&mut self) {
        let fbm = Fbm::<SuperSimplex>::new(1337);

        for y in 0..self.height {
            for x in 0..self.width {
                let idx = (y * self.width + x) as usize;

                let nx = x as f64 * 0.008;
                let height_noise = fbm.get([nx, 0.0]) * 45.0;
                let ground_y = 220.0 + height_noise;

                let density_noise = fbm.get([x as f64 * 0.02, y as f64 * 0.02]);

                if (y as f64) >= ground_y {
                    let depth = (y as f64) - ground_y;
                    let tex_idx = (((y % 8) * 8) + (x % 8)) as u8;

                    if depth > 35.0 || density_noise > 0.35 {
                        self.cells[idx] = Cell::with_shade(STONE, tex_idx);
                    } else if density_noise < -0.45 && depth > 10.0 {
                        self.cells[idx] = Cell::new(AIR);
                    } else {
                        self.cells[idx] = Cell::with_shade(DIRT, tex_idx);
                    }
                } else if (210..225).contains(&y) && (260..=370).contains(&x) {
                    let tex_idx = (((y % 8) * 8) + (x % 8)) as u8;
                    self.cells[idx] = Cell::with_shade(WATER, tex_idx);
                } else if (200..222).contains(&y)
                    && (370..=400).contains(&x)
                    && (x - 370) > (222 - y)
                {
                    let tex_idx = (((y % 8) * 8) + (x % 8)) as u8;
                    self.cells[idx] = Cell::with_shade(SAND, tex_idx);
                } else {
                    self.cells[idx] = Cell::new(AIR);
                }
            }
        }
    }

    /// ワールド内の全セル、粒子、および構造支持崩落を 1 tick（1/60 秒）進めます。
    pub fn update(&mut self) -> Result<(), WorldError> {
        self.ensure_tick_available()?;
        let remote_count = self
            .active_remote_chunks
            .len()
            .checked_mul(CHUNK_AREA)
            .ok_or(WorldError::WorldCapacityExceeded)?;
        let mut remote_coordinates = Vec::new();
        remote_coordinates
            .try_reserve_exact(remote_count)
            .map_err(|_| WorldError::AllocationFailed)?;
        let mut active_chunks = self
            .active_remote_chunks
            .iter()
            .copied()
            .collect::<Vec<_>>();
        active_chunks.sort_unstable();
        for coord in active_chunks {
            let origin_x = coord
                .x
                .checked_mul(CHUNK_EDGE)
                .ok_or(WorldError::InvalidWorldDimensions)?;
            let origin_y = coord
                .y
                .checked_mul(CHUNK_EDGE)
                .ok_or(WorldError::InvalidWorldDimensions)?;
            for local_y in 0..CHUNK_EDGE {
                for local_x in 0..CHUNK_EDGE {
                    remote_coordinates.push((origin_x + local_x, origin_y + local_y));
                }
            }
        }
        remote_coordinates.sort_unstable_by(|left, right| {
            right.1.cmp(&left.1).then_with(|| {
                if (self.tick_count + 1).is_multiple_of(2) {
                    left.0.cmp(&right.0)
                } else {
                    right.0.cmp(&left.0)
                }
            })
        });
        self.remote_movement_stamps
            .try_reserve(remote_count)
            .map_err(|_| WorldError::AllocationFailed)?;
        self.tick_count += 1;

        // 1. 体積不足の 4x4 半ブロックおよび孤立浮遊ピクセルの崩落判定
        self.check_structural_collapse();
        self.check_remote_structural_collapse();

        // 2. 下から上への物理シミュレーション走査
        let ascending = self.tick_count.is_multiple_of(2);
        let mut remote_index = 0;
        for y in (0..self.height as i32).rev() {
            while remote_index < remote_coordinates.len() && remote_coordinates[remote_index].1 > y
            {
                self.update_remote_cell(
                    remote_coordinates[remote_index].0,
                    remote_coordinates[remote_index].1,
                );
                remote_index += 1;
            }
            if ascending {
                while remote_index < remote_coordinates.len()
                    && remote_coordinates[remote_index].1 == y
                    && remote_coordinates[remote_index].0 < 0
                {
                    self.update_remote_cell(remote_coordinates[remote_index].0, y);
                    remote_index += 1;
                }
                for x in 0..self.width as i32 {
                    if self.initial_chunk_active(x, y) {
                        self.update_cell(x as u32, y as u32);
                    }
                }
                while remote_index < remote_coordinates.len()
                    && remote_coordinates[remote_index].1 == y
                {
                    self.update_remote_cell(remote_coordinates[remote_index].0, y);
                    remote_index += 1;
                }
            } else {
                while remote_index < remote_coordinates.len()
                    && remote_coordinates[remote_index].1 == y
                    && remote_coordinates[remote_index].0 >= self.width as i32
                {
                    self.update_remote_cell(remote_coordinates[remote_index].0, y);
                    remote_index += 1;
                }
                for x in (0..self.width as i32).rev() {
                    if self.initial_chunk_active(x, y) {
                        self.update_cell(x as u32, y as u32);
                    }
                }
                while remote_index < remote_coordinates.len()
                    && remote_coordinates[remote_index].1 == y
                {
                    self.update_remote_cell(remote_coordinates[remote_index].0, y);
                    remote_index += 1;
                }
            }
        }
        while remote_index < remote_coordinates.len() {
            self.update_remote_cell(
                remote_coordinates[remote_index].0,
                remote_coordinates[remote_index].1,
            );
            remote_index += 1;
        }

        // P6: diffuse after material movement so energy follows the cell sidecar through P5 swaps.
        crate::simulation::thermal::diffuse_energy_into(
            self.width as usize,
            self.height as usize,
            &self.thermal_energy,
            1,
            8,
            &mut self.thermal_delta_scratch,
            &mut self.thermal_next_scratch,
        )
        .map_err(|_| WorldError::AllocationFailed)?;
        for y in 0..self.height as usize {
            for x in 0..self.width as usize {
                if self.initial_chunk_active(x as i32, y as i32) {
                    let index = y * self.width as usize + x;
                    self.thermal_energy[index] = self.thermal_next_scratch[index];
                }
            }
        }

        // 3. 自由粒子の更新
        self.particles.retain_mut(|p| p.update(0.12));
        Ok(())
    }

    /// Set a finite-cell energy value. Energy uses an integer engine unit and is conserved
    /// by diffusion; material-specific heat capacity and remote sidecars are not yet enabled.
    pub fn set_thermal_energy(&mut self, x: u32, y: u32, energy: i32) -> bool {
        if x >= self.width || y >= self.height || !self.initial_chunk_active(x as i32, y as i32) {
            return false;
        }
        self.thermal_energy[(y * self.width + x) as usize] = energy;
        true
    }

    pub fn thermal_energy(&self, x: u32, y: u32) -> Option<i32> {
        if x >= self.width || y >= self.height {
            return None;
        }
        Some(self.thermal_energy[(y * self.width + x) as usize])
    }

    fn initial_chunk_active(&self, x: i32, y: i32) -> bool {
        self.active_initial_chunks
            .as_ref()
            .is_none_or(|active| active.contains(&ChunkCoord::from_cell(x, y)))
    }

    fn world_cell_active(&self, x: i32, y: i32) -> bool {
        let coordinate = ChunkCoord::from_cell(x, y);
        if self.cells.is_initial_chunk(coordinate) {
            self.initial_chunk_active(x, y)
        } else {
            self.active_remote_chunks.contains(&coordinate)
        }
    }

    /// Pin a remote or finite chunk region until its stable lease ID is explicitly released.
    pub fn request_chunk_lease(
        &mut self,
        policy: StreamingPolicy,
        source_id: u32,
        region: ChunkRect,
        mode: LoadMode,
    ) -> Result<u64, WorldError> {
        let request = policy
            .plan_request(region, mode)
            .map_err(|error| match error {
                crate::simulation::streaming::StreamError::ActiveCapacityExceeded => {
                    WorldError::ActiveChunkCapacityExceeded
                }
                _ => WorldError::WorldCapacityExceeded,
            })?;
        if self.chunk_leases.len() >= policy.max_pending_requests {
            return Err(WorldError::LeaseCapacityExceeded);
        }
        let id = self.next_chunk_lease_id;
        let next_id = id.checked_add(1).ok_or(WorldError::TickOverflow)?;
        let mut pinned = HashSet::new();
        let requested_count = request.chunks;
        pinned
            .try_reserve(requested_count.saturating_add(policy.max_pinned_chunks))
            .map_err(|_| WorldError::AllocationFailed)?;
        for lease in self.chunk_leases.values() {
            for y in lease.region.min.y..lease.region.max.y {
                for x in lease.region.min.x..lease.region.max.x {
                    pinned.insert(ChunkCoord { x, y });
                }
            }
        }
        for y in region.min.y..region.max.y {
            for x in region.min.x..region.max.x {
                pinned.insert(ChunkCoord { x, y });
            }
        }
        if pinned.len() > policy.max_pinned_chunks {
            return Err(WorldError::PinnedChunkCapacityExceeded);
        }
        self.chunk_leases
            .try_reserve(1)
            .map_err(|_| WorldError::AllocationFailed)?;
        self.request_remote_chunks(
            request,
            policy.max_resident_chunks,
            policy.max_active_chunks,
        )?;
        self.chunk_leases.insert(
            id,
            ChunkLease {
                id,
                source_id,
                region,
                mode,
            },
        );
        self.next_chunk_lease_id = next_id;
        Ok(id)
    }

    pub fn release_chunk_lease(&mut self, lease_id: u64) -> bool {
        self.chunk_leases.remove(&lease_id).is_some()
    }

    pub fn chunk_lease(&self, id: u64) -> Option<ChunkLease> {
        self.chunk_leases.get(&id).copied()
    }

    pub fn chunk_lease_count(&self) -> usize {
        self.chunk_leases.len()
    }

    pub fn active_chunk_count(&self) -> usize {
        let initial = self.active_initial_chunks.as_ref().map_or_else(
            || {
                (self.width as usize).div_ceil(CHUNK_EDGE as usize)
                    * (self.height as usize).div_ceil(CHUNK_EDGE as usize)
            },
            HashSet::len,
        );
        initial + self.active_remote_chunks.len()
    }

    pub fn resident_chunk_count(&self) -> usize {
        self.cells.chunk_count()
    }

    pub fn pinned_chunk_count(&self) -> usize {
        let mut pinned = HashSet::new();
        for lease in self.chunk_leases.values() {
            for y in lease.region.min.y..lease.region.max.y {
                for x in lease.region.min.x..lease.region.max.x {
                    pinned.insert(ChunkCoord { x, y });
                }
            }
        }
        pinned.len()
    }

    /// Apply a visible-area plan to initial and remote chunks atomically with capacity validation.
    pub fn apply_visible_chunk_plan(
        &mut self,
        plan: crate::simulation::streaming::VisibleChunkPlan,
        resident_limit: usize,
        active_limit: usize,
    ) -> Result<(), WorldError> {
        let resident_count = plan
            .resident
            .chunk_count()
            .map_err(|_| WorldError::InvalidWorldDimensions)?;
        let active_count = plan
            .active
            .chunk_count()
            .map_err(|_| WorldError::InvalidWorldDimensions)?;
        if active_count > active_limit {
            return Err(WorldError::ActiveChunkCapacityExceeded);
        }
        if plan.active.min.x < plan.resident.min.x
            || plan.active.min.y < plan.resident.min.y
            || plan.active.max.x > plan.resident.max.x
            || plan.active.max.y > plan.resident.max.y
        {
            return Err(WorldError::InvalidWorldDimensions);
        }
        if resident_count > resident_limit {
            return Err(WorldError::WorldCapacityExceeded);
        }
        let mut active_initial = HashSet::new();
        active_initial
            .try_reserve(active_count.min(resident_count))
            .map_err(|_| WorldError::AllocationFailed)?;
        let mut resident = Vec::new();
        resident
            .try_reserve_exact(resident_count)
            .map_err(|_| WorldError::AllocationFailed)?;
        let mut active_remote = Vec::new();
        active_remote
            .try_reserve_exact(active_count)
            .map_err(|_| WorldError::AllocationFailed)?;
        for y in plan.resident.min.y..plan.resident.max.y {
            for x in plan.resident.min.x..plan.resident.max.x {
                let coord = ChunkCoord { x, y };
                let is_initial = self.cells.is_initial_chunk(coord);
                let active = x >= plan.active.min.x
                    && x < plan.active.max.x
                    && y >= plan.active.min.y
                    && y < plan.active.max.y;
                resident.push(coord);
                if is_initial {
                    if active {
                        active_initial.insert(coord);
                    }
                } else if active {
                    active_remote.push(coord);
                }
            }
        }
        for lease in self.chunk_leases.values() {
            for y in lease.region.min.y..lease.region.max.y {
                for x in lease.region.min.x..lease.region.max.x {
                    let coord = ChunkCoord { x, y };
                    resident.push(coord);
                    if lease.mode == LoadMode::Simulate {
                        if self.cells.is_initial_chunk(coord) {
                            active_initial.insert(coord);
                        } else {
                            active_remote.push(coord);
                        }
                    }
                }
            }
        }
        resident.sort_unstable();
        resident.dedup();
        active_remote.sort_unstable();
        active_remote.dedup();
        let remote_active = active_remote.len();
        if active_initial
            .len()
            .checked_add(remote_active)
            .is_none_or(|count| count > active_limit)
        {
            return Err(WorldError::ActiveChunkCapacityExceeded);
        }
        let initial_chunk_count = self.cells.chunk_count() - self.cells.remote_chunks().count();
        let missing_resident = resident
            .iter()
            .filter(|coord| {
                !self.cells.is_initial_chunk(**coord)
                    && self
                        .cells
                        .get_world(coord.x * CHUNK_EDGE, coord.y * CHUNK_EDGE)
                        .is_none()
            })
            .count();
        if initial_chunk_count
            .checked_add(self.cells.remote_chunks().count())
            .and_then(|n| n.checked_add(missing_resident))
            .is_none_or(|n| n > resident_limit)
        {
            return Err(WorldError::WorldCapacityExceeded);
        }
        if remote_active > active_limit {
            return Err(WorldError::ActiveChunkCapacityExceeded);
        }
        self.active_remote_chunks
            .try_reserve(remote_active)
            .map_err(|_| WorldError::AllocationFailed)?;
        self.cells
            .ensure_chunks(&resident, resident_limit, Cell::new(AIR))
            .map_err(map_chunk_error)?;
        self.active_remote_chunks.clear();
        self.active_remote_chunks.extend(active_remote);
        self.active_initial_chunks = Some(active_initial);
        Ok(())
    }

    fn update_remote_cell(&mut self, x: i32, y: i32) {
        let Some(cell) = self.cells.get_world(x, y).copied() else {
            return;
        };
        if cell.material == AIR
            || !self.registry.is_registered(cell.material)
            || self.remote_movement_stamps.get(&(x, y)) == Some(&self.tick_count)
        {
            return;
        }
        let definition = self.registry.get(cell.material);
        let behavior = definition.behavior;
        let mass = definition.mass_per_cell;
        let max_fall = i32::from(definition.max_fall_speed.max(1));
        let dispersion = i32::from(definition.dispersion);
        let mut target = None;
        if cell.is_falling() || behavior == Behavior::Powder {
            if let Some(below) = self.cells.get_world(x, y + 1)
                && (below.material == AIR || self.registry.get(below.material).mass_per_cell < mass)
            {
                target = Some((x, y + 1));
            }
            if target.is_none() {
                let offsets = if (x ^ y ^ self.tick_count as i32).rem_euclid(2) == 0 {
                    [-1, 1]
                } else {
                    [1, -1]
                };
                for dx in offsets {
                    if let Some(next) = self.cells.get_world(x + dx, y + 1)
                        && (next.material == AIR
                            || self.registry.get(next.material).mass_per_cell < mass)
                    {
                        target = Some((x + dx, y + 1));
                        break;
                    }
                }
            }
        } else if behavior == Behavior::Liquid {
            for step in 1..=max_fall {
                match self.cells.get_world(x, y + step) {
                    Some(next) if next.material == AIR => target = Some((x, y + step)),
                    _ => break,
                }
            }
            if target.is_none() {
                let offsets = if (x + y + self.tick_count as i32).rem_euclid(2) == 0 {
                    [-1, 1]
                } else {
                    [1, -1]
                };
                for dx in offsets {
                    if self
                        .cells
                        .get_world(x + dx, y + 1)
                        .is_some_and(|next| next.material == AIR)
                    {
                        target = Some((x + dx, y + 1));
                        break;
                    }
                }
            }
            if target.is_none() {
                for dx in [-1, 1] {
                    for step in 1..=dispersion {
                        let nx = x + dx * step;
                        match self.cells.get_world(nx, y) {
                            Some(next) if next.material == AIR => target = Some((nx, y)),
                            Some(next)
                                if self.registry.get(next.material).behavior
                                    == Behavior::Liquid => {}
                            _ => break,
                        }
                    }
                    if target.is_some() {
                        break;
                    }
                }
            }
        } else if behavior == Behavior::Solid {
            let supported = [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)]
                .into_iter()
                .filter_map(|(nx, ny)| self.cells.get_world(nx, ny))
                .any(|neighbor| {
                    self.registry.get(neighbor.material).behavior == Behavior::Solid
                        && !neighbor.is_falling()
                });
            if !supported && let Some(current) = self.cells.get_world_mut(x, y) {
                current.set_falling(true);
            }
        }
        if let Some((tx, ty)) = target {
            self.move_world_cells((x, y), (tx, ty));
        }
    }

    fn move_world_cells(&mut self, source: (i32, i32), target: (i32, i32)) {
        if !self.world_cell_active(source.0, source.1)
            || !self.world_cell_active(target.0, target.1)
            || self.moved_at(source.0, source.1)
            || self.moved_at(target.0, target.1)
        {
            return;
        }
        let Some(source_cell) = self.cells.get_world(source.0, source.1).copied() else {
            return;
        };
        let Some(target_cell) = self.cells.get_world(target.0, target.1).copied() else {
            return;
        };
        if let Some(cell) = self.cells.get_world_mut(source.0, source.1) {
            *cell = target_cell;
        }
        if let Some(cell) = self.cells.get_world_mut(target.0, target.1) {
            *cell = source_cell;
        }
        self.mark_moved_at(source.0, source.1);
        self.mark_moved_at(target.0, target.1);
    }

    fn moved_at(&self, x: i32, y: i32) -> bool {
        if x >= 0 && x < self.width as i32 && y >= 0 && y < self.height as i32 {
            self.movement_stamps[y as usize * self.width as usize + x as usize] == self.tick_count
        } else {
            self.remote_movement_stamps.get(&(x, y)) == Some(&self.tick_count)
        }
    }

    fn mark_moved_at(&mut self, x: i32, y: i32) {
        if x >= 0 && x < self.width as i32 && y >= 0 && y < self.height as i32 {
            self.movement_stamps[y as usize * self.width as usize + x as usize] = self.tick_count;
        } else {
            self.remote_movement_stamps.insert((x, y), self.tick_count);
        }
    }

    fn check_remote_structural_collapse(&mut self) {
        let mut origins = self
            .active_remote_chunks
            .iter()
            .map(|coord| (coord.x * CHUNK_EDGE, coord.y * CHUNK_EDGE))
            .collect::<Vec<_>>();
        origins.sort_unstable();
        for (origin_x, origin_y) in origins {
            for local_y in (0..CHUNK_EDGE).step_by(4) {
                for local_x in (0..CHUNK_EDGE).step_by(4) {
                    let mut indices = [(0_i32, 0_i32); 16];
                    let mut count = 0;
                    for dy in 0..4 {
                        for dx in 0..4 {
                            let x = origin_x + local_x + dx;
                            let y = origin_y + local_y + dy;
                            let Some(cell) = self.cells.get_world(x, y) else {
                                continue;
                            };
                            if self.registry.get(cell.material).behavior == Behavior::Solid
                                && !cell.is_falling()
                            {
                                indices[count] = (x, y);
                                count += 1;
                            }
                        }
                    }
                    if (1..8).contains(&count) {
                        for (x, y) in indices.into_iter().take(count) {
                            if let Some(cell) = self.cells.get_world_mut(x, y) {
                                cell.set_falling(true);
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn remove_remote_chunk(&mut self, coordinate: ChunkCoord) -> bool {
        self.active_remote_chunks.remove(&coordinate);
        if !self.cells.remove_chunk(coordinate) {
            return false;
        }
        self.remote_movement_stamps
            .retain(|(x, y), _| ChunkCoord::from_cell(*x, *y) != coordinate);
        true
    }

    /// Apply a prevalidated streaming request. Prefetch creates resident data; Simulate also activates it.
    pub fn request_remote_chunks(
        &mut self,
        request: ChunkRequestPlan,
        resident_limit: usize,
        active_limit: usize,
    ) -> Result<usize, WorldError> {
        let actual_count = request
            .region
            .chunk_count()
            .map_err(|_| WorldError::InvalidWorldDimensions)?;
        if actual_count != request.chunks {
            return Err(WorldError::InvalidWorldDimensions);
        }
        let mut coordinates = Vec::new();
        coordinates
            .try_reserve_exact(actual_count)
            .map_err(|_| WorldError::AllocationFailed)?;
        for y in request.region.min.y..request.region.max.y {
            for x in request.region.min.x..request.region.max.x {
                coordinates.push(ChunkCoord { x, y });
            }
        }
        if request.mode == LoadMode::Simulate {
            let newly_active = coordinates
                .iter()
                .filter(|coord| {
                    !self.cells.is_initial_chunk(**coord)
                        && !self.active_remote_chunks.contains(coord)
                })
                .count();
            let initially_active = self.active_initial_chunks.as_ref().map_or_else(
                || {
                    (self.width as usize).div_ceil(CHUNK_EDGE as usize)
                        * (self.height as usize).div_ceil(CHUNK_EDGE as usize)
                },
                HashSet::len,
            );
            if initially_active
                .checked_add(self.active_remote_chunks.len())
                .and_then(|n| n.checked_add(newly_active))
                .is_none_or(|count| count > active_limit)
            {
                return Err(WorldError::ActiveChunkCapacityExceeded);
            }
            self.active_remote_chunks
                .try_reserve(newly_active)
                .map_err(|_| WorldError::AllocationFailed)?;
        }
        let added = self
            .cells
            .ensure_chunks(&coordinates, resident_limit, Cell::new(AIR))
            .map_err(map_chunk_error)?;
        if request.mode == LoadMode::Simulate {
            self.active_remote_chunks.extend(
                coordinates
                    .into_iter()
                    .filter(|coord| !self.cells.is_initial_chunk(*coord)),
            );
        }
        Ok(added)
    }

    pub fn active_remote_chunks(&self) -> impl Iterator<Item = ChunkCoord> + '_ {
        self.active_remote_chunks.iter().copied()
    }

    pub fn deactivate_remote_chunks(&mut self, coordinates: &[ChunkCoord]) {
        for coordinate in coordinates {
            self.active_remote_chunks.remove(coordinate);
        }
    }

    /// 現在の正規tickに次のsimulation stepを加算できることを確認します。
    pub fn ensure_tick_available(&self) -> Result<(), WorldError> {
        self.tick_count
            .checked_add(1)
            .map(|_| ())
            .ok_or(WorldError::TickOverflow)
    }

    /// Worldとregistryの診断用fingerprint。暗号学的hashや完全なsave hashではありません。
    pub fn state_fingerprint(&self) -> u64 {
        let mut hash = Fnv64::default();
        hash.write(b"charon-world-fingerprint-v4");
        hash.write_u32(self.width);
        hash.write_u32(self.height);
        hash.write_u64(self.tick_count);
        for energy in &self.thermal_energy {
            hash.write(&energy.to_le_bytes());
        }
        self.registry.write_fingerprint(&mut hash);
        for cell in &self.cells {
            hash.write_u16(cell.material);
            hash.write(&[cell.flags, cell.data]);
        }
        let mut remote = self.cells.remote_chunks().collect::<Vec<_>>();
        remote.sort_unstable_by_key(|(coord, _)| *coord);
        for (coord, cells) in remote {
            hash.write(&coord.x.to_le_bytes());
            hash.write(&coord.y.to_le_bytes());
            for cell in cells {
                hash.write_u16(cell.material);
                hash.write(&[cell.flags, cell.data]);
            }
        }
        hash.write(&[u8::from(self.active_initial_chunks.is_some())]);
        if let Some(active) = &self.active_initial_chunks {
            let mut active: Vec<_> = active.iter().copied().collect();
            active.sort_unstable();
            for coord in active {
                hash.write(&coord.x.to_le_bytes());
                hash.write(&coord.y.to_le_bytes());
            }
        }
        let mut active: Vec<_> = self.active_remote_chunks.iter().copied().collect();
        active.sort_unstable();
        for coord in active {
            hash.write(&coord.x.to_le_bytes());
            hash.write(&coord.y.to_le_bytes());
        }
        hash.write_u64(self.next_chunk_lease_id);
        let mut leases: Vec<_> = self.chunk_leases.values().copied().collect();
        leases.sort_unstable_by_key(|lease| lease.id);
        for lease in leases {
            hash.write_u64(lease.id);
            hash.write_u32(lease.source_id);
            hash.write(&[match lease.mode {
                LoadMode::Prefetch => 0,
                LoadMode::Simulate => 1,
            }]);
            for value in [
                lease.region.min.x,
                lease.region.min.y,
                lease.region.max.x,
                lease.region.max.y,
            ] {
                hash.write(&value.to_le_bytes());
            }
        }
        for particle in &self.particles {
            hash.write(&particle.x.to_bits().to_le_bytes());
            hash.write(&particle.y.to_bits().to_le_bytes());
            hash.write(&particle.vx.to_bits().to_le_bytes());
            hash.write(&particle.vy.to_bits().to_le_bytes());
            hash.write(&particle.life.to_bits().to_le_bytes());
            hash.write(&particle.max_life.to_bits().to_le_bytes());
            hash.write(&particle.color);
        }
        hash.finish()
    }

    /// Encode the current stopped-tick world state. Registry definitions remain external content.
    pub fn encode_snapshot(&self) -> Result<Vec<u8>, SnapshotError> {
        let mut out = Vec::new();
        let remote_count = self.cells.chunk_count().saturating_sub(
            (self.width as usize).div_ceil(CHUNK_EDGE as usize)
                * (self.height as usize).div_ceil(CHUNK_EDGE as usize),
        );
        let reserve = 256_usize
            .checked_add(
                self.cells
                    .len()
                    .checked_mul(4)
                    .ok_or(SnapshotError::CapacityExceeded)?,
            )
            .and_then(|size| size.checked_add(remote_count.checked_mul(CHUNK_AREA * 4 + 8)?))
            .and_then(|size| {
                size.checked_add(
                    self.active_initial_chunks
                        .as_ref()
                        .map_or(0, HashSet::len)
                        .checked_mul(8)?,
                )
            })
            .and_then(|size| size.checked_add(self.chunk_leases.len().checked_mul(29)?))
            .ok_or(SnapshotError::CapacityExceeded)?;
        out.try_reserve(reserve)
            .map_err(|_| SnapshotError::AllocationFailed)?;
        out.extend_from_slice(SNAPSHOT_MAGIC);
        push_u16(&mut out, SNAPSHOT_VERSION);
        push_u32(&mut out, SIMULATION_VERSION);
        push_u32(&mut out, GENERATOR_VERSION);
        push_u32(&mut out, self.width);
        push_u32(&mut out, self.height);
        push_u64(&mut out, self.tick_count);
        push_u64(&mut out, registry_fingerprint(&self.registry));
        push_u32(
            &mut out,
            u32::try_from(self.cells.len()).map_err(|_| SnapshotError::CapacityExceeded)?,
        );
        for cell in &self.cells {
            push_cell(&mut out, *cell);
        }
        for energy in &self.thermal_energy {
            push_i32(&mut out, *energy);
        }
        let mut remote = self.cells.remote_chunks().collect::<Vec<_>>();
        remote.sort_unstable_by_key(|(coord, _)| *coord);
        push_u16(
            &mut out,
            u16::try_from(remote.len()).map_err(|_| SnapshotError::CapacityExceeded)?,
        );
        for (coord, cells) in remote {
            push_i32(&mut out, coord.x);
            push_i32(&mut out, coord.y);
            for cell in cells {
                push_cell(&mut out, *cell);
            }
        }
        let mut active: Vec<_> = self.active_remote_chunks.iter().copied().collect();
        active.sort_unstable();
        push_u16(
            &mut out,
            u16::try_from(active.len()).map_err(|_| SnapshotError::CapacityExceeded)?,
        );
        for coord in active {
            push_i32(&mut out, coord.x);
            push_i32(&mut out, coord.y);
        }
        out.push(u8::from(self.active_initial_chunks.is_some()));
        let mut active_initial = self
            .active_initial_chunks
            .as_ref()
            .map(|set| set.iter().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        active_initial.sort_unstable();
        push_u16(
            &mut out,
            u16::try_from(active_initial.len()).map_err(|_| SnapshotError::CapacityExceeded)?,
        );
        for coord in active_initial {
            push_i32(&mut out, coord.x);
            push_i32(&mut out, coord.y);
        }
        push_u64(&mut out, self.next_chunk_lease_id);
        let mut leases: Vec<_> = self.chunk_leases.values().copied().collect();
        leases.sort_unstable_by_key(|lease| lease.id);
        push_u16(
            &mut out,
            u16::try_from(leases.len()).map_err(|_| SnapshotError::CapacityExceeded)?,
        );
        for lease in leases {
            push_u64(&mut out, lease.id);
            push_u32(&mut out, lease.source_id);
            out.push(match lease.mode {
                LoadMode::Prefetch => 0,
                LoadMode::Simulate => 1,
            });
            for value in [
                lease.region.min.x,
                lease.region.min.y,
                lease.region.max.x,
                lease.region.max.y,
            ] {
                push_i32(&mut out, value);
            }
        }
        push_u16(
            &mut out,
            u16::try_from(self.particles.len()).map_err(|_| SnapshotError::CapacityExceeded)?,
        );
        for particle in &self.particles {
            for value in [
                particle.x,
                particle.y,
                particle.vx,
                particle.vy,
                particle.life,
                particle.max_life,
            ] {
                push_u32(&mut out, value.to_bits());
            }
            out.extend_from_slice(&particle.color);
        }
        if out.len() > MAX_SNAPSHOT_BYTES {
            return Err(SnapshotError::CapacityExceeded);
        }
        Ok(out)
    }

    /// Validate a complete candidate before returning the restored world; the caller's world is untouched.
    pub fn decode_snapshot(
        bytes: &[u8],
        registry: MaterialRegistry,
    ) -> Result<Self, SnapshotError> {
        if bytes.len() > MAX_SNAPSHOT_BYTES {
            return Err(SnapshotError::CapacityExceeded);
        }
        let mut reader = SnapshotReader { bytes, offset: 0 };
        if reader.take(8)? != SNAPSHOT_MAGIC {
            return Err(SnapshotError::InvalidFormat);
        }
        if reader.u16()? != SNAPSHOT_VERSION {
            return Err(SnapshotError::UnsupportedVersion);
        }
        if reader.u32()? != SIMULATION_VERSION || reader.u32()? != GENERATOR_VERSION {
            return Err(SnapshotError::UnsupportedVersion);
        }
        let width = reader.u32()?;
        let height = reader.u32()?;
        if width == 0
            || height == 0
            || width > 65_536
            || height > 65_536
            || (width as usize).div_ceil(CHUNK_EDGE as usize)
                * (height as usize).div_ceil(CHUNK_EDGE as usize)
                > 512
        {
            return Err(SnapshotError::CapacityExceeded);
        }
        let tick = reader.u64()?;
        let content_hash = reader.u64()?;
        let mut registry = registry;
        registry
            .try_freeze()
            .map_err(|_| SnapshotError::ContentMismatch)?;
        if registry_fingerprint(&registry) != content_hash {
            return Err(SnapshotError::ContentMismatch);
        }
        let cell_count = reader.u32()? as usize;
        let expected = (width as usize)
            .checked_mul(height as usize)
            .ok_or(SnapshotError::CapacityExceeded)?;
        if cell_count != expected {
            return Err(SnapshotError::InvalidFormat);
        }
        let mut cells = Vec::new();
        cells
            .try_reserve_exact(cell_count)
            .map_err(|_| SnapshotError::AllocationFailed)?;
        for _ in 0..cell_count {
            cells.push(reader.cell(&registry)?);
        }
        let mut thermal_energy = Vec::new();
        thermal_energy
            .try_reserve_exact(cell_count)
            .map_err(|_| SnapshotError::AllocationFailed)?;
        for _ in 0..cell_count {
            thermal_energy.push(reader.i32()?);
        }
        let remote_count = reader.u16()? as usize;
        if remote_count
            + ((width as usize).div_ceil(CHUNK_EDGE as usize)
                * (height as usize).div_ceil(CHUNK_EDGE as usize))
            > 512
        {
            return Err(SnapshotError::CapacityExceeded);
        }
        let mut remote = Vec::new();
        remote
            .try_reserve_exact(remote_count)
            .map_err(|_| SnapshotError::AllocationFailed)?;
        let mut seen_remote = std::collections::HashSet::new();
        seen_remote
            .try_reserve(remote_count)
            .map_err(|_| SnapshotError::AllocationFailed)?;
        for _ in 0..remote_count {
            let coord = ChunkCoord {
                x: reader.i32()?,
                y: reader.i32()?,
            };
            if !seen_remote.insert(coord)
                || (i64::from(coord.x) * i64::from(CHUNK_EDGE) < i64::from(width)
                    && i64::from(coord.x) * i64::from(CHUNK_EDGE) + i64::from(CHUNK_EDGE) > 0
                    && i64::from(coord.y) * i64::from(CHUNK_EDGE) < i64::from(height)
                    && i64::from(coord.y) * i64::from(CHUNK_EDGE) + i64::from(CHUNK_EDGE) > 0)
            {
                return Err(SnapshotError::InvalidState);
            }
            let mut chunk = Vec::new();
            chunk
                .try_reserve_exact(CHUNK_AREA)
                .map_err(|_| SnapshotError::AllocationFailed)?;
            for _ in 0..CHUNK_AREA {
                chunk.push(reader.cell(&registry)?);
            }
            remote.push((coord, chunk));
        }
        let active_count = reader.u16()? as usize;
        if active_count > remote_count {
            return Err(SnapshotError::InvalidState);
        }
        let mut active = HashSet::new();
        active
            .try_reserve(active_count)
            .map_err(|_| SnapshotError::AllocationFailed)?;
        for _ in 0..active_count {
            let coord = ChunkCoord {
                x: reader.i32()?,
                y: reader.i32()?,
            };
            if !seen_remote.contains(&coord) || !active.insert(coord) {
                return Err(SnapshotError::InvalidState);
            }
        }
        let has_initial_active_set = match reader.take(1)?[0] {
            0 => false,
            1 => true,
            _ => return Err(SnapshotError::InvalidState),
        };
        let initial_active_count = reader.u16()? as usize;
        if !has_initial_active_set && initial_active_count != 0 {
            return Err(SnapshotError::InvalidState);
        }
        let initial_chunk_count = (width as usize).div_ceil(CHUNK_EDGE as usize)
            * (height as usize).div_ceil(CHUNK_EDGE as usize);
        if initial_active_count > initial_chunk_count {
            return Err(SnapshotError::InvalidState);
        }
        let mut active_initial = HashSet::new();
        active_initial
            .try_reserve(initial_active_count)
            .map_err(|_| SnapshotError::AllocationFailed)?;
        for _ in 0..initial_active_count {
            let coord = ChunkCoord {
                x: reader.i32()?,
                y: reader.i32()?,
            };
            if coord.x < 0
                || coord.y < 0
                || coord.x >= ((width as i32 + CHUNK_EDGE - 1) / CHUNK_EDGE)
                || coord.y >= ((height as i32 + CHUNK_EDGE - 1) / CHUNK_EDGE)
                || !active_initial.insert(coord)
            {
                return Err(SnapshotError::InvalidState);
            }
        }
        let next_lease_id = reader.u64()?;
        if next_lease_id == 0 {
            return Err(SnapshotError::InvalidState);
        }
        let lease_count = reader.u16()? as usize;
        if lease_count > crate::simulation::streaming::HARD_MAX_PENDING_REQUESTS {
            return Err(SnapshotError::CapacityExceeded);
        }
        let mut leases = HashMap::new();
        leases
            .try_reserve(lease_count)
            .map_err(|_| SnapshotError::AllocationFailed)?;
        let mut pinned_chunks = HashSet::new();
        pinned_chunks
            .try_reserve(crate::simulation::streaming::HARD_MAX_PINNED_CHUNKS)
            .map_err(|_| SnapshotError::AllocationFailed)?;
        let mut largest_lease_id = 0;
        for _ in 0..lease_count {
            let id = reader.u64()?;
            let source_id = reader.u32()?;
            let mode = match reader.take(1)?[0] {
                0 => LoadMode::Prefetch,
                1 => LoadMode::Simulate,
                _ => return Err(SnapshotError::InvalidState),
            };
            let region = ChunkRect::new(
                ChunkCoord {
                    x: reader.i32()?,
                    y: reader.i32()?,
                },
                ChunkCoord {
                    x: reader.i32()?,
                    y: reader.i32()?,
                },
            )
            .map_err(|_| SnapshotError::InvalidState)?;
            let region_chunks = region
                .chunk_count()
                .map_err(|_| SnapshotError::InvalidState)?;
            if region_chunks > crate::simulation::streaming::HARD_MAX_PINNED_CHUNKS {
                return Err(SnapshotError::CapacityExceeded);
            }
            pinned_chunks
                .try_reserve(region_chunks)
                .map_err(|_| SnapshotError::AllocationFailed)?;
            if id == 0 || id >= next_lease_id || leases.contains_key(&id) {
                return Err(SnapshotError::InvalidState);
            }
            largest_lease_id = largest_lease_id.max(id);
            for y in region.min.y..region.max.y {
                for x in region.min.x..region.max.x {
                    let coord = ChunkCoord { x, y };
                    pinned_chunks.insert(coord);
                    if pinned_chunks.len() > crate::simulation::streaming::HARD_MAX_PINNED_CHUNKS {
                        return Err(SnapshotError::CapacityExceeded);
                    }
                    let initial = coord.x >= 0
                        && coord.y >= 0
                        && coord.x < ((width as i32 + CHUNK_EDGE - 1) / CHUNK_EDGE)
                        && coord.y < ((height as i32 + CHUNK_EDGE - 1) / CHUNK_EDGE);
                    if (!initial && !seen_remote.contains(&coord))
                        || (mode == LoadMode::Simulate
                            && if initial {
                                has_initial_active_set && !active_initial.contains(&coord)
                            } else {
                                !active.contains(&coord)
                            })
                    {
                        return Err(SnapshotError::InvalidState);
                    }
                }
            }
            leases.insert(
                id,
                ChunkLease {
                    id,
                    source_id,
                    region,
                    mode,
                },
            );
        }
        if pinned_chunks.len() > crate::simulation::streaming::HARD_MAX_PINNED_CHUNKS
            || (lease_count > 0 && largest_lease_id >= next_lease_id)
        {
            return Err(SnapshotError::CapacityExceeded);
        }
        let particle_count = reader.u16()? as usize;
        if particle_count > 500 {
            return Err(SnapshotError::CapacityExceeded);
        }
        let mut particles = Vec::new();
        particles
            .try_reserve_exact(particle_count)
            .map_err(|_| SnapshotError::AllocationFailed)?;
        for _ in 0..particle_count {
            let mut values = [0.0_f32; 6];
            for value in &mut values {
                *value = f32::from_bits(reader.u32()?);
                if !value.is_finite() {
                    return Err(SnapshotError::InvalidState);
                }
            }
            let color: [u8; 4] = reader
                .take(4)?
                .try_into()
                .map_err(|_| SnapshotError::InvalidFormat)?;
            particles.push(SplashParticle {
                x: values[0],
                y: values[1],
                vx: values[2],
                vy: values[3],
                life: values[4],
                max_life: values[5],
                color,
            });
        }
        if reader.offset != bytes.len() {
            return Err(SnapshotError::InvalidFormat);
        }
        let mut world = Self::try_with_registry(width, height, registry)
            .map_err(|_| SnapshotError::CapacityExceeded)?;
        world.tick_count = tick;
        world.thermal_energy = thermal_energy;
        for (index, cell) in cells.into_iter().enumerate() {
            world.cells[index] = cell;
        }
        for (coord, chunk) in remote {
            world
                .cells
                .ensure_chunks(&[coord], 512, Cell::new(AIR))
                .map_err(|_| SnapshotError::CapacityExceeded)?;
            let origin_x = coord
                .x
                .checked_mul(CHUNK_EDGE)
                .ok_or(SnapshotError::InvalidState)?;
            let origin_y = coord
                .y
                .checked_mul(CHUNK_EDGE)
                .ok_or(SnapshotError::InvalidState)?;
            for (index, cell) in chunk.into_iter().enumerate() {
                let x = origin_x + (index % CHUNK_EDGE as usize) as i32;
                let y = origin_y + (index / CHUNK_EDGE as usize) as i32;
                world.cells.set_remote_cell(x, y, cell);
            }
        }
        world.active_remote_chunks = active;
        world.active_initial_chunks = has_initial_active_set.then_some(active_initial);
        world.chunk_leases = leases;
        world.next_chunk_lease_id = next_lease_id;
        world.particles = particles;
        Ok(world)
    }

    /// 【構造崩落システム（体積不足・孤立ピクセルの粒子化）】
    ///
    /// 1. 4x4 半ブロック領域内で固体セル数が 1〜7 個（体積の半分未満）しかない場合、
    ///    構造強度を維持できずに自由落下状態（FLAG_FALLING）へと移行させます。
    /// 2. 上下左右に他の固体が一切接続していない完全な孤立ピクセルも、重力で落下させます。
    ///    マテリアル ID を書き換えることなく、フラグのみで状態を切り替えます。
    ///
    /// 新規マテリアル追加時も分岐コードの修正が一切不要です。
    fn check_structural_collapse(&mut self) {
        let width = self.width;
        let height = self.height;

        // パフォーマンスのため、tick ごとに異なるオフセットで 4x4 グリッドをインターリーブ走査
        let offset = (self.tick_count % 4) as u32;
        let grid_w = width / 4;
        let grid_h = height / 4;

        for gy in 0..grid_h {
            for gx in 0..grid_w {
                // 走査の分散
                if (gx + gy) % 4 != offset {
                    continue;
                }

                if !self.initial_chunk_active((gx * 4) as i32, (gy * 4) as i32) {
                    continue;
                }

                let bx = gx * 4;
                let by = gy * 4;

                let mut solid_indices = Vec::with_capacity(16);

                for py in 0..4 {
                    for px in 0..4 {
                        let x = bx + px;
                        let y = by + py;
                        let idx = (y * width + x) as usize;
                        let cell = self.cells[idx];

                        // 静止している固体セルを収集
                        let behavior = self.registry.get(cell.material).behavior;
                        if self.registry.is_registered(cell.material)
                            && behavior == Behavior::Solid
                            && !cell.is_falling()
                        {
                            solid_indices.push(idx);
                        }
                    }
                }

                let total_solid = solid_indices.len();

                // 4x4 半ブロック（16セル）中、固体が 1〜7 個しかない場合（体積が半分未満に破壊された）
                // 構造を保てなくなり、自由落下状態（FLAG_FALLING）に変化して重力で崩落する
                if total_solid > 0 && total_solid < 8 {
                    for idx in solid_indices {
                        self.cells[idx].set_falling(true);
                    }
                }
            }
        }
    }

    /// 単一セルのシミュレーション規則を評価します。
    fn update_cell(&mut self, x: u32, y: u32) {
        let idx = (y * self.width + x) as usize;
        let cell = self.cells[idx];

        if cell.material == AIR
            || !self.registry.is_registered(cell.material)
            || self.movement_stamps[idx] == self.tick_count
        {
            return;
        }

        let is_falling = cell.is_falling();
        let behavior = self.registry.get(cell.material).behavior;

        // 自由落下中のセル、または粉体（砂）は重力崩落処理へ
        if is_falling || behavior == Behavior::Powder {
            self.update_powder_or_falling(x, y);
        } else {
            match behavior {
                Behavior::Liquid => {
                    self.update_liquid(x, y);
                }
                Behavior::Solid => {
                    // 上下左右に接続支持がない完全な浮遊孤立固体セルは落下フラグを付与
                    self.check_isolated_solid(x, y);
                }
                _ => {}
            }
        }
    }

    /// 上下左右に一切固体が接続していない孤立ピクセルを検知し、自由落下させます。
    fn check_isolated_solid(&mut self, x: u32, y: u32) {
        if y + 1 >= self.height {
            return; // 最底辺は安定
        }

        let idx = (y * self.width + x) as usize;

        let has_support = (y > 0 && self.is_solid_neighbor(x, y - 1))
            || self.is_solid_neighbor(x, y + 1)
            || (x > 0 && self.is_solid_neighbor(x - 1, y))
            || (x + 1 < self.width && self.is_solid_neighbor(x + 1, y));

        if !has_support {
            self.cells[idx].set_falling(true);
        }
    }

    /// 指定座標が静止した構造支持固体（壁・床）であるかを判定します。
    fn is_solid_neighbor(&self, x: u32, y: u32) -> bool {
        let cell = self.cells[(y * self.width + x) as usize];
        let behavior = self.registry.get(cell.material).behavior;
        // 静止している固体のみが接続支持として機能する（落下中のセルは支えにならない）
        behavior == Behavior::Solid && !cell.is_falling()
    }

    /// 液体（水など）の流動シミュレーション
    ///
    /// 【水流侵食・粒子押し流し機能】
    /// 水が横へ流れる際、進行方向に砂（SAND）や崩落土が存在する場合、
    /// 水流の運動量によってその粒子を横へ押し退けて一緒に流します。
    fn update_liquid(&mut self, x: u32, y: u32) {
        let curr_idx = (y * self.width + x) as usize;
        let curr_cell = self.cells[curr_idx];

        let mat = self.registry.get(curr_cell.material);
        let max_fall = mat.max_fall_speed as u32;

        // 1. 直下への多段高速落下（最大 5 セル）
        let mut fall_target_y = None;
        for step in 1..=max_fall {
            let ny = y + step;
            if ny < self.height {
                let down_idx = (ny * self.width + x) as usize;
                if self.cells[down_idx].material == AIR {
                    fall_target_y = Some(ny);
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        if let Some(target_y) = fall_target_y {
            let target_idx = (target_y * self.width + x) as usize;
            self.move_cells(curr_idx, target_idx);
            return;
        }

        // 2. 斜め下（左下 / 右下）への流動
        let rand_side = (x + y + (self.tick_count as u32)).is_multiple_of(2);
        let offsets = if rand_side { [-1, 1] } else { [1, -1] };

        if y + 1 < self.height {
            for &dx in &offsets {
                let nx = x as i32 + dx;
                if nx >= 0 && nx < self.width as i32 {
                    let target_idx = ((y + 1) * self.width + nx as u32) as usize;
                    if self.cells[target_idx].material == AIR {
                        self.move_cells(curr_idx, target_idx);
                        return;
                    }
                }
            }
        }

        // 3. 左右への水平拡散 ＆ 水流による小粒子（砂・崩落土）の押し流し
        let dispersion = mat.dispersion as i32;
        let flow_speed = mat.flow_speed as i32;

        for &dir in &offsets {
            let mut best_target_x = None;
            for step in 1..=dispersion {
                let nx = x as i32 + (dir * step);
                if nx >= 0 && nx < self.width as i32 {
                    let target_idx = (y * self.width + nx as u32) as usize;
                    let target_cell = self.cells[target_idx];
                    let target_mat = target_cell.material;
                    let target_def = self.registry.get(target_mat);

                    // 水流押し流し判定:
                    // 1. 粉体で流されやすい物質（砂）
                    // 2. または、構造破壊されて落下状態（is_falling）かつ流されやすい物質（土など）
                    let is_washable = target_def.can_be_washed_away
                        && (target_def.behavior == Behavior::Powder || target_cell.is_falling());

                    if target_mat == AIR {
                        best_target_x = Some(nx);
                        if step >= flow_speed {
                            break;
                        }
                    } else if is_washable {
                        // 【水流押し流し】進行方向に砂や崩落土がある場合、そのさらに先が空きなら押し出す
                        let push_to_x = nx + dir;
                        if push_to_x >= 0 && push_to_x < self.width as i32 {
                            let push_idx = (y * self.width + push_to_x as u32) as usize;
                            if self.cells[push_idx].material == AIR
                                && self.initial_chunk_active(push_to_x, y as i32)
                            {
                                // 粒子を 1 歩押し出し、水がその位置へ滑り込む
                                let pushed_cell = self.cells[target_idx];
                                self.cells[push_idx] = pushed_cell;
                                self.thermal_energy.swap(target_idx, push_idx);
                                self.cells[target_idx] = curr_cell;
                                self.thermal_energy.swap(curr_idx, target_idx);
                                self.cells[curr_idx] = Cell::new(AIR);
                                self.stamp_movement(curr_idx);
                                self.stamp_movement(target_idx);
                                self.stamp_movement(push_idx);
                                return;
                            }
                        }
                        break;
                    } else if target_def.behavior != Behavior::Liquid {
                        break;
                    }
                } else {
                    break;
                }
            }

            if let Some(target_x) = best_target_x {
                let target_idx = (y * self.width + target_x as u32) as usize;
                self.move_cells(curr_idx, target_idx);
                return;
            }
        }
    }

    /// 粉体（砂）および構造崩落セル（落下中の土・石）の物理落下シミュレーション
    fn update_powder_or_falling(&mut self, x: u32, y: u32) {
        let curr_idx = (y * self.width + x) as usize;
        let mut curr_cell = self.cells[curr_idx];

        let my_mass = self.registry.get(curr_cell.material).mass_per_cell;

        // 1. 直下への落下（水より重ければ水を押しのけて沈降）
        if y + 1 < self.height {
            let down_idx = ((y + 1) * self.width + x) as usize;
            let below_mat = self.cells[down_idx].material;
            if below_mat == AIR || self.registry.get(below_mat).mass_per_cell < my_mass {
                self.move_cells(curr_idx, down_idx);
                return;
            }
        }

        // 2. 斜め下への崩落・滑落
        let rand_side = (x ^ y ^ (self.tick_count as u32)).is_multiple_of(2);
        let offsets = if rand_side { [-1, 1] } else { [1, -1] };

        if y + 1 < self.height {
            for &dx in &offsets {
                let nx = x as i32 + dx;
                if nx >= 0 && nx < self.width as i32 {
                    let target_idx = ((y + 1) * self.width + nx as u32) as usize;
                    let target_mat = self.cells[target_idx].material;
                    if target_mat == AIR || self.registry.get(target_mat).mass_per_cell < my_mass {
                        self.move_cells(curr_idx, target_idx);
                        return;
                    }
                }
            }
        }

        // 3. 着地時の再結合安定化
        // 落下中だった固体（土や石）が着地し、直下に安定支持がある場合、
        // 落下フラグを解除（set_falling(false)）して再び静止固体ブロックへと戻る
        if curr_cell.is_falling() {
            let mat_def = self.registry.get(curr_cell.material);
            if mat_def.behavior == Behavior::Solid
                && (y + 1 >= self.height || self.is_solid_neighbor(x, y + 1))
            {
                curr_cell.set_falling(false);
            }
        }

        // 着地後の状態を反映
        self.cells[curr_idx] = curr_cell;
    }

    /// 2セルの移動をまとめて確定し、両方を現在tickの処理済みとして記録します。
    fn move_cells(&mut self, first: usize, second: usize) {
        let first_x = (first % self.width as usize) as i32;
        let first_y = (first / self.width as usize) as i32;
        let second_x = (second % self.width as usize) as i32;
        let second_y = (second / self.width as usize) as i32;
        if !self.initial_chunk_active(first_x, first_y)
            || !self.initial_chunk_active(second_x, second_y)
        {
            return;
        }
        self.cells.swap(first, second);
        self.thermal_energy.swap(first, second);
        self.stamp_movement(first);
        self.stamp_movement(second);
    }

    fn stamp_movement(&mut self, index: usize) {
        self.movement_stamps[index] = self.tick_count;
    }

    /// プレイヤーなどの移動により水セルを押し退け、波立ち・攪拌を起こします。
    pub fn displace_water(&mut self, center_x: i32, center_y: i32, horizontal_direction: i8) {
        let Some(next_tick) = self.tick_count.checked_add(1) else {
            return;
        };
        let dir = if horizontal_direction > 0 { 1 } else { -1 };
        let radius = 3;

        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let sx = center_x + dx;
                let sy = center_y + dy;

                if sx >= 0 && sx < self.width as i32 && sy >= 0 && sy < self.height as i32 {
                    let src_idx = (sy as u32 * self.width + sx as u32) as usize;
                    if self.cells[src_idx].material == WATER
                        && self.initial_chunk_active(sx, sy)
                        && self.movement_stamps[src_idx] != next_tick
                    {
                        let target_x = sx + dir;
                        let target_y = sy - 1;

                        if target_x >= 0 && target_x < self.width as i32 && target_y >= 0 {
                            let dst_idx = (target_y as u32 * self.width + target_x as u32) as usize;
                            if self.cells[dst_idx].material == AIR
                                && self.initial_chunk_active(target_x, target_y)
                                && self.movement_stamps[dst_idx] != next_tick
                            {
                                self.cells.swap(src_idx, dst_idx);
                                self.thermal_energy.swap(src_idx, dst_idx);
                                self.movement_stamps[src_idx] = next_tick;
                                self.movement_stamps[dst_idx] = next_tick;
                            }
                        }
                    }
                }
            }
        }
    }

    /// 自由粒子（水しぶきなど）を追加します。
    pub fn add_particle(&mut self, particle: SplashParticle) {
        if self.particles.len() < 500 {
            self.particles.push(particle);
        }
    }

    /// 指定されたピクセル座標のセルを取得します。
    #[inline]
    pub fn get_cell(&self, x: i32, y: i32) -> Cell {
        if x < 0 || x >= self.width as i32 || y < 0 || y >= self.height as i32 {
            Cell::new(STONE)
        } else {
            self.cells[(y as u32 * self.width + x as u32) as usize]
        }
    }

    pub fn query_cell(&self, x: i32, y: i32) -> CellQuery {
        if !(crate::simulation::chunk::WORLD_COORD_MIN..=crate::simulation::chunk::WORLD_COORD_MAX)
            .contains(&x)
            || !(crate::simulation::chunk::WORLD_COORD_MIN
                ..=crate::simulation::chunk::WORLD_COORD_MAX)
                .contains(&y)
        {
            return CellQuery::WorldBoundary;
        }
        self.cells
            .get_world(x, y)
            .copied()
            .map_or(CellQuery::Unavailable, CellQuery::Available)
    }

    /// Remove a cell only if it still contains the inspected material (finite region only).
    pub fn remove_cell_if_material(&mut self, x: i32, y: i32, expected_material: u16) -> bool {
        if x < 0 || x >= self.width as i32 || y < 0 || y >= self.height as i32 {
            return false;
        }
        let index = y as usize * self.width as usize + x as usize;
        if self.cells[index].material != expected_material || expected_material == AIR {
            return false;
        }
        self.cells[index] = Cell::new(AIR);
        self.thermal_energy[index] = 0;
        true
    }

    /// 検証に成功した編集コマンドだけを適用します。
    pub fn apply_command(&mut self, command: WorldCommand) -> Result<CommandResult, CommandError> {
        match command {
            WorldCommand::PaintBrush {
                center_x,
                center_y,
                radius,
                material,
            } => {
                if !(0..=MAX_BRUSH_RADIUS).contains(&radius) {
                    return Err(CommandError::InvalidRadius);
                }
                if !self.registry.is_registered(material) {
                    return Err(CommandError::UnknownMaterial);
                }
                let side = (radius as u32) * 2 + 1;
                let candidates = side * side;
                if candidates > MAX_COMMAND_CANDIDATE_CELLS {
                    return Err(CommandError::WriteLimitExceeded);
                }

                let radius_sq = (radius as i64) * (radius as i64);
                let mut cells_written = 0;
                for dy in -radius..=radius {
                    for dx in -radius..=radius {
                        let dx64 = i64::from(dx);
                        let dy64 = i64::from(dy);
                        if dx64 * dx64 + dy64 * dy64 > radius_sq {
                            continue;
                        }
                        let x = i64::from(center_x) + dx64;
                        let y = i64::from(center_y) + dy64;
                        if x < 0
                            || x >= i64::from(self.width)
                            || y < 0
                            || y >= i64::from(self.height)
                        {
                            continue;
                        }
                        let x = x as u32;
                        let y = y as u32;
                        let idx = (y * self.width + x) as usize;
                        let shade = (((y % 8) * 8) + (x % 8)) as u8;
                        self.cells[idx] = Cell::with_shade(material, shade);
                        self.thermal_energy[idx] = 0;
                        cells_written += 1;
                    }
                }
                Ok(CommandResult { cells_written })
            }
        }
    }
}

fn map_chunk_error(error: ChunkStoreError) -> WorldError {
    match error {
        ChunkStoreError::InvalidDimensions | ChunkStoreError::CoordinateOverflow => {
            WorldError::InvalidWorldDimensions
        }
        ChunkStoreError::CapacityExceeded => WorldError::WorldCapacityExceeded,
        ChunkStoreError::AllocationFailed => WorldError::AllocationFailed,
    }
}

fn registry_fingerprint(registry: &MaterialRegistry) -> u64 {
    let mut hash = Fnv64::default();
    hash.write(b"charon-content-fingerprint-v1");
    registry.write_fingerprint(&mut hash);
    hash.finish()
}

fn push_cell(out: &mut Vec<u8>, cell: Cell) {
    push_u16(out, cell.material);
    out.extend_from_slice(&[cell.flags, cell.data]);
}
fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}
fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}
fn push_i32(out: &mut Vec<u8>, value: i32) {
    out.extend_from_slice(&value.to_le_bytes());
}
fn push_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

struct SnapshotReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl SnapshotReader<'_> {
    fn take(&mut self, count: usize) -> Result<&[u8], SnapshotError> {
        let end = self
            .offset
            .checked_add(count)
            .ok_or(SnapshotError::InvalidFormat)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(SnapshotError::InvalidFormat)?;
        self.offset = end;
        Ok(value)
    }
    fn u16(&mut self) -> Result<u16, SnapshotError> {
        Ok(u16::from_le_bytes(
            self.take(2)?
                .try_into()
                .map_err(|_| SnapshotError::InvalidFormat)?,
        ))
    }
    fn u32(&mut self) -> Result<u32, SnapshotError> {
        Ok(u32::from_le_bytes(
            self.take(4)?
                .try_into()
                .map_err(|_| SnapshotError::InvalidFormat)?,
        ))
    }
    fn i32(&mut self) -> Result<i32, SnapshotError> {
        Ok(i32::from_le_bytes(
            self.take(4)?
                .try_into()
                .map_err(|_| SnapshotError::InvalidFormat)?,
        ))
    }
    fn u64(&mut self) -> Result<u64, SnapshotError> {
        Ok(u64::from_le_bytes(
            self.take(8)?
                .try_into()
                .map_err(|_| SnapshotError::InvalidFormat)?,
        ))
    }
    fn cell(&mut self, registry: &MaterialRegistry) -> Result<Cell, SnapshotError> {
        let material = self.u16()?;
        let flags = self.take(1)?[0];
        let data = self.take(1)?[0];
        if !registry.is_registered(material) || flags & !1 != 0 {
            return Err(SnapshotError::InvalidState);
        }
        Ok(Cell {
            material,
            flags,
            data,
        })
    }
}

#[cfg(test)]
mod command_tests {
    use super::*;
    use crate::simulation::command::{CommandError, WorldCommand};

    #[test]
    fn brush_command_clips_to_world_and_reports_commit_count() {
        let mut world = World::new(8, 8).unwrap();
        let result = world
            .apply_command(WorldCommand::PaintBrush {
                center_x: 0,
                center_y: 0,
                radius: 2,
                material: WATER,
            })
            .unwrap();

        assert_eq!(result.cells_written, 6);
        assert_eq!(world.get_cell(2, 0).material, WATER);
        assert_eq!(world.get_cell(0, 2).material, WATER);
        assert_eq!(world.get_cell(2, 2).material, AIR);
    }

    #[test]
    fn invalid_command_is_rejected_without_mutation() {
        let mut world = World::new(8, 8).unwrap();
        let before = world.cells.clone();
        let result = world.apply_command(WorldCommand::PaintBrush {
            center_x: 4,
            center_y: 4,
            radius: 2,
            material: u16::MAX - 1,
        });

        assert_eq!(result, Err(CommandError::UnknownMaterial));
        assert_eq!(world.cells, before);
    }

    #[test]
    fn moved_cell_is_stamped_and_cannot_move_twice_in_one_tick() {
        let mut world = World::new(8, 8).unwrap();
        world
            .apply_command(WorldCommand::PaintBrush {
                center_x: 4,
                center_y: 1,
                radius: 0,
                material: WATER,
            })
            .unwrap();

        world.update().unwrap();

        assert_eq!(world.tick_count, 1);
        assert_eq!(world.get_cell(4, 6).material, WATER);
        assert_eq!(
            world
                .cells
                .iter()
                .filter(|cell| cell.material == WATER)
                .count(),
            1
        );
    }

    #[test]
    fn tick_overflow_returns_error_without_mutating_world() {
        let mut world = World::new(8, 8).unwrap();
        world.tick_count = u64::MAX;
        let before = world.cells.clone();

        assert_eq!(world.update(), Err(WorldError::TickOverflow));
        assert_eq!(world.tick_count, u64::MAX);
        assert_eq!(world.cells, before);
    }

    #[test]
    fn identical_worlds_produce_identical_fingerprints() {
        let mut first = World::new(16, 16).unwrap();
        let mut second = World::new(16, 16).unwrap();
        let command = WorldCommand::PaintBrush {
            center_x: 5,
            center_y: 5,
            radius: 2,
            material: SAND,
        };
        first.apply_command(command).unwrap();
        second.apply_command(command).unwrap();
        first.update().unwrap();
        second.update().unwrap();

        assert_eq!(first.state_fingerprint(), second.state_fingerprint());
    }

    #[test]
    fn fallible_world_constructor_reports_capacity_errors() {
        assert!(matches!(
            World::try_new(65_536, 65_536),
            Err(WorldError::WorldCapacityExceeded)
        ));
        assert!(matches!(
            World::try_new(0, 10),
            Err(WorldError::InvalidWorldDimensions)
        ));
    }

    #[test]
    fn snapshot_roundtrip_preserves_state_and_rejects_trailing_bytes() {
        let mut world = World::new(16, 16).unwrap();
        world
            .apply_command(WorldCommand::PaintBrush {
                center_x: 5,
                center_y: 5,
                radius: 1,
                material: WATER,
            })
            .unwrap();
        world.update().unwrap();
        let bytes = world.encode_snapshot().unwrap();
        let restored = World::decode_snapshot(&bytes, MaterialRegistry::with_defaults()).unwrap();
        assert_eq!(restored.state_fingerprint(), world.state_fingerprint());
        let mut invalid = bytes;
        invalid.push(0);
        assert!(matches!(
            World::decode_snapshot(&invalid, MaterialRegistry::with_defaults()),
            Err(SnapshotError::InvalidFormat)
        ));
    }

    #[test]
    fn thermal_energy_diffuses_conservatively_and_roundtrips_in_snapshot() {
        let mut world = World::new(4, 4).unwrap();
        assert!(world.set_thermal_energy(1, 1, 800));
        let mut before = 0_i64;
        for y in 0..4 {
            for x in 0..4 {
                before += i64::from(world.thermal_energy(x, y).unwrap());
            }
        }
        world.update().unwrap();
        let mut after = 0_i64;
        for y in 0..4 {
            for x in 0..4 {
                after += i64::from(world.thermal_energy(x, y).unwrap());
            }
        }
        assert_eq!(before, after);
        assert!(world.thermal_energy(2, 1).unwrap() > 0);
        let restored = World::decode_snapshot(
            &world.encode_snapshot().unwrap(),
            MaterialRegistry::with_defaults(),
        )
        .unwrap();
        assert_eq!(restored.state_fingerprint(), world.state_fingerprint());
        assert_eq!(restored.thermal_energy(2, 1), world.thermal_energy(2, 1));
    }

    #[test]
    fn snapshot_roundtrip_preserves_remote_chunk_cells() {
        let mut world = World::new(64, 64).unwrap();
        let remote = ChunkCoord { x: -2, y: 2 };
        world
            .cells
            .ensure_chunks(&[remote], 512, Cell::new(AIR))
            .unwrap();
        world
            .cells
            .set_remote_cell(-65, 130, Cell::with_shade(WATER, 7));
        let restored = World::decode_snapshot(
            &world.encode_snapshot().unwrap(),
            MaterialRegistry::with_defaults(),
        )
        .unwrap();
        assert_eq!(restored.cells.get_world(-65, 130).unwrap().material, WATER);
        assert_eq!(restored.state_fingerprint(), world.state_fingerprint());
    }

    #[test]
    fn resident_remote_chunks_advance_in_global_world_scan_order() {
        let mut world = World::new(8, 8).unwrap();
        let region = crate::simulation::streaming::ChunkRect::new(
            ChunkCoord { x: 2, y: 2 },
            ChunkCoord { x: 3, y: 3 },
        )
        .unwrap();
        let policy = crate::simulation::streaming::StreamingPolicy::default();
        let prefetch = policy.plan_request(region, LoadMode::Prefetch).unwrap();
        world.request_remote_chunks(prefetch, 512, 256).unwrap();
        world.cells.set_remote_cell(130, 130, Cell::new(SAND));
        assert_eq!(
            world.query_cell(130, 130),
            CellQuery::Available(Cell::new(SAND))
        );
        assert_eq!(world.query_cell(64, 130), CellQuery::Unavailable);
        assert_eq!(
            world.query_cell(crate::simulation::chunk::WORLD_COORD_MAX + 1, 0),
            CellQuery::WorldBoundary
        );
        world.update().unwrap();
        assert_eq!(world.cells.get_world(130, 130).unwrap().material, SAND);
        let request = policy.plan_request(region, LoadMode::Simulate).unwrap();
        assert_eq!(
            world.request_remote_chunks(request, 512, 1),
            Err(WorldError::ActiveChunkCapacityExceeded)
        );
        world.request_remote_chunks(request, 512, 2).unwrap();
        world.update().unwrap();
        assert_eq!(world.cells.get_world(130, 130).unwrap().material, AIR);
        assert_eq!(world.cells.get_world(130, 131).unwrap().material, SAND);

        let restored = World::decode_snapshot(
            &world.encode_snapshot().unwrap(),
            MaterialRegistry::with_defaults(),
        )
        .unwrap();
        world.update().unwrap();
        let mut restored = restored;
        restored.update().unwrap();
        assert_eq!(restored.state_fingerprint(), world.state_fingerprint());
    }

    #[test]
    fn simulate_request_overlapping_initial_region_does_not_double_activate_it() {
        let mut world = World::new(8, 8).unwrap();
        let region = crate::simulation::streaming::ChunkRect::new(
            ChunkCoord { x: -1, y: 0 },
            ChunkCoord { x: 1, y: 1 },
        )
        .unwrap();
        let request = crate::simulation::streaming::StreamingPolicy::default()
            .plan_request(region, LoadMode::Simulate)
            .unwrap();
        world.request_remote_chunks(request, 512, 2).unwrap();
        assert_eq!(world.active_remote_chunks().count(), 1);
        assert_eq!(world.cells.chunk_count(), 2);
    }

    #[test]
    fn visible_plan_freezes_resident_chunks_and_reactivates_them_deterministically() {
        let mut world = World::new(128, 64).unwrap();
        world
            .apply_command(WorldCommand::PaintBrush {
                center_x: 70,
                center_y: 10,
                radius: 0,
                material: SAND,
            })
            .unwrap();
        let policy = crate::simulation::streaming::StreamingPolicy {
            offscreen_enabled: false,
            ..Default::default()
        };
        let left = crate::simulation::streaming::ChunkRect::new(
            ChunkCoord { x: 0, y: 0 },
            ChunkCoord { x: 1, y: 1 },
        )
        .unwrap();
        world
            .apply_visible_chunk_plan(policy.plan_visible(left).unwrap(), 8, 8)
            .unwrap();
        let hash = world.state_fingerprint();
        world.update().unwrap();
        assert_eq!(world.get_cell(70, 10).material, SAND);
        assert_ne!(world.state_fingerprint(), hash); // tick advances, frozen cell does not.

        let both = crate::simulation::streaming::ChunkRect::new(
            ChunkCoord { x: 0, y: 0 },
            ChunkCoord { x: 2, y: 1 },
        )
        .unwrap();
        world
            .apply_visible_chunk_plan(policy.plan_visible(both).unwrap(), 8, 8)
            .unwrap();
        world.update().unwrap();
        assert_eq!(world.get_cell(70, 11).material, SAND);

        let restored = World::decode_snapshot(
            &world.encode_snapshot().unwrap(),
            MaterialRegistry::with_defaults(),
        )
        .unwrap();
        assert_eq!(restored.state_fingerprint(), world.state_fingerprint());
        world.update().unwrap();
        let mut restored = restored;
        restored.update().unwrap();
        assert_eq!(restored.state_fingerprint(), world.state_fingerprint());
    }

    #[test]
    fn visible_plan_capacity_rejection_is_atomic() {
        let mut world = World::new(128, 64).unwrap();
        let before = world.state_fingerprint();
        let two = crate::simulation::streaming::ChunkRect::new(
            ChunkCoord { x: 0, y: 0 },
            ChunkCoord { x: 2, y: 1 },
        )
        .unwrap();
        let plan = crate::simulation::streaming::StreamingPolicy {
            offscreen_enabled: false,
            ..Default::default()
        }
        .plan_visible(two)
        .unwrap();
        assert_eq!(
            world.apply_visible_chunk_plan(plan, 8, 1),
            Err(WorldError::ActiveChunkCapacityExceeded)
        );
        assert_eq!(world.state_fingerprint(), before);
        assert_eq!(world.cells.chunk_count(), 2);
    }

    #[test]
    fn engine_default_viewport_plan_fits_bounded_offscreen_policy() {
        let mut world = World::new(640, 360).unwrap();
        let policy = crate::simulation::streaming::StreamingPolicy::default();
        let visible = crate::simulation::streaming::ChunkRect::new(
            ChunkCoord { x: 0, y: 0 },
            ChunkCoord { x: 10, y: 6 },
        )
        .unwrap();
        let plan = policy.plan_visible(visible).unwrap();
        assert_eq!(plan.active.chunk_count().unwrap(), 96);
        assert_eq!(plan.resident.chunk_count().unwrap(), 140);
        world
            .apply_visible_chunk_plan(plan, policy.max_resident_chunks, policy.max_active_chunks)
            .unwrap();
        assert_eq!(world.active_remote_chunks().count(), 36);
        assert_eq!(world.cells.chunk_count(), 140);
        let restored = World::decode_snapshot(
            &world.encode_snapshot().unwrap(),
            MaterialRegistry::with_defaults(),
        )
        .unwrap();
        assert_eq!(restored.state_fingerprint(), world.state_fingerprint());
    }

    #[test]
    fn remote_simulation_lease_is_pinned_saved_and_released_at_next_domain_apply() {
        let mut world = World::new(8, 8).unwrap();
        let policy = crate::simulation::streaming::StreamingPolicy::default();
        let remote = crate::simulation::streaming::ChunkRect::new(
            ChunkCoord { x: 100, y: -30 },
            ChunkCoord { x: 101, y: -29 },
        )
        .unwrap();
        let lease = world
            .request_chunk_lease(policy, 7, remote, LoadMode::Simulate)
            .unwrap();
        assert_eq!(world.pinned_chunk_count(), 1);
        assert_eq!(world.active_remote_chunks().count(), 1);
        let saved = world.encode_snapshot().unwrap();
        let mut restored =
            World::decode_snapshot(&saved, MaterialRegistry::with_defaults()).unwrap();
        assert_eq!(restored.state_fingerprint(), world.state_fingerprint());
        assert_eq!(restored.chunk_lease(lease).unwrap().source_id, 7);

        assert!(world.release_chunk_lease(lease));
        let visible = crate::simulation::streaming::ChunkRect::new(
            ChunkCoord { x: 0, y: 0 },
            ChunkCoord { x: 1, y: 1 },
        )
        .unwrap();
        world
            .apply_visible_chunk_plan(
                policy.plan_visible(visible).unwrap(),
                policy.max_resident_chunks,
                policy.max_active_chunks,
            )
            .unwrap();
        assert!(
            !world
                .active_remote_chunks()
                .any(|coord| coord == ChunkCoord { x: 100, y: -30 })
        );
        assert_eq!(
            world.query_cell(100 * CHUNK_EDGE, -30 * CHUNK_EDGE),
            CellQuery::Available(Cell::new(AIR))
        );

        assert!(restored.release_chunk_lease(lease));
        restored
            .apply_visible_chunk_plan(
                policy.plan_visible(visible).unwrap(),
                policy.max_resident_chunks,
                policy.max_active_chunks,
            )
            .unwrap();
        assert_eq!(restored.state_fingerprint(), world.state_fingerprint());
    }

    #[test]
    fn rejected_chunk_lease_does_not_publish_chunks_or_consume_an_id() {
        let mut world = World::new(8, 8).unwrap();
        let policy = crate::simulation::streaming::StreamingPolicy {
            max_pinned_chunks: 0,
            ..Default::default()
        };
        let remote = crate::simulation::streaming::ChunkRect::new(
            ChunkCoord { x: 100, y: 100 },
            ChunkCoord { x: 101, y: 101 },
        )
        .unwrap();
        assert_eq!(
            world.request_chunk_lease(policy, 1, remote, LoadMode::Prefetch),
            Err(WorldError::PinnedChunkCapacityExceeded)
        );
        assert_eq!(world.cells.chunk_count(), 1);
        assert_eq!(world.chunk_lease_count(), 0);
        assert_eq!(
            world
                .request_chunk_lease(
                    crate::simulation::streaming::StreamingPolicy::default(),
                    1,
                    remote,
                    LoadMode::Prefetch
                )
                .unwrap(),
            1
        );
    }
}
