//! 作者の別解像度channelとWorld domainの接続。
//!
//! GridChannel自体は疎な配列で、未確保を既定値として扱います。この層では先にWorldの
//! coverageを確認し、未読込を既定値へ置換しません。borrow中はWorld全体が不変借用され、
//! batchを読みながらunloadや再配置を実行することはRustの借用規則で禁止されます。

use super::{
    ChunkCoord, ChunkCoverage, ChunkRegion, ContentRegistry, RegionStatus, World, WorldEdit,
    invalid,
};
use crate::foundation::{
    EngineError, EngineResult, ErrorCode, StableKey,
    codec::{Encoder, StateCodec},
    error::allocation,
    grid::{CellPoint, GridChannel, GridLayout, GridLimits, GridValue},
};
use std::{any::Any, sync::Arc};

/// 一つの独自channelの型・格子・既定値・容量。定義はContent freeze後に変えません。
pub struct ChannelDefinition<T: GridValue> {
    /// 独自channelキー。
    pub key: StableKey,
    /// record版。
    pub version: u32,
    /// 一要素の型schema。ContentRegistry::schemaで同じTを登録します。
    pub value_schema: StableKey,
    /// Worldのcellに対する原点と各軸の有理数格子比。
    pub layout: GridLayout,
    /// 読込済みだが配列を未確保の領域で返す値。
    pub default: T,
    /// channelの独立した確保・codec上限。
    pub limits: GridLimits,
}
pub(crate) trait ChannelFactory: Send + Sync {
    fn key(&self) -> StableKey;
    fn value_schema(&self) -> StableKey;
    fn descriptor(&self, limit: usize) -> EngineResult<Vec<u8>>;
    fn create(&self) -> EngineResult<Box<dyn ChannelState>>;
    fn restore(&self, bytes: &[u8]) -> EngineResult<Box<dyn ChannelState>>;
    fn validate_type(
        &self,
        schemas: &crate::foundation::schema::SchemaRegistry,
    ) -> EngineResult<()>;
}
pub(crate) trait ChannelState: Send + Sync {
    fn key(&self) -> StableKey;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn try_copy(&self) -> EngineResult<Box<dyn ChannelState>>;
    fn checkpoint(&self) -> EngineResult<Vec<u8>>;
    fn bytes(&self) -> usize;
    fn validate(&self, world: &World) -> EngineResult<()>;
    fn extract(&self, region: ChunkRegion) -> EngineResult<Vec<u8>>;
    fn clear(&mut self, region: ChunkRegion) -> EngineResult<()>;
    fn merge(&mut self, bytes: &[u8]) -> EngineResult<()>;
}
struct Factory<T: GridValue + Send + Sync + 'static>(ChannelDefinition<T>);
struct Storage<T: GridValue + Send + Sync + 'static> {
    definition: Arc<Factory<T>>,
    grid: GridChannel<T>,
}
impl<T: GridValue + Send + Sync + 'static> Factory<T> {
    fn copy(&self) -> Self {
        Self(ChannelDefinition {
            key: self.0.key,
            version: self.0.version,
            value_schema: self.0.value_schema,
            layout: self.0.layout,
            default: self.0.default,
            limits: self.0.limits,
        })
    }
    fn empty(&self) -> EngineResult<GridChannel<T>> {
        GridChannel::new(
            self.0.key,
            self.0.version,
            self.0.layout,
            self.0.default,
            self.0.limits,
        )
    }
    fn decode(&self, bytes: &[u8]) -> EngineResult<GridChannel<T>> {
        GridChannel::restore(
            bytes,
            self.0.key,
            self.0.version,
            self.0.layout,
            self.0.default,
            self.0.limits,
        )
    }
}
impl<T: GridValue + Send + Sync + 'static> ChannelFactory for Factory<T> {
    fn key(&self) -> StableKey {
        self.0.key
    }
    fn value_schema(&self) -> StableKey {
        self.0.value_schema
    }
    fn descriptor(&self, limit: usize) -> EngineResult<Vec<u8>> {
        let mut out = Encoder::new(limit);
        self.0.key.encode(&mut out)?;
        out.u32(self.0.version)?;
        self.0.value_schema.encode(&mut out)?;
        self.0.layout.encode(&mut out)?;
        self.0.default.encode(&mut out)?;
        out.u32(self.0.limits.chunks)?;
        out.u64(self.0.limits.checkpoint_bytes as u64)?;
        Ok(out.finish())
    }
    fn create(&self) -> EngineResult<Box<dyn ChannelState>> {
        Ok(Box::new(Storage {
            definition: Arc::new(self.copy()),
            grid: self.empty()?,
        }))
    }
    fn restore(&self, bytes: &[u8]) -> EngineResult<Box<dyn ChannelState>> {
        Ok(Box::new(Storage {
            definition: Arc::new(self.copy()),
            grid: self.decode(bytes)?,
        }))
    }
    fn validate_type(
        &self,
        schemas: &crate::foundation::schema::SchemaRegistry,
    ) -> EngineResult<()> {
        let schema = schemas.get(self.0.value_schema)?;
        if !schema.is::<T>() {
            return Err(EngineError::new(
                ErrorCode::SchemaMismatch,
                "channel値の型schema",
            ));
        }
        let mut out = Encoder::new(schema.max_bytes());
        self.0.default.encode(&mut out)?;
        schema.validate(&out.finish())
    }
}
impl<T: GridValue + Send + Sync + 'static> Storage<T> {
    fn non_default(&self) -> impl Iterator<Item = (CellPoint, T)> + '_ {
        self.grid.batches().flat_map(move |((cx, cy), values)| {
            values
                .iter()
                .copied()
                .enumerate()
                .filter_map(move |(index, value)| {
                    if value.same(self.definition.0.default) {
                        return None;
                    }
                    let x = i64::from(cx) * 64 + (index % 64) as i64;
                    let y = i64::from(cy) * 64 + (index / 64) as i64;
                    Some((
                        CellPoint {
                            x: x as i32,
                            y: y as i32,
                        },
                        value,
                    ))
                })
        })
    }
    fn selected(&self, region: ChunkRegion) -> EngineResult<Vec<(CellPoint, T)>> {
        let mut result = Vec::new();
        for (point, value) in self.non_default() {
            let coverage = coverage(self.grid.layout(), point, 1)?;
            let intersects = (coverage.min().x as i64) < region.max()[0]
                && (region.min().x as i64) < coverage.max()[0]
                && (coverage.min().y as i64) < region.max()[1]
                && (region.min().y as i64) < coverage.max()[1];
            if intersects {
                if !region.contains(coverage.min())
                    || coverage.max()[0] > region.max()[0]
                    || coverage.max()[1] > region.max()[1]
                {
                    return Err(EngineError::new(
                        ErrorCode::Conflict,
                        "channel値が退避regionの境界を跨ぐ",
                    ));
                }
                result.try_reserve(1).map_err(allocation)?;
                result.push((point, value));
            }
        }
        Ok(result)
    }
}
impl<T: GridValue + Send + Sync + 'static> ChannelState for Storage<T> {
    fn key(&self) -> StableKey {
        self.definition.0.key
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn try_copy(&self) -> EngineResult<Box<dyn ChannelState>> {
        Ok(Box::new(Self {
            definition: Arc::clone(&self.definition),
            grid: self.grid.try_copy()?,
        }))
    }
    fn checkpoint(&self) -> EngineResult<Vec<u8>> {
        self.grid.checkpoint()
    }
    fn bytes(&self) -> usize {
        self.grid.allocated_chunks() * 4096 * size_of::<T>()
    }
    fn validate(&self, world: &World) -> EngineResult<()> {
        for (point, value) in self.non_default() {
            value.validate()?;
            let (min, max) = cell_bounds(self.grid.layout(), point, 1)?;
            if min[0] < i64::from(world.bounds.min().x)
                || min[1] < i64::from(world.bounds.min().y)
                || max[0] > world.bounds.max()[0]
                || max[1] > world.bounds.max()[1]
            {
                return Err(invalid("channel値がWorld境界外"));
            }
            let region = coverage(self.grid.layout(), point, 1)?;
            world.require_coverage(region)?;
        }
        Ok(())
    }
    fn extract(&self, region: ChunkRegion) -> EngineResult<Vec<u8>> {
        let mut grid = self.definition.empty()?;
        for (point, value) in self.selected(region)? {
            grid.set(point, value)?;
        }
        grid.checkpoint()
    }
    fn clear(&mut self, region: ChunkRegion) -> EngineResult<()> {
        for (point, _) in self.selected(region)? {
            self.grid.set(point, self.definition.0.default)?;
        }
        Ok(())
    }
    fn merge(&mut self, bytes: &[u8]) -> EngineResult<()> {
        let source = Self {
            definition: Arc::clone(&self.definition),
            grid: self.definition.decode(bytes)?,
        };
        for (point, value) in source.non_default() {
            if !self.grid.get(point).same(self.definition.0.default) {
                return Err(EngineError::new(
                    ErrorCode::Conflict,
                    "復帰channelの上書き競合",
                ));
            }
            self.grid.set(point, value)?;
        }
        Ok(())
    }
}
impl ContentRegistry {
    /// 状態・block個体・channel値に使う型schemaを登録します。
    pub fn schema(&mut self, schema: crate::foundation::schema::Schema) -> EngineResult<()> {
        self.schemas.register(schema)
    }
    /// 独自channelを登録します。型・既定値の検証と重複検査を済ませてから公開します。
    pub fn channel<T: GridValue + Send + Sync + 'static>(
        &mut self,
        definition: ChannelDefinition<T>,
    ) -> EngineResult<()> {
        if self.channels.len() >= self.limits.channels as usize {
            return Err(EngineError::capacity(
                "channel定義数",
                self.channels.len() as u64 + 1,
                self.limits.channels.into(),
            ));
        }
        let factory = Factory(definition);
        factory.validate_type(&self.schemas)?;
        factory.empty()?;
        let index = match self
            .channels
            .binary_search_by_key(&factory.key(), |old| old.key())
        {
            Ok(_) => return Err(EngineError::new(ErrorCode::DuplicateKey, "channelキー")),
            Err(index) => index,
        };
        self.channels.try_reserve(1).map_err(allocation)?;
        self.channels.insert(index, Arc::new(factory));
        Ok(())
    }
}
/// 読込状態を失わない型付きchannel query。
pub enum ChannelQuery<T> {
    /// 全coverageがWorld内かつresident。既定値もこの経路で返します。
    Loaded(T),
    /// 必要なchunkに未準備部分がある。各chunkの状態を保持します。
    Unavailable(Vec<ChunkCoverage>),
    /// 要素がWorldの境界を跨ぐか、座標の表現範囲外。
    WorldBoundary,
}
/// 連続した64×64の型付きbatch。coverageは値配列と別に提供します。
pub struct ChannelBatch<'a, T> {
    /// channel内のchunk座標。
    pub coordinate: (i32, i32),
    /// row-majorの固定長値配列。
    pub values: &'a [T],
    /// Worldへの格子変換。
    pub layout: GridLayout,
    /// このbatchが必要とする各World chunk。haloは呼出側が追加regionを照会できます。
    pub coverage: Vec<ChunkCoverage>,
}
impl World {
    fn typed_channel<T: GridValue + Send + Sync + 'static>(
        &self,
        key: StableKey,
    ) -> EngineResult<&Storage<T>> {
        let channel = self
            .channels
            .iter()
            .find(|channel| channel.key() == key)
            .ok_or_else(|| EngineError::new(ErrorCode::MissingKey, "World channel"))?;
        channel
            .as_any()
            .downcast_ref::<Storage<T>>()
            .ok_or_else(|| EngineError::new(ErrorCode::SchemaMismatch, "channelの要求型"))
    }
    pub(crate) fn require_coverage(&self, region: ChunkRegion) -> EngineResult<()> {
        for coord in region.coordinates(self.limits.resident_chunks as usize)? {
            if !self.chunks.contains_key(&coord) {
                return Err(EngineError::new(
                    ErrorCode::InvalidState,
                    "channelの必要regionが未準備",
                ));
            }
        }
        Ok(())
    }
    /// 読込状態を照合して型付き値を返します。未読込をdefaultへ丸めません。
    pub fn channel_value<T: GridValue + Send + Sync + 'static>(
        &self,
        key: StableKey,
        point: CellPoint,
    ) -> EngineResult<ChannelQuery<T>> {
        let channel = self.typed_channel::<T>(key)?;
        let (min, max) = cell_bounds(channel.grid.layout(), point, 1)?;
        let bounds = self.bounds;
        if min[0] < i64::from(bounds.min().x)
            || min[1] < i64::from(bounds.min().y)
            || max[0] > bounds.max()[0]
            || max[1] > bounds.max()[1]
        {
            return Ok(ChannelQuery::WorldBoundary);
        }
        let region = coverage(channel.grid.layout(), point, 1)?;
        let states = self.region_state(region, self.limits.resident_chunks as usize)?;
        if states.iter().any(|state| {
            !matches!(
                state.status,
                RegionStatus::Active | RegionStatus::ResidentInactive
            )
        }) {
            return Ok(ChannelQuery::Unavailable(states));
        }
        Ok(ChannelQuery::Loaded(channel.grid.get(point)))
    }
    /// 配列をcellごとのdynamic callbackへ変換せず公開します。借用中はunloadできません。
    pub fn channel_batches<T: GridValue + Send + Sync + 'static>(
        &self,
        key: StableKey,
        coverage_limit: usize,
    ) -> EngineResult<Vec<ChannelBatch<'_, T>>> {
        let channel = self.typed_channel::<T>(key)?;
        let mut result = Vec::new();
        result
            .try_reserve_exact(channel.grid.allocated_chunks())
            .map_err(allocation)?;
        for (coordinate, values) in channel.grid.batches() {
            let point = CellPoint {
                x: i32::try_from(i64::from(coordinate.0) * 64)
                    .map_err(|_| invalid("channel batch座標"))?,
                y: i32::try_from(i64::from(coordinate.1) * 64)
                    .map_err(|_| invalid("channel batch座標"))?,
            };
            let coverage =
                self.region_state(coverage(channel.grid.layout(), point, 64)?, coverage_limit)?;
            result.push(ChannelBatch {
                coordinate,
                values,
                layout: channel.grid.layout(),
                coverage,
            });
        }
        Ok(result)
    }
    /// channelの確保済み値配列の合計byte数。
    pub fn channel_bytes(&self) -> usize {
        self.channels.iter().map(|channel| channel.bytes()).sum()
    }
    pub(crate) fn validate_channels(&self) -> EngineResult<()> {
        if self.channels.len() != self.content.channels.len() {
            return Err(invalid("World channel数"));
        }
        if self.channel_bytes() > self.limits.channel_bytes {
            return Err(EngineError::capacity(
                "channel byte数",
                self.channel_bytes() as u64,
                self.limits.channel_bytes as u64,
            ));
        }
        for (state, definition) in self.channels.iter().zip(&self.content.channels) {
            if state.key() != definition.key() {
                return Err(invalid("World channel辞書"));
            }
            state.validate(self)?;
        }
        Ok(())
    }
}
impl WorldEdit<'_> {
    /// 読込済みcoverageの独自値を候補へ設定します。素材や熱等へ自動変換しません。
    pub fn set_channel<T: GridValue + Send + Sync + 'static>(
        &mut self,
        key: StableKey,
        point: CellPoint,
        value: T,
    ) -> EngineResult<()> {
        self.run(|editor| {
            if !matches!(
                editor.world.channel_value::<T>(key, point)?,
                ChannelQuery::Loaded(_)
            ) {
                return Err(EngineError::new(
                    ErrorCode::InvalidState,
                    "channel編集のcoverageが未準備または境界外",
                ));
            }
            let storage = editor.world.typed_channel::<T>(key)?;
            let allocated = storage
                .grid
                .batches()
                .any(|(coordinate, _)| coordinate == point.chunk_local().0);
            let extra = if allocated || value.same(storage.definition.0.default) {
                0
            } else {
                4096 * size_of::<T>()
            };
            let required = editor
                .world
                .channel_bytes()
                .checked_add(extra)
                .ok_or_else(|| invalid("channel byte数overflow"))?;
            if required > editor.world.limits.channel_bytes {
                return Err(EngineError::capacity(
                    "channel byte数",
                    required as u64,
                    editor.world.limits.channel_bytes as u64,
                ));
            }
            editor.reserve_writes(1)?;
            let channel = editor
                .world
                .channels
                .iter_mut()
                .find(|channel| channel.key() == key)
                .ok_or_else(|| invalid("channelが見つからない"))?;
            let storage = channel
                .as_any_mut()
                .downcast_mut::<Storage<T>>()
                .ok_or_else(|| EngineError::new(ErrorCode::SchemaMismatch, "channel型"))?;
            let region = coverage(storage.grid.layout(), point, 1)?;
            storage.grid.set(point, value)?;
            for coord in region.coordinates(editor.world.limits.resident_chunks as usize)? {
                let chunk = editor
                    .world
                    .chunks
                    .get_mut(&coord)
                    .expect("channel coverageを検査済み");
                chunk.revision = chunk
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| invalid("channelによるchunk revision上限"))?;
                chunk.edited = true;
            }
            Ok(())
        })
    }
}
fn cell_bounds(
    layout: GridLayout,
    point: CellPoint,
    side: i64,
) -> EngineResult<([i64; 2], [i64; 2])> {
    let mut min = [0; 2];
    let mut max = [0; 2];
    for (axis, (origin, coordinate, scale)) in [
        (layout.origin.x, point.x, layout.x_scale),
        (layout.origin.y, point.y, layout.y_scale),
    ]
    .into_iter()
    .enumerate()
    {
        let (n, d) = scale.ratio();
        let d = i128::from(d);
        let start = i128::from(origin) * d + i128::from(coordinate) * i128::from(n);
        let end = start + i128::from(side) * i128::from(n);
        min[axis] =
            i64::try_from(start.div_euclid(d)).map_err(|_| invalid("channel coverage開始座標"))?;
        max[axis] = i64::try_from((end + d - 1).div_euclid(d))
            .map_err(|_| invalid("channel coverage終了座標"))?;
    }
    Ok((min, max))
}
fn coverage(layout: GridLayout, point: CellPoint, side: i64) -> EngineResult<ChunkRegion> {
    let (min, max) = cell_bounds(layout, point, side)?;
    let min = ChunkCoord::new(
        i32::try_from(min[0].div_euclid(64)).map_err(|_| invalid("channel chunk開始座標"))?,
        i32::try_from(min[1].div_euclid(64)).map_err(|_| invalid("channel chunk開始座標"))?,
    )?;
    ChunkRegion::new(
        min,
        (max[0] + 63).div_euclid(64),
        (max[1] + 63).div_euclid(64),
    )
}
