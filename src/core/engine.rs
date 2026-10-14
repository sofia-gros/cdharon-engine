//! # エンジン（Engine）モジュール
//!
//! Charon Engine のメインループとウィンドウ管理、描画パイプラインを担当します。
//! winit の `ApplicationHandler` を実装し、マウス入力やキーボード入力を拾って
//! シミュレーションワールドおよびプレイヤーエンティティと連携させます。

use pixels::{Pixels, SurfaceTexture};
use std::sync::Arc;
use std::time::Instant;
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow},
    keyboard::{KeyCode, PhysicalKey},
    window::{Fullscreen, Window, WindowId},
};

use crate::core::config::Config;
use crate::core::fixed::Fixed;
use crate::core::input::InputState;
use crate::core::timing::FixedTickAccumulator;
use crate::entities::player::Player;
use crate::render::render_to_frame;
use crate::simulation::chunk::{CHUNK_EDGE, ChunkCoord};
use crate::simulation::command::{CommandEnvelope, CommandQueueError, WorldCommand};
use crate::simulation::material::{AIR, SAND, WATER};
use crate::simulation::streaming::{ChunkRect, StreamingPolicy};
use crate::simulation::world::World;

/// ゲーム内部の論理描画解像度の横幅（ピクセル）
pub const LOGICAL_WIDTH: u32 = 640;
/// ゲーム内部の論理描画解像度の縦幅（ピクセル）
pub const LOGICAL_HEIGHT: u32 = 360;

/// Charon Engine のコアアプリケーション構造体
pub struct CharonEngine {
    /// OS ウィンドウハンドル
    window: Option<Arc<Window>>,
    /// ピクセルフレームバッファ描画エンジン（pixels / wgpu）
    pixels: Option<Pixels<'static>>,
    /// セルオートマトン物理ワールド
    world: World,
    /// 作者が上限内で変更できる画面外active領域と先読み領域のpolicyです。
    streaming_policy: StreamingPolicy,
    applied_streaming_plan: Option<crate::simulation::streaming::VisibleChunkPlan>,
    applied_streaming_revision: u64,
    /// プレイヤーキャラクター
    player: Player,
    /// 入力状態管理
    input: InputState,
    /// フルスクリーン状態フラグ
    is_fullscreen: bool,
    /// 設定データ（conf.yml）
    config: Config,
    /// 固定tick用の経過時間accumulator
    tick_accumulator: FixedTickAccumulator,
    /// 前回イベントループ処理時刻
    last_update: Instant,
    /// フォーカスを失った間はsimulationを停止
    focused: bool,
}

impl CharonEngine {
    /// エンジンを初期化し、初期ワールドおよびプレイヤーを生成します。
    pub fn new() -> Self {
        let config = Config::load_or_default("conf.yml");

        Self {
            window: None,
            pixels: None,
            world: World::new(LOGICAL_WIDTH, LOGICAL_HEIGHT)
                .expect("built-in world dimensions fit configured capacity"),
            streaming_policy: StreamingPolicy::default(),
            applied_streaming_plan: None,
            applied_streaming_revision: 0,
            // 地上の中央付近に初期配置
            player: Player::new(
                Fixed::from_raw(310 * Fixed::SCALE),
                Fixed::from_raw(160 * Fixed::SCALE),
            ),
            input: InputState::new(),
            is_fullscreen: config.engine.window.fullscreen,
            config,
            tick_accumulator: FixedTickAccumulator::default(),
            last_update: Instant::now(),
            focused: true,
        }
    }

    /// 作者指定の画面外policyを上限検証後に置き換えます。既存domainに収まらない設定はWorld側が拒否します。
    pub fn set_streaming_policy(
        &mut self,
        policy: StreamingPolicy,
    ) -> Result<(), crate::simulation::streaming::StreamError> {
        let policy = policy.validate()?;
        if self.world.pinned_chunk_count() > policy.max_pinned_chunks
            || self.world.chunk_lease_count() > policy.max_pending_requests
            || self.world.active_chunk_count() > policy.max_active_chunks
            || self.world.resident_chunk_count() > policy.max_resident_chunks
        {
            return Err(crate::simulation::streaming::StreamError::InvalidPolicy);
        }
        self.streaming_policy = policy;
        self.applied_streaming_plan = None;
        Ok(())
    }

    pub fn request_chunk_lease(
        &mut self,
        source_id: u32,
        region: ChunkRect,
        mode: crate::simulation::streaming::LoadMode,
    ) -> Result<u64, crate::simulation::command::WorldError> {
        let id = self
            .world
            .request_chunk_lease(self.streaming_policy, source_id, region, mode)?;
        self.applied_streaming_plan = None;
        Ok(id)
    }

