//! # 入力（Input）モジュール
//!
//! キーボード操作およびマウス入力を集約して管理します。
//! ウィンドウの物理ピクセル座標からワールド論理ピクセル座標への正規化変換も行います。

use std::collections::HashSet;
use winit::keyboard::KeyCode;

/// ユーザー入力の状態を保持する構造体
#[derive(Debug, Default)]
pub struct InputState {
    /// 現在押されているキーコードの集合
    pressed_keys: HashSet<KeyCode>,
    /// 次のsimulation tickで一度だけ消費される押下edge
    just_pressed_keys: HashSet<KeyCode>,
    /// 次のsimulation tickで一度だけ消費される解放edge
    just_released_keys: HashSet<KeyCode>,
    /// マウス左ボタンが押されているか（水配置）
    pub mouse_left: bool,
    /// マウス右ボタンが押されているか（掘削・ブロック破壊）
    pub mouse_right: bool,
    /// マウス中央（ホイール）ボタンが押されているか（砂配置）
    pub mouse_middle: bool,
    /// 現在のマウス論理座標 X（0 .. 640）
    pub mouse_x: i32,
    /// 現在のマウス論理座標 Y（0 .. 360）
    pub mouse_y: i32,
}

impl InputState {
    /// 新しい入力状態を作成します。
    pub fn new() -> Self {
        Self::default()
    }

    /// キーが押されたことを記録します。
    pub fn key_down(&mut self, key: KeyCode) {
        if self.pressed_keys.insert(key) {
            self.just_pressed_keys.insert(key);
        }
    }

    /// キーが離されたことを記録します。
    pub fn key_up(&mut self, key: KeyCode) {
        if self.pressed_keys.remove(&key) {
            self.just_released_keys.insert(key);
        }
    }

    /// 特定のキーが押されているかを判定します。
    pub fn is_key_down(&self, key: KeyCode) -> bool {
        self.pressed_keys.contains(&key)
    }

    /// キーが押されたedgeを次のtickで消費する。
    pub fn was_key_just_pressed(&self, key: KeyCode) -> bool {
        self.just_pressed_keys.contains(&key)
    }

    /// キーが解放されたedgeを次のtickで消費する。
    pub fn was_key_just_released(&self, key: KeyCode) -> bool {
        self.just_released_keys.contains(&key)
    }

    /// 1tick分のedgeを消費し、held状態は維持する。
    pub fn finish_tick(&mut self) {
        self.just_pressed_keys.clear();
        self.just_released_keys.clear();
    }

    /// focus喪失時にキーheldとedgeを消去してstuck inputを防ぐ。
    pub fn clear(&mut self) {
        self.pressed_keys.clear();
        self.finish_tick();
    }

    /// プレイヤーの水平移動入力ベクトル（-1.0, 0.0, 1.0）を取得します。
    ///
    /// A キーまたは左矢印で左（-1.0）、D キーまたは右矢印で右（1.0）を返します。
    pub fn get_horizontal_axis(&self) -> i8 {
        let mut axis = 0_i8;
        if self.is_key_down(KeyCode::KeyA) || self.is_key_down(KeyCode::ArrowLeft) {
            axis -= 1;
        }
        if self.is_key_down(KeyCode::KeyD) || self.is_key_down(KeyCode::ArrowRight) {
            axis += 1;
        }
        axis
    }

    /// ジャンプ操作（W キー、スペースキー、上矢印）が押されているかを判定します。
    pub fn is_jump_held(&self) -> bool {
        self.is_key_down(KeyCode::KeyW)
            || self.is_key_down(KeyCode::Space)
            || self.is_key_down(KeyCode::ArrowUp)
    }

    /// ジャンプ相当キーが押されたedge。
    pub fn is_jump_just_pressed(&self) -> bool {
        self.was_key_just_pressed(KeyCode::KeyW)
            || self.was_key_just_pressed(KeyCode::Space)
            || self.was_key_just_pressed(KeyCode::ArrowUp)
    }

    /// Shift キーが押されているかを判定します。
    pub fn is_shift_pressed(&self) -> bool {
        self.is_key_down(KeyCode::ShiftLeft) || self.is_key_down(KeyCode::ShiftRight)
    }

    /// マウスのウィンドウ物理座標を、ゲーム内論理解像度（640x360）のピクセル座標に変換して更新します。
    pub fn update_mouse_position(
        &mut self,
        phys_x: f64,
        phys_y: f64,
        win_width: u32,
        win_height: u32,
        logical_width: u32,
        logical_height: u32,
    ) {
        if win_width == 0 || win_height == 0 {
            return;
        }
        let scale_x = logical_width as f64 / win_width as f64;
        let scale_y = logical_height as f64 / win_height as f64;

        self.mouse_x = (phys_x * scale_x) as i32;
        self.mouse_y = (phys_y * scale_y) as i32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_edges_are_consumed_once_while_held_state_persists() {
        let mut input = InputState::new();
        input.key_down(KeyCode::Space);
        input.key_down(KeyCode::Space); // OS repeat must not create another edge.
        assert!(input.was_key_just_pressed(KeyCode::Space));
        assert!(input.is_key_down(KeyCode::Space));

        input.finish_tick();
        assert!(!input.was_key_just_pressed(KeyCode::Space));
        assert!(input.is_key_down(KeyCode::Space));

        input.key_up(KeyCode::Space);
        assert!(input.was_key_just_released(KeyCode::Space));
        assert!(!input.is_key_down(KeyCode::Space));
    }
}
