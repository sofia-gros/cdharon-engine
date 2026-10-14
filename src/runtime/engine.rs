//! Runtimeの所有者。作者callbackとWorld候補の公開を別の境界として扱います。

use super::{
    config::{EngineSettings, EngineSettingsPatch},
    input::{InputManager, ScopeKind, TickInput},
    scene::{
        FrameContext, LoadContext, Scene, SceneContext, SceneEntry, SceneRequest, SceneState,
        StopContext,
    },
    schedule::{self, System, SystemContext, SystemDescriptor, SystemEntry, WorkStatus},
};
use crate::foundation::{
    EngineError, EngineResult, ErrorCode, StableKey,
    codec::StateCodec,
    id::{Arena, WorldId, WorldKind},
    state::StateStore,
    transaction::CandidateState,
};
use std::{any::Any, path::PathBuf, time::Duration};

/// pluginの依存を解決してからbuild、全登録後にready、終了時にshutdownを呼びます。
pub trait Plugin: 'static {
    /// 実装と実行設定の安定キー。
    fn key(&self) -> StableKey;
    /// 自分より先にbuild/readyされるplugin。
    fn dependencies(&self) -> &[StableKey] {
        &[]
    }
    /// plugin固有の実行設定を検証します。未定義の設定を無視しません。
    fn configure(&self, value: &serde_yaml::Value) -> EngineResult<()> {
        if value.is_null() || value.as_mapping().is_some_and(|map| map.is_empty()) {
            Ok(())
        } else {
            Err(error(
                ErrorCode::InvalidValue,
                "pluginが設定schemaを提供していない",
            ))
        }
    }
    /// 型、World、service、Systemを登録します。
    fn build(&self, app: &mut AppBuilder) -> EngineResult<()>;
    /// 依存serviceが揃った後の共有定義を準備します。
    fn ready(&self, _app: &mut AppBuilder) -> EngineResult<()> {
        Ok(())
    }
    /// build済みの逆順で終了します。途中の失敗でも残りを試行します。
    fn shutdown(&self) -> EngineResult<()> {
        Ok(())
    }
}

struct Service {
    slot: StableKey,
    implementation: StableKey,
    dependencies: Vec<StableKey>,
    value: Box<dyn Any>,
}

/// 描画adapterが実際の環境を調べて登録する能力です。設定値を能力とみなしません。
#[derive(Clone, Debug)]
pub struct RenderCapabilities {
    /// 使用可能なbackend。先頭はauto選択時のadapterの推奨です。
    pub backends: Vec<String>,
    /// 描画先の各軸上限。
    pub max_dimension: u32,
    /// 格子照明passを提供できるか。
    pub grid_lighting: bool,
}
/// slotごとの排他的な実装登録です。providerの正規状態はWorldに別途登録します。
#[derive(Default)]
pub struct ServiceRegistry {
    entries: Vec<Service>,
    render: Option<RenderCapabilities>,
    audio_available: Option<bool>,
}
impl ServiceRegistry {
    /// slotの存在を調べます。
    pub fn contains(&self, slot: StableKey) -> bool {
        self.entries.iter().any(|entry| entry.slot == slot)
    }
    /// 型付きで実装を借ります。型違いと欠損を区別します。
    pub fn get<T: 'static>(&self, slot: StableKey) -> EngineResult<&T> {
        self.entries
            .iter()
            .find(|entry| entry.slot == slot)
            .ok_or_else(|| error(ErrorCode::MissingDependency, "service slot"))?
            .value
            .downcast_ref()
            .ok_or_else(|| error(ErrorCode::SchemaMismatch, "serviceのRust型"))
    }
    /// manifestへ使うslotと実装キーを安定順で返します。
    pub fn manifest(&self) -> Vec<(StableKey, StableKey)> {
        let mut pairs: Vec<_> = self
            .entries
            .iter()
            .map(|entry| (entry.slot, entry.implementation))
            .collect();
        pairs.sort();
        pairs
    }
    fn validate(&self) -> EngineResult<()> {
        let mut done = Vec::new();
        while done.len() != self.entries.len() {
            let next = self
                .entries
                .iter()
                .filter(|entry| !done.contains(&entry.slot))
                .find(|entry| entry.dependencies.iter().all(|key| done.contains(key)));
            if let Some(next) = next {
                done.push(next.slot);
            } else {
                let missing = self
                    .entries
                    .iter()
                    .any(|entry| entry.dependencies.iter().any(|key| !self.contains(*key)));
                return Err(error(
                    if missing {
                        ErrorCode::MissingDependency
                    } else {
                        ErrorCode::DependencyCycle
                    },
                    "serviceの依存",
                ));
            }
        }
        Ok(())
    }
}

