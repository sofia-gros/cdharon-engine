//! 満杯の流体経路を通した、保存可能な押出し探索。
//!
//! 探索状態は作者のWorld StateStoreへ登録して保存できます。探索中の正規地形は
//! 変更せず、全経路の完成後にだけ地形と付随channelをまとめて循環輸送します。

use super::{
    CellQuery, ChannelQuery, ChunkCoord, LayerId, MaterialBase, MaterialId, World, WorldEdit,
    conflict, invalid,
};
use crate::foundation::{
    EngineError, EngineResult, ErrorCode, StableKey,
    codec::{Decoder, Encoder, StateCodec},
    error::allocation,
    grid::CellPoint,
    id::WorldId,
    transaction::CandidateState,
};

/// 探索の保存対象となる段階です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PressurePushStatus {
    /// 次の固定件数stepを待っています。まだ物質を動かしていません。
    Searching,
    /// 圧力差を満たす空き先までの経路が完成し、原子的な公開が可能です。
    Ready,
    /// 読込済み活動領域内に、条件を満たす空き先がありませんでした。
    NoOutlet,
}

/// 圧力から押出し候補を選ぶ条件。solverの呼出側が明示します。
#[derive(Clone, Copy, Debug)]
pub struct PressurePushConfig {
    /// 保存された非負Q16圧力の、位置に固定されたi64 channel。
    pub channel: StableKey,
    /// 単位質量・単位高さ当たりの重力圧力差。下向きが正です。
    pub gravity_q16: u32,
    /// 経路全体のdriveがこの値より大きい場合だけ採用します。
    pub threshold_q16: u64,
    /// 保存する探索nodeの上限。上限到達は容量エラーであり、出口なしとは別です。
    pub nodes: u32,
}