    pub fn release_chunk_lease(&mut self, lease_id: u64) -> bool {
        let released = self.world.release_chunk_lease(lease_id);
        if released {
            self.applied_streaming_plan = None;
        }
        released
    }

    /// フルスクリーンの ON/OFF を切り替えます（F11 キー対応）。
    fn toggle_fullscreen(&mut self) {
        if let Some(window) = &self.window {
            if self.is_fullscreen {
                window.set_fullscreen(None);
                self.is_fullscreen = false;
            } else {
                window.set_fullscreen(Some(Fullscreen::Borderless(None)));
                self.is_fullscreen = true;
            }

            self.config.engine.window.fullscreen = self.is_fullscreen;
            self.config.save("conf.yml");
        }
    }
}

impl Default for CharonEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ApplicationHandler for CharonEngine {
    /// アプリケーション起動時および再開時のウィンドウ・サーフェス初期化
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.last_update = Instant::now();
        if self.window.is_none() {
            let win_config = &self.config.engine.window;
            let window_size = LogicalSize::new(win_config.width as f64, win_config.height as f64);

            let mut window_attributes = Window::default_attributes()
                .with_title(&self.config.engine.name)
                .with_inner_size(window_size)
                .with_resizable(win_config.resizable);

            if self.is_fullscreen {
                window_attributes =
                    window_attributes.with_fullscreen(Some(Fullscreen::Borderless(None)));
            }

            let window = Arc::new(event_loop.create_window(window_attributes).unwrap());
            self.window = Some(window.clone());

            let physical_size = window.inner_size();
            let surface_texture =
                SurfaceTexture::new(physical_size.width, physical_size.height, window.clone());

            self.pixels =
                Some(Pixels::new(LOGICAL_WIDTH, LOGICAL_HEIGHT, surface_texture).unwrap());
        }
    }

    /// ウィンドウイベントハンドラ（入力、リサイズ、再描画）
    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            // ウィンドウ終了
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }

            // キーボード入力
            WindowEvent::KeyboardInput {
                event: key_event, ..
            } => {
                if let PhysicalKey::Code(key_code) = key_event.physical_key {
                    match key_event.state {
                        ElementState::Pressed => {
                            if key_code == KeyCode::F11 {
                                self.toggle_fullscreen();
                            } else {
                                self.input.key_down(key_code);
                            }
                        }
                        ElementState::Released => {
                            self.input.key_up(key_code);
                        }
                    }
                }
            }

            // マウスカーソル移動
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(window) = &self.window {
                    let win_size = window.inner_size();
                    self.input.update_mouse_position(
                        position.x,
                        position.y,
                        win_size.width,
                        win_size.height,
                        LOGICAL_WIDTH,
                        LOGICAL_HEIGHT,
                    );
                }
            }

            // マウスクリック（左クリック: 水配置、右クリック: 砂配置）
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = state == ElementState::Pressed;
                match button {
                    MouseButton::Left => self.input.mouse_left = pressed,
                    MouseButton::Right => self.input.mouse_right = pressed,
                    MouseButton::Middle => self.input.mouse_middle = pressed,
                    _ => {}
                }
            }

            // ウィンドウリサイズ
            WindowEvent::Resized(physical_size) => {
                if let Some(pixels) = self.pixels.as_mut()
                    && let Err(err) =
                        pixels.resize_surface(physical_size.width, physical_size.height)
                {
                    eprintln!("Surface resize error: {:?}", err);
                    event_loop.exit();
                }
            }

            WindowEvent::Focused(focused) => {
                self.focused = focused;
                self.last_update = Instant::now();
                if !focused {
                    self.input.clear();
                }
                if focused && let Some(window) = &self.window {
                    window.request_redraw();
                }
            }

            // 再描画要求
            WindowEvent::RedrawRequested => {
                if let Some(pixels) = self.pixels.as_mut() {
                    // ワールドとプレイヤーをフレームバッファへ描画
                    render_to_frame(&self.world, &self.player, pixels.frame_mut());

                    if let Err(err) = pixels.render() {
                        eprintln!("Pixels render error: {:?}", err);
                        event_loop.exit();
                    }
                }
            }

            _ => {}
        }
    }

    /// イベントループの待機直前処理（固定tickの実行と描画リクエスト）
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if !self.focused {
            self.last_update = now;
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
        let elapsed = now.saturating_duration_since(self.last_update);
        self.last_update = now;
        let due_ticks = self.tick_accumulator.advance(elapsed, 4);

        for _ in 0..due_ticks {
            if let Err(error) = self.simulation_tick() {
                eprintln!("Simulation stopped: {error}");
                self.focused = false;
                break;
            }
        }

        if due_ticks > 0
            && let Some(window) = self.window.as_ref()
        {
            window.request_redraw();
        }

        event_loop.set_control_flow(ControlFlow::WaitUntil(
            now + self.tick_accumulator.until_next_tick(),
        ));
    }
}