/// 起動前の登録先です。buildが終わるまで公開Runtimeを作りません。
#[derive(Default)]
pub struct AppBuilder {
    settings: EngineSettings,
    worlds: Vec<(StableKey, StateStore)>,
    services: ServiceRegistry,
    systems: Vec<SystemEntry>,
}
impl AppBuilder {
    /// pluginが能力を解決する際に使用する、統合済み共通設定です。
    pub fn settings(&self) -> &EngineSettings {
        &self.settings
    }
    /// Worldの正規状態を登録します。Sceneの終了では破棄されません。
    pub fn world(&mut self, key: StableKey, state: StateStore) -> EngineResult<()> {
        if self.worlds.iter().any(|(existing, _)| *existing == key) {
            return Err(error(ErrorCode::DuplicateKey, "World登録"));
        }
        self.worlds
            .try_reserve(1)
            .map_err(crate::foundation::error::allocation)?;
        self.worlds.push((key, state));
        Ok(())
    }
    /// 排他的slotへ実装を登録します。後勝ちによる置換は認めません。
    pub fn service<T: 'static>(
        &mut self,
        slot: StableKey,
        implementation: StableKey,
        dependencies: Vec<StableKey>,
        value: T,
    ) -> EngineResult<()> {
        if self.services.contains(slot) {
            return Err(error(ErrorCode::DuplicateKey, "service slot"));
        }
        self.services
            .entries
            .try_reserve(1)
            .map_err(crate::foundation::error::allocation)?;
        self.services.entries.push(Service {
            slot,
            implementation,
            dependencies,
            value: Box::new(value),
        });
        Ok(())
    }
    /// 作者Systemとアクセス宣言を登録します。
    pub fn system(
        &mut self,
        descriptor: SystemDescriptor,
        system: impl System,
    ) -> EngineResult<()> {
        if self
            .systems
            .iter()
            .any(|entry| entry.descriptor.key == descriptor.key)
        {
            return Err(error(ErrorCode::DuplicateKey, "System登録"));
        }
        self.systems
            .try_reserve(1)
            .map_err(crate::foundation::error::allocation)?;
        self.systems.push(SystemEntry {
            descriptor,
            system: Box::new(system),
        });
        Ok(())
    }
    /// ready段階で依存serviceを確認します。
    pub fn services(&self) -> &ServiceRegistry {
        &self.services
    }
    /// 描画adapterが取得した能力を一度だけ登録します。
    pub fn render_capabilities(&mut self, profile: RenderCapabilities) -> EngineResult<()> {
        if self.services.render.is_some() {
            return Err(error(ErrorCode::DuplicateKey, "描画能力"));
        }
        self.services.render = Some(profile);
        Ok(())
    }
    /// 音響adapterが装置取得を試した結果です。falseを対応済みとして扱いません。
    pub fn audio_available(&mut self, available: bool) -> EngineResult<()> {
        if self.services.audio_available.is_some() {
            return Err(error(ErrorCode::DuplicateKey, "音響能力"));
        }
        self.services.audio_available = Some(available);
        Ok(())
    }
}

