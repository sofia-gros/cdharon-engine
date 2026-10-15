//! 描画を起動せず、作者の状態・component・液量channelを保存する利用例。
//!
//! 実行時のSceneや物理solverは使わず、公開データAPIの組合せだけを示します。
//! ゲーム固有の「生産数」「道具の残量」は例の側が定義し、エンジンへ固定しません。

use charon_engine::foundation::{
    EngineResult, StableKey,
    codec::{Decoder, Encoder, StateCodec},
    component::{ComponentStore, EntityPool},
    grid::{CellPoint, GridChannel, GridLayout, GridLimits, GridScale},
    id::{Arena, WorldKind},
    random::{RandomSource, SplitMix64},
    schema::{Schema, SchemaRegistry},
    state::{StateStore, StoreLimits},
    transaction::AtomicState,
};

/// この作品だけの道具状態です。保存順をRustのメモリ配置と切り離します。
struct ToolState {
    remaining: u32,
}

impl StateCodec for ToolState {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.u32(self.remaining)
    }

    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        Ok(Self {
            remaining: input.u32()?,
        })
    }
}

/// 保存の読み込みにも同じキー・版のregistryを渡します。
/// TypeIdや関数pointerを保存ファイルへ埋め込む必要はありません。
fn registry() -> EngineResult<SchemaRegistry> {
    let mut schemas = SchemaRegistry::default();
    schemas.register(Schema::of::<u64>(StableKey::parse("example:count")?, 1, 8)?)?;
    schemas.register(Schema::of::<SplitMix64>(
        StableKey::parse("example:random")?,
        1,
        8,
    )?)?;
    Ok(schemas)
}

fn main() -> EngineResult<()> {
    let scope = StableKey::parse("example:world-state")?;
    let count = StableKey::parse("example:factory-count")?;
    let random_key = StableKey::parse("example:factory-random")?;
    let limits = StoreLimits {
        records: 8,
        payload_bytes: 1024,
        checkpoint_bytes: 4096,
    };
    let mut state = StateStore::new(scope, registry()?, limits)?;

    // 二つのrecordは候補へ書き、最後のcommitでまとめて公開します。
    // ?で途中から戻った場合、候補が破棄されて元のstoreが残ります。
    let mut candidate = state.transaction()?;
    candidate.put(count, StableKey::parse("example:count")?, &12_u64)?;
    candidate.put(
        random_key,
        StableKey::parse("example:random")?,
        &SplitMix64::new(2026),
    )?;
    candidate.commit()?;

    let mut random: SplitMix64 = state.get(random_key)?;
    let produced = state.get::<u64>(count)? + random.below(4)?;
    let mut candidate = state.transaction()?;
    candidate.put(count, StableKey::parse("example:count")?, &produced)?;
    candidate.put(random_key, StableKey::parse("example:random")?, &random)?;
    candidate.commit()?;
    let saved_state = state.checkpoint()?;
    let restored = StateStore::restore(&saved_state, scope, registry()?, limits)?;

    // Worldも同じ世代付きallocatorから発行します。
    let mut worlds: Arena<u8, WorldKind> = Arena::new(4)?;
    let world = worlds.insert(0)?;
    let mut entities = EntityPool::new(world, 32)?;
    let entity = entities.spawn()?;
    let component_key = StableKey::parse("example:tool")?;
    let mut tools = ComponentStore::new(&entities, component_key, 1, 16)?;
    tools.insert(&entities, entity, ToolState { remaining: 50 })?;
    let saved_entities = entities.checkpoint(4096)?;
    let saved_tools = tools.checkpoint(&entities, 4096)?;
    let restored_entities = EntityPool::restore(&saved_entities, world, 32, 4096)?;
    let restored_tools = ComponentStore::<ToolState>::restore(
        &saved_tools,
        &restored_entities,
        component_key,
        1,
        16,
        4096,
    )?;

    // 建築tile一つが16cellに相当しても、その液量をu8で独立に保持できます。
    let layout = GridLayout {
        origin: CellPoint { x: 0, y: 0 },
        x_scale: GridScale::new(16, 1)?,
        y_scale: GridScale::new(16, 1)?,
    };
    let grid_limits = GridLimits {
        chunks: 8,
        checkpoint_bytes: 65536,
    };
    let liquid_key = StableKey::parse("example:liquid-amount")?;
    let mut liquid = GridChannel::new(liquid_key, 1, layout, 0_u8, grid_limits)?;
    let position = CellPoint { x: -1, y: 3 };
    liquid.set(position, 127)?;
    let saved_liquid = liquid.checkpoint()?;
    let restored_liquid =
        GridChannel::restore(&saved_liquid, liquid_key, 1, layout, 0_u8, grid_limits)?;

    // 作者の正規recordと別解像度channelも、一つの候補へまとめられます。
    // 後半のchannel変更に失敗した場合、先行する生産数変更も公開されません。
    let mut world_data = AtomicState::new(scope, (restored, restored_liquid))?;
    let mut candidate = world_data.transaction()?;
    candidate.apply(|(state, liquid)| {
        let mut state_change = state.transaction()?;
        state_change.put(count, StableKey::parse("example:count")?, &(produced + 1))?;
        state_change.commit()?;
        liquid.set(position, 126)?;
        Ok(())
    })?;
    candidate.prepare()?.commit();

    println!(
        "生産数: {}、道具の残量: {}、液量: {}",
        world_data.view().0.get::<u64>(count)?,
        restored_tools
            .get(&restored_entities, entity)?
            .map(|tool| tool.remaining)
            .unwrap_or(0),
        world_data.view().1.get(position)
    );
    Ok(())
}
