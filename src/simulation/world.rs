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
    CommandBatchError, CommandEnvelope, CommandError, CommandQueueError, CommandResult,
    MAX_BRUSH_RADIUS, MAX_COMMAND_CANDIDATE_CELLS, MAX_PENDING_WORLD_COMMANDS, WorldCommand,
    WorldError, dispatch_command_batch,
};
use crate::simulation::determinism::Fnv64;
use crate::simulation::entity_id::{EntityAllocator, EntityId, EntityIdError};
use crate::simulation::ledger::{LedgerError, MaterialBalance, MaterialDelta, MaterialLedger};
use crate::simulation::material::{AIR, Behavior, MaterialRegistry, STONE, WATER};
use crate::simulation::particle::SplashParticle;
use crate::simulation::streaming::{ChunkRect, ChunkRequestPlan, LoadMode, StreamingPolicy};

/// シミュレーションワールド構造体
#[derive(Clone)]
pub struct World {
    /// ワールドの幅（セル単位・論理ピクセル）
    pub width: u32,
    /// ワールドの高さ（セル単位・論理ピクセル）
    pub height: u32,
    /// 地形生成の入力seed。保存し、遠隔chunkも同じ座標規則から再生成します。
    seed: u32,
    /// 有限worldと追加resident chunkのセル格納。
    pub(crate) cells: ChunkedCells,
    /// 動的マテリアルレジストリ（最大 65,535 種のブロックを管理）
    pub registry: MaterialRegistry,
    /// 材質ごとのtoken発行・消費累計です。セル移動では数を変えず、生成・採掘等の境界操作で更新します。
    material_ledger: MaterialLedger,
    entity_allocator: EntityAllocator,
    pending_commands: Vec<CommandEnvelope>,
    /// 二重移動防止用のセル外scratch。保存対象ではなく復元時に再構築します。
    movement_stamps: Vec<u64>,
    /// 有限領域cellの整数熱energy sidecarです。Cell::dataのbit領域を流用せず別配列に保持します。
    thermal_energy: Vec<i32>,
    /// 有限領域外のresident chunkごとに保持する、materialと一緒に移動する熱energy配列。
    /// keyと配列indexはsnapshotとhashの正規順でも同じworld座標へ対応付けます。
    remote_thermal_energy: HashMap<ChunkCoord, Box<[i32]>>,
    thermal_delta_scratch: Vec<i64>,
    thermal_next_scratch: Vec<i32>,
    remote_movement_stamps: HashMap<(i32, i32), u64>,
    active_remote_chunks: HashSet<ChunkCoord>,
    active_initial_chunks: Option<HashSet<ChunkCoord>>,
    /// 次状態が恒等になると確認済みのactive chunk。保存対象ではなく、復元時は全て起床します。
    sleeping_chunks: HashSet<ChunkCoord>,
    /// 物理状態が変化して睡眠判定をやり直すchunkと、そのhaloです。
    /// 最大resident数×近傍数を構築時に予約し、tick途中のHashSet再確保を避けます。
    sleep_candidates: HashSet<ChunkCoord>,
    sleep_scan_initialized: bool,
    /// Engine用の派生revision。lease command後にvisible planを再適用する通知で、保存/hash対象ではありません。
    domain_revision: u64,
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
const SNAPSHOT_VERSION: u16 = 9;
const SIMULATION_VERSION: u32 = 10;
const GENERATOR_VERSION: u32 = 2;
const DEFAULT_WORLD_SEED: u32 = 1337;
const MAX_SNAPSHOT_BYTES: usize = 512 * CHUNK_AREA * 4 + 16_000_000;
const WORLD_ENTITY_ID_CAPACITY: u32 = 1_000_000;

/// terrain generatorが参照する作者registry内の標準material IDです。
/// 作者が一部の標準物質を登録していない場合でも、未登録IDをWorldへ書かず、
/// その生成結果を空気へ置き換えてregistry不整合を避けます。
#[derive(Clone, Copy)]
struct TerrainMaterials {
    dirt: u16,
    stone: u16,
    water: u16,
    sand: u16,
}

impl TerrainMaterials {
    fn from_registry(registry: &MaterialRegistry) -> Self {
        Self {
            dirt: registry.get_id_by_name("core:dirt").unwrap_or(AIR),
            stone: registry.get_id_by_name("core:stone").unwrap_or(AIR),
            water: registry.get_id_by_name("core:water").unwrap_or(AIR),
            sand: registry.get_id_by_name("core:sand").unwrap_or(AIR),
        }
    }
}

/// seedと絶対world座標から一つのterrain cellを返します。
/// 有限領域を跨ぐchunk境界でも同じ座標は同じ初期値になり、ロード順に結果が変わりません。
fn generated_terrain_cell(
    fbm: &Fbm<SuperSimplex>,
    materials: TerrainMaterials,
    x: i32,
    y: i32,
) -> Cell {
    let height_noise = fbm.get([x as f64 * 0.008, 0.0]) * 45.0;
    let ground_y = 220.0 + height_noise;
    let density_noise = fbm.get([x as f64 * 0.02, y as f64 * 0.02]);
    let shade = ((y.rem_euclid(8) * 8) + x.rem_euclid(8)) as u8;

    if y as f64 >= ground_y {
        let depth = y as f64 - ground_y;
        if depth > 35.0 || density_noise > 0.35 {
            Cell::with_shade(materials.stone, shade)
        } else if density_noise < -0.45 && depth > 10.0 {
            Cell::new(AIR)
        } else {
            Cell::with_shade(materials.dirt, shade)
        }
    } else if (210..225).contains(&y) && (260..=370).contains(&x) {
        Cell::with_shade(materials.water, shade)
    } else if (200..222).contains(&y) && (370..=400).contains(&x) && (x - 370) > (222 - y) {
        Cell::with_shade(materials.sand, shade)
    } else {
        Cell::new(AIR)
    }
}

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

    pub fn cell_count(&self) -> usize {
        self.cells.len()
    }

    pub fn material_balance(&self, material_id: u16) -> Option<MaterialBalance> {
        self.material_ledger.balance(material_id)
    }

    pub fn reserve_entity_id(&mut self) -> Result<EntityId, WorldError> {
        self.entity_allocator
            .allocate()
            .map_err(map_entity_id_error)
    }

    pub fn release_entity_id(&mut self, id: EntityId) -> Result<bool, WorldError> {
        self.entity_allocator
            .release(id)
            .map_err(map_entity_id_error)
    }

    pub fn contains_entity_id(&self, id: EntityId) -> bool {
        self.entity_allocator.contains(id)
    }

    pub fn submit_command(&mut self, command: CommandEnvelope) -> Result<(), CommandQueueError> {
        let horizon = self.tick_count.saturating_add(1_000_000);
        if command.target_tick <= self.tick_count || command.target_tick > horizon {
            return Err(CommandQueueError::WrongTick);
        }
        if self.pending_commands.len() >= MAX_PENDING_WORLD_COMMANDS {
            return Err(CommandQueueError::CapacityExceeded);
        }
        if self.pending_commands.iter().any(|pending| {
            pending.target_tick == command.target_tick
                && pending.source_id == command.source_id
                && pending.sequence == command.sequence
        }) {
            return Err(CommandQueueError::DuplicateSourceSequence);
        }
        self.pending_commands
            .try_reserve(1)
            .map_err(|_| CommandQueueError::AllocationFailed)?;
        self.pending_commands.push(command);
        self.pending_commands.sort_unstable_by_key(|command| {
            (command.target_tick, command.source_id, command.sequence)
        });
        Ok(())
    }

    pub fn pending_command_count(&self) -> usize {
        self.pending_commands.len()
    }

    pub fn next_command_sequence(&self, target_tick: u64, source_id: u32) -> Option<u64> {
        self.pending_commands
            .iter()
            .filter(|command| command.target_tick == target_tick && command.source_id == source_id)
            .map(|command| command.sequence)
            .max()
            .map_or(Some(0), |sequence| sequence.checked_add(1))
    }

    /// 次tickに指定されたcommandを正規順で一括適用します。未来tickのcommandはqueueに残します。
    pub fn dispatch_pending_commands(
        &mut self,
    ) -> Result<Vec<crate::simulation::command::CommandEvent>, CommandQueueError> {
        let next_tick = self
            .tick_count
            .checked_add(1)
            .ok_or(CommandQueueError::WrongTick)?;
        if self
            .pending_commands
            .iter()
            .any(|command| command.target_tick <= self.tick_count)
        {
            return Err(CommandQueueError::InvalidPendingState);
        }
        let count = self
            .pending_commands
            .iter()
            .filter(|command| command.target_tick == next_tick)
            .count();
        let mut due = Vec::new();
        due.try_reserve_exact(count)
            .map_err(|_| CommandQueueError::AllocationFailed)?;
        due.extend(
            self.pending_commands
                .iter()
                .copied()
                .filter(|command| command.target_tick == next_tick),
        );
        let events = dispatch_command_batch(self, due).map_err(|error| match error {
            CommandBatchError::AllocationFailed => CommandQueueError::AllocationFailed,
            CommandBatchError::WrongTick => CommandQueueError::InvalidPendingState,
            CommandBatchError::DuplicateSourceSequence => CommandQueueError::InvalidPendingState,
        })?;
        self.pending_commands
            .retain(|command| command.target_tick != next_tick);
        Ok(events)
    }

    fn rebuild_material_ledger(&mut self) -> Result<(), WorldError> {
        for cell in self.cells.iter().chain(
            self.cells
                .remote_chunks()
                .flat_map(|(_, cells)| cells.iter()),
        ) {
            if !self.registry.is_registered(cell.material) {
                return Err(WorldError::InvalidMaterialState);
            }
        }
        self.material_ledger.clear_for_rebuild();
        for material in self
            .cells
            .iter()
            .chain(
                self.cells
                    .remote_chunks()
                    .flat_map(|(_, cells)| cells.iter()),
            )
            .map(|cell| cell.material)
            .filter(|material| *material != AIR)
        {
            self.material_ledger
                .issue_genesis(material)
                .map_err(map_ledger_world_error)?;
        }
        Ok(())
    }

    fn population_counts(&self) -> Result<Vec<u64>, SnapshotError> {
        let mut counts = Vec::new();
        counts
            .try_reserve_exact(self.registry.len())
            .map_err(|_| SnapshotError::AllocationFailed)?;
        counts.resize(self.registry.len(), 0_u64);
        for cell in self.cells.iter().chain(
            self.cells
                .remote_chunks()
                .flat_map(|(_, cells)| cells.iter()),
        ) {
            let count = counts
                .get_mut(cell.material as usize)
                .ok_or(SnapshotError::InvalidState)?;
            if cell.material != AIR {
                *count = count
                    .checked_add(1)
                    .ok_or(SnapshotError::CapacityExceeded)?;
            }
        }
        Ok(counts)
    }

    fn apply_material_deltas(&mut self, deltas: &[MaterialDelta]) -> Result<(), WorldError> {
        self.material_ledger
            .apply(deltas)
            .map_err(map_ledger_world_error)
    }

