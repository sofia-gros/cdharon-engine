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
    pixels: Option<pixels::Pixels<'static>>,
    next_frame: Instant,
    window: Option<Arc<Window>>,
    last: Instant,
    started: Instant,
    initial_tick: u64,
    sequence: u64,
    // OSの物理座標を保持し、resize/HiDPI変更時にも静止中のpointerを再投影します。
    cursor: Option<[f64; 2]>,
    failure: Option<EngineError>,
}

// 描画と同じ論理frame/物理surfaceの変換を使います。無効化はNaNや座標0で表さず、
// 専用の正規入力としてtickへ届けるため、再生時も余白への退出を再現できます。
fn surface_pointer_value(
    logical: [u32; 2],
    surface: [u32; 2],
    integer: bool,
    cursor: Option<[f64; 2]>,
) -> EngineResult<InputValue> {
    let Some(cursor) = cursor else {
        return Ok(InputValue::PointerUnavailable);
    };
    if surface.contains(&0) {
        return Ok(InputValue::PointerUnavailable);
    }
    let mapping = crate::presentation::SurfaceMapping::fit(logical, surface, integer)?;
    Ok(match mapping.to_logical(cursor)? {
        Some([x, y]) => InputValue::Pointer { x, y },
        None => InputValue::PointerUnavailable,
    })
}

