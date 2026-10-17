//! 高速物質の広域整数DDAを、保存可能な固定予算Jobへ分割します。
//!
//! 複数chunkをまたぐ検査の途中ではWorldを変更せず、確認済み経路と各chunkの
//! revisionをjob内に保存します。Readyになってから地形と全物質channelを
//! 既存transport_pathで原子的に輸送します。

use super::{
    CellCore, CellQuery, ChannelQuery, ChunkCoord, LayerId, MaterialId, MaterialMotion, World,
    WorldEdit, conflict, invalid,
};
use crate::foundation::{
    EngineError, EngineResult, StableKey,
    codec::{Decoder, Encoder, StateCodec},
    error::allocation,
    grid::{CellPoint, GridValue},
    id::WorldId,
};

const SCALE: i128 = 65536;

/// 保存可能な広域移動の検査段階。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaterialMotionJobStatus {
    /// まだ目標移動量の経路検査を終えていません。
    Computing,
    /// 入力revisionが変わっていなければ、原子取引で公開できます。
    Ready,
}

/// 分割DDAの完了時の診断。外部入力の失効と処理途中の未公開状態を区別します。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MaterialMotionJobReport {
    /// 走査した格子境界数。
    pub checked_steps: u64,
    /// 輸送する整数cell経路の境界数。
    pub crossed_cells: usize,
    /// 障害物・World端で止まった場合。
    pub collision: bool,
    /// 未読込・非活動領域により先へ進めず延期した場合。
    pub waiting_for_region: bool,
}

/// 正規状態へ保存できる、特定cellに所属する広域DDA進捗。
///
/// 他の物質との交換・複数候補の競合は扱わず、単一トークンの空cell経路を
/// 原子移動する。複数Jobを同時公開するときは呼出側の順序と失効を確認する。
/// 限定的な仕事量を同じ呼出で終了したと偽らず、budget一単位を一交差で数える。
#[derive(Clone, Debug)]
pub struct MaterialMotionJob {
    world: WorldId,
    content: [u8; 32],
    epoch: u64,
    layer: StableKey,
    channel: StableKey,
    source: CellPoint,
    source_core: CellCore,
    before: MaterialMotion,
    after: MaterialMotion,
    counts: [u64; 2],
    signs: [i32; 2],
    progressed: [u64; 2],
    path_limit: usize,
    path: Vec<CellPoint>,
    chunks: Vec<(ChunkCoord, u64)>,
    finished: bool,
    blocked: u8,
    checked_steps: u64,
}