    #[cfg(test)]
    fn set_remote_cell_counted(&mut self, x: i32, y: i32, cell: Cell) -> Result<bool, WorldError> {
        if !self.registry.is_registered(cell.material) {
            return Err(WorldError::InvalidMaterialState);
        }
        let coordinate = ChunkCoord::from_cell(x, y);
        if self.cells.is_initial_chunk(coordinate) {
            return Ok(false);
        }
        let Some(previous) = self.cells.get_world(x, y).copied() else {
            return Ok(false);
        };
        if previous.material != cell.material {
            let mut deltas = [MaterialDelta::default(); 2];
            let mut len = 0;
            if previous.material != AIR {
                deltas[len] = MaterialDelta {
                    material_id: previous.material,
                    issue: 0,
                    consume: 1,
                };
                len += 1;
            }
            if cell.material != AIR {
                deltas[len] = MaterialDelta {
                    material_id: cell.material,
                    issue: 1,
                    consume: 0,
                };
                len += 1;
            }
            deltas[..len].sort_unstable_by_key(|delta| delta.material_id);
            self.apply_material_deltas(&deltas[..len])?;
        }
        if !self.cells.set_remote_cell(x, y, cell) {
            return Err(WorldError::InvalidMaterialState);
        }
        Ok(true)
    }

    /// デフォルトの解像度と基本レジストリで新しいワールドを初期化し、地形を自動生成します。
    pub fn new(width: u32, height: u32) -> Result<Self, WorldError> {
        Self::try_new(width, height)
    }

    pub fn try_new(width: u32, height: u32) -> Result<Self, WorldError> {
        Self::try_new_with_seed(width, height, DEFAULT_WORLD_SEED)
    }

    /// 指定seedで地形を生成するWorldを作ります。
    /// seedは初期terrainだけでなく、後から要求されるremote chunkにも同じように適用されます。
    pub fn try_new_with_seed(width: u32, height: u32, seed: u32) -> Result<Self, WorldError> {
        Self::try_with_registry_and_seed(width, height, MaterialRegistry::with_defaults(), seed)
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
        registry: MaterialRegistry,
    ) -> Result<Self, WorldError> {
        Self::try_with_registry_and_seed(width, height, registry, DEFAULT_WORLD_SEED)
    }

    /// 作者registryと地形seedを指定してWorldを構築します。
    /// 標準terrain materialが作者registryにない場合、その種類の生成結果は空気へ置換します。
    pub fn try_with_registry_and_seed(
        width: u32,
        height: u32,
        mut registry: MaterialRegistry,
        seed: u32,
    ) -> Result<Self, WorldError> {
        registry
            .try_freeze()
            .map_err(|_| WorldError::InvalidMaterialRegistry)?;
        let material_ledger =
            MaterialLedger::try_new(registry.len()).map_err(|_| WorldError::AllocationFailed)?;
        let entity_allocator =
            EntityAllocator::new(WORLD_ENTITY_ID_CAPACITY).map_err(map_entity_id_error)?;
        let mut sleeping_chunks = HashSet::new();
        let initial_chunk_count = (width as usize)
            .div_ceil(CHUNK_EDGE as usize)
            .checked_mul((height as usize).div_ceil(CHUNK_EDGE as usize))
            .ok_or(WorldError::WorldCapacityExceeded)?;
        sleeping_chunks
            .try_reserve(
                initial_chunk_count
                    .saturating_add(crate::simulation::streaming::HARD_MAX_RESIDENT_CHUNKS),
            )
            .map_err(|_| WorldError::AllocationFailed)?;
        let mut sleep_candidates = HashSet::new();
        sleep_candidates
            .try_reserve(
                initial_chunk_count
                    .saturating_add(crate::simulation::streaming::HARD_MAX_RESIDENT_CHUNKS)
                    * 9,
            )
            .map_err(|_| WorldError::AllocationFailed)?;
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
            seed,
            cells,
            registry,
            material_ledger,
            entity_allocator,
            pending_commands: Vec::new(),
            movement_stamps,
            thermal_energy,
            remote_thermal_energy: HashMap::new(),
            thermal_delta_scratch,
            thermal_next_scratch,
            remote_movement_stamps: HashMap::new(),
            active_remote_chunks: HashSet::new(),
            active_initial_chunks: None,
            sleeping_chunks,
            sleep_candidates,
            sleep_scan_initialized: false,
            domain_revision: 0,
            chunk_leases: HashMap::new(),
            next_chunk_lease_id: 1,
            tick_count: 0,
            particles: Vec::new(),
        };