/// 起動設定をチェーン形式で集め、buildで一括検証します。
#[derive(Default)]
pub struct EngineBuilder {
    config: Option<PathBuf>,
    patch: EngineSettingsPatch,
    plugins: Vec<Box<dyn Plugin>>,
    scenes: Vec<SceneEntry>,
    initial: Vec<StableKey>,
    app: AppBuilder,
    failure: Option<EngineError>,
    work_budget: usize,
    same_tick_limit: u64,
    action_capacity: u32,
    scope_capacity: u32,
}
impl EngineBuilder {
    /// YAMLの所在を指定します。
    pub fn config_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.config = Some(path.into());
        self
    }
    /// 明示した項目だけを上書きします。
    pub fn settings(mut self, patch: EngineSettingsPatch) -> Self {
        self.patch = patch;
        self
    }
    /// pluginを所有して登録します。依存順はbuildで決めます。
    pub fn plugin(mut self, plugin: impl Plugin) -> Self {
        self.plugins.push(Box::new(plugin));
        self
    }
    /// Sceneを登録します。短いキーはgame packageで修飾します。
    pub fn scene(mut self, key: &str, scene: impl Scene) -> Self {
        match StableKey::resolve("game", key) {
            Ok(key) if !self.scenes.iter().any(|entry| entry.key == key) => {
                self.scenes.push(SceneEntry {
                    key,
                    state: SceneState::Created,
                    scope: None,
                    scene: Box::new(scene),
                    cleanup: Vec::new(),
                    exclusive: false,
                })
            }
            Ok(_) => self.failure = Some(error(ErrorCode::DuplicateKey, "Scene登録")),
            Err(error) => self.failure = Some(error),
        };
        self
    }
    /// 初期Sceneを指定します。複数回なら順にlaunchします。
    pub fn start_scene(mut self, key: &str) -> Self {
        match StableKey::resolve("game", key) {
            Ok(key) => self.initial.push(key),
            Err(error) => self.failure = Some(error),
        };
        self
    }
    /// WorldをSceneから独立して登録します。
    pub fn world(mut self, key: &str, state: StateStore) -> Self {
        match StableKey::resolve("game", key).and_then(|key| self.app.world(key, state)) {
            Ok(()) => {}
            Err(error) => self.failure = Some(error),
        };
        self
    }
    /// pluginを作らず作者Systemを登録する入口です。
    pub fn system(mut self, descriptor: SystemDescriptor, system: impl System) -> Self {
        if let Err(error) = self.app.system(descriptor, system) {
            self.failure = Some(error);
        }
        self
    }
    /// 一回のpollに割り当てる固定仕事件数です。未完了tickを捨てません。
    pub fn work_budget(mut self, count: usize) -> Self {
        if count == 0 {
            self.failure = Some(error(ErrorCode::InvalidValue, "poll仕事予算"));
        }
        self.work_budget = count;
        self
    }
    /// アクション数の容量を指定します。0はbuildで拒否します。
    pub fn action_capacity(mut self, count: u32) -> Self {
        if count == 0 {
            self.failure = Some(error(ErrorCode::InvalidValue, "アクション容量"));
        }
        self.action_capacity = count;
        self
    }
    /// 同時に保持するScene scope数を指定します。
    pub fn scope_capacity(mut self, count: u32) -> Self {
        if count == 0 {
            self.failure = Some(error(ErrorCode::InvalidValue, "scope容量"));
        }
        self.scope_capacity = count;
        self
    }
    /// 同じSystemが一つのtickで継続できる回数です。既定は65536です。
    ///
    /// pollの件数予算とは独立した決定的な上限です。進捗のない無限SameTickを停止し、
    /// 大きな仕事を意図する作者は必要量へ明示的に引き上げられます。ゼロは禁止します。
    pub fn same_tick_limit(mut self, limit: u64) -> Self {
        if limit == 0 {
            self.failure = Some(error(ErrorCode::InvalidValue, "SameTick継続上限"));
        }
        self.same_tick_limit = limit;
        self
    }
    /// 検証、依存解決、初期Scene準備まで行います。
    pub fn build(mut self) -> EngineResult<Engine> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        let mut settings = EngineSettings::resolve(self.config.as_deref(), &self.patch)?;
        self.app.settings = settings.clone();
        for key in &self.initial {
            if !self.scenes.iter().any(|scene| scene.key == *key) {
                return Err(error(ErrorCode::MissingKey, "初期Scene"));
            }
        }
        let scopes = Arena::new(if self.scope_capacity == 0 {
            4096
        } else {
            self.scope_capacity
        })?;
        let input = InputManager::new(if self.action_capacity == 0 {
            4096
        } else {
            self.action_capacity
        })?;
        self.plugins.sort_by_key(|plugin| plugin.key());
        if self
            .plugins
            .windows(2)
            .any(|pair| pair[0].key() == pair[1].key())
        {
            return Err(error(ErrorCode::DuplicateKey, "plugin登録"));
        }
        for key in settings.plugins.keys() {
            if !self
                .plugins
                .iter()
                .any(|plugin| plugin.key().as_str() == key)
            {
                return Err(error(
                    ErrorCode::MissingDependency,
                    "未登録pluginの実行設定",
                ));
            }
        }
        let mut ordered: Vec<Box<dyn Plugin>> = Vec::new();
        while !self.plugins.is_empty() {
            let index = self.plugins.iter().position(|plugin| {
                plugin
                    .dependencies()
                    .iter()
                    .all(|key| ordered.iter().any(|prior| prior.key() == *key))
            });
            let Some(index) = index else {
                let missing = self.plugins.iter().any(|plugin| {
                    plugin.dependencies().iter().any(|key| {
                        !self
                            .plugins
                            .iter()
                            .chain(ordered.iter())
                            .any(|other| other.key() == *key)
                    })
                });
                return Err(error(
                    if missing {
                        ErrorCode::MissingDependency
                    } else {
                        ErrorCode::DependencyCycle
                    },
                    "plugin依存",
                ));
            };
            ordered.push(self.plugins.remove(index));
        }
        let mut built = 0;
        let result = (|| {
            for plugin in &ordered {
                // buildで部分準備したpluginも終了対象に含めます。
                built += 1;
                plugin.configure(
                    settings
                        .plugins
                        .get(plugin.key().as_str())
                        .unwrap_or(&serde_yaml::Value::Null),
                )?;
                plugin.build(&mut self.app)?;
            }
            self.app.services.validate()?;
            for plugin in &ordered {
                plugin.ready(&mut self.app)?;
            }
            self.app.services.validate()?;
            if settings.audio.required
                && (self.app.services.audio_available != Some(true)
                    || !self
                        .app
                        .services
                        .contains(StableKey::parse("engine:audio")?))
            {
                return Err(error(
                    ErrorCode::UnsupportedCapability,
                    "必須audio providerがない",
                ));
            }
            if settings.render.backend != "auto"
                && !self
                    .app
                    .services
                    .contains(StableKey::parse("engine:render")?)
            {
                return Err(error(
                    ErrorCode::UnsupportedCapability,
                    "明示したrender backendのproviderがない",
                ));
            }
            if self
                .app
                .services
                .contains(StableKey::parse("engine:render")?)
            {
                let capabilities =
                    self.app.services.render.as_ref().ok_or_else(|| {
                        error(ErrorCode::UnsupportedCapability, "描画能力が未検証")
                    })?;
                let backend = if settings.render.backend == "auto" {
                    capabilities
                        .backends
                        .first()
                        .ok_or_else(|| {
                            error(
                                ErrorCode::UnsupportedCapability,
                                "利用可能な描画backendがない",
                            )
                        })?
                        .clone()
                } else {
                    settings.render.backend.clone()
                };
                if !capabilities.backends.contains(&backend)
                    || !["vulkan", "dx12", "metal"].contains(&backend.as_str())
                    || capabilities.max_dimension == 0
                    || (settings.render.lighting == "grid" && !capabilities.grid_lighting)
                {
                    return Err(error(
                        ErrorCode::UnsupportedCapability,
                        "要求した描画機能は利用できない",
                    ));
                }
                for dimension in [settings.window.width, settings.window.height] {
                    let dimension = f64::from(dimension) * settings.render.resolution_scale;
                    if !dimension.is_finite()
                        || dimension.ceil() > f64::from(capabilities.max_dimension)
                    {
                        return Err(error(ErrorCode::UnsupportedCapability, "描画先の寸法上限"));
                    }
                }
                settings.render.backend = backend;
            }
            Ok(())
        })();
        if let Err(error) = result {
            for plugin in ordered[..built].iter().rev() {
                let _ = plugin.shutdown();
            }
            return Err(error);
        }
        let systems = match schedule::plan(self.app.systems, &self.app.services) {
            Ok(plan) => plan,
            Err(error) => {
                for plugin in ordered.iter().rev() {
                    let _ = plugin.shutdown();
                }
                return Err(error);
            }
        };
        for system in &systems {
            if system
                .descriptor
                .world
                .is_some_and(|key| !self.app.worlds.iter().any(|(world, _)| *world == key))
            {
                for plugin in ordered.iter().rev() {
                    let _ = plugin.shutdown();
                }
                return Err(error(ErrorCode::MissingDependency, "SystemのWorld所属"));
            }
            if system
                .descriptor
                .scene
                .is_some_and(|key| !self.scenes.iter().any(|entry| entry.key == key))
            {
                for plugin in ordered.iter().rev() {
                    let _ = plugin.shutdown();
                }
                return Err(error(ErrorCode::MissingDependency, "SystemのScene所属"));
            }
        }
        self.app.worlds.sort_by_key(|(key, _)| *key);
        let worlds = match WorldSet::new(self.app.worlds) {
            Ok(worlds) => worlds,
            Err(error) => {
                for plugin in ordered.iter().rev() {
                    let _ = plugin.shutdown();
                }
                return Err(error);
            }
        };
        let limit = settings.limits.commands_per_tick as usize;
        let mut engine = Engine {
            settings,
            worlds,
            services: self.app.services,
            plugins: ordered,
            scenes: self.scenes,
            systems,
            scopes,
            input,
            requests: Vec::new(),
            lifecycle_commands: CommandBuffer::new(limit),
            tick: 0,
            pending: None,
            failure: None,
            quit: false,
            closed: false,
            work_budget: if self.work_budget == 0 {
                1024
            } else {
                self.work_budget
            },
            same_tick_limit: if self.same_tick_limit == 0 {
                65536
            } else {
                self.same_tick_limit
            },
            clock_units: 0,
            buffered_input: Vec::new(),
            input_resets: Vec::new(),
            last_input: None,
            scene_order: Vec::new(),
        };
        for key in self.initial {
            engine.requests.push(SceneRequest::Start(key));
        }
        if let Err(error) = engine.transitions() {
            engine.fail(error.clone());
            return Err(error);
        }
        engine.flush_lifecycle()?;
        Ok(engine)
    }
}