impl MaterialMotionJob {
    /// 1体の高速物質を検査するJobを生成します。
    ///
    /// 活動chunk集合とrevisionを採取し、全候補の一括読込はしません。
    /// `step`と`publish`は同じ活動集合を保守的に照合します。
    pub fn new(
        world: &World,
        layer: LayerId,
        source: CellPoint,
        channel: StableKey,
        max_distance: u32,
        max_path_cells: usize,
        max_active_chunks: usize,
    ) -> EngineResult<Self> {
        if max_distance == 0 || max_path_cells == 0 {
            return Err(invalid("広域移動の移動距離・経路容量"));
        }
        let layer_key = world.content.layer(layer)?.key;
        if world.channel_material_layer::<MaterialMotion>(channel)? != Some(layer_key) {
            return Err(invalid("広域移動channelの所有layer"));
        }
        let chunk = world
            .chunks
            .get(&ChunkCoord::at(source))
            .ok_or_else(|| invalid("広域移動の未読込source"))?;
        if !chunk.active {
            return Err(conflict("広域移動の非活動source"));
        }
        let cell = match world.cell(layer, source)? {
            CellQuery::Loaded(cell)
                if cell.core.material != MaterialId::AIR && cell.block.is_none() =>
            {
                cell
            }
            _ => return Err(conflict("広域移動の空cell・block source")),
        };
        let origin = world.channel_layout::<MaterialMotion>(channel)?.origin;
        let coord = channel_point(source, origin)?;
        let before = match world.channel_value::<MaterialMotion>(channel, coord)? {
            ChannelQuery::Loaded(value) => value,
            _ => return Err(invalid("広域移動channel coverage")),
        };
        before.validate()?;
        let mut after = before;
        let mut counts = [0u64; 2];
        let mut signs = [0i32; 2];
        for axis in 0..2 {
            let q16 =
                i128::from(before.velocity_q16[axis]) + i128::from(before.remainder_q16[axis]);
            let integral = q16 / SCALE;
            if integral.unsigned_abs() > u128::from(max_distance) {
                return Err(EngineError::capacity(
                    "広域移動の軸距離",
                    integral.unsigned_abs() as u64,
                    max_distance as u64,
                ));
            }
            counts[axis] = integral.unsigned_abs() as u64;
            signs[axis] = integral.signum() as i32;
            after.remainder_q16[axis] = (q16 % SCALE) as i32;
        }
        let mut chunks = Vec::new();
        for (position, chunk) in &world.chunks {
            if !chunk.active {
                continue;
            }
            if chunks.len() == max_active_chunks {
                return Err(EngineError::capacity(
                    "広域移動の活動chunk記録数",
                    chunks.len() as u64 + 1,
                    max_active_chunks as u64,
                ));
            }
            chunks.try_reserve(1).map_err(allocation)?;
            chunks.push((*position, chunk.revision));
        }
        let mut path = Vec::new();
        path.try_reserve(1).map_err(allocation)?;
        path.push(source);
        let value = Self {
            world: world.id,
            content: world.content.fingerprint(),
            epoch: world.remaps.epoch(),
            layer: layer_key,
            channel,
            source,
            source_core: cell.core,
            before,
            after,
            counts,
            signs,
            progressed: [0; 2],
            path_limit: max_path_cells,
            path,
            chunks,
            finished: counts == [0, 0],
            blocked: 0,
            checked_steps: 0,
        };
        value.validate_progress()?;
        Ok(value)
    }

    /// 検査途中の保存・復元をしても同じ順序でReadyへ進みます。
    pub fn status(&self) -> MaterialMotionJobStatus {
        if self.finished {
            MaterialMotionJobStatus::Ready
        } else {
            MaterialMotionJobStatus::Computing
        }
    }
    /// DDA検査の進捗・移動距離・障害の診断。
    pub fn report(&self) -> MaterialMotionJobReport {
        MaterialMotionJobReport {
            checked_steps: self.checked_steps,
            crossed_cells: self.path.len() - 1,
            collision: self.blocked == 1,
            waiting_for_region: self.blocked == 2,
        }
    }
    /// 一交差を一budgetとし、Worldを変更せずに整数DDAを進めます。
    pub fn step(&mut self, world: &World, budget: usize) -> EngineResult<MaterialMotionJobStatus> {
        self.validate_context(world)?;
        for _ in 0..budget {
            if self.finished {
                break;
            }
            self.step_one(world)?;
        }
        Ok(self.status())
    }
    fn step_one(&mut self, world: &World) -> EngineResult<()> {
        let p = self.progressed;
        let horizontal = u128::from(2 * p[0] + 1) * u128::from(self.counts[1]);
        let vertical = u128::from(2 * p[1] + 1) * u128::from(self.counts[0]);
        let move_x = p[0] < self.counts[0] && (p[1] == self.counts[1] || horizontal <= vertical);
        let move_y = p[1] < self.counts[1] && (p[0] == self.counts[0] || vertical <= horizontal);
        if !move_x && !move_y {
            return Err(invalid("広域DDAの進捗が停止"));
        }
        let current = *self.path.last().expect("経路に始点あり");
        let dx = if move_x { self.signs[0] } else { 0 };
        let dy = if move_y { self.signs[1] } else { 0 };
        let target = offset(current, dx, dy)?;
        let mut points = [None; 3];
        if move_x && move_y {
            points[0] = Some(offset(current, dx, 0)?);
            points[1] = Some(offset(current, 0, dy)?);
        }
        points[2] = Some(target);
        for check in points.into_iter().flatten() {
            match world.cell(world.content.layer_id(self.layer)?, check)? {
                CellQuery::WorldBoundary => {
                    self.blocked = 1;
                    self.after = MaterialMotion::default();
                    self.finished = true;
                    self.checked_steps += 1;
                    return Ok(());
                }
                CellQuery::Loaded(cell) => {
                    if !world
                        .chunks
                        .get(&ChunkCoord::at(check))
                        .is_some_and(|chunk| chunk.active)
                    {
                        self.blocked = 2;
                        self.finished = true;
                        self.checked_steps += 1;
                        return Ok(());
                    }
                    if cell.block.is_some() || cell.core.material != MaterialId::AIR {
                        self.blocked = 1;
                        self.after = MaterialMotion::default();
                        self.finished = true;
                        self.checked_steps += 1;
                        return Ok(());
                    }
                }
                _ => {
                    self.blocked = 2;
                    self.finished = true;
                    self.checked_steps += 1;
                    return Ok(());
                }
            }
        }
        if self.path.len() == self.path_limit {
            return Err(EngineError::capacity(
                "広域DDAの経路記録数",
                self.path.len() as u64 + 1,
                self.path_limit as u64,
            ));
        }
        self.path.try_reserve(1).map_err(allocation)?;
        self.path.push(target);
        self.progressed[0] += u64::from(move_x);
        self.progressed[1] += u64::from(move_y);
        self.checked_steps += 1;
        self.finished = self.progressed == self.counts;
        Ok(())
    }