/// 一つのWorldに所属する幅優先探索。可変状態をproviderのglobalへ隠しません。
///
/// `StateCodec`を実装しているので、作者はschemaを登録しWorld StateStoreへ格納できます。
/// 入力した全活動chunkのrevisionを保持する保守的な失効方針です。無関係な地形編集でも
/// 同じ活動集合内なら再探索が必要ですが、古い探索結果を新しい地形へ公開しません。
pub struct PressurePushSearch {
    world: WorldId,
    content: [u8; 32],
    epoch: u64,
    layer: StableKey,
    material: MaterialId,
    config: PressurePushConfig,
    source_pressure: i64,
    mass: u32,
    chunks: Vec<(ChunkCoord, u64)>,
    nodes: Vec<(CellPoint, u32)>,
    cursor: usize,
    goal: Option<u32>,
}
impl PressurePushSearch {
    /// 活動中の液体または気体cellから探索を始めます。開始だけでは輸送しません。
    pub fn new(
        world: &World,
        layer: LayerId,
        source: CellPoint,
        config: PressurePushConfig,
    ) -> EngineResult<Self> {
        if config.nodes == 0 || !world.content.layer(layer)?.simulated {
            return Err(invalid("圧力探索の容量またはlayer"));
        }
        let layout = world.channel_layout::<i64>(config.channel)?;
        if layout.x_scale.ratio() != (1, 1)
            || layout.y_scale.ratio() != (1, 1)
            || world
                .channel_material_layer::<i64>(config.channel)?
                .is_some()
        {
            return Err(invalid("圧力探索には位置固定の1対1 channelが必要"));
        }
        let cell = match world.cell(layer, source)? {
            CellQuery::Loaded(cell) if cell.block.is_none() => cell,
            _ => return Err(invalid("圧力探索の始点が未準備またはblock")),
        };
        let material = world
            .content
            .material(cell.core.material)?
            .ok_or_else(|| invalid("圧力探索の始点が空"))?;
        if !matches!(material.base(), MaterialBase::Liquid | MaterialBase::Gas) {
            return Err(invalid("圧力探索の始点が流体ではない"));
        }
        let source_pressure = read_pressure(world, config.channel, source)?;
        let mut chunks = Vec::new();
        for (coord, chunk) in &world.chunks {
            if chunk.active {
                chunks.try_reserve(1).map_err(allocation)?;
                chunks.push((*coord, chunk.revision));
            }
        }
        if chunks
            .binary_search_by_key(&ChunkCoord::at(source), |(coord, _)| *coord)
            .is_err()
        {
            return Err(invalid("圧力探索の始点が活動外"));
        }
        let mut nodes = Vec::new();
        nodes.try_reserve_exact(1).map_err(allocation)?;
        nodes.push((source, 0));
        Ok(Self {
            world: world.id,
            content: world.content.fingerprint(),
            epoch: world.remaps.epoch(),
            layer: world.content.layer(layer)?.key,
            material: cell.core.material,
            config,
            source_pressure,
            mass: material.mass_per_cell(),
            chunks,
            nodes,
            cursor: 0,
            goal: None,
        })
    }
    /// 現在の段階。容量不足や入力失効はstepのエラーとして区別して返します。
    pub fn status(&self) -> PressurePushStatus {
        if self.goal.is_some() {
            PressurePushStatus::Ready
        } else if self.cursor == self.nodes.len() {
            PressurePushStatus::NoOutlet
        } else {
            PressurePushStatus::Searching
        }
    }
    /// 保存済みの探索node数。始点を含みます。
    pub fn discovered_nodes(&self) -> usize {
        self.nodes.len()
    }
    /// 探索frontierを失わずにnode上限を変更します。
    ///
    /// 既存node数未満への縮小は拒否します。この呼出では追加配列を確保せず、
    /// 次のstepが必要分だけ確保します。StateStoreのrecord byte上限は独立して残るため、
    /// 上限変更だけで無制限に保存できるようにはなりません。
    pub fn set_node_capacity(&mut self, nodes: u32) -> EngineResult<()> {
        if (nodes as usize) < self.nodes.len() {
            return Err(EngineError::capacity(
                "圧力探索の既存node数",
                self.nodes.len() as u64,
                nodes.into(),
            ));
        }
        self.config.nodes = nodes;
        Ok(())
    }
    /// 正規状態を変更せず、最大budget個のnodeから四近傍を調べます。
    ///
    /// 近傍は上・左・右・下の固定順です。同じ素材の活動cellだけを通過し、空cellで
    /// `Psrc - Pdst + mass*g*(ydst-ysrc) > threshold`を満たす経路を採用します。
    /// 未読込と活動外はno-flow境界です。step失敗時には探索cursorも元のままです。
    pub fn step(&mut self, world: &World, budget: usize) -> EngineResult<PressurePushStatus> {
        self.validate_inputs(world)?;
        let mut candidate = self.try_copy()?;
        let layer = world.content.layer_id(candidate.layer)?;
        let mut visited = super::transport::CellReservations::new(world)?;
        for (point, _) in &candidate.nodes {
            visited.insert(*point)?;
        }
        for _ in 0..budget {
            if candidate.status() != PressurePushStatus::Searching {
                break;
            }
            let parent = candidate.cursor;
            let point = candidate.nodes[parent].0;
            for (dx, dy) in [(0, -1), (-1, 0), (1, 0), (0, 1)] {
                let (Some(x), Some(y)) = (point.x.checked_add(dx), point.y.checked_add(dy)) else {
                    continue;
                };
                let next = CellPoint { x, y };
                if visited.contains(next) {
                    continue;
                }
                if !world
                    .chunks
                    .get(&ChunkCoord::at(next))
                    .is_some_and(|chunk| chunk.active)
                {
                    continue;
                }
                let cell = match world.cell(layer, next)? {
                    CellQuery::Loaded(cell) if cell.block.is_none() => cell,
                    _ => continue,
                };
                let empty = cell.core.material == MaterialId::AIR;
                if !empty && cell.core.material != candidate.material {
                    continue;
                }
                if empty {
                    let pressure = read_pressure(world, candidate.config.channel, next)?;
                    let drive = i128::from(candidate.source_pressure) - i128::from(pressure)
                        + i128::from(candidate.mass)
                            * i128::from(candidate.config.gravity_q16)
                            * (i128::from(next.y) - i128::from(candidate.nodes[0].0.y));
                    if drive <= i128::from(candidate.config.threshold_q16) {
                        continue;
                    }
                }
                if candidate.nodes.len() >= candidate.config.nodes as usize {
                    return Err(EngineError::capacity(
                        "圧力探索のnode数",
                        candidate.nodes.len() as u64 + 1,
                        candidate.config.nodes.into(),
                    ));
                }
                candidate.nodes.try_reserve(1).map_err(allocation)?;
                candidate.nodes.push((next, parent as u32));
                visited.insert(next)?;
                if empty {
                    candidate.goal = Some(candidate.nodes.len() as u32 - 1);
                    break;
                }
            }
            candidate.cursor += 1;
        }
        let status = candidate.status();
        *self = candidate;
        Ok(status)
    }
    fn validate_inputs(&self, world: &World) -> EngineResult<()> {
        self.validate()?;
        if world.id != self.world {
            return Err(EngineError::new(ErrorCode::WrongScope, "圧力探索のWorld"));
        }
        if self.content != world.content.fingerprint()
            || self.epoch != world.remaps.epoch()
            || world.chunks.values().filter(|chunk| chunk.active).count() != self.chunks.len()
            || self.chunks.iter().any(|(coord, revision)| {
                !world
                    .chunks
                    .get(coord)
                    .is_some_and(|chunk| chunk.active && chunk.revision == *revision)
            })
        {
            return Err(conflict("圧力探索の入力が変更されたため再探索が必要"));
        }
        // codecの構造検証だけでは、外部recordが現在の地形を正しく表している保証に
        // なりません。復元後はnodeの物質・空き先・圧力条件まで実データと照合します。
        let layer = world.content.layer_id(self.layer)?;
        let layout = world.channel_layout::<i64>(self.config.channel)?;
        if layout.x_scale.ratio() != (1, 1)
            || layout.y_scale.ratio() != (1, 1)
            || world
                .channel_material_layer::<i64>(self.config.channel)?
                .is_some()
        {
            return Err(invalid("復元された圧力探索のchannel定義"));
        }
        let definition = world
            .content
            .material(self.material)?
            .ok_or_else(|| invalid("圧力探索の素材定義"))?;
        if !world.content.layer(layer)?.simulated
            || !matches!(definition.base(), MaterialBase::Liquid | MaterialBase::Gas)
            || definition.mass_per_cell() != self.mass
            || read_pressure(world, self.config.channel, self.nodes[0].0)? != self.source_pressure
        {
            return Err(conflict("圧力探索の物性または始点圧力が不一致"));
        }
        for (index, (point, _)) in self.nodes.iter().enumerate() {
            let expected = if self.goal == Some(index as u32) {
                MaterialId::AIR
            } else {
                self.material
            };
            if !matches!(world.cell(layer,*point)?,CellQuery::Loaded(cell)
                if cell.block.is_none() && cell.core.material==expected)
            {
                return Err(conflict("圧力探索nodeの占有が不一致"));
            }
        }
        if let Some(goal) = self.goal {
            let destination = self.nodes[goal as usize].0;
            let drive = i128::from(self.source_pressure)
                - i128::from(read_pressure(world, self.config.channel, destination)?)
                + i128::from(self.mass)
                    * i128::from(self.config.gravity_q16)
                    * (i128::from(destination.y) - i128::from(self.nodes[0].0.y));
            if drive <= i128::from(self.config.threshold_q16) {
                return Err(conflict("圧力探索の完成経路がdrive条件を満たさない"));
            }
        }
        Ok(())
    }
}
impl WorldEdit<'_> {
    /// 完成した押出し経路だけを公開します。未完成・失効・容量不足では一切輸送しません。
    /// 公開後はchunk revisionが変わるので、同じ探索を二度公開することも拒否されます。
    pub fn publish_pressure_push(&mut self, search: &PressurePushSearch) -> EngineResult<()> {
        self.run(|editor| {
            search.validate_inputs(editor.world)?;
            let mut index = search
                .goal
                .ok_or_else(|| invalid("圧力探索の経路が未完成"))?
                as usize;
            let mut path = Vec::new();
            path.try_reserve_exact(search.nodes.len())
                .map_err(allocation)?;
            loop {
                path.push(search.nodes[index].0);
                if index == 0 {
                    break;
                }
                index = search.nodes[index].1 as usize;
            }
            path.reverse();
            editor.transport_path(editor.world.content.layer_id(search.layer)?, &path)
        })
    }
}
impl CandidateState for PressurePushSearch {
    fn try_copy(&self) -> EngineResult<Self> {
        Ok(Self {
            world: self.world,
            content: self.content,
            epoch: self.epoch,
            layer: self.layer,
            material: self.material,
            config: self.config,
            source_pressure: self.source_pressure,
            mass: self.mass,
            chunks: super::terrain::copy_slice(&self.chunks)?,
            nodes: super::terrain::copy_slice(&self.nodes)?,
            cursor: self.cursor,
            goal: self.goal,
        })
    }
    fn validate(&self) -> EngineResult<()> {
        if self.nodes.is_empty()
            || self.nodes.len() > self.config.nodes as usize
            || self.cursor > self.nodes.len()
            || self.source_pressure < 0
            || self.mass == 0
            || self.material == MaterialId::AIR
            || self.nodes[0].1 != 0
            || self.chunks.is_empty()
            || self.chunks.windows(2).any(|pair| pair[0].0 >= pair[1].0)
            || self
                .goal
                .is_some_and(|index| index == 0 || index as usize != self.nodes.len() - 1)
            || self.goal.is_some_and(|index| {
                self.cursor == 0
                    || (self.nodes[index as usize].1 as usize).checked_add(1) != Some(self.cursor)
            })
        {
            return Err(invalid("圧力探索recordの値域"));
        }
        let mut points = Vec::new();
        points
            .try_reserve_exact(self.nodes.len())
            .map_err(allocation)?;
        for (index, (point, parent)) in self.nodes.iter().enumerate() {
            if self
                .chunks
                .binary_search_by_key(&ChunkCoord::at(*point), |(coord, _)| *coord)
                .is_err()
            {
                return Err(invalid("圧力探索nodeの領域"));
            }
            if index > 0 {
                if *parent as usize >= index {
                    return Err(invalid("圧力探索の親node"));
                }
                let from = self.nodes[*parent as usize].0;
                if (i64::from(from.x) - i64::from(point.x)).abs()
                    + (i64::from(from.y) - i64::from(point.y)).abs()
                    != 1
                {
                    return Err(invalid("圧力探索の辺が非隣接"));
                }
            }
            points.push((point.x, point.y));
        }
        points.sort_unstable();
        if points.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(invalid("圧力探索nodeの重複"));
        }
        Ok(())
    }
}
impl StateCodec for PressurePushSearch {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.validate()?;
        out.raw(b"CHPUSH01")?;
        self.world.encode(out)?;
        out.raw(&self.content)?;
        out.u64(self.epoch)?;
        self.layer.encode(out)?;
        self.material.encode(out)?;
        self.config.channel.encode(out)?;
        out.u32(self.config.gravity_q16)?;
        out.u64(self.config.threshold_q16)?;
        out.u32(self.config.nodes)?;
        out.i64(self.source_pressure)?;
        out.u32(self.mass)?;
        out.u32(self.chunks.len() as u32)?;
        for (coord, revision) in &self.chunks {
            coord.encode(out)?;
            out.u64(*revision)?;
        }
        out.u32(self.nodes.len() as u32)?;
        for (point, parent) in &self.nodes {
            point.encode(out)?;
            out.u32(*parent)?;
        }
        out.u32(self.cursor as u32)?;
        self.goal.encode(out)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        if input.raw(8)? != b"CHPUSH01" {
            return Err(invalid("圧力探索recordの形式"));
        }
        let world = WorldId::decode(input)?;
        let mut content = [0u8; 32];
        content.copy_from_slice(input.raw(32)?);
        let epoch = input.u64()?;
        let layer = StableKey::decode(input)?;
        let material = MaterialId::decode(input)?;
        let config = PressurePushConfig {
            channel: StableKey::decode(input)?,
            gravity_q16: input.u32()?,
            threshold_q16: input.u64()?,
            nodes: input.u32()?,
        };
        let source_pressure = input.i64()?;
        let mass = input.u32()?;
        let count = input.u32()? as usize;
        if count > input.remaining() / 16 {
            return Err(invalid("圧力探索chunk列の切断"));
        }
        let mut chunks = Vec::new();
        chunks.try_reserve_exact(count).map_err(allocation)?;
        for _ in 0..count {
            chunks.push((ChunkCoord::decode(input)?, input.u64()?));
        }
        let count = input.u32()? as usize;
        if count > input.remaining() / 12 || count > config.nodes as usize {
            return Err(invalid("圧力探索node列の切断または容量"));
        }
        let mut nodes = Vec::new();
        nodes.try_reserve_exact(count).map_err(allocation)?;
        for _ in 0..count {
            nodes.push((CellPoint::decode(input)?, input.u32()?));
        }
        let cursor = input.u32()? as usize;
        let goal = Option::<u32>::decode(input)?;
        let result = Self {
            world,
            content,
            epoch,
            layer,
            material,
            config,
            source_pressure,
            mass,
            chunks,
            nodes,
            cursor,
            goal,
        };
        result.validate()?;
        Ok(result)
    }
}
fn read_pressure(world: &World, key: StableKey, point: CellPoint) -> EngineResult<i64> {
    let origin = world.channel_layout::<i64>(key)?.origin;
    let coordinate = CellPoint {
        x: point
            .x
            .checked_sub(origin.x)
            .ok_or_else(|| invalid("圧力探索channel水平座標"))?,
        y: point
            .y
            .checked_sub(origin.y)
            .ok_or_else(|| invalid("圧力探索channel垂直座標"))?,
    };
    match world.channel_value::<i64>(key, coordinate)? {
        ChannelQuery::Loaded(value) if value >= 0 => Ok(value),
        _ => Err(invalid("圧力探索の圧力値またはcoverage")),
    }
}
