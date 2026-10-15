//! Sceneの所有期間と、一回の呼出しにだけ有効なContext。
//!
//! 遷移要求はqueueへ入り、callback中のSceneを破棄しません。停止後はscopeを
//! 回収し、再開始で古い入力IDやcleanupを再利用しないようにします。

use super::{
    config::EngineSettings,
    engine::{CommandBuffer, ServiceRegistry, WorldAccess, WorldSet},
    input::{InputAccess, InputManager, ScopeId},
};
use crate::foundation::{EngineResult, StableKey};

/// Sceneの現在のlifecycleです。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneState {
    /// 登録されたが起動していない。
    Created,
    /// load callbackを実行中。
    Loading,
    /// load済みでstart前。
    Ready,
    /// 固定更新と描画の対象。
    Running,
    /// 固定更新だけを止め、scopeと描画を保持。
    Paused,
    /// scopeを終了した。
    Stopped,
    /// callbackまたは依存処理に失敗した。
    Failed,
}

/// 作者が実装するScene。Send/Syncを一律に要求しません。
pub trait Scene: 'static {
    /// この開始期間に必要な同期準備を行います。失敗ならstartしません。
    fn load(&mut self, _context: &mut LoadContext<'_>) -> EngineResult<()> {
        Ok(())
    }
    /// 新しいscopeで一度だけ呼ばれます。pauseからのresumeでは呼びません。
    fn start(&mut self, _context: &mut SceneContext<'_>) -> EngineResult<()> {
        Ok(())
    }
    /// 固定tickのゲーム処理です。永続的なゲーム状態はWorld側へ置きます。
    fn update(&mut self, _context: &mut SceneContext<'_>) -> EngineResult<()> {
        Ok(())
    }
    /// 表示frameごとの処理です。正規Worldの変更権限はありません。
    fn render(&mut self, _context: &mut FrameContext<'_>) -> EngineResult<()> {
        Ok(())
    }
    /// 正常停止でも失敗時でも後始末を行います。失敗してもscopeは回収されます。
    fn stop(&mut self, _context: &mut StopContext<'_>) -> EngineResult<()> {
        Ok(())
    }
}

/// loadで使える設定とサービスです。資産の具体的なdecodeはE03のproviderが担います。
pub struct LoadContext<'a> {
    pub(crate) scope: ScopeId,
    pub(crate) settings: &'a EngineSettings,
    pub(crate) assets: &'a mut crate::assets::AssetServer,
    pub(crate) services: &'a ServiceRegistry,
}
impl LoadContext<'_> {
    /// このSceneの開始期間が保持する資産を読み込みます。停止時に保持権を一括解除します。
    pub fn assets(&mut self) -> crate::assets::AssetLoader<'_> {
        self.assets
            .loader(self.scope)
            .expect("実行中Sceneの資産scope")
    }

    /// 解決済み設定。
    pub fn settings(&self) -> &EngineSettings {
        self.settings
    }
    /// 作者の同期loader等の登録先。
    pub fn services(&self) -> &ServiceRegistry {
        self.services
    }
}

pub(crate) type Cleanup = Box<dyn FnOnce() -> EngineResult<()>>;
#[derive(Clone, Copy)]
pub(crate) enum SceneRequest {
    Start(StableKey),
    Switch(StableKey, StableKey),
    Pause(StableKey),
    Resume(StableKey),
    Stop(StableKey),
    Remove(StableKey),
    InputMode(StableKey, bool),
    Quit,
}