/// 表示adapterがsurfaceを接続するためのOS側handleです。正規Worldへ保存しません。
#[derive(Clone)]
pub struct DesktopWindow(Arc<Window>);
/// 実際にsurfaceへ接続したGPUの情報です。設定の要求値とは区別します。
#[derive(Clone, Debug)]
pub struct DesktopGpu {
    /// Vulkan、Dx12、Metal等の実際のbackend名。
    pub backend: String,
    /// adapterが報告したdevice名。
    pub name: String,
    /// このdeviceの最大texture寸法。
    pub max_texture_dimension: u32,
}
impl DesktopWindow {
    /// winitへの明示的な接続です。依存するのは表示adapterだけに限定します。
    pub fn window(&self) -> &Arc<Window> {
        &self.0
    }
}
impl Host {
    fn refresh_pointer(&mut self) -> EngineResult<()> {
        let surface = self.window.as_ref().map_or([0, 0], |window| {
            let size = window.inner_size();
            [size.width, size.height]
        });
        let frame = self.engine.render_frame();
        let value = surface_pointer_value(
            [frame.width, frame.height],
            surface,
            self.engine.settings().render.scaling == "integer",
            self.cursor,
        )?;
        self.input(InputBinding::Pointer(0), value);
        Ok(())
    }
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
                let size = window.inner_size();
                let settings = self.engine.settings();
                let frame = self.engine.render_frame();
                let backend = match settings.render.backend.as_str() {
                    "vulkan" => pixels::wgpu::Backends::VULKAN,
                    "dx12" => pixels::wgpu::Backends::DX12,
                    "metal" => pixels::wgpu::Backends::METAL,
                    _ => pixels::wgpu::Backends::PRIMARY,
                };
                let surface = pixels::SurfaceTexture::new(
                    size.width.max(1),
                    size.height.max(1),
                    Arc::clone(&window),
                );
                match pixels::PixelsBuilder::new(frame.width, frame.height, surface)
                    .wgpu_backend(backend)
                    .enable_vsync(settings.render.vsync)
                    .build()
                {
                    Ok(mut pixels) => {
                        pixels.set_scaling_mode(if settings.render.scaling == "integer" {
                            pixels::ScalingMode::PixelPerfect
                        } else {
                            pixels::ScalingMode::Fill
                        });
                        let info = pixels.adapter().get_info();
                        let profile = DesktopGpu {
                            backend: format!("{:?}", info.backend),
                            name: info.name,
                            max_texture_dimension: pixels
                                .device()
                                .limits()
                                .max_texture_dimension_2d,
                        };
                        if let Err(error) = self.engine.attach_gpu(profile) {
                            self.stop(event_loop, error);
                            return;
                        }
                        self.pixels = Some(pixels);
                    }
                    Err(cause) => {
                        self.stop(
                            event_loop,
                            error(ErrorCode::DeviceLost, "wgpu surface準備")
                                .detail(cause.to_string()),
                        );
                        return;
                    }
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
                self.cursor = None;
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
            WindowEvent::Resized(size) => {
                if size.width > 0
                    && size.height > 0
                    && let Some(pixels) = &mut self.pixels
                    && let Err(cause) = pixels.resize_surface(size.width, size.height)
                {
                    self.stop(
                        event_loop,
                        error(ErrorCode::DeviceLost, "surface resize").detail(cause.to_string()),
                    );
                }
                if let Err(error) = self.refresh_pointer() {
                    self.stop(event_loop, error);
                }
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                if let Err(error) = self.refresh_pointer() {
                    self.stop(event_loop, error);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = Some([position.x, position.y]);
                if let Err(error) = self.refresh_pointer() {
                    self.stop(event_loop, error);
                }
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor = None;
                if let Err(error) = self.refresh_pointer() {
                    self.stop(event_loop, error);
                }
            }
            WindowEvent::RedrawRequested => {
                if self.window.as_ref().is_some_and(|window| {
                    let size = window.inner_size();
                    size.width == 0 || size.height == 0
                }) {
                    return;
                }
                let result = (|| {
                    self.engine.frame(self.engine.interpolation())?;
                    if let Some(pixels) = &mut self.pixels {
                        self.engine.render_into(pixels.frame_mut())?;
                        pixels.render().map_err(|cause| {
                            error(ErrorCode::DeviceLost, "wgpu frame表示").detail(cause.to_string())
                        })?;
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    self.stop(event_loop, error);
                }
                let delay = self
                    .engine
                    .settings()
                    .render
                    .frame_limit
                    .map(|hz| std::time::Duration::from_secs_f64(1.0 / f64::from(hz)))
                    .unwrap_or_default();
                self.next_frame = Instant::now() + delay;
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
        if now >= self.next_frame
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
        let next_tick = now + self.engine.until_next_tick();
        let wake = if self.engine.settings().render.frame_limit.is_some() {
            next_tick.min(self.next_frame.max(now))
        } else {
            next_tick
        };
        event_loop.set_control_flow(if self.engine.settings().render.frame_limit.is_some() {
            ControlFlow::WaitUntil(wake)
        } else {
            ControlFlow::Poll
        });
    }
}

impl Engine {
    /// OS event loopを実行します。headlessのstepと同じ固定更新へ接続します。
    ///
    /// CPUの表示snapshotを作者adapterで描画し、pixels/wgpuでsurfaceへ表示します。
    /// 音声再生とdevice復帰の完全な統合はE09の音響・表示サービスが担当します。
    pub fn run(self) -> EngineResult<()> {
        let event_loop =
            EventLoop::new().map_err(|_| error(ErrorCode::DeviceLost, "OS event loop作成"))?;
        let initial_tick = self.tick();
        let now = Instant::now();
        let mut host = Host {
            engine: self,
            pixels: None,
            next_frame: now,
            window: None,
            last: now,
            started: now,
            initial_tick,
            sequence: 0,
            cursor: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_mapping_tracks_hidpi_resize_letterbox_and_integer_clipping() {
        // 同じ論理位置は、通常・HiDPI・縦横比の異なるsurfaceでも一致します。
        for (surface, position) in [
            ([400, 200], [100.0, 50.0]),
            ([800, 400], [200.0, 100.0]),
            ([800, 600], [200.0, 200.0]),
        ] {
            assert_eq!(
                surface_pointer_value([200, 100], surface, false, Some(position)).unwrap(),
                InputValue::Pointer { x: 50.0, y: 25.0 }
            );
        }
        assert_eq!(
            surface_pointer_value([200, 100], [800, 600], false, Some([10.0, 99.0])).unwrap(),
            InputValue::PointerUnavailable
        );
        assert_eq!(
            surface_pointer_value([200, 100], [800, 600], false, Some([800.0, 300.0])).unwrap(),
            InputValue::PointerUnavailable
        );
        // 整数倍率で小さなwindowへ縮めた場合は中央clipの逆変換です。
        assert_eq!(
            surface_pointer_value([200, 100], [100, 50], true, Some([0.0, 0.0])).unwrap(),
            InputValue::Pointer { x: 50.0, y: 25.0 }
        );
        assert_eq!(
            surface_pointer_value([200, 100], [500, 400], true, Some([50.0, 100.0])).unwrap(),
            InputValue::Pointer { x: 0.0, y: 0.0 }
        );
    }
    #[test]
    fn unavailable_pointer_never_reuses_a_previous_frame_position() {
        assert_eq!(
            surface_pointer_value([200, 100], [800, 600], false, None).unwrap(),
            InputValue::PointerUnavailable
        );
        assert_eq!(
            surface_pointer_value([200, 100], [0, 0], false, Some([10.0, 10.0])).unwrap(),
            InputValue::PointerUnavailable
        );
        assert!(
            surface_pointer_value([200, 100], [800, 600], false, Some([f64::NAN, 10.0])).is_err()
        );
    }
}
