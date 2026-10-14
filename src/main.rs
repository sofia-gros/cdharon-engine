//! # Charon Engine メインエントリーポイント
//!
//! 2D Falling Sand 物理サンドボックスエンジンの初期化とイベントループの起動を行います。

use charon_engine::core::engine::CharonEngine;
use winit::event_loop::{ControlFlow, EventLoop};

fn main() {
    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = CharonEngine::new();
    event_loop.run_app(&mut app).unwrap();
}