    /// Readyの経路を一つのWorld取引で公開します。途中状態は一切書き込みません。
    pub fn publish(&self, editor: &mut WorldEdit<'_>) -> EngineResult<MaterialMotionJobReport> {
        editor.run(|edit| {
            // 全経路の構造検証は計算の途中では繰り返さず、確定入口で一度だけ行う。
            self.validate_progress()?;
            self.validate_context(edit.world)?;
            if !self.finished {
                return Err(conflict("広域移動Jobが未完成"));
            }
            let layer = edit.world.content.layer_id(self.layer)?;
            if self.path.len() > 1 {
                let mut reversed = Vec::new();
                reversed
                    .try_reserve_exact(self.path.len())
                    .map_err(allocation)?;
                reversed.extend(self.path.iter().rev().copied());
                edit.transport_path(layer, &reversed)?;
            }
            if self.after != self.before {
                let origin = edit
                    .world
                    .channel_layout::<MaterialMotion>(self.channel)?
                    .origin;
                let target = *self.path.last().expect("始点を含む経路");
                edit.set_channel(self.channel, channel_point(target, origin)?, self.after)?;
            }
            Ok(self.report())
        })
    }

    fn validate_context(&self, world: &World) -> EngineResult<()> {
        // progress自体はnew/復元の際に一度検証済みであり、privateなstepしか
        // 変更しません。毎stepにpath全体を再走査するとbudget=1でもO(N²)
        // になるので、ここではWorldの入力失効のみ確認します。
        if world.id != self.world
            || world.content.fingerprint() != self.content
            || world.remaps.epoch() != self.epoch
            || world.content.layer_id(self.layer).is_err()
            || world.channel_material_layer::<MaterialMotion>(self.channel)? != Some(self.layer)
        {
            return Err(conflict("広域移動JobのWorld・Content・epoch・channel"));
        }
        let active_count = world.chunks.values().filter(|chunk| chunk.active).count();
        if active_count != self.chunks.len() {
            return Err(conflict("広域移動Jobの活動集合が変化"));
        }
        for &(coord, revision) in &self.chunks {
            if !world
                .chunks
                .get(&coord)
                .is_some_and(|chunk| chunk.active && chunk.revision == revision)
            {
                return Err(conflict("広域移動Jobの入力revisionが失効"));
            }
        }
        let layer = world.content.layer_id(self.layer)?;
        let cell = match world.cell(layer, self.source)? {
            CellQuery::Loaded(cell) => cell,
            _ => return Err(conflict("広域移動Jobの始点coverage")),
        };
        if cell.block.is_some() || cell.core != self.source_core {
            return Err(conflict("広域移動Jobの始点が変更"));
        }
        let origin = world.channel_layout::<MaterialMotion>(self.channel)?.origin;
        if !matches!(
            world.channel_value::<MaterialMotion>(self.channel, channel_point(self.source, origin)?)?,
            ChannelQuery::Loaded(value) if value == self.before
        ) {
            return Err(conflict("広域移動Jobの速度入力が変更"));
        }
        Ok(())
    }
    fn validate_progress(&self) -> EngineResult<()> {
        self.before.validate()?;
        self.after.validate()?;
        if self.path_limit == 0
            || self.path.len() > self.path_limit
            || self.path.first() != Some(&self.source)
            || self.path.is_empty()
            || self.blocked > 2
            || self.progressed[0] > self.counts[0]
            || self.progressed[1] > self.counts[1]
            || self.signs.iter().any(|s| !(-1..=1).contains(s))
            || (self.finished && self.blocked == 0 && self.progressed != self.counts)
            || (!self.finished && (self.blocked != 0 || self.progressed == self.counts))
            || self.chunks.windows(2).any(|pair| pair[0].0 >= pair[1].0)
        {
            return Err(invalid("広域移動Jobの保存進捗"));
        }
        let mut crossed = [0u64; 2];
        for pair in self.path.windows(2) {
            let dx = i64::from(pair[1].x) - i64::from(pair[0].x);
            let dy = i64::from(pair[1].y) - i64::from(pair[0].y);
            if dx.unsigned_abs() > 1
                || dy.unsigned_abs() > 1
                || (dx == 0 && dy == 0)
                || (dx != 0 && dx.signum() as i32 != self.signs[0])
                || (dy != 0 && dy.signum() as i32 != self.signs[1])
            {
                return Err(invalid("広域DDAの保存経路"));
            }
            // 外部から読んだcodecに斜め角抜け等を仕込めないよう、
            // 純粋な整数DDAの交差順と経路を先頭から再照合します。
            let horizontal = u128::from(2 * crossed[0] + 1) * u128::from(self.counts[1]);
            let vertical = u128::from(2 * crossed[1] + 1) * u128::from(self.counts[0]);
            let expected_x = crossed[0] < self.counts[0]
                && (crossed[1] == self.counts[1] || horizontal <= vertical);
            let expected_y = crossed[1] < self.counts[1]
                && (crossed[0] == self.counts[0] || vertical <= horizontal);
            if (dx != 0) != expected_x || (dy != 0) != expected_y {
                return Err(invalid("広域DDAの保存経路と整数交差順"));
            }
            crossed[0] += u64::from(dx != 0);
            crossed[1] += u64::from(dy != 0);
        }
        if crossed != self.progressed {
            return Err(invalid("広域DDAのcursorと保存経路"));
        }
        let expected_steps = self.path.len() as u64 - 1 + u64::from(self.blocked != 0);
        if self.checked_steps != expected_steps
            || (self.blocked != 0 && !self.finished)
            || (self.blocked == 1 && self.after != MaterialMotion::default())
            || (self.blocked != 0 && self.progressed == self.counts)
        {
            return Err(invalid("広域DDAの検査数・終了理由・速度"));
        }
        Ok(())
    }
}