struct WorldEntry {
    id: WorldId,
    key: StableKey,
    state: StateStore,
    tick: u64,
    paused: bool,
}
pub(crate) struct WorldSet {
    entries: Vec<WorldEntry>,
}
impl WorldSet {
    fn new(worlds: Vec<(StableKey, StateStore)>) -> EngineResult<Self> {
        let mut ids: Arena<u8, WorldKind> = Arena::new(
            u32::try_from(worlds.len().max(1))
                .map_err(|_| error(ErrorCode::CapacityExceeded, "World数"))?,
        )?;
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(worlds.len())
            .map_err(crate::foundation::error::allocation)?;
        for (key, state) in worlds {
            entries.push(WorldEntry {
                id: ids.insert(0)?,
                key,
                state,
                tick: 0,
                paused: false,
            });
        }
        Ok(Self { entries })
    }
    fn copy(&self) -> EngineResult<Self> {
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(self.entries.len())
            .map_err(crate::foundation::error::allocation)?;
        for entry in &self.entries {
            entries.push(WorldEntry {
                id: entry.id,
                key: entry.key,
                state: entry.state.try_copy()?,
                tick: entry.tick,
                paused: entry.paused,
            });
        }
        Ok(Self { entries })
    }
    fn get(&self, id: WorldId) -> EngineResult<&WorldEntry> {
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .ok_or_else(|| error(ErrorCode::StaleId, "World参照"))
    }
    fn get_mut(&mut self, id: WorldId) -> EngineResult<&mut WorldEntry> {
        self.entries
            .iter_mut()
            .find(|entry| entry.id == id)
            .ok_or_else(|| error(ErrorCode::StaleId, "World参照"))
    }
}

/// 不変のWorld情報。Systemの場合はアクセス宣言も検証します。
pub struct WorldView<'a> {
    pub(crate) worlds: &'a WorldSet,
    pub(crate) access: Option<&'a SystemDescriptor>,
}
impl WorldView<'_> {
    /// 安定キーをWorldIdへ解決します。
    pub fn id(&self, key: &str) -> EngineResult<WorldId> {
        let key = StableKey::resolve("game", key)?;
        self.worlds
            .entries
            .iter()
            .find(|entry| entry.key == key)
            .map(|entry| entry.id)
            .ok_or_else(|| error(ErrorCode::MissingKey, "Worldキー"))
    }
    /// 最後のbarrierで確定した作者recordを読みます。
    pub fn get<T: StateCodec + 'static>(&self, world: WorldId, key: StableKey) -> EngineResult<T> {
        check_access(self.access, key, false)?;
        self.worlds.get(world)?.state.get(key)
    }
    /// World独自のtick。停止したWorldでは増えません。
    pub fn tick(&self, world: WorldId) -> EngineResult<u64> {
        Ok(self.worlds.get(world)?.tick)
    }
    /// 保存・診断用に公開状態のcheckpointを取得します。
    pub fn checkpoint(&self, world: WorldId) -> EngineResult<Vec<u8>> {
        if self.access.is_some() {
            return Err(error(
                ErrorCode::InvalidState,
                "Systemは全recordの読取権限を持たない",
            ));
        }
        self.worlds.get(world)?.state.checkpoint()
    }
}

/// 公開Viewの読取と、後のbarrierへ反映する書込を分けたFacade。
pub struct WorldAccess<'a> {
    pub(crate) worlds: &'a WorldSet,
    pub(crate) commands: &'a mut CommandBuffer,
    pub(crate) access: Option<&'a SystemDescriptor>,
}
impl WorldAccess<'_> {
    /// 読取専用のViewへ移ります。
    pub fn view(&self) -> WorldView<'_> {
        WorldView {
            worlds: self.worlds,
            access: self.access,
        }
    }
    /// 作者recordの変更を提出します。この場でViewは変わりません。
    pub fn set<T: StateCodec + 'static>(
        &mut self,
        world: WorldId,
        key: StableKey,
        schema: StableKey,
        value: &T,
    ) -> EngineResult<()> {
        let result = (|| {
            check_access(self.access, key, true)?;
            let payload = self.worlds.get(world)?.state.encode_value(schema, value)?;
            self.commands.push(Command::Put {
                world,
                key,
                schema,
                payload,
            })
        })();
        self.commands.remember(result)
    }
    /// Worldの更新を止めます。Sceneのpauseとは独立です。
    pub fn pause(&mut self, world: WorldId, paused: bool) -> EngineResult<()> {
        let result = (|| {
            if self.access.is_some() {
                return Err(error(ErrorCode::InvalidState, "SystemのWorld寿命変更権限"));
            }
            self.worlds.get(world)?;
            self.commands.push(Command::Pause { world, paused })
        })();
        self.commands.remember(result)
    }
}
fn check_access(
    access: Option<&SystemDescriptor>,
    key: StableKey,
    write: bool,
) -> EngineResult<()> {
    if access.is_some_and(|descriptor| !descriptor.allows(key, write)) {
        Err(error(
            ErrorCode::InvalidState,
            "Systemが未宣言のrecordへアクセス",
        ))
    } else {
        Ok(())
    }
}

