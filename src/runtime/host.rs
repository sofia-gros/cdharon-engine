//! winitをRuntimeへ接続するOS側のhost。
//!
//! InstantとWindowはこの層だけが所有します。SceneやSystemへwall-clockを渡さず、
//! elapsedを整数の固定更新へ変換します。具体的な画像描画は別のrender providerです。

use super::{
    engine::{Engine, error},
    input::{InputBinding, InputEvent, InputValue},
};
use crate::foundation::{EngineError, EngineResult, ErrorCode, StableKey};
use std::{sync::Arc, time::Instant};
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::PhysicalKey,
    window::{Fullscreen, Window, WindowId},
};

struct Host {
    engine: Engine,
    window: Option<Arc<Window>>,
    last: Instant,
    started: Instant,
    initial_tick: u64,
    sequence: u64,
    failure: Option<EngineError>,
}

/// 表示adapterがsurfaceを接続するためのOS側handleです。正規Worldへ保存しません。
#[derive(Clone)]
pub struct DesktopWindow(Arc<Window>);
impl DesktopWindow {
    /// winitへの明示的な接続です。依存するのは表示adapterだけに限定します。
    pub fn window(&self) -> &Arc<Window> {
        &self.0
    }
}
impl Host {
    fn input(&mut self, binding: InputBinding, value: InputValue) {
        let Some(sequence) = self.sequence.checked_add(1) else {
            self.failure = Some(error(ErrorCode::CapacityExceeded, "OS入力sequence"));
            return;
        };
        self.sequence = sequence;
        let tick = self.input_tick();
        if let Err(error) = tick.and_then(|tick| {
            self.engine.queue_input_at(
                tick,
                InputEvent {
                    source: 0,
                    sequence,
                    binding,
                    value,
                },
            )
        }) {
            self.failure = Some(error);
        }
    }
    fn input_tick(&self) -> EngineResult<u64> {
        let offset = Instant::now().duration_since(self.started).as_nanos()
            * u128::from(self.engine.settings().simulation.step_hz)
            / 1_000_000_000;
        let requested = self
            .initial_tick
            .checked_add(
                u64::try_from(offset)
                    .map_err(|_| error(ErrorCode::CapacityExceeded, "OS入力時刻"))?,
            )
            .and_then(|tick| tick.checked_add(1))
            .ok_or_else(|| error(ErrorCode::CapacityExceeded, "OS入力tick"))?;
        Ok(requested.max(self.engine.next_input_tick()?))
    }
    fn stop(&mut self, event_loop: &ActiveEventLoop, error: EngineError) {
        self.failure = Some(error);
        event_loop.exit();
    }
}
impl ApplicationHandler for Host {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let config = &self.engine.settings.window;
        let monitor = if let Some(name) = &config.display {
            match event_loop
                .available_monitors()
                .find(|monitor| monitor.name().as_deref() == Some(name.as_str()))
            {
                Some(monitor) => Some(monitor),
                None => {
                    self.stop(
                        event_loop,
                        error(ErrorCode::UnsupportedCapability, "指定displayが存在しない"),
                    );
                    return;
                }
            }
        } else {
            None
        };
        let mut attributes = Window::default_attributes()
            .with_title("Charon Engine")
            .with_inner_size(LogicalSize::new(config.width, config.height))
            .with_resizable(config.resizable);
        if config.fullscreen {
            attributes = attributes.with_fullscreen(Some(Fullscreen::Borderless(monitor.clone())));
        }
        match event_loop.create_window(attributes) {
            Ok(window) => {
                if !config.fullscreen
                    && let Some(monitor) = monitor
                {
                    window.set_outer_position(monitor.position());
                }
                let window = Arc::new(window);
                if let Err(error) = self
                    .engine
                    .attach_window(DesktopWindow(Arc::clone(&window)))
                {
                    self.stop(event_loop, error);
                    return;
                }
                self.window = Some(window);
                self.last = Instant::now();
                self.started = self.last;
            }
            Err(_) => self.stop(event_loop, error(ErrorCode::DeviceLost, "window作成")),
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.window.as_ref().is_none_or(|window| window.id() != id) {
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Focused(false) => {
                if let Err(error) = self
                    .input_tick()
                    .and_then(|tick| self.engine.queue_reset_at(tick))
                {
                    self.stop(event_loop, error);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let raw = match event.physical_key {
                    PhysicalKey::Code(code) => format!("{code:?}"),
                    PhysicalKey::Unidentified(native) => format!("Native{native:?}"),
                };
                let mut name = String::new();
                for character in raw.chars() {
                    if character.is_ascii_uppercase() {
                        if !name.is_empty() {
                            name.push('-');
                        }
                        name.push(character.to_ascii_lowercase());
                    } else if character.is_ascii_alphanumeric() {
                        name.push(character);
                    } else if !name.ends_with('-') {
                        name.push('-');
                    }
                }
                if let Ok(key) = StableKey::resolve("keyboard", name.trim_end_matches('-')) {
                    self.input(
                        InputBinding::Keyboard(key),
                        InputValue::Button(event.state == ElementState::Pressed),
                    );
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let button = match button {
                    MouseButton::Left => 0,
                    MouseButton::Right => 1,
                    MouseButton::Middle => 2,
                    MouseButton::Back => 3,
                    MouseButton::Forward => 4,
                    MouseButton::Other(button) => u32::from(button) + 5,
                };
                self.input(
                    InputBinding::MouseButton(button),
                    InputValue::Button(state == ElementState::Pressed),
                );
            }
            WindowEvent::CursorMoved { position, .. } => self.input(
                InputBinding::Pointer(0),
                InputValue::Pointer {
                    x: position.x,
                    y: position.y,
                },
            ),
            WindowEvent::RedrawRequested => {
                if let Err(error) = self.engine.frame(self.engine.interpolation()) {
                    self.stop(event_loop, error);
                }
            }
            _ => {}
        }
        if self.failure.is_some() {
            event_loop.exit();
        }
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.failure.is_some() {
            event_loop.exit();
            return;
        }
        let now = Instant::now();
        let elapsed = now.duration_since(self.last);
        self.last = now;
        if let Err(error) = self.engine.advance(elapsed, 8) {
            self.stop(event_loop, error);
            return;
        }
        if self.engine.is_quit() {
            event_loop.exit();
            return;
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(now + self.engine.until_next_tick()));
    }
}

impl Engine {
    /// OS event loopを実行します。headlessのstepと同じ固定更新へ接続します。
    ///
    /// このhostはwindow/input/lifecycleを担当し、画像や音の機能を装いません。
    /// 描画・音響providerはそれぞれの登録と認定が必要です。
    pub fn run(self) -> EngineResult<()> {
        let event_loop =
            EventLoop::new().map_err(|_| error(ErrorCode::DeviceLost, "OS event loop作成"))?;
        let initial_tick = self.tick();
        let now = Instant::now();
        let mut host = Host {
            engine: self,
            window: None,
            last: now,
            started: now,
            initial_tick,
            sequence: 0,
            failure: None,
        };
        let result = event_loop
            .run_app(&mut host)
            .map_err(|_| error(ErrorCode::DeviceLost, "OS event loop実行"));
        let cleanup = host.engine.shutdown();
        result?;
        if let Some(error) = host.failure {
            return Err(error);
        }
        cleanup
    }
}