        world.generate_terrain()?;
        Ok(world)
    }

    /// ノイズアルゴリズム（Simplex FBM）を用いて地形を生成します。
    fn generate_terrain(&mut self) -> Result<(), WorldError> {
        let fbm = Fbm::<SuperSimplex>::new(self.seed);
        let materials = TerrainMaterials::from_registry(&self.registry);

        for y in 0..self.height {
            for x in 0..self.width {
                let idx = (y * self.width + x) as usize;

                self.cells[idx] = generated_terrain_cell(&fbm, materials, x as i32, y as i32);
            }
        }
        self.rebuild_material_ledger()?;
        Ok(())
    }

    /// ワールド内の全セル、粒子、および構造支持崩落を 1 tick（1/60 秒）進めます。
    pub fn update(&mut self) -> Result<(), WorldError> {
        self.ensure_tick_available()?;
        let mut resident_remote = Vec::new();
        resident_remote
            .try_reserve_exact(self.cells.remote_chunks().count())
            .map_err(|_| WorldError::AllocationFailed)?;
        resident_remote.extend(self.cells.remote_chunks().map(|(coord, _)| coord));
        self.ensure_world_chunks(
            &resident_remote,
            crate::simulation::streaming::HARD_MAX_RESIDENT_CHUNKS,
        )?;
        let active_coordinates = self.active_chunk_coordinates()?;
        self.wake_neighbors_of_awake_chunks(&active_coordinates);
        let mut sleep_candidates = Vec::new();
        sleep_candidates
            .try_reserve(active_coordinates.len().saturating_mul(10))
            .map_err(|_| WorldError::AllocationFailed)?;
        if !self.sleep_scan_initialized {
            sleep_candidates.extend_from_slice(&active_coordinates);
            self.sleep_scan_initialized = true;
        }
        sleep_candidates.extend(self.sleep_candidates.drain());
        // 表示粒子の更新対象をtick開始時に記録します。確保に失敗した場合は、
        // cellやtickを一切変更する前にエラーを返し、半端なsimulation stepを防ぎます。
        let mut particle_active = Vec::new();
        particle_active
            .try_reserve_exact(self.particles.len())
            .map_err(|_| WorldError::AllocationFailed)?;
        particle_active.extend(self.particles.iter().map(|particle| {
            self.world_cell_active(particle.x.floor() as i32, particle.y.floor() as i32)
        }));
        let mut active_remote = Vec::new();
        active_remote
            .try_reserve_exact(self.active_remote_chunks.len())
            .map_err(|_| WorldError::AllocationFailed)?;
        active_remote.extend(active_coordinates.iter().copied().filter(|coord| {
            !self.cells.is_initial_chunk(*coord) && !self.sleeping_chunks.contains(coord)
        }));
        let remote_count = active_remote
            .len()
            .checked_mul(CHUNK_AREA)
            .ok_or(WorldError::WorldCapacityExceeded)?;
        let mut remote_coordinates = Vec::new();
        remote_coordinates
            .try_reserve_exact(remote_count)
            .map_err(|_| WorldError::AllocationFailed)?;
        for coord in active_remote {
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
                self.update_world_cell(
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
                    self.update_world_cell(remote_coordinates[remote_index].0, y);
                    remote_index += 1;
                }
                let chunks_x = (self.width as usize).div_ceil(CHUNK_EDGE as usize);
                let chunk_y = y.div_euclid(CHUNK_EDGE);
                for chunk_x in 0..chunks_x {
                    let coordinate = ChunkCoord {
                        x: chunk_x as i32,
                        y: chunk_y,
                    };
                    if !self.initial_chunk_active((chunk_x as i32) * CHUNK_EDGE, y)
                        || self.sleeping_chunks.contains(&coordinate)
                    {
                        continue;
                    }
                    let start_x = (chunk_x * CHUNK_EDGE as usize) as i32;
                    let end_x = (start_x + CHUNK_EDGE).min(self.width as i32);
                    for x in start_x..end_x {
                        self.update_world_cell(x, y);
                    }
                }
                while remote_index < remote_coordinates.len()
                    && remote_coordinates[remote_index].1 == y
                {
                    self.update_world_cell(remote_coordinates[remote_index].0, y);
                    remote_index += 1;
                }
            } else {
                while remote_index < remote_coordinates.len()
                    && remote_coordinates[remote_index].1 == y
                    && remote_coordinates[remote_index].0 >= self.width as i32
                {
                    self.update_world_cell(remote_coordinates[remote_index].0, y);
                    remote_index += 1;
                }
                let chunks_x = (self.width as usize).div_ceil(CHUNK_EDGE as usize);
                let chunk_y = y.div_euclid(CHUNK_EDGE);
                for chunk_x in (0..chunks_x).rev() {
                    let coordinate = ChunkCoord {
                        x: chunk_x as i32,
                        y: chunk_y,
                    };
                    if !self.initial_chunk_active((chunk_x as i32) * CHUNK_EDGE, y)
                        || self.sleeping_chunks.contains(&coordinate)
                    {
                        continue;
                    }
                    let start_x = (chunk_x * CHUNK_EDGE as usize) as i32;
                    let end_x = (start_x + CHUNK_EDGE).min(self.width as i32);
                    for x in (start_x..end_x).rev() {
                        self.update_world_cell(x, y);
                    }
                }
                while remote_index < remote_coordinates.len()
                    && remote_coordinates[remote_index].1 == y
                {
                    self.update_world_cell(remote_coordinates[remote_index].0, y);
                    remote_index += 1;
                }
            }
        }
        while remote_index < remote_coordinates.len() {
            self.update_world_cell(
                remote_coordinates[remote_index].0,
                remote_coordinates[remote_index].1,
            );
            remote_index += 1;
        }

        // 物質移動後に熱を拡散します。先に移動を済ませることで、cell交換に追従したenergyを使います。
        let active_initial_chunks = self.active_initial_chunks.as_ref();
        let width = self.width as usize;
        crate::simulation::thermal::diffuse_energy_masked_into(
            self.width as usize,
            self.height as usize,
            &self.thermal_energy,
            1,
            8,
            &mut self.thermal_delta_scratch,
            &mut self.thermal_next_scratch,
            |index| {
                active_initial_chunks.is_none_or(|active| {
                    active.contains(&ChunkCoord::from_cell(
                        (index % width) as i32,
                        (index / width) as i32,
                    ))
                })
            },
        )
        .map_err(|_| WorldError::AllocationFailed)?;
        self.thermal_energy
            .copy_from_slice(&self.thermal_next_scratch);

        // 3. 自由粒子の更新
        let mut active_index = 0;
        self.particles.retain_mut(|particle| {
            let active = particle_active[active_index];
            active_index += 1;
            !active || particle.update(0.12)
        });
        self.refresh_sleeping_chunks(&mut sleep_candidates, &active_coordinates);
        Ok(())
    }

    /// 有限領域内の熱energyを整数単位で設定します。
    /// 凍結chunkには書き込まず、等熱容量の拡散solverが保存則を保つ値として扱います。
    pub fn set_thermal_energy(&mut self, x: u32, y: u32, energy: i32) -> bool {
        if x >= self.width || y >= self.height || !self.initial_chunk_active(x as i32, y as i32) {
            return false;
        }
        self.thermal_energy[(y * self.width + x) as usize] = energy;
        self.wake_chunk_neighborhood(ChunkCoord::from_cell(x as i32, y as i32));
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

    #[cfg(test)]
    fn touches_active_remote_chunk_boundary(&self, x: i32, y: i32) -> bool {
        let own = ChunkCoord::from_cell(x, y);
        [
            ChunkCoord {
                x: own.x - 1,
                y: own.y,
            },
            ChunkCoord {
                x: own.x + 1,
                y: own.y,
            },
            ChunkCoord {
                x: own.x,
                y: own.y - 1,
            },
            ChunkCoord {
                x: own.x,
                y: own.y + 1,
            },
        ]
        .into_iter()
        .any(|neighbor| {
            !self.cells.is_initial_chunk(neighbor) && self.active_remote_chunks.contains(&neighbor)
        })
    }

    /// 現在activeなfinite/remote chunkを座標順に列挙します。
    /// HashSetの反復順を更新順に流さず、以降の準備・判定を常に同じ順で行います。
    fn active_chunk_coordinates(&self) -> Result<Vec<ChunkCoord>, WorldError> {
        let initial_count = self.active_initial_chunks.as_ref().map_or_else(
            || {
                (self.width as usize).div_ceil(CHUNK_EDGE as usize)
                    * (self.height as usize).div_ceil(CHUNK_EDGE as usize)
            },
            HashSet::len,
        );
        let capacity = initial_count
            .checked_add(self.active_remote_chunks.len())
            .ok_or(WorldError::WorldCapacityExceeded)?;
        let mut coordinates = Vec::new();
        coordinates
            .try_reserve_exact(capacity)
            .map_err(|_| WorldError::AllocationFailed)?;
        if let Some(active) = &self.active_initial_chunks {
            coordinates.extend(active.iter().copied());
        } else {
            let chunks_x = (self.width as usize).div_ceil(CHUNK_EDGE as usize);
            let chunks_y = (self.height as usize).div_ceil(CHUNK_EDGE as usize);
            for y in 0..chunks_y {
                for x in 0..chunks_x {
                    coordinates.push(ChunkCoord {
                        x: x as i32,
                        y: y as i32,
                    });
                }
            }
        }
        coordinates.extend(self.active_remote_chunks.iter().copied());
        coordinates.sort_unstable();
        coordinates.dedup();
        Ok(coordinates)
    }

    /// 変更点と1chunkのhaloを起床させ、後で恒等性を再判定する候補へ登録します。
    /// 全てのセル移動・コマンド編集・熱入力はこの入口を使い、隣chunkから届く物理を
    /// 凍結領域が見落とさないようにします。候補集合の容量はWorld構築時に予約済みです。
    fn wake_chunk_neighborhood(&mut self, coordinate: ChunkCoord) {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (Some(x), Some(y)) =
                    (coordinate.x.checked_add(dx), coordinate.y.checked_add(dy))
                else {
                    continue;
                };
                let neighbor = ChunkCoord { x, y };
                self.sleeping_chunks.remove(&neighbor);
                self.sleep_candidates.insert(neighbor);
            }
        }
    }

    /// awake chunkの隣に眠っているactive chunkがあれば、tick開始前に起床させます。
    /// これによりy/x走査順にかかわらず、同tickに隣の流体や粉体が到達できます。
    fn wake_neighbors_of_awake_chunks(&mut self, active: &[ChunkCoord]) {
        for coordinate in active.iter().copied() {
            if !self.sleeping_chunks.contains(&coordinate) {
                self.wake_chunk_neighborhood(coordinate);
            }
        }
    }

    /// chunk全体の次状態が恒等となる十分条件を検査します。
    /// 空気と静止solidだけで、4×4支持groupが崩落閾値未満でなく、熱energyも0の場合だけ
    /// sleepを許可します。条件を狭く保つことでtimerや反応など未追跡状態の見落としを防ぎます。
    fn chunk_is_quiescent(&self, coordinate: ChunkCoord) -> bool {
        let Some(origin_x) = coordinate.x.checked_mul(CHUNK_EDGE) else {
            return false;
        };
        let Some(origin_y) = coordinate.y.checked_mul(CHUNK_EDGE) else {
            return false;
        };
        let mut solid_counts = [0_u8; 16 * 16];
        for local_y in 0..CHUNK_EDGE {
            for local_x in 0..CHUNK_EDGE {
                let x = origin_x + local_x;
                let y = origin_y + local_y;
                let Some(cell) = self.cells.get_world(x, y) else {
                    continue;
                };
                if self.world_thermal_energy(x, y).unwrap_or(0) != 0 {
                    return false;
                }
                if cell.material == AIR {
                    continue;
                }
                if !self.registry.is_registered(cell.material)
                    || self.registry.get(cell.material).behavior != Behavior::Solid
                    || cell.is_falling()
                {
                    return false;
                }
                let group = (local_y as usize / 4) * 16 + local_x as usize / 4;
                solid_counts[group] += 1;
            }
        }
        solid_counts.iter().all(|count| *count == 0 || *count >= 8)
    }

    /// tick後に睡眠判定が必要なchunkだけ再検査し、十分条件を満たすものをsleepへ移します。
    fn refresh_sleeping_chunks(&mut self, candidates: &mut Vec<ChunkCoord>, active: &[ChunkCoord]) {
        candidates.extend(self.sleep_candidates.drain());
        candidates.sort_unstable();
        candidates.dedup();
        for coordinate in candidates.iter().copied() {
            if active.binary_search(&coordinate).is_ok() && self.chunk_is_quiescent(coordinate) {
                self.sleeping_chunks.insert(coordinate);
            }
        }
    }

    /// 遠隔または有限領域のchunkを、lease IDが明示解放されるまでresidentに保ちます。
    /// `Simulate` leaseは領域をactiveにもします。操作をtick順に再現する場合は、
    /// この直接APIではなくWorldCommandとしてtarget tickへ登録してください。
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
        self.bump_domain_revision();
        Ok(id)
    }

    pub fn release_chunk_lease(&mut self, lease_id: u64) -> bool {
        let released = self.chunk_leases.remove(&lease_id).is_some();
        if released {
            self.bump_domain_revision();
        }
        released
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

    /// Engineがdomain cacheの再計算要否を判定するための単調revisionを返します。
    pub fn domain_revision(&self) -> u64 {
        self.domain_revision
    }

    fn bump_domain_revision(&mut self) {
        self.domain_revision = self.domain_revision.wrapping_add(1);
    }

    pub fn resident_chunk_count(&self) -> usize {
        self.cells.chunk_count()
    }

    /// residentな有限セルまたはremoteセルの熱energy sidecarを読む。
    /// 未ロードchunkは`None`を返し、凍結domainの保存値は読み取り可能です。
    pub fn world_thermal_energy(&self, x: i32, y: i32) -> Option<i32> {
        if x >= 0 && x < self.width as i32 && y >= 0 && y < self.height as i32 {
            return self
                .thermal_energy
                .get(y as usize * self.width as usize + x as usize)
                .copied();
        }
        let sidecar = self
            .remote_thermal_energy
            .get(&ChunkCoord::from_cell(x, y))?;
        let index = y.rem_euclid(CHUNK_EDGE) as usize * CHUNK_EDGE as usize
            + x.rem_euclid(CHUNK_EDGE) as usize;
        sidecar.get(index).copied()
    }

    /// activeな有限セルまたはremoteセルの熱energyを設定する。
    /// 未ロードまたは凍結中のセルは変更せず`false`を返します。
    pub fn set_world_thermal_energy(&mut self, x: i32, y: i32, energy: i32) -> bool {
        if !self.world_cell_active(x, y) {
            return false;
        }
        if x >= 0 && x < self.width as i32 && y >= 0 && y < self.height as i32 {
            let index = y as usize * self.width as usize + x as usize;
            self.thermal_energy[index] = energy;
            self.wake_chunk_neighborhood(ChunkCoord::from_cell(x, y));
            return true;
        }
        let Some(sidecar) = self
            .remote_thermal_energy
            .get_mut(&ChunkCoord::from_cell(x, y))
        else {
            return false;
        };
        let index = y.rem_euclid(CHUNK_EDGE) as usize * CHUNK_EDGE as usize
            + x.rem_euclid(CHUNK_EDGE) as usize;
        let Some(target) = sidecar.get_mut(index) else {
            return false;
        };
        *target = energy;
        self.wake_chunk_neighborhood(ChunkCoord::from_cell(x, y));
        true
    }

    fn ensure_world_chunks(
        &mut self,
        coordinates: &[ChunkCoord],
        resident_limit: usize,
    ) -> Result<usize, WorldError> {
        let mut missing_cells = Vec::new();
        missing_cells
            .try_reserve_exact(coordinates.len())
            .map_err(|_| WorldError::AllocationFailed)?;
        for coord in coordinates.iter().copied() {
            if !self.cells.is_initial_chunk(coord)
                && self
                    .cells
                    .get_world(coord.x * CHUNK_EDGE, coord.y * CHUNK_EDGE)
                    .is_none()
            {
                missing_cells.push(coord);
            }
        }
        missing_cells.sort_unstable();
        missing_cells.dedup();

        let mut missing_energy = Vec::new();
        missing_energy
            .try_reserve_exact(coordinates.len())
            .map_err(|_| WorldError::AllocationFailed)?;
        for coord in coordinates.iter().copied() {
            if !self.cells.is_initial_chunk(coord)
                && !self.remote_thermal_energy.contains_key(&coord)
            {
                missing_energy.push(coord);
            }
        }
        missing_energy.sort_unstable();
        missing_energy.dedup();

        let fbm = Fbm::<SuperSimplex>::new(self.seed);
        let terrain_materials = TerrainMaterials::from_registry(&self.registry);
        let mut generated = Vec::new();
        generated
            .try_reserve_exact(missing_cells.len())
            .map_err(|_| WorldError::AllocationFailed)?;
        let mut issued_counts = Vec::new();
        issued_counts
            .try_reserve_exact(self.registry.len())
            .map_err(|_| WorldError::AllocationFailed)?;
        issued_counts.resize(self.registry.len(), 0_u64);
        for coord in &missing_cells {
            let mut cells = Vec::new();
            cells
                .try_reserve_exact(CHUNK_AREA)
                .map_err(|_| WorldError::AllocationFailed)?;
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
                    let cell = generated_terrain_cell(
                        &fbm,
                        terrain_materials,
                        origin_x + local_x,
                        origin_y + local_y,
                    );
                    if cell.material != AIR {
                        let count = issued_counts
                            .get_mut(cell.material as usize)
                            .ok_or(WorldError::InvalidMaterialState)?;
                        *count = count.checked_add(1).ok_or(WorldError::AccountingOverflow)?;
                    }
                    cells.push(cell);
                }
            }
            generated.push((*coord, cells.into_boxed_slice()));
        }

        let mut deltas = Vec::new();
        deltas
            .try_reserve_exact(issued_counts.len())
            .map_err(|_| WorldError::AllocationFailed)?;
        for (material_id, issue) in issued_counts.into_iter().enumerate() {
            if issue > 0 {
                deltas.push(MaterialDelta {
                    material_id: u16::try_from(material_id)
                        .map_err(|_| WorldError::InvalidMaterialState)?,
                    issue,
                    consume: 0,
                });
            }
        }
        let mut candidate_ledger = self
            .material_ledger
            .try_clone()
            .map_err(map_ledger_world_error)?;
        candidate_ledger
            .apply(&deltas)
            .map_err(map_ledger_world_error)?;

        let mut prepared_energy = Vec::new();
        prepared_energy
            .try_reserve_exact(missing_energy.len())
            .map_err(|_| WorldError::AllocationFailed)?;
        for coord in &missing_energy {
            let mut energy = Vec::new();
            energy
                .try_reserve_exact(CHUNK_AREA)
                .map_err(|_| WorldError::AllocationFailed)?;
            energy.resize(CHUNK_AREA, 0_i32);
            prepared_energy.push((*coord, energy.into_boxed_slice()));
        }
        self.remote_thermal_energy
            .try_reserve(missing_energy.len())
            .map_err(|_| WorldError::AllocationFailed)?;
        let added = self
            .cells
            .ensure_chunks(coordinates, resident_limit, Cell::new(AIR))
            .map_err(map_chunk_error)?;
        for (coord, cells) in generated {
            let origin_x = coord.x * CHUNK_EDGE;
            let origin_y = coord.y * CHUNK_EDGE;
            for (index, cell) in cells.iter().copied().enumerate() {
                let x = origin_x + (index % CHUNK_EDGE as usize) as i32;
                let y = origin_y + (index / CHUNK_EDGE as usize) as i32;
                if let Some(target) = self.cells.get_world_mut(x, y) {
                    *target = cell;
                }
            }
        }
        self.material_ledger = candidate_ledger;
        for (coord, energy) in prepared_energy {
            self.remote_thermal_energy.insert(coord, energy);
        }
        Ok(added)
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

    /// 可視領域から計算したresident/active範囲を、容量検証後に一括適用します。
    /// 新規remote chunkの地形生成を含むため、失敗時には既存domainもセル状態も変えません。
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
        self.ensure_world_chunks(&resident, resident_limit)?;
        // 全remote cell・energyの確保に成功してからsleep cacheを変えます。
        // 容量やallocation拒否でvisible planが失敗した場合、既存domainの派生状態にも触れません。
        for coordinate in active_initial
            .iter()
            .copied()
            .chain(active_remote.iter().copied())
        {
            self.wake_chunk_neighborhood(coordinate);
        }
        self.active_remote_chunks.clear();
        self.active_remote_chunks.extend(active_remote);
        self.active_initial_chunks = Some(active_initial);
        self.bump_domain_revision();
        Ok(())
    }

    fn update_world_cell(&mut self, x: i32, y: i32) {
        let Some(cell) = self.cells.get_world(x, y).copied() else {
            return;
        };
        if cell.material == AIR
            || !self.registry.is_registered(cell.material)
            || self.moved_at(x, y)
        {
            return;
        }
        let definition = self.registry.get(cell.material);
        let behavior = definition.behavior;
        let mass = definition.mass_per_cell;
        let max_fall = i32::from(definition.max_fall_speed);
        let dispersion = i32::from(definition.dispersion);
        let flow_speed = i32::from(definition.flow_speed);
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
                            Some(next) if next.material == AIR => {
                                target = Some((nx, y));
                                if step >= flow_speed {
                                    break;
                                }
                            }
                            Some(next)
                                if self.registry.get(next.material).behavior
                                    == Behavior::Liquid => {}
                            Some(next)
                                if self.registry.get(next.material).can_be_washed_away
                                    && (self.registry.get(next.material).behavior
                                        == Behavior::Powder
                                        || next.is_falling()) =>
                            {
                                let push = (nx + dx, y);
                                if self
                                    .cells
                                    .get_world(push.0, push.1)
                                    .is_some_and(|cell| cell.material == AIR)
                                    && self.push_world_cells((x, y), (nx, y), push)
                                {
                                    return;
                                }
                                break;
                            }
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
                .any(|(nx, ny)| self.is_world_solid_support(nx, ny));
            if !supported {
                if let Some(current) = self.cells.get_world_mut(x, y) {
                    current.set_falling(true);
                }
                self.wake_chunk_neighborhood(ChunkCoord::from_cell(x, y));
            }
        }
        if let Some((tx, ty)) = target {
            self.move_world_cells((x, y), (tx, ty));
        } else if cell.is_falling()
            && behavior == Behavior::Solid
            && self.is_world_solid_support(x, y + 1)
            && let Some(current) = self.cells.get_world_mut(x, y)
        {
            current.set_falling(false);
            self.wake_chunk_neighborhood(ChunkCoord::from_cell(x, y));
        }
    }

    /// 未ロード・世界端を衝突面として扱い、隣の安定固体も落下セルの支持に使います。
    /// domain境界を空気として扱うと、境界でセルが移動したり静止solidが永久に落下扱いになるため、
    /// 状態が存在しない隣接位置は保守的に支持面とします。ロード後は実セルで再判定します。
    fn is_world_solid_support(&self, x: i32, y: i32) -> bool {
        self.cells.get_world(x, y).is_none_or(|neighbor| {
            self.registry.get(neighbor.material).behavior == Behavior::Solid
                && !neighbor.is_falling()
        })
    }

    /// 水流が粉体を押す3セル交換を、material・energy・movement stamp一括で確定します。
    /// 3地点のどれかが未ロード・凍結・処理済みなら何も変更せず、片側だけの移動を防ぎます。
    fn push_world_cells(
        &mut self,
        source: (i32, i32),
        displaced: (i32, i32),
        destination: (i32, i32),
    ) -> bool {
        let points = [source, displaced, destination];
        if points
            .iter()
            .any(|(x, y)| !self.world_cell_active(*x, *y) || self.moved_at(*x, *y))
        {
            return false;
        }
        let (Some(source_cell), Some(displaced_cell), Some(destination_cell)) = (
            self.cells.get_world(source.0, source.1).copied(),
            self.cells.get_world(displaced.0, displaced.1).copied(),
            self.cells.get_world(destination.0, destination.1).copied(),
        ) else {
            return false;
        };
        if destination_cell.material != AIR {
            return false;
        }
        let energies = points.map(|(x, y)| self.world_thermal_energy(x, y).unwrap_or(0));
        for ((x, y), cell) in points
            .into_iter()
            .zip([Cell::new(AIR), source_cell, displaced_cell])
        {
            if let Some(target) = self.cells.get_world_mut(x, y) {
                *target = cell;
            }
        }
        for ((x, y), energy) in points
            .into_iter()
            .zip([energies[2], energies[0], energies[1]])
        {
            self.set_world_thermal_energy(x, y, energy);
        }
        for (x, y) in points {
            self.mark_moved_at(x, y);
            self.wake_chunk_neighborhood(ChunkCoord::from_cell(x, y));
        }
        true
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
        let source_energy = self.world_thermal_energy(source.0, source.1).unwrap_or(0);
        let target_energy = self.world_thermal_energy(target.0, target.1).unwrap_or(0);
        if let Some(cell) = self.cells.get_world_mut(source.0, source.1) {
            *cell = target_cell;
        }
        if let Some(cell) = self.cells.get_world_mut(target.0, target.1) {
            *cell = source_cell;
        }
        self.set_world_thermal_energy(source.0, source.1, target_energy);
        self.set_world_thermal_energy(target.0, target.1, source_energy);
        self.mark_moved_at(source.0, source.1);
        self.mark_moved_at(target.0, target.1);
        self.wake_chunk_neighborhood(ChunkCoord::from_cell(source.0, source.1));
        self.wake_chunk_neighborhood(ChunkCoord::from_cell(target.0, target.1));
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
            .filter(|coordinate| !self.sleeping_chunks.contains(coordinate))
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
                            self.wake_chunk_neighborhood(ChunkCoord::from_cell(x, y));
                        }
                    }
                }
            }
        }
    }

    /// leaseまたはactive domainから外れたremote chunkを明示破棄します。
    /// これはメモリ退避ではなく、chunk内の物質tokenを消費台帳へ記録して世界から破壊する操作です。
    /// 保存して再ロードする退避APIとは区別し、lease中・active中のデータは破棄できません。
    pub fn remove_remote_chunk(&mut self, coordinate: ChunkCoord) -> Result<bool, WorldError> {
        if self.active_remote_chunks.contains(&coordinate) {
            return Err(WorldError::ChunkIsActive);
        }
        if self.chunk_leases.values().any(|lease| {
            coordinate.x >= lease.region.min.x
                && coordinate.x < lease.region.max.x
                && coordinate.y >= lease.region.min.y
                && coordinate.y < lease.region.max.y
        }) {
            return Err(WorldError::ChunkIsLeased);
        }
        let Some((_, cells)) = self
            .cells
            .remote_chunks()
            .find(|(chunk_coordinate, _)| *chunk_coordinate == coordinate)
        else {
            return Ok(false);
        };
        let mut deltas = Vec::new();
        deltas
            .try_reserve_exact(cells.len())
            .map_err(|_| WorldError::AllocationFailed)?;
        for cell in cells.iter().filter(|cell| cell.material != AIR) {
            if !self.registry.is_registered(cell.material) {
                return Err(WorldError::InvalidMaterialState);
            }
            deltas.push(MaterialDelta {
                material_id: cell.material,
                issue: 0,
                consume: 1,
            });
        }
        deltas.sort_unstable_by_key(|delta| delta.material_id);
        let mut write = 0;
        for read in 0..deltas.len() {
            let delta = deltas[read];
            if write > 0 && deltas[write - 1].material_id == delta.material_id {
                deltas[write - 1].consume += delta.consume;
            } else {
                deltas[write] = delta;
                write += 1;
            }
        }
        deltas.truncate(write);
        self.apply_material_deltas(&deltas)?;
        if !self.cells.remove_chunk(coordinate) {
            return Err(WorldError::InvalidMaterialState);
        }
        self.remote_thermal_energy.remove(&coordinate);
        self.active_remote_chunks.remove(&coordinate);
        self.sleeping_chunks.remove(&coordinate);
        self.sleep_candidates.remove(&coordinate);
        self.remote_movement_stamps
            .retain(|(x, y), _| ChunkCoord::from_cell(*x, *y) != coordinate);
        Ok(true)
    }

    /// 検証済み要求範囲を確保します。Prefetchはresident化だけ、Simulateはactive化まで行います。
    /// 未生成chunkは保存seedと絶対world座標から決定的に生成し、material ledgerの
    /// 発行数もセル公開と同じatomic境界で更新します。
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
        let added = self.ensure_world_chunks(&coordinates, resident_limit)?;
        if request.mode == LoadMode::Simulate {
            for coordinate in coordinates.iter().copied() {
                if !self.cells.is_initial_chunk(coordinate) {
                    self.active_remote_chunks.insert(coordinate);
                    self.wake_chunk_neighborhood(coordinate);
                }
            }
        }
        if added > 0 || request.mode == LoadMode::Simulate {
            self.bump_domain_revision();
        }
        Ok(added)
    }

    pub fn active_remote_chunks(&self) -> impl Iterator<Item = ChunkCoord> + '_ {
        self.active_remote_chunks.iter().copied()
    }

    pub fn deactivate_remote_chunks(&mut self, coordinates: &[ChunkCoord]) {
        let mut changed = false;
        for coordinate in coordinates {
            changed |= self.active_remote_chunks.remove(coordinate);
        }
        if changed {
            self.bump_domain_revision();
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
        hash.write(b"charon-world-fingerprint-v8");
        hash.write_u32(self.width);
        hash.write_u32(self.height);
        hash.write_u32(self.seed);
        hash.write_u64(self.tick_count);
        for energy in &self.thermal_energy {
            hash.write(&energy.to_le_bytes());
        }
        let mut remote_thermal: Vec<_> =
            self.cells.remote_chunks().map(|(coord, _)| coord).collect();
        remote_thermal.sort_unstable();
        for coordinate in remote_thermal {
            hash.write(&coordinate.x.to_le_bytes());
            hash.write(&coordinate.y.to_le_bytes());
            if let Some(energy) = self.remote_thermal_energy.get(&coordinate) {
                for value in energy.iter() {
                    hash.write(&value.to_le_bytes());
                }
            } else {
                for _ in 0..CHUNK_AREA {
                    hash.write(&0_i32.to_le_bytes());
                }
            }
        }
        self.registry.write_fingerprint(&mut hash);
        for balance in self.material_ledger.balances() {
            hash.write_u64(balance.issued);
            hash.write_u64(balance.consumed);
        }
        self.entity_allocator.write_fingerprint(&mut hash);
        for command in &self.pending_commands {
            hash.write_u64(command.target_tick);
            hash.write_u32(command.source_id);
            hash.write_u64(command.sequence);
            let mut payload = Vec::new();
            push_world_command(&mut payload, command.payload);
            hash.write(&payload);
        }
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
        hash.finish()
    }

    /// tick境界で停止したWorldをsnapshot bytesへ符号化します。registry定義は外部contentとしてfingerprintで照合します。
    pub fn encode_snapshot(&self) -> Result<Vec<u8>, SnapshotError> {
        self.encode_snapshot_impl(true)
    }

    pub(crate) fn encode_canonical_snapshot(&self) -> Result<Vec<u8>, SnapshotError> {
        self.encode_snapshot_impl(false)
    }

    fn encode_snapshot_impl(
        &self,
        include_visual_particles: bool,
    ) -> Result<Vec<u8>, SnapshotError> {
        let entity_allocator = self
            .entity_allocator
            .checkpoint()
            .map_err(|_| SnapshotError::AllocationFailed)?;
        let pending_reserve = self
            .pending_commands
            .len()
            .checked_mul(87)
            .ok_or(SnapshotError::CapacityExceeded)?;
        let particle_count = if include_visual_particles {
            self.particles.len()
        } else {
            0
        };
        let particle_reserve = particle_count
            .checked_mul(28)
            .and_then(|size| size.checked_add(2))
            .ok_or(SnapshotError::CapacityExceeded)?;
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
            .and_then(|size| size.checked_add(remote_count.checked_mul(CHUNK_AREA * 8 + 8)?))
            .and_then(|size| {
                size.checked_add(
                    self.active_initial_chunks
                        .as_ref()
                        .map_or(0, HashSet::len)
                        .checked_mul(8)?,
                )
            })
            .and_then(|size| size.checked_add(self.chunk_leases.len().checked_mul(29)?))
            .and_then(|size| size.checked_add(2 + self.material_ledger.balances().len() * 16))
            .and_then(|size| size.checked_add(4 + entity_allocator.len()))
            .and_then(|size| size.checked_add(2 + pending_reserve))
            .and_then(|size| size.checked_add(particle_reserve))
            .ok_or(SnapshotError::CapacityExceeded)?;
        out.try_reserve(reserve)
            .map_err(|_| SnapshotError::AllocationFailed)?;
        out.extend_from_slice(SNAPSHOT_MAGIC);
        push_u16(&mut out, SNAPSHOT_VERSION);
        push_u32(&mut out, SIMULATION_VERSION);
        push_u32(&mut out, GENERATOR_VERSION);
        push_u32(&mut out, self.width);
        push_u32(&mut out, self.height);
        push_u32(&mut out, self.seed);
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
        push_u16(
            &mut out,
            u16::try_from(self.material_ledger.balances().len())
                .map_err(|_| SnapshotError::CapacityExceeded)?,
        );
        for balance in self.material_ledger.balances() {
            push_u64(&mut out, balance.issued);
            push_u64(&mut out, balance.consumed);
        }
        push_u32(
            &mut out,
            u32::try_from(entity_allocator.len()).map_err(|_| SnapshotError::CapacityExceeded)?,
        );
        out.extend_from_slice(&entity_allocator);
        push_u16(
            &mut out,
            u16::try_from(self.pending_commands.len())
                .map_err(|_| SnapshotError::CapacityExceeded)?,
        );
        for command in &self.pending_commands {
            push_u64(&mut out, command.target_tick);
            push_u32(&mut out, command.source_id);
            push_u64(&mut out, command.sequence);
            push_world_command(&mut out, command.payload);
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
            if let Some(energy) = self.remote_thermal_energy.get(&coord) {
                for value in energy.iter() {
                    push_i32(&mut out, *value);
                }
            } else {
                for _ in 0..CHUNK_AREA {
                    push_i32(&mut out, 0);
                }
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
            u16::try_from(particle_count).map_err(|_| SnapshotError::CapacityExceeded)?,
        );
        for particle in self.particles.iter().take(particle_count) {
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

    /// 入力bytesから候補Worldを全項目検証して復元します。失敗時は呼び出し元のWorldを変更しません。
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
        let seed = reader.u32()?;
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
        let ledger_count = reader.u16()? as usize;
        if ledger_count != registry.len() {
            return Err(SnapshotError::InvalidState);
        }
        let mut material_ledger =
            MaterialLedger::try_new(ledger_count).map_err(|_| SnapshotError::AllocationFailed)?;
        for material_id in 0..ledger_count {
            let issued = reader.u64()?;
            let consumed = reader.u64()?;
            if consumed > issued {
                return Err(SnapshotError::InvalidState);
            }
            material_ledger
                .apply(&[MaterialDelta {
                    material_id: material_id as u16,
                    issue: issued,
                    consume: consumed,
                }])
                .map_err(|_| SnapshotError::InvalidState)?;
        }
        let allocator_size = reader.u32()? as usize;
        let entity_allocator = EntityAllocator::restore(reader.take(allocator_size)?)
            .map_err(|_| SnapshotError::InvalidState)?;
        if entity_allocator.capacity() != WORLD_ENTITY_ID_CAPACITY {
            return Err(SnapshotError::InvalidState);
        }
        let pending_count = reader.u16()? as usize;
        if pending_count > MAX_PENDING_WORLD_COMMANDS {
            return Err(SnapshotError::CapacityExceeded);
        }
        let mut pending_commands = Vec::new();
        pending_commands
            .try_reserve_exact(pending_count)
            .map_err(|_| SnapshotError::AllocationFailed)?;
        let mut pending_keys = HashSet::new();
        pending_keys
            .try_reserve(pending_count)
            .map_err(|_| SnapshotError::AllocationFailed)?;
        for _ in 0..pending_count {
            let command = CommandEnvelope {
                target_tick: reader.u64()?,
                source_id: reader.u32()?,
                sequence: reader.u64()?,
                payload: reader.world_command()?,
            };
            if command.target_tick <= tick
                || command.target_tick > tick.saturating_add(1_000_000)
                || !pending_keys.insert((command.target_tick, command.source_id, command.sequence))
            {
                return Err(SnapshotError::InvalidState);
            }
            pending_commands.push(command);
        }
        if pending_commands.windows(2).any(|pair| {
            (pair[0].target_tick, pair[0].source_id, pair[0].sequence)
                >= (pair[1].target_tick, pair[1].source_id, pair[1].sequence)
        }) {
            return Err(SnapshotError::InvalidState);
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
            let mut energy = Vec::new();
            energy
                .try_reserve_exact(CHUNK_AREA)
                .map_err(|_| SnapshotError::AllocationFailed)?;
            for _ in 0..CHUNK_AREA {
                energy.push(reader.i32()?);
            }
            remote.push((coord, chunk, energy.into_boxed_slice()));
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
        let mut world = Self::try_with_registry_and_seed(width, height, registry, seed)
            .map_err(|_| SnapshotError::CapacityExceeded)?;
        world.tick_count = tick;
        world.thermal_energy = thermal_energy;
        for (index, cell) in cells.into_iter().enumerate() {
            world.cells[index] = cell;
        }
        for (coord, chunk, energy) in remote {
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
            world.remote_thermal_energy.insert(coord, energy);
        }
        world.active_remote_chunks = active;
        world.active_initial_chunks = has_initial_active_set.then_some(active_initial);
        world.chunk_leases = leases;
        world.next_chunk_lease_id = next_lease_id;
        world.particles = particles;
        material_ledger
            .validate_population(&world.population_counts()?)
            .map_err(|_| SnapshotError::InvalidState)?;
        world.material_ledger = material_ledger;
        world.entity_allocator = entity_allocator;
        world.pending_commands = pending_commands;
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

                let group_x = (gx * 4) as i32;
                let group_y = (gy * 4) as i32;
                if !self.initial_chunk_active(group_x, group_y)
                    || self
                        .sleeping_chunks
                        .contains(&ChunkCoord::from_cell(group_x, group_y))
                {
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
                        let x = (idx % width as usize) as i32;
                        let y = (idx / width as usize) as i32;
                        self.cells[idx].set_falling(true);
                        self.wake_chunk_neighborhood(ChunkCoord::from_cell(x, y));
                    }
                }
            }
        }
    }

    /// 単一セルの有限領域参照規則です。remoteとの比較用fixtureだけから呼び出します。
    #[cfg(test)]
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

    #[cfg(test)]
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

    #[cfg(test)]
    /// 指定座標が静止した構造支持固体（壁・床）であるかを判定します。
    fn is_solid_neighbor(&self, x: u32, y: u32) -> bool {
        let cell = self.cells[(y * self.width + x) as usize];
        let behavior = self.registry.get(cell.material).behavior;
        // 静止している固体のみが接続支持として機能する（落下中のセルは支えにならない）
        behavior == Behavior::Solid && !cell.is_falling()
    }

    #[cfg(test)]
    /// 液体（水など）の有限領域参照流動規則。
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

    #[cfg(test)]
    /// 粉体（砂）および構造崩落セル（落下中の土・石）の有限領域参照規則。
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

    #[cfg(test)]
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

    #[cfg(test)]
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
                                // 外部入力で変化したchunkと境界隣接chunkを同時に起こし、
                                // 凍結中に隣接物理を取りこぼさないよう再判定候補にも登録します。
                                self.wake_chunk_neighborhood(ChunkCoord::from_cell(sx, sy));
                                self.wake_chunk_neighborhood(ChunkCoord::from_cell(
                                    target_x, target_y,
                                ));
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
        self.cells
            .get_world(x, y)
            .copied()
            .unwrap_or_else(|| Cell::new(STONE))
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

    /// 有限領域の対象cellが事前確認した材質のままなら除去し、差し替わっていれば変更しません。
    /// 採掘確認と実際の除去の間にWorldが変化した場合、誤ったcellに報酬を出すのを防ぎます。
    pub fn remove_cell_if_material(
        &mut self,
        x: i32,
        y: i32,
        expected_material: u16,
    ) -> Result<bool, WorldError> {
        if x < 0 || x >= self.width as i32 || y < 0 || y >= self.height as i32 {
            return Ok(false);
        }
        if !self.initial_chunk_active(x, y) {
            return Err(WorldError::DomainRejected);
        }
        let index = y as usize * self.width as usize + x as usize;
        if self.cells[index].material != expected_material || expected_material == AIR {
            return Ok(false);
        }
        self.material_ledger
            .apply(&[MaterialDelta {
                material_id: expected_material,
                issue: 0,
                consume: 1,
            }])
            .map_err(map_ledger_world_error)?;
        self.cells[index] = Cell::new(AIR);
        self.thermal_energy[index] = 0;
        self.wake_chunk_neighborhood(ChunkCoord::from_cell(x, y));
        Ok(true)
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
                let mut deltas = Vec::new();
                deltas
                    .try_reserve_exact(candidates as usize * 2)
                    .map_err(|_| CommandError::AllocationFailed)?;
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
                        let previous = self.cells[idx].material;
                        if previous == material {
                            continue;
                        }
                        if !self.initial_chunk_active(x as i32, y as i32) {
                            return Err(CommandError::DomainRejected);
                        }
                        if !self.registry.is_registered(previous) {
                            return Err(CommandError::UnknownMaterial);
                        }
                        if previous != AIR {
                            deltas.push(MaterialDelta {
                                material_id: previous,
                                issue: 0,
                                consume: 1,
                            });
                        }
                        if material != AIR {
                            deltas.push(MaterialDelta {
                                material_id: material,
                                issue: 1,
                                consume: 0,
                            });
                        }
                    }
                }
                deltas.sort_unstable_by_key(|delta| delta.material_id);
                let mut write = 0;
                for read in 0..deltas.len() {
                    let delta = deltas[read];
                    if write > 0 && deltas[write - 1].material_id == delta.material_id {
                        deltas[write - 1].issue += delta.issue;
                        deltas[write - 1].consume += delta.consume;
                    } else {
                        deltas[write] = delta;
                        write += 1;
                    }
                }
                deltas.truncate(write);
                self.apply_material_deltas(&deltas)
                    .map_err(|error| match error {
                        WorldError::AccountingOverflow => CommandError::AccountingOverflow,
                        _ => CommandError::AccountingMismatch,
                    })?;

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
                        if self.cells[idx].material == material {
                            continue;
                        }
                        let shade = (((y % 8) * 8) + (x % 8)) as u8;
                        self.cells[idx] = Cell::with_shade(material, shade);
                        self.thermal_energy[idx] = 0;
                        self.wake_chunk_neighborhood(ChunkCoord::from_cell(x as i32, y as i32));
                        cells_written += 1;
                    }
                }
                Ok(CommandResult {
                    cells_written,
                    domain_id: None,
                })
            }
            WorldCommand::RequestChunkLease {
                source_id,
                policy,
                region,
                mode,
            } => {
                let id = self
                    .request_chunk_lease(policy, source_id, region, mode)
                    .map_err(|_| CommandError::DomainRejected)?;
                Ok(CommandResult {
                    cells_written: 0,
                    domain_id: Some(id),
                })
            }
            WorldCommand::ReleaseChunkLease { lease_id } => {
                if !self.release_chunk_lease(lease_id) {
                    return Err(CommandError::DomainRejected);
                }
                Ok(CommandResult {
                    cells_written: 0,
                    domain_id: None,
                })
            }
            WorldCommand::ApplyVisibleChunkPlan {
                plan,
                resident_limit,
                active_limit,
            } => {
                self.apply_visible_chunk_plan(plan, resident_limit, active_limit)
                    .map_err(|_| CommandError::DomainRejected)?;
                Ok(CommandResult {
                    cells_written: 0,
                    domain_id: None,
                })
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

fn map_ledger_world_error(error: LedgerError) -> WorldError {
    match error {
        LedgerError::AllocationFailed => WorldError::AllocationFailed,
        LedgerError::InvalidMaterial => WorldError::InvalidMaterialState,
        LedgerError::InvalidDeltaOrder => WorldError::InvalidMaterialState,
        LedgerError::Unbalanced => WorldError::AccountingMismatch,
        LedgerError::CounterOverflow => WorldError::AccountingOverflow,
    }
}

fn map_entity_id_error(error: EntityIdError) -> WorldError {
    match error {
        EntityIdError::InvalidCapacity | EntityIdError::InvalidState => {
            WorldError::InvalidEntityIdState
        }
        EntityIdError::CapacityExceeded => WorldError::EntityCapacityExceeded,
        EntityIdError::AllocationFailed => WorldError::AllocationFailed,
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

fn push_world_command(out: &mut Vec<u8>, command: WorldCommand) {
    match command {
        WorldCommand::PaintBrush {
            center_x,
            center_y,
            radius,
            material,
        } => {
            out.push(0);
            push_i32(out, center_x);
            push_i32(out, center_y);
            push_i32(out, radius);
            push_u16(out, material);
        }
        WorldCommand::RequestChunkLease {
            source_id,
            policy,
            region,
            mode,
        } => {
            out.push(1);
            push_u32(out, source_id);
            out.push(u8::from(policy.offscreen_enabled));
            push_u16(out, policy.active_margin_chunks);
            push_u16(out, policy.prefetch_margin_chunks);
            for value in [
                policy.max_active_chunks,
                policy.max_resident_chunks,
                policy.max_pinned_chunks,
                policy.max_pending_requests,
                policy.chunk_memory_budget_bytes,
            ] {
                push_u64(out, value as u64);
            }
            push_chunk_rect(out, region);
            out.push(match mode {
                LoadMode::Prefetch => 0,
                LoadMode::Simulate => 1,
            });
        }
        WorldCommand::ReleaseChunkLease { lease_id } => {
            out.push(2);
            push_u64(out, lease_id);
        }
        WorldCommand::ApplyVisibleChunkPlan {
            plan,
            resident_limit,
            active_limit,
        } => {
            out.push(3);
            push_chunk_rect(out, plan.active);
            push_chunk_rect(out, plan.resident);
            push_u64(out, resident_limit as u64);
            push_u64(out, active_limit as u64);
        }
    }
}

fn push_chunk_rect(out: &mut Vec<u8>, rect: crate::simulation::streaming::ChunkRect) {
    for value in [rect.min.x, rect.min.y, rect.max.x, rect.max.y] {
        push_i32(out, value);
    }
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
    fn world_command(&mut self) -> Result<WorldCommand, SnapshotError> {
        Ok(match self.take(1)?[0] {
            0 => WorldCommand::PaintBrush {
                center_x: self.i32()?,
                center_y: self.i32()?,
                radius: self.i32()?,
                material: self.u16()?,
            },
            1 => {
                let source_id = self.u32()?;
                let offscreen_enabled = match self.take(1)?[0] {
                    0 => false,
                    1 => true,
                    _ => return Err(SnapshotError::InvalidState),
                };
                let active_margin_chunks = self.u16()?;
                let prefetch_margin_chunks = self.u16()?;
                let mut limits = [0_usize; 5];
                for limit in &mut limits {
                    *limit = usize::try_from(self.u64()?)
                        .map_err(|_| SnapshotError::CapacityExceeded)?;
                }
                let region = self.chunk_rect()?;
                let mode = match self.take(1)?[0] {
                    0 => LoadMode::Prefetch,
                    1 => LoadMode::Simulate,
                    _ => return Err(SnapshotError::InvalidState),
                };
                WorldCommand::RequestChunkLease {
                    source_id,
                    policy: StreamingPolicy {
                        offscreen_enabled,
                        active_margin_chunks,
                        prefetch_margin_chunks,
                        max_active_chunks: limits[0],
                        max_resident_chunks: limits[1],
                        max_pinned_chunks: limits[2],
                        max_pending_requests: limits[3],
                        chunk_memory_budget_bytes: limits[4],
                    },
                    region,
                    mode,
                }
            }
            2 => WorldCommand::ReleaseChunkLease {
                lease_id: self.u64()?,
            },
            3 => WorldCommand::ApplyVisibleChunkPlan {
                plan: crate::simulation::streaming::VisibleChunkPlan {
                    active: self.chunk_rect()?,
                    resident: self.chunk_rect()?,
                },
                resident_limit: usize::try_from(self.u64()?)
                    .map_err(|_| SnapshotError::CapacityExceeded)?,
                active_limit: usize::try_from(self.u64()?)
                    .map_err(|_| SnapshotError::CapacityExceeded)?,
            },
            _ => return Err(SnapshotError::InvalidFormat),
        })
    }
    fn chunk_rect(&mut self) -> Result<crate::simulation::streaming::ChunkRect, SnapshotError> {
        crate::simulation::streaming::ChunkRect::new(
            ChunkCoord {
                x: self.i32()?,
                y: self.i32()?,
            },
            ChunkCoord {
                x: self.i32()?,
                y: self.i32()?,
            },
        )
        .map_err(|_| SnapshotError::InvalidState)
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
    use crate::simulation::material::SAND;

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
    fn brush_and_mining_update_material_ledger_atomically() {
        let mut world = World::new(16, 16).unwrap();
        world
            .apply_command(WorldCommand::PaintBrush {
                center_x: 8,
                center_y: 8,
                radius: 1,
                material: WATER,
            })
            .unwrap();
        assert_eq!(
            world.material_balance(WATER),
            Some(MaterialBalance {
                issued: 5,
                consumed: 0
            })
        );
        world
            .apply_command(WorldCommand::PaintBrush {
                center_x: 8,
                center_y: 8,
                radius: 1,
                material: SAND,
            })
            .unwrap();
        assert_eq!(
            world.material_balance(WATER),
            Some(MaterialBalance {
                issued: 5,
                consumed: 5
            })
        );
        assert!(world.remove_cell_if_material(8, 8, SAND).unwrap());
        assert_eq!(
            world.material_balance(SAND),
            Some(MaterialBalance {
                issued: 5,
                consumed: 1
            })
        );
        let population = world.population_counts().unwrap();
        world
            .material_ledger
            .validate_population(&population)
            .unwrap();
    }

    #[test]
    fn water_sand_and_dig_use_validated_world_commands() {
        let mut world = World::new(32, 32).unwrap();
        for (x, material) in [(3, WATER), (12, SAND)] {
            world
                .apply_command(WorldCommand::PaintBrush {
                    center_x: x,
                    center_y: 0,
                    radius: 0,
                    material,
                })
                .unwrap();
        }
        assert_eq!(world.get_cell(3, 0).material, WATER);
        assert_eq!(world.get_cell(12, 0).material, SAND);
        world
            .apply_command(WorldCommand::PaintBrush {
                center_x: 3,
                center_y: 0,
                radius: 0,
                material: AIR,
            })
            .unwrap();
        assert_eq!(world.get_cell(3, 0).material, AIR);
        assert_eq!(world.material_balance(WATER).unwrap().issued, 1);
        assert_eq!(world.material_balance(WATER).unwrap().consumed, 1);
        assert_eq!(world.material_balance(SAND).unwrap().issued, 1);
        assert_eq!(world.material_balance(SAND).unwrap().consumed, 0);
    }

    #[test]
    fn philox_seeded_small_field_preserves_material_population_over_ticks() {
        let mut world = World::new(32, 32).unwrap();
        for sample in 0..256_u32 {
            let x = (sample % 32) as i32;
            let y = (sample / 32) as i32;
            let value =
                crate::simulation::determinism::sample_u32(0x00C0_FFEE, 0, x, y, 0x4D31, sample);
            let material = if value & 1 == 0 { WATER } else { SAND };
            world
                .apply_command(WorldCommand::PaintBrush {
                    center_x: x,
                    center_y: y,
                    radius: 0,
                    material,
                })
                .unwrap();
        }
        let balances = [
            world.material_balance(WATER).unwrap(),
            world.material_balance(SAND).unwrap(),
        ];
        for _ in 0..30 {
            world.update().unwrap();
        }
        assert_eq!(world.material_balance(WATER).unwrap(), balances[0]);
        assert_eq!(world.material_balance(SAND).unwrap(), balances[1]);
        let checkpoint = world.encode_snapshot().unwrap();
        World::decode_snapshot(&checkpoint, world.registry.clone()).unwrap();
    }

    #[test]
    fn ledger_overflow_rejects_brush_without_partial_world_change() {
        let mut world = World::new(16, 16).unwrap();
        world.material_ledger.set_balance_for_test(
            WATER,
            MaterialBalance {
                issued: u64::MAX,
                consumed: 0,
            },
        );
        let before = world.state_fingerprint();
        assert_eq!(
            world.apply_command(WorldCommand::PaintBrush {
                center_x: 8,
                center_y: 8,
                radius: 0,
                material: WATER,
            }),
            Err(CommandError::AccountingOverflow)
        );
        assert_eq!(world.state_fingerprint(), before);
        assert_eq!(world.get_cell(8, 8).material, AIR);
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
    fn snapshot_roundtrip_preserves_entity_allocator_and_rejects_stale_ids() {
        let mut world = World::new(16, 16).unwrap();
        let first = world.reserve_entity_id().unwrap();
        let second = world.reserve_entity_id().unwrap();
        assert!(world.release_entity_id(first).unwrap());
        let bytes = world.encode_snapshot().unwrap();
        let mut restored = World::decode_snapshot(&bytes, world.registry.clone()).unwrap();
        assert!(!restored.contains_entity_id(first));
        assert!(restored.contains_entity_id(second));
        let replacement = restored.reserve_entity_id().unwrap();
        assert_eq!(replacement.index, first.index);
        assert_ne!(replacement.generation, first.generation);
        assert_eq!(restored.state_fingerprint(), {
            let mut expected = world.clone();
            expected.reserve_entity_id().unwrap();
            expected.state_fingerprint()
        });
    }

    #[test]
    fn future_commands_are_sorted_bounded_and_survive_checkpoint() {
        let mut world = World::new(16, 16).unwrap();
        let command = |source_id, x, material| CommandEnvelope {
            target_tick: 2,
            source_id,
            sequence: 0,
            payload: WorldCommand::PaintBrush {
                center_x: x,
                center_y: 0,
                radius: 0,
                material,
            },
        };
        let later = command(2, 2, WATER);
        let earlier = command(1, 1, SAND);
        world.submit_command(later).unwrap();
        world.submit_command(earlier).unwrap();
        assert_eq!(
            world.submit_command(earlier),
            Err(CommandQueueError::DuplicateSourceSequence)
        );
        assert_eq!(world.dispatch_pending_commands().unwrap(), Vec::new());
        let checkpoint = world.encode_snapshot().unwrap();
        let mut world =
            World::decode_snapshot(&checkpoint, MaterialRegistry::with_defaults()).unwrap();
        assert_eq!(world.pending_command_count(), 2);
        world.update().unwrap();
        let events = world.dispatch_pending_commands().unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| event.source_id)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(world.get_cell(1, 0).material, SAND);
        assert_eq!(world.get_cell(2, 0).material, WATER);
        assert_eq!(world.pending_command_count(), 0);
        assert_eq!(
            world.submit_command(CommandEnvelope {
                target_tick: 1,
                source_id: 3,
                sequence: 0,
                payload: WorldCommand::PaintBrush {
                    center_x: 3,
                    center_y: 0,
                    radius: 0,
                    material: SAND,
                },
            }),
            Err(CommandQueueError::WrongTick)
        );
    }

    #[test]
    fn unified_world_coordinate_rules_match_the_finite_reference_inside_one_chunk() {
        let mut reference = World::new(24, 24).unwrap();
        let mut unified = reference.clone();
        for world in [&mut reference, &mut unified] {
            *world.cells.get_world_mut(10, 4).unwrap() = Cell::new(SAND);
            *world.cells.get_world_mut(8, 7).unwrap() = Cell::new(WATER);
            *world.cells.get_world_mut(9, 8).unwrap() = Cell::new(STONE);
            *world.cells.get_world_mut(10, 8).unwrap() = Cell::new(STONE);
            assert!(world.set_thermal_energy(10, 4, 420));
            assert!(world.set_thermal_energy(8, 7, 180));
            world.tick_count = 1;
            world.check_structural_collapse();
        }

        // 同じworld座標順・tickで旧finite参照規則と共通chunk規則を並走させます。
        // sceneはchunk継ぎ目とworld外から十分離し、比較対象の局所物理だけを測ります。
        for y in (0..24_i32).rev() {
            for x in 0..24_i32 {
                reference.update_cell(x as u32, y as u32);
                unified.update_world_cell(x, y);
            }
        }
        for y in 0..24_i32 {
            for x in 0..24_i32 {
                assert_eq!(
                    reference.cells.get_world(x, y),
                    unified.cells.get_world(x, y),
                    "共通cell規則がfinite参照から逸脱しました: ({x}, {y})"
                );
                assert_eq!(
                    reference.world_thermal_energy(x, y),
                    unified.world_thermal_energy(x, y),
                    "material移動時に熱sidecarの結果が参照と異なります: ({x}, {y})"
                );
            }
        }
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
            .set_remote_cell_counted(-65, 130, Cell::with_shade(WATER, 7))
            .unwrap();
        let restored = World::decode_snapshot(
            &world.encode_snapshot().unwrap(),
            MaterialRegistry::with_defaults(),
        )
        .unwrap();
        assert_eq!(restored.cells.get_world(-65, 130).unwrap().material, WATER);
        assert_eq!(restored.state_fingerprint(), world.state_fingerprint());
    }

    #[test]
    fn remote_chunks_continue_the_seeded_finite_terrain_and_snapshot_the_seed() {
        let seed = 8_314_159;
        let finite = World::try_new_with_seed(192, 384, seed).unwrap();
        let mut streamed = World::try_new_with_seed(64, 64, seed).unwrap();
        let region = ChunkRect::new(ChunkCoord { x: 1, y: 2 }, ChunkCoord { x: 2, y: 3 }).unwrap();
        let plan = StreamingPolicy::default()
            .plan_request(region, LoadMode::Prefetch)
            .unwrap();
        streamed.request_remote_chunks(plan, 512, 256).unwrap();

        for y in 128..192 {
            for x in 64..128 {
                assert_eq!(
                    streamed.query_cell(x, y),
                    CellQuery::Available(finite.get_cell(x, y)),
                    "remote生成が絶対座標の有限terrainと一致しない: ({x}, {y})"
                );
            }
        }
        streamed
            .material_ledger
            .validate_population(&streamed.population_counts().unwrap())
            .unwrap();

        let snapshot = streamed.encode_snapshot().unwrap();
        let restored =
            World::decode_snapshot(&snapshot, MaterialRegistry::with_defaults()).unwrap();
        assert_eq!(restored.state_fingerprint(), streamed.state_fingerprint());
        assert_eq!(restored.seed, seed);
    }

    #[test]
    fn remote_thermal_sidecar_roundtrips_and_inactive_visual_particles_freeze() {
        let mut world = World::new(64, 64).unwrap();
        let remote = ChunkRect::new(ChunkCoord { x: -1, y: 0 }, ChunkCoord { x: 0, y: 1 }).unwrap();
        let prefetch = StreamingPolicy::default()
            .plan_request(remote, LoadMode::Simulate)
            .unwrap();
        world.request_remote_chunks(prefetch, 512, 256).unwrap();
        assert!(world.set_world_thermal_energy(-1, 3, 917));
        world
            .apply_visible_chunk_plan(
                crate::simulation::streaming::VisibleChunkPlan {
                    active: ChunkRect::new(ChunkCoord { x: 0, y: 0 }, ChunkCoord { x: 1, y: 1 })
                        .unwrap(),
                    resident: ChunkRect::new(ChunkCoord { x: -1, y: 0 }, ChunkCoord { x: 1, y: 1 })
                        .unwrap(),
                },
                512,
                256,
            )
            .unwrap();
        world.add_particle(SplashParticle::new(-1.0, 3.0, 0.0, 0.0, 2.0, [0; 4]));
        world.update().unwrap();
        assert_eq!(world.particles[0].life, 2.0);

        let mut restored = World::decode_snapshot(
            &world.encode_snapshot().unwrap(),
            MaterialRegistry::with_defaults(),
        )
        .unwrap();
        assert_eq!(restored.world_thermal_energy(-1, 3), Some(917));
        assert_eq!(restored.state_fingerprint(), world.state_fingerprint());
        let plan = crate::simulation::streaming::VisibleChunkPlan {
            active: remote,
            resident: ChunkRect::new(ChunkCoord { x: -1, y: 0 }, ChunkCoord { x: 1, y: 1 })
                .unwrap(),
        };
        for candidate in [&mut world, &mut restored] {
            candidate.apply_visible_chunk_plan(plan, 512, 256).unwrap();
            candidate.update().unwrap();
            assert!(candidate.particles[0].life < 2.0);
        }
        assert_eq!(world.particles[0].life, restored.particles[0].life);
        assert_eq!(world.state_fingerprint(), restored.state_fingerprint());
    }

    #[test]
    fn powder_crosses_a_finite_to_remote_chunk_edge_in_the_same_tick() {
        let mut world = World::new(64, 64).unwrap();
        let below = ChunkRect::new(ChunkCoord { x: 0, y: 1 }, ChunkCoord { x: 1, y: 2 }).unwrap();
        let request = StreamingPolicy::default()
            .plan_request(below, LoadMode::Simulate)
            .unwrap();
        world.request_remote_chunks(request, 512, 256).unwrap();
        world
            .apply_command(WorldCommand::PaintBrush {
                center_x: 5,
                center_y: 63,
                radius: 0,
                material: SAND,
            })
            .unwrap();

        world.update().unwrap();
        assert_eq!(
            world.query_cell(5, 63),
            CellQuery::Available(Cell::new(AIR))
        );
        assert!(matches!(
            world.query_cell(5, 64),
            CellQuery::Available(Cell { material: SAND, .. })
        ));
    }

    #[test]
    fn powder_and_energy_cross_a_remote_to_finite_chunk_edge_together() {
        let mut world = World::new(64, 64).unwrap();
        let above = ChunkRect::new(ChunkCoord { x: 0, y: -1 }, ChunkCoord { x: 1, y: 0 }).unwrap();
        let request = StreamingPolicy::default()
            .plan_request(above, LoadMode::Simulate)
            .unwrap();
        world.request_remote_chunks(request, 512, 256).unwrap();
        world.cells.set_remote_cell(5, -1, Cell::new(SAND));
        assert!(world.set_world_thermal_energy(5, -1, 321));

        world.update().unwrap();
        assert!(matches!(
            world.query_cell(5, 0),
            CellQuery::Available(Cell { material: SAND, .. })
        ));
        let finite_energy = world
            .thermal_energy
            .iter()
            .map(|value| i64::from(*value))
            .sum::<i64>();
        let remote_energy = world
            .remote_thermal_energy
            .values()
            .flat_map(|chunk| chunk.iter())
            .map(|value| i64::from(*value))
            .sum::<i64>();
        assert_eq!(finite_energy + remote_energy, 321);
        assert!(finite_energy > 0);
        assert!(matches!(
            world.query_cell(5, -1),
            CellQuery::Available(Cell { material: AIR, .. })
        ));
    }

    #[test]
    fn liquid_push_transaction_can_span_a_chunk_boundary() {
        let mut world = World::new(64, 64).unwrap();
        let region = ChunkRect::new(ChunkCoord { x: 1, y: 0 }, ChunkCoord { x: 2, y: 1 }).unwrap();
        world
            .request_remote_chunks(
                StreamingPolicy::default()
                    .plan_request(region, LoadMode::Simulate)
                    .unwrap(),
                512,
                256,
            )
            .unwrap();
        for y in 8..12 {
            for x in 60..68 {
                *world.cells.get_world_mut(x, y).unwrap() = Cell::new(STONE);
            }
        }
        *world.cells.get_world_mut(63, 9).unwrap() = Cell::new(WATER);
        *world.cells.get_world_mut(64, 9).unwrap() = Cell::new(SAND);
        *world.cells.get_world_mut(65, 9).unwrap() = Cell::new(AIR);
        assert!(world.touches_active_remote_chunk_boundary(63, 9));
        assert!(world.world_cell_active(64, 9));
        assert!(world.registry.get(SAND).can_be_washed_away);

        world.update().unwrap();
        for (coordinate, material) in [((63, 9), AIR), ((64, 9), WATER), ((65, 9), SAND)] {
            assert!(
                matches!(
                    world.query_cell(coordinate.0, coordinate.1),
                    CellQuery::Available(Cell { material: found, .. }) if found == material
                ),
                "cell {:?} was {:?}, expected material {}",
                coordinate,
                world.query_cell(coordinate.0, coordinate.1),
                material
            );
        }
    }

    #[test]
    fn particles_in_inactive_finite_chunks_do_not_advance() {
        let mut world = World::new(128, 64).unwrap();
        world
            .apply_visible_chunk_plan(
                crate::simulation::streaming::VisibleChunkPlan {
                    active: ChunkRect::new(ChunkCoord { x: 0, y: 0 }, ChunkCoord { x: 1, y: 1 })
                        .unwrap(),
                    resident: ChunkRect::new(ChunkCoord { x: 0, y: 0 }, ChunkCoord { x: 2, y: 1 })
                        .unwrap(),
                },
                512,
                256,
            )
            .unwrap();
        world.add_particle(SplashParticle::new(65.0, 4.0, 0.0, 0.0, 2.0, [0; 4]));
        world.update().unwrap();
        assert_eq!(world.particles[0].life, 2.0);
    }

    #[test]
    fn frozen_quiescent_chunk_matches_full_scan_and_rejoins_on_wake() {
        let mut full_scan = World::new(128, 64).unwrap();
        for y in 0..64 {
            for x in 64..128 {
                *full_scan.cells.get_world_mut(x, y).unwrap() = Cell::new(STONE);
            }
        }
        let mut frozen = full_scan.clone();
        let resident =
            ChunkRect::new(ChunkCoord { x: 0, y: 0 }, ChunkCoord { x: 2, y: 1 }).unwrap();
        frozen
            .apply_visible_chunk_plan(
                crate::simulation::streaming::VisibleChunkPlan {
                    active: resident,
                    resident,
                },
                512,
                256,
            )
            .unwrap();
        for _ in 0..8 {
            full_scan.update().unwrap();
            frozen.update().unwrap();
        }
        assert!(frozen.sleeping_chunks.contains(&ChunkCoord { x: 1, y: 0 }));
        for y in 0..64_i32 {
            for x in 64..128_i32 {
                assert_eq!(
                    frozen.cells.get_world(x, y),
                    full_scan.cells.get_world(x, y)
                );
                assert_eq!(
                    frozen.world_thermal_energy(x, y),
                    full_scan.world_thermal_energy(x, y)
                );
            }
        }

        frozen
            .apply_visible_chunk_plan(
                crate::simulation::streaming::VisibleChunkPlan {
                    active: resident,
                    resident,
                },
                512,
                256,
            )
            .unwrap();
        for _ in 0..4 {
            full_scan.update().unwrap();
            frozen.update().unwrap();
        }
        for y in 0..64_i32 {
            for x in 0..128_i32 {
                assert_eq!(
                    frozen.cells.get_world(x, y),
                    full_scan.cells.get_world(x, y)
                );
                assert_eq!(
                    frozen.world_thermal_energy(x, y),
                    full_scan.world_thermal_energy(x, y)
                );
            }
        }
    }

    #[test]
    fn inactive_finite_chunk_insulates_thermal_energy_until_reactivated() {
        let mut world = World::new(128, 64).unwrap();
        assert!(world.set_thermal_energy(63, 8, 800));
        assert!(world.set_thermal_energy(64, 8, 100));
        let plan = crate::simulation::streaming::VisibleChunkPlan {
            active: ChunkRect::new(ChunkCoord { x: 0, y: 0 }, ChunkCoord { x: 1, y: 1 }).unwrap(),
            resident: ChunkRect::new(ChunkCoord { x: 0, y: 0 }, ChunkCoord { x: 2, y: 1 }).unwrap(),
        };
        world.apply_visible_chunk_plan(plan, 512, 256).unwrap();
        world.update().unwrap();
        assert_eq!(world.thermal_energy(64, 8), Some(100));
        assert_eq!(world.thermal_energy(63, 8), Some(500));
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
        world
            .set_remote_cell_counted(130, 130, Cell::new(SAND))
            .unwrap();
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
        assert_eq!(
            world.material_balance(SAND),
            Some(MaterialBalance {
                issued: 1,
                consumed: 0
            })
        );

        let remote_coordinate = ChunkCoord { x: 2, y: 2 };
        assert_eq!(
            world.remove_remote_chunk(remote_coordinate),
            Err(WorldError::ChunkIsActive)
        );
        world.deactivate_remote_chunks(&[remote_coordinate]);
        assert!(world.remove_remote_chunk(remote_coordinate).unwrap());
        assert_eq!(world.material_balance(SAND).unwrap().consumed, 1);

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
    fn prefetched_lease_prevents_destructive_chunk_removal_until_release() {
        let mut world = World::new(64, 64).unwrap();
        let policy = StreamingPolicy::default();
        let region = ChunkRect::new(ChunkCoord { x: 3, y: 4 }, ChunkCoord { x: 4, y: 5 }).unwrap();
        let lease = world
            .request_chunk_lease(policy, 9, region, LoadMode::Prefetch)
            .unwrap();
        let coordinate = ChunkCoord { x: 3, y: 4 };
        assert_eq!(
            world.remove_remote_chunk(coordinate),
            Err(WorldError::ChunkIsLeased)
        );
        assert!(world.release_chunk_lease(lease));
        assert!(world.remove_remote_chunk(coordinate).unwrap());
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
    fn queued_remote_activation_waits_for_its_tick_and_survives_save_restore() {
        let policy = StreamingPolicy::default();
        let region =
            ChunkRect::new(ChunkCoord { x: 10, y: 4 }, ChunkCoord { x: 11, y: 5 }).unwrap();
        let envelope = CommandEnvelope {
            target_tick: 3,
            source_id: 17,
            sequence: 0,
            payload: WorldCommand::RequestChunkLease {
                source_id: 17,
                policy,
                region,
                mode: LoadMode::Simulate,
            },
        };
        let mut world = World::new(64, 64).unwrap();
        world.submit_command(envelope).unwrap();
        assert_eq!(world.query_cell(640, 256), CellQuery::Unavailable);
        for _ in 0..2 {
            assert!(world.dispatch_pending_commands().unwrap().is_empty());
            world.update().unwrap();
        }
        assert_eq!(world.query_cell(640, 256), CellQuery::Unavailable);

        let saved = world.encode_snapshot().unwrap();
        let mut restored =
            World::decode_snapshot(&saved, MaterialRegistry::with_defaults()).unwrap();
        for candidate in [&mut world, &mut restored] {
            let events = candidate.dispatch_pending_commands().unwrap();
            assert_eq!(events.len(), 1);
            assert!(events[0].result.is_ok());
            assert!(matches!(
                candidate.query_cell(640, 256),
                CellQuery::Available(_)
            ));
            assert!(
                candidate
                    .active_remote_chunks()
                    .any(|coord| coord == ChunkCoord { x: 10, y: 4 })
            );
            candidate.update().unwrap();
        }
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