impl CharonEngine {
    /// 入力を現在の固定tickへ適用し、ゲーム状態を1段進める。
    fn simulation_tick(&mut self) -> Result<(), crate::simulation::command::WorldError> {
        self.world.ensure_tick_available()?;

        // 入力commandを反映する前に、可視範囲と画面外余白のchunk容量を確保してdomainを確定します。
        let visible = ChunkRect::new(
            ChunkCoord { x: 0, y: 0 },
            ChunkCoord {
                x: LOGICAL_WIDTH.div_ceil(CHUNK_EDGE as u32) as i32,
                y: LOGICAL_HEIGHT.div_ceil(CHUNK_EDGE as u32) as i32,
            },
        )
        .map_err(|_| crate::simulation::command::WorldError::InvalidWorldDimensions)?;
        let plan = self
            .streaming_policy
            .plan_visible(visible)
            .map_err(|_| crate::simulation::command::WorldError::WorldCapacityExceeded)?;
        if self.applied_streaming_plan != Some(plan)
            || self.applied_streaming_revision != self.world.domain_revision()
        {
            self.world.apply_visible_chunk_plan(
                plan,
                self.streaming_policy.max_resident_chunks,
                self.streaming_policy.max_active_chunks,
            )?;
            self.applied_streaming_plan = Some(plan);
            self.applied_streaming_revision = self.world.domain_revision();
        }

        // 1. マウス操作によるマテリアル注ぎ込み & 掘削破壊
        if self.input.mouse_left {
            if self.input.is_shift_pressed() {
                // Shift + 左クリック: 砂を配置
                self.apply_brush(3, SAND);
            } else {
                // 左クリック: 半径 4 ピクセルで水を注水
                self.apply_brush(4, WATER);
            }
        }
        if self.input.mouse_right {
            // 右クリック: 地形を掘削・破壊（AIR を配置）
            // 4x4 ブロックの体積が半分未満に削られると、周囲の残存土や石が自動崩落する様子を確認可能
            self.apply_brush(4, AIR);
        }
        if self.input.mouse_middle {
            // ホイール中クリック: 砂を配置
            self.apply_brush(3, SAND);
        }

        let events = self
            .world
            .dispatch_pending_commands()
            .map_err(|error| match error {
                CommandQueueError::AllocationFailed => {
                    crate::simulation::command::WorldError::AllocationFailed
                }
                CommandQueueError::WrongTick if self.world.tick_count == u64::MAX => {
                    crate::simulation::command::WorldError::TickOverflow
                }
                _ => crate::simulation::command::WorldError::InvalidWorldDimensions,
            })?;
        for event in events {
            if let Err(error) = event.result {
                eprintln!("World command rejected: {error:?}");
            }
        }

        // 2. プレイヤーの移動・ジャンプ・水相互作用の更新
        let input_x = self.input.get_horizontal_axis();
        let jump_pressed = self.input.is_jump_just_pressed();
        let jump_held = self.input.is_jump_held();
        self.player
            .update(input_x, jump_pressed, jump_held, &mut self.world)
            .map_err(|_| crate::simulation::command::WorldError::InvalidWorldDimensions)?;
        self.input.finish_tick();

        // 3. セルオートマトンおよび粒子の物理シミュレーションを 1 tick 実行
        self.world.update()
    }

    fn apply_brush(&mut self, radius: i32, material: u16) {
        let Some(target_tick) = self.world.tick_count.checked_add(1) else {
            eprintln!("World command rejected: tick counter overflow");
            return;
        };
        let Some(sequence) = self.world.next_command_sequence(target_tick, 0) else {
            eprintln!("World command rejected: source sequence overflow");
            return;
        };
        let command = WorldCommand::PaintBrush {
            center_x: self.input.mouse_x,
            center_y: self.input.mouse_y,
            radius,
            material,
        };
        if let Err(error) = self.world.submit_command(CommandEnvelope {
            target_tick,
            source_id: 0,
            sequence,
            payload: command,
        }) {
            eprintln!("World command rejected: {error:?}");
        }
    }
}
