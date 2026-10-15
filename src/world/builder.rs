//! RuntimeがWorld IDを割り当ててから地形を構築する型付きbuilder。

use super::{Content, World, WorldBounds, WorldLimits};
use crate::foundation::{EngineError, EngineResult, ErrorCode, id::WorldId};
use std::sync::Arc;

type Initializer = Box<dyn FnOnce(&mut World) -> EngineResult<()>>;
/// 作者の世界設定。window設定やYAMLの画面寸法から世界を生成しません。
pub struct WorldBuilder {
    content: Arc<Content>,
    seed: u64,
    bounds: WorldBounds,
    limits: WorldLimits,
    initializers: Vec<Initializer>,
    restored: Option<World>,
}
impl WorldBuilder {
    /// 内容定義を共有し、必要なchunkだけ増やすWorldを準備します。
    pub fn new(content: Arc<Content>) -> Self {
        Self {
            content,
            seed: 0,
            bounds: WorldBounds::expandable(),
            limits: WorldLimits::default(),
            initializers: Vec::new(),
            restored: None,
        }
    }
    /// 再現可能な生成seed。
    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }
    /// 有限境界または表現可能な全域。
    pub fn bounds(mut self, bounds: WorldBounds) -> Self {
        self.bounds = bounds;
        self
    }
    /// resident、作者状態、Job等の容量。
    pub fn limits(mut self, limits: WorldLimits) -> Self {
        self.limits = limits;
        self
    }
    /// 起動時の初期地形等を設定します。失敗したWorldはRuntimeへ公開しません。
    pub fn initialize(
        mut self,
        callback: impl FnOnce(&mut World) -> EngineResult<()> + 'static,
    ) -> Self {
        self.initializers.push(Box::new(callback));
        self
    }
    /// 検証済みsnapshotの再接続を準備します。ID辞書が異なる場合は黙って変更しません。
    pub fn restored(world: World) -> Self {
        let mut builder = Self::new(Arc::clone(&world.content));
        builder.restored = Some(world);
        builder
    }
    /// 割当済みのWorld IDで構築します。通常はEngineBuilderが呼びます。
    pub fn build(self, id: WorldId) -> EngineResult<World> {
        let mut world = match self.restored {
            Some(world) => {
                if world.id() != id {
                    return Err(EngineError::new(
                        ErrorCode::WrongScope,
                        "保存World IDとRuntimeの辞書が不一致",
                    ));
                }
                world
            }
            None => World::new(id, self.seed, self.bounds, self.content, self.limits)?,
        };
        for initialize in self.initializers {
            initialize(&mut world)?;
        }
        Ok(world)
    }
}