/// callbackの安全な終了境界に提出するScene操作です。
pub struct SceneCommands<'a> {
    pub(crate) source: StableKey,
    pub(crate) queue: &'a mut Vec<SceneRequest>,
    pub(crate) limit: usize,
}
impl SceneCommands<'_> {
    fn push(&mut self, request: SceneRequest) -> EngineResult<()> {
        if self.queue.len() >= self.limit {
            return Err(crate::foundation::EngineError::capacity(
                "Scene遷移queue",
                self.queue.len() as u64 + 1,
                self.limit as u64,
            ));
        }
        self.queue
            .try_reserve(1)
            .map_err(crate::foundation::error::allocation)?;
        self.queue.push(request);
        Ok(())
    }
    /// 別Sceneを並存させます。既にRunningなら二重startしません。
    pub fn launch(&mut self, key: &str) -> EngineResult<()> {
        self.push(SceneRequest::Start(StableKey::resolve("game", key)?))
    }
    /// launchと同じ開始要求です。
    pub fn start(&mut self, key: &str) -> EngineResult<()> {
        self.launch(key)
    }
    /// 自分を停止して別Sceneを開始します。対象の存在は停止前に確認します。
    pub fn switch(&mut self, key: &str) -> EngineResult<()> {
        self.push(SceneRequest::Switch(
            self.source,
            StableKey::resolve("game", key)?,
        ))
    }
    /// updateを止めて描画を残します。
    pub fn pause(&mut self, key: &str) -> EngineResult<()> {
        self.push(SceneRequest::Pause(StableKey::resolve("game", key)?))
    }
    /// startを呼ばず再開します。
    pub fn resume(&mut self, key: &str) -> EngineResult<()> {
        self.push(SceneRequest::Resume(StableKey::resolve("game", key)?))
    }
    /// scopeを終了します。Worldは削除しません。
    pub fn stop(&mut self, key: &str) -> EngineResult<()> {
        self.push(SceneRequest::Stop(StableKey::resolve("game", key)?))
    }
    /// 停止後にScene登録も削除します。
    pub fn remove(&mut self, key: &str) -> EngineResult<()> {
        self.push(SceneRequest::Remove(StableKey::resolve("game", key)?))
    }
    /// trueならこのSceneより下にあるSceneの入力を遮断します。
    pub fn exclusive_input(&mut self, exclusive: bool) -> EngineResult<()> {
        self.push(SceneRequest::InputMode(self.source, exclusive))
    }
    /// 終了要求を提出します。外側hostが停止とplugin解放を行います。
    pub fn quit(&mut self) -> EngineResult<()> {
        self.push(SceneRequest::Quit)
    }
}

/// 固定更新にだけ渡す作者の入口です。長期間の可変参照を保持できません。
pub struct SceneContext<'a> {
    pub(crate) tick: u64,
    pub(crate) key: StableKey,
    pub(crate) scope: ScopeId,
    pub(crate) settings: &'a EngineSettings,
    pub(crate) assets: &'a mut crate::assets::AssetServer,
    pub(crate) services: &'a ServiceRegistry,
    pub(crate) worlds: &'a WorldSet,
    pub(crate) commands: &'a mut CommandBuffer,
    pub(crate) input: &'a mut InputManager,
    pub(crate) input_enabled: bool,
    pub(crate) requests: &'a mut Vec<SceneRequest>,
    pub(crate) cleanup: &'a mut Vec<Cleanup>,
    pub(crate) request_limit: usize,
}
impl SceneContext<'_> {
    /// このSceneの開始期間が保持する資産を読み込みます。停止時に保持権を一括解除します。
    pub fn assets(&mut self) -> crate::assets::AssetLoader<'_> {
        self.assets
            .loader(self.scope)
            .expect("実行中Sceneの資産scope")
    }

    /// 現在処理するtick。
    pub fn tick(&self) -> u64 {
        self.tick
    }
    /// この開始期間のscope。
    pub fn scope(&self) -> ScopeId {
        self.scope
    }
    /// 解決済み実行設定。
    pub fn settings(&self) -> &EngineSettings {
        self.settings
    }
    /// 現行scopeの入力登録と読取。
    pub fn input(&mut self) -> InputAccess<'_> {
        self.input.access(self.scope, self.input_enabled)
    }
    /// 最後の公開Worldを読み、次barrierの変更を提出します。
    pub fn worlds(&mut self) -> WorldAccess<'_> {
        WorldAccess {
            worlds: self.worlds,
            commands: self.commands,
            access: None,
        }
    }
    /// 遷移をqueueへ提出します。
    pub fn scenes(&mut self) -> SceneCommands<'_> {
        SceneCommands {
            source: self.key,
            queue: self.requests,
            limit: self.request_limit,
        }
    }
    /// stateless/transientなサービスを読みます。正規stateはWorldへ置きます。
    pub fn services(&self) -> &ServiceRegistry {
        self.services
    }
    /// 購読・外部資源等の解除を登録します。停止時に逆順で必ず試行します。
    pub fn on_stop(
        &mut self,
        cleanup: impl FnOnce() -> EngineResult<()> + 'static,
    ) -> EngineResult<()> {
        if self.cleanup.len() >= self.request_limit {
            return Err(crate::foundation::EngineError::capacity(
                "scopeの解除登録数",
                self.cleanup.len() as u64 + 1,
                self.request_limit as u64,
            ));
        }
        self.cleanup
            .try_reserve(1)
            .map_err(crate::foundation::error::allocation)?;
        self.cleanup.push(Box::new(cleanup));
        Ok(())
    }
}