// 固定長キーをinlineで保持します。小さいPauseのために各Putへ追加の失敗不能なBox確保を
// 導入せず、queue全体のtry_reserveで容量と確保失敗を検査できることを優先します。
#[allow(clippy::large_enum_variant)]
enum Command {
    Put {
        world: WorldId,
        key: StableKey,
        schema: StableKey,
        payload: Vec<u8>,
    },
    Pause {
        world: WorldId,
        paused: bool,
    },
}
pub(crate) struct CommandBuffer {
    commands: Vec<Command>,
    failure: Option<EngineError>,
    total: usize,
    limit: usize,
}
impl CommandBuffer {
    fn new(limit: usize) -> Self {
        Self {
            commands: Vec::new(),
            failure: None,
            total: 0,
            limit,
        }
    }
    fn push(&mut self, command: Command) -> EngineResult<()> {
        if self.total >= self.limit {
            return Err(EngineError::capacity(
                "tick内のcommand数",
                self.total as u64 + 1,
                self.limit as u64,
            ));
        }
        self.commands
            .try_reserve(1)
            .map_err(crate::foundation::error::allocation)?;
        self.commands.push(command);
        self.total += 1;
        Ok(())
    }
    fn remember(&mut self, result: EngineResult<()>) -> EngineResult<()> {
        if let Err(error) = &result {
            self.failure.get_or_insert_with(|| error.clone());
        }
        result
    }
    fn flush(&mut self, worlds: &mut WorldSet) -> EngineResult<()> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        for command in self.commands.drain(..) {
            match command {
                Command::Put {
                    world,
                    key,
                    schema,
                    payload,
                } => {
                    let state = &mut worlds.get_mut(world)?.state;
                    let mut transaction = state.transaction()?;
                    transaction.put_encoded(key, schema, payload)?;
                    transaction.commit()?;
                }
                Command::Pause { world, paused } => worlds.get_mut(world)?.paused = paused,
            }
        }
        Ok(())
    }
}

struct PendingTick {
    tick: u64,
    input: TickInput,
    worlds: WorldSet,
    commands: CommandBuffer,
    cursor: usize,
    scenes_updated: bool,
    same_tick_runs: u64,
}

/// 一回のpollでtickが完了したかを区別します。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepResult {
    /// 全候補を公開した。
    Complete(u64),
    /// 同tickに残作業がある。
    Pending(u64),
    /// 終了要求を処理済み。
    Quit,
}

/// Scene、World、serviceを所有するRuntime。描画なしでもそのまま使用できます。
pub struct Engine {
    pub(crate) settings: EngineSettings,
    worlds: WorldSet,
    services: ServiceRegistry,
    plugins: Vec<Box<dyn Plugin>>,
    scenes: Vec<SceneEntry>,
    systems: Vec<SystemEntry>,
    scopes: Arena<u8, ScopeKind>,
    input: InputManager,
    requests: Vec<SceneRequest>,
    lifecycle_commands: CommandBuffer,
    tick: u64,
    pending: Option<PendingTick>,
    failure: Option<EngineError>,
    quit: bool,
    closed: bool,
    work_budget: usize,
    same_tick_limit: u64,
    clock_units: u128,
    buffered_input: Vec<(u64, super::input::InputEvent)>,
    input_resets: Vec<u64>,
    last_input: Option<TickInput>,
    scene_order: Vec<StableKey>,
}
/// 同じRuntimeをwindowなしでstepする際の名前です。別の物理経路は持ちません。
pub type HeadlessRuntime = Engine;