fn offset(point: CellPoint, dx: i32, dy: i32) -> EngineResult<CellPoint> {
    Ok(CellPoint {
        x: point
            .x
            .checked_add(dx)
            .ok_or_else(|| invalid("広域DDAの水平端"))?,
        y: point
            .y
            .checked_add(dy)
            .ok_or_else(|| invalid("広域DDAの垂直端"))?,
    })
}

fn channel_point(point: CellPoint, origin: CellPoint) -> EngineResult<CellPoint> {
    Ok(CellPoint {
        x: point
            .x
            .checked_sub(origin.x)
            .ok_or_else(|| invalid("広域DDAのchannel水平座標"))?,
        y: point
            .y
            .checked_sub(origin.y)
            .ok_or_else(|| invalid("広域DDAのchannel垂直座標"))?,
    })
}

impl StateCodec for MaterialMotionJob {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.validate_progress()?;
        out.raw(b"CHMMJOB1")?;
        self.world.encode(out)?;
        out.raw(&self.content)?;
        out.u64(self.epoch)?;
        self.layer.encode(out)?;
        self.channel.encode(out)?;
        self.source.encode(out)?;
        self.source_core.encode(out)?;
        self.before.encode(out)?;
        self.after.encode(out)?;
        for count in self.counts {
            out.u64(count)?;
        }
        for sign in self.signs {
            out.i32(sign)?;
        }
        for value in self.progressed {
            out.u64(value)?;
        }
        out.u64(u64::try_from(self.path_limit).map_err(|_| invalid("広域DDA保存容量"))?)?;
        out.u8(u8::from(self.finished))?;
        out.u8(self.blocked)?;
        out.u64(self.checked_steps)?;
        out.u32(u32::try_from(self.chunks.len()).map_err(|_| invalid("広域DDA chunk数"))?)?;
        for &(coord, revision) in &self.chunks {
            coord.encode(out)?;
            out.u64(revision)?;
        }
        out.u32(u32::try_from(self.path.len()).map_err(|_| invalid("広域DDA経路長"))?)?;
        for point in &self.path {
            point.encode(out)?;
        }
        Ok(())
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        if input.raw(8)? != b"CHMMJOB1" {
            return Err(invalid("広域DDAの形式"));
        }
        let world = WorldId::decode(input)?;
        let content = input
            .raw(32)?
            .try_into()
            .map_err(|_| invalid("広域DDA Content hash"))?;
        let epoch = input.u64()?;
        let layer = StableKey::decode(input)?;
        let channel = StableKey::decode(input)?;
        let source = CellPoint::decode(input)?;
        let source_core = CellCore::decode(input)?;
        let before = MaterialMotion::decode(input)?;
        let after = MaterialMotion::decode(input)?;
        let counts = [input.u64()?, input.u64()?];
        let signs = [input.i32()?, input.i32()?];
        let progressed = [input.u64()?, input.u64()?];
        let path_limit = usize::try_from(input.u64()?).map_err(|_| invalid("広域DDA経路容量"))?;
        let finished = match input.u8()? {
            0 => false,
            1 => true,
            _ => return Err(invalid("広域DDA完成bit")),
        };
        let blocked = input.u8()?;
        let checked_steps = input.u64()?;
        let chunk_count = input.u32()? as usize;
        if chunk_count > input.remaining() / 16 {
            return Err(invalid("広域DDA chunk件数"));
        }
        let mut chunks = Vec::new();
        chunks.try_reserve_exact(chunk_count).map_err(allocation)?;
        for _ in 0..chunk_count {
            chunks.push((ChunkCoord::decode(input)?, input.u64()?));
        }
        let count = input.u32()? as usize;
        if count > path_limit || count > input.remaining() / 8 {
            return Err(invalid("広域DDA経路件数"));
        }
        let mut path = Vec::new();
        path.try_reserve_exact(count).map_err(allocation)?;
        for _ in 0..count {
            path.push(CellPoint::decode(input)?);
        }
        let job = Self {
            world,
            content,
            epoch,
            layer,
            channel,
            source,
            source_core,
            before,
            after,
            counts,
            signs,
            progressed,
            path_limit,
            path,
            chunks,
            finished,
            blocked,
            checked_steps,
        };
        job.validate_progress()?;
        Ok(job)
    }
}