/// 表示callbackへ渡します。Worldへのcommandは提供しません。
pub struct FrameContext<'a> {
    pub(crate) assets: &'a mut crate::assets::AssetServer,
    pub(crate) render_frame: &'a mut crate::presentation::RenderFrame,
    pub(crate) scope: ScopeId,
    pub(crate) tick: u64,
    pub(crate) alpha: f64,
    pub(crate) worlds: &'a WorldSet,
    pub(crate) services: &'a ServiceRegistry,
}
impl FrameContext<'_> {
    /// frame抽出中の表示側だけで非同期結果をpollできます。World更新とは独立です。
    pub fn assets(&mut self) -> &mut crate::assets::AssetServer {
        self.assets
    }
    /// 停止後に使えないscope付きreload要求の入口です。
    pub fn loader(&mut self) -> crate::assets::AssetLoader<'_> {
        self.assets
            .loader(self.scope)
            .expect("実行中Sceneの資産scope")
    }
    /// 既定のscreen cameraでspriteを提出します。
    pub fn draw(&mut self) -> crate::presentation::DrawContext<'_> {
        let camera = crate::presentation::Camera2d::screen(
            self.render_frame.width,
            self.render_frame.height,
        );
        self.render_frame.draw(self.assets, camera)
    }
    /// 作者のcameraでspriteを提出します。複数cameraへ同じ画像を描けます。
    pub fn draw_with(
        &mut self,
        camera: crate::presentation::Camera2d,
    ) -> crate::presentation::DrawContext<'_> {
        self.render_frame.draw(self.assets, camera)
    }

    /// 最後に公開されたtick。
    pub fn tick(&self) -> u64 {
        self.tick
    }
    /// 次tickへの表示用補間係数。
    pub fn alpha(&self) -> f64 {
        self.alpha
    }
    /// 公開済みWorldの不変View。
    pub fn worlds(&self) -> super::engine::WorldView<'_> {
        super::engine::WorldView {
            worlds: self.worlds,
            access: None,
        }
    }
    /// 描画provider等の不変参照。
    pub fn services(&self) -> &ServiceRegistry {
        self.services
    }
}

/// 終了処理に必要な読取権限だけを渡します。
pub struct StopContext<'a> {
    pub(crate) scope: ScopeId,
    pub(crate) worlds: &'a WorldSet,
}
impl StopContext<'_> {
    /// 終了するscope。
    pub fn scope(&self) -> ScopeId {
        self.scope
    }
    /// Worldは読めますが、停止処理から正規状態を書き換えません。
    pub fn worlds(&self) -> super::engine::WorldView<'_> {
        super::engine::WorldView {
            worlds: self.worlds,
            access: None,
        }
    }
}

pub(crate) struct SceneEntry {
    pub key: StableKey,
    pub state: SceneState,
    pub scope: Option<ScopeId>,
    pub scene: Box<dyn Scene>,
    pub cleanup: Vec<Cleanup>,
    pub exclusive: bool,
}