impl Engine {
    /// 未起動の設定を作ります。
    pub fn builder() -> EngineBuilder {
        EngineBuilder::default()
    }
    /// 解決済み設定です。実行中に物理profileを無断で変更しません。
    pub fn settings(&self) -> &EngineSettings {
        &self.settings
    }
    /// 公開済みtick。
    pub fn tick(&self) -> u64 {
        self.tick
    }
    /// OS adapterのeventを次の未開始tickへ蓄積します。
    pub fn queue_input(&mut self, event: super::input::InputEvent) -> EngineResult<()> {
        self.queue_input_at(self.next_input_tick()?, event)
    }
    /// 機器adapterで量子化した対象tickへ予約します。描画頻度から切り離します。
    pub fn queue_input_at(
        &mut self,
        tick: u64,
        event: super::input::InputEvent,
    ) -> EngineResult<()> {
        self.healthy()?;
        if tick < self.next_input_tick()? {
            return Err(error(ErrorCode::InvalidValue, "開始済みtickへの入力"));
        }
        if self.buffered_input.len() >= self.settings.limits.commands_per_tick as usize {
            return Err(error(ErrorCode::CapacityExceeded, "OS入力queue"));
        }
        self.buffered_input
            .try_reserve(1)
            .map_err(crate::foundation::error::allocation)?;
        self.buffered_input.push((tick, event));
        Ok(())
    }
    /// 次tickで機器stateを中立へ戻します。
    pub fn reset_input(&mut self) -> EngineResult<()> {
        self.queue_reset_at(self.next_input_tick()?)
    }
    /// focus喪失等を対象tickへ予約します。そのtickの喪失前eventは除外します。
    pub fn queue_reset_at(&mut self, tick: u64) -> EngineResult<()> {
        self.healthy()?;
        if tick < self.next_input_tick()? {
            return Err(error(ErrorCode::InvalidValue, "開始済みtickの入力reset"));
        }
        if !self.input_resets.contains(&tick) {
            if self.input_resets.len() >= self.settings.limits.commands_per_tick as usize {
                return Err(error(ErrorCode::CapacityExceeded, "入力reset queue"));
            }
            self.input_resets
                .try_reserve(1)
                .map_err(crate::foundation::error::allocation)?;
            self.input_resets.push(tick);
        }
        self.buffered_input.retain(|(target, _)| *target != tick);
        Ok(())
    }
    /// 外部eventを受け付けられる最初のtick。pending自身は変更しません。
    pub fn next_input_tick(&self) -> EngineResult<u64> {
        self.pending
            .as_ref()
            .map(|pending| pending.tick)
            .unwrap_or(self.tick)
            .checked_add(1)
            .ok_or_else(|| error(ErrorCode::CapacityExceeded, "入力対象tick"))
    }
    /// 最後に公開したtickに実際に使用した入力です。録画・replay側が取得できます。
    pub fn last_input(&self) -> Option<&TickInput> {
        self.last_input.as_ref()
    }
    /// 表示用補間です。Worldやclock profileを書き換えません。
    pub fn interpolation(&self) -> f64 {
        (self.clock_units as f64 / 1_000_000_000.0).min(1.0)
    }
    /// backlogまたはpendingがあれば即時再開し、過負荷分を捨てません。
    pub fn until_next_tick(&self) -> Duration {
        if self.pending.is_some() || self.clock_units >= 1_000_000_000 {
            Duration::ZERO
        } else {
            Duration::from_nanos(
                (1_000_000_000 - self.clock_units)
                    .div_ceil(u128::from(self.settings.simulation.step_hz)) as u64,
            )
        }
    }
    /// 公開Worldへの読み取りだけを返します。
    pub fn worlds(&self) -> WorldView<'_> {
        WorldView {
            worlds: &self.worlds,
            access: None,
        }
    }
    /// 起動で確定したSystem順序・権限・barrierを複製します。毎tickの取得は避けてください。
    pub fn schedule_profile(&self) -> EngineResult<super::schedule::ScheduleProfile> {
        let mut systems = Vec::new();
        systems
            .try_reserve_exact(self.systems.len())
            .map_err(crate::foundation::error::allocation)?;
        systems.extend(self.systems.iter().map(|entry| entry.descriptor.clone()));
        Ok(super::schedule::ScheduleProfile {
            systems,
            same_tick_limit: self.same_tick_limit,
        })
    }
    /// Sceneの現在の状態を取得します。
    pub fn scene_state(&self, key: &str) -> EngineResult<SceneState> {
        Ok(self.scenes[self.scene_index(StableKey::resolve("game", key)?)?].state)
    }
    /// 次の固定tickを開始します。未完了tickにはresumeを使用します。
    pub fn step(&mut self, mut input: TickInput) -> EngineResult<StepResult> {
        self.healthy()?;
        if self.quit {
            return Ok(StepResult::Quit);
        }
        if self.pending.is_some() || self.tick.checked_add(1) != Some(input.tick) {
            return Err(error(ErrorCode::InvalidState, "tick番号または未完了tick"));
        }
        let scheduled = self
            .buffered_input
            .iter()
            .filter(|(tick, _)| *tick == input.tick)
            .count();
        let total = input
            .events
            .len()
            .checked_add(scheduled)
            .ok_or_else(|| error(ErrorCode::CapacityExceeded, "tick入力数"))?;
        if total > self.settings.limits.commands_per_tick as usize {
            return Err(EngineError::capacity(
                "tick入力数",
                total as u64,
                u64::from(self.settings.limits.commands_per_tick),
            ));
        }
        input
            .events
            .try_reserve(scheduled)
            .map_err(crate::foundation::error::allocation)?;
        input.events.extend(
            self.buffered_input
                .iter()
                .filter(|(tick, _)| *tick == input.tick)
                .map(|(_, event)| *event),
        );
        input.reset |= self.input_resets.contains(&input.tick);
        input
            .events
            .sort_by_key(|event| (event.source, event.sequence));
        if input.events.len() > self.settings.limits.commands_per_tick as usize {
            return Err(EngineError::capacity(
                "tick入力数",
                input.events.len() as u64,
                u64::from(self.settings.limits.commands_per_tick),
            ));
        }
        // 不正な外部入力は再提出できます。取込が成功するまでは予約とRuntimeを保持します。
        self.input.apply(&input)?;
        self.buffered_input.retain(|(tick, _)| *tick != input.tick);
        self.input_resets.retain(|tick| *tick != input.tick);
        let result = (|| {
            let pending = PendingTick {
                tick: input.tick,
                input,
                worlds: self.worlds.copy()?,
                commands: CommandBuffer::new(self.settings.limits.commands_per_tick as usize),
                cursor: 0,
                scenes_updated: false,
                same_tick_runs: 0,
            };
            self.pending = Some(pending);
            self.resume()
        })();
        if let Err(error) = &result {
            self.fail(error.clone());
        }
        result
    }
    // Sceneは論理tickにつき一度だけ呼びます。Systemの再開予算では繰り返しません。
    fn update_scenes(&mut self, pending: &mut PendingTick) -> EngineResult<()> {
        let order = self.scene_order.clone();
        let exclusive = order.iter().rposition(|key| {
            self.scenes.iter().any(|entry| {
                entry.key == *key && entry.state == SceneState::Running && entry.exclusive
            })
        });
        for (stack_index, key) in order.into_iter().enumerate() {
            let entry_index = self.scene_index(key)?;
            let entry = &mut self.scenes[entry_index];
            if entry.state != SceneState::Running {
                continue;
            }
            let scope = entry
                .scope
                .ok_or_else(|| error(ErrorCode::InvalidState, "Scene scope"))?;
            let mut context = SceneContext {
                tick: pending.tick,
                key: entry.key,
                scope,
                settings: &self.settings,
                services: &self.services,
                worlds: &self.worlds,
                commands: &mut pending.commands,
                input: &mut self.input,
                input_enabled: exclusive.is_none_or(|index| stack_index >= index),
                requests: &mut self.requests,
                cleanup: &mut entry.cleanup,
                request_limit: self.settings.limits.commands_per_tick as usize,
            };
            if let Err(error) = entry.scene.update(&mut context) {
                entry.state = SceneState::Failed;
                return Err(error);
            }
        }
        // SceneのX1変更を最初の作者Systemの候補Viewへ反映します。
        pending.commands.flush(&mut pending.worlds)?;
        pending.scenes_updated = true;
        Ok(())
    }
    /// 件数予算を追加して同じtickのSystem cursorから再開します。
    pub fn resume(&mut self) -> EngineResult<StepResult> {
        self.healthy()?;
        let mut pending = self
            .pending
            .take()
            .ok_or_else(|| error(ErrorCode::InvalidState, "再開するtickがない"))?;
        let result = (|| {
            for _ in 0..self.work_budget {
                if !pending.scenes_updated
                    && (pending.cursor == self.systems.len()
                        || self.systems[pending.cursor].descriptor.phase
                            != super::schedule::Phase::BeforeScenes)
                {
                    self.update_scenes(&mut pending)?;
                }
                if pending.cursor == self.systems.len() {
                    break;
                }
                let entry = &mut self.systems[pending.cursor];
                if entry.descriptor.world.is_some_and(|key| {
                    pending
                        .worlds
                        .entries
                        .iter()
                        .any(|world| world.key == key && world.paused)
                }) {
                    pending.cursor += 1;
                    continue;
                }
                if entry.descriptor.scene.is_some_and(|key| {
                    !self
                        .scenes
                        .iter()
                        .any(|scene| scene.key == key && scene.state == SceneState::Running)
                }) {
                    pending.cursor += 1;
                    continue;
                }
                let status = entry.system.run(&mut SystemContext {
                    tick: pending.tick,
                    worlds: &pending.worlds,
                    commands: &mut pending.commands,
                    services: &self.services,
                    descriptor: &entry.descriptor,
                    input: &pending.input,
                })?;
                if entry.descriptor.barrier_after {
                    pending.commands.flush(&mut pending.worlds)?;
                }
                if status == WorkStatus::ContinueSameTick {
                    pending.same_tick_runs = pending
                        .same_tick_runs
                        .checked_add(1)
                        .ok_or_else(|| error(ErrorCode::CapacityExceeded, "SameTick継続回数"))?;
                    if pending.same_tick_runs >= self.same_tick_limit {
                        return Err(EngineError::capacity(
                            "SameTick継続上限",
                            pending.same_tick_runs.saturating_add(1),
                            self.same_tick_limit,
                        ));
                    }
                    if !entry.descriptor.barrier_after && !pending.commands.commands.is_empty() {
                        return Err(error(
                            ErrorCode::InvalidState,
                            "SameTickの書込にはbarrierが必要",
                        ));
                    }
                } else {
                    pending.same_tick_runs = 0;
                    let phase = entry.descriptor.phase;
                    pending.cursor += 1;
                    if pending.cursor == self.systems.len()
                        || self.systems[pending.cursor].descriptor.phase != phase
                    {
                        pending.commands.flush(&mut pending.worlds)?;
                    }
                }
            }
            if pending.cursor < self.systems.len() {
                return Ok(false);
            }
            if !pending.scenes_updated {
                self.update_scenes(&mut pending)?;
            }
            pending.commands.flush(&mut pending.worlds)?;
            for world in &mut pending.worlds.entries {
                if !world.paused {
                    world.tick = world
                        .tick
                        .checked_add(1)
                        .ok_or_else(|| error(ErrorCode::CapacityExceeded, "World tick"))?;
                }
                world.state.validate()?;
            }
            Ok(true)
        })();
        match result {
            Ok(false) => {
                let tick = pending.tick;
                self.pending = Some(pending);
                Ok(StepResult::Pending(tick))
            }
            Ok(true) => {
                let previous = std::mem::replace(&mut self.worlds, pending.worlds);
                let previous_tick = self.tick;
                self.tick = pending.tick;
                if let Err(error) = self.transitions().and_then(|_| self.flush_lifecycle()) {
                    self.worlds = previous;
                    self.tick = previous_tick;
                    self.fail(error.clone());
                    return Err(error);
                }
                self.last_input = Some(pending.input);
                Ok(if self.quit {
                    StepResult::Quit
                } else {
                    StepResult::Complete(self.tick)
                })
            }
            Err(error) => {
                self.fail(error.clone());
                Err(error)
            }
        }
    }
    /// 表示だけを更新します。pending中でも最後の公開Worldを使用します。
    pub fn frame(&mut self, alpha: f64) -> EngineResult<()> {
        self.healthy()?;
        if !alpha.is_finite() || !(0.0..=1.0).contains(&alpha) {
            return Err(error(ErrorCode::InvalidValue, "表示補間alpha"));
        }
        for key in self.scene_order.clone() {
            let index = self.scene_index(key)?;
            let entry = &mut self.scenes[index];
            if matches!(entry.state, SceneState::Running | SceneState::Paused)
                && let Err(error) = entry.scene.render(&mut FrameContext {
                    tick: self.tick,
                    alpha,
                    worlds: &self.worlds,
                    services: &self.services,
                })
            {
                entry.state = SceneState::Failed;
                self.fail(error.clone());
                return Err(error);
            }
        }
        Ok(())
    }
    /// wall-clockの余りを整数で保持し、完了したtickだけ予算から取り出します。
    pub fn advance(&mut self, elapsed: Duration, max_ticks: u32) -> EngineResult<u32> {
        self.healthy()?;
        self.clock_units = self
            .clock_units
            .checked_add(
                elapsed
                    .as_nanos()
                    .checked_mul(u128::from(self.settings.simulation.step_hz))
                    .ok_or_else(|| error(ErrorCode::CapacityExceeded, "経過時間"))?,
            )
            .ok_or_else(|| error(ErrorCode::CapacityExceeded, "時計のbacklog"))?;
        let mut completed = 0;
        while self.clock_units >= 1_000_000_000 && completed < max_ticks && !self.quit {
            let result = if self.pending.is_some() {
                self.resume()?
            } else {
                let input = TickInput {
                    tick: self
                        .tick
                        .checked_add(1)
                        .ok_or_else(|| error(ErrorCode::CapacityExceeded, "Engine tick"))?,
                    events: Vec::new(),
                    reset: false,
                };
                self.step(input)?
            };
            if matches!(result, StepResult::Pending(_)) {
                break;
            }
            self.clock_units -= 1_000_000_000;
            completed += 1;
        }
        Ok(completed)
    }
    /// 全Sceneとpluginを終了します。繰返し呼んでも二重解放しません。
    pub fn shutdown(&mut self) -> EngineResult<()> {
        if self.closed {
            return Ok(());
        }
        self.pending = None;
        let mut first = None;
        for key in self.scene_order.clone().into_iter().rev() {
            let index = self.scene_index(key)?;
            if let Err(error) = self.stop_scene(index) {
                first.get_or_insert(error);
            }
        }
        for index in (0..self.scenes.len()).rev() {
            if let Err(error) = self.stop_scene(index) {
                first.get_or_insert(error);
            }
        }
        for plugin in self.plugins.iter().rev() {
            if let Err(error) = plugin.shutdown() {
                first.get_or_insert(error);
            }
        }
        self.closed = true;
        if let Some(error) = first {
            Err(error)
        } else {
            Ok(())
        }
    }
    pub(crate) fn is_quit(&self) -> bool {
        self.quit
    }
    pub(crate) fn attach_window(&mut self, window: super::host::DesktopWindow) -> EngineResult<()> {
        let slot = StableKey::parse("engine:window")?;
        if self.services.contains(slot) {
            return Err(error(ErrorCode::DuplicateKey, "OS window slot"));
        }
        self.services.entries.push(Service {
            slot,
            implementation: StableKey::parse("engine:winit")?,
            dependencies: Vec::new(),
            value: Box::new(window),
        });
        Ok(())
    }
    fn healthy(&self) -> EngineResult<()> {
        if let Some(error) = &self.failure {
            Err(error.clone())
        } else if self.closed {
            Err(error(ErrorCode::InvalidState, "終了したRuntime"))
        } else {
            Ok(())
        }
    }
    fn scene_index(&self, key: StableKey) -> EngineResult<usize> {
        self.scenes
            .iter()
            .position(|entry| entry.key == key)
            .ok_or_else(|| error(ErrorCode::MissingKey, "Sceneキー"))
    }
    fn fail(&mut self, error: EngineError) {
        self.failure = Some(error);
        self.pending = None;
        let _ = self.shutdown();
    }
    fn stop_scene(&mut self, index: usize) -> EngineResult<()> {
        let entry = &mut self.scenes[index];
        self.scene_order.retain(|key| *key != entry.key);
        let Some(scope) = entry.scope.take() else {
            return Ok(());
        };
        let mut first = entry
            .scene
            .stop(&mut StopContext {
                scope,
                worlds: &self.worlds,
            })
            .err();
        while let Some(cleanup) = entry.cleanup.pop() {
            if let Err(error) = cleanup() {
                first.get_or_insert(error);
            }
        }
        if let Err(error) = self.input.release_scope(scope) {
            first.get_or_insert(error);
        }
        if let Err(error) = self.scopes.remove(scope) {
            first.get_or_insert(error);
        }
        entry.state = if first.is_some() || entry.state == SceneState::Failed {
            SceneState::Failed
        } else {
            SceneState::Stopped
        };
        if let Some(error) = first {
            Err(error)
        } else {
            Ok(())
        }
    }
    fn start_scene(&mut self, index: usize) -> EngineResult<()> {
        if matches!(
            self.scenes[index].state,
            SceneState::Running | SceneState::Paused
        ) {
            return Ok(());
        }
        let scope = self.scopes.insert(0)?;
        let entry = &mut self.scenes[index];
        entry.scope = Some(scope);
        entry.state = SceneState::Loading;
        if let Err(error) = entry.scene.load(&mut LoadContext {
            settings: &self.settings,
            services: &self.services,
        }) {
            entry.state = SceneState::Failed;
            return Err(error);
        }
        entry.state = SceneState::Ready;
        let mut context = SceneContext {
            tick: self.tick,
            key: entry.key,
            scope,
            settings: &self.settings,
            services: &self.services,
            worlds: &self.worlds,
            commands: &mut self.lifecycle_commands,
            input: &mut self.input,
            input_enabled: true,
            requests: &mut self.requests,
            cleanup: &mut entry.cleanup,
            request_limit: self.settings.limits.commands_per_tick as usize,
        };
        if let Err(error) = entry.scene.start(&mut context) {
            entry.state = SceneState::Failed;
            return Err(error);
        }
        entry.state = SceneState::Running;
        self.scene_order.push(entry.key);
        Ok(())
    }
    fn transitions(&mut self) -> EngineResult<()> {
        let limit = self.settings.limits.commands_per_tick as usize;
        let mut count = 0;
        while !self.requests.is_empty() {
            if count >= limit {
                return Err(error(ErrorCode::CapacityExceeded, "Scene遷移の循環"));
            }
            count += 1;
            let request = self.requests.remove(0);
            match request {
                SceneRequest::Start(key) => {
                    let index = self.scene_index(key)?;
                    self.start_scene(index)?;
                }
                SceneRequest::Switch(source, target) => {
                    let next = self.scene_index(target)?;
                    let current = self.scene_index(source)?;
                    self.stop_scene(current)?;
                    self.start_scene(next)?;
                }
                SceneRequest::Pause(key) => {
                    let index = self.scene_index(key)?;
                    if self.scenes[index].state != SceneState::Running {
                        return Err(error(ErrorCode::InvalidState, "pause対象"));
                    }
                    self.scenes[index].state = SceneState::Paused;
                }
                SceneRequest::Resume(key) => {
                    let index = self.scene_index(key)?;
                    if self.scenes[index].state != SceneState::Paused {
                        return Err(error(ErrorCode::InvalidState, "resume対象"));
                    }
                    self.scenes[index].state = SceneState::Running;
                }
                SceneRequest::Stop(key) => {
                    let index = self.scene_index(key)?;
                    self.stop_scene(index)?;
                }
                SceneRequest::Remove(key) => {
                    let index = self.scene_index(key)?;
                    self.stop_scene(index)?;
                    self.scenes.remove(index);
                }
                SceneRequest::InputMode(key, exclusive) => {
                    let index = self.scene_index(key)?;
                    self.scenes[index].exclusive = exclusive;
                }
                SceneRequest::Quit => {
                    self.quit = true;
                }
            }
        }
        Ok(())
    }
    fn flush_lifecycle(&mut self) -> EngineResult<()> {
        let mut candidate = self.worlds.copy()?;
        self.lifecycle_commands.flush(&mut candidate)?;
        self.worlds = candidate;
        self.lifecycle_commands.total = 0;
        Ok(())
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

pub(crate) fn error(code: ErrorCode, context: &'static str) -> EngineError {
    EngineError::new(code, context)
}
