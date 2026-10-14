use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    pub engine: EngineConfig,
    pub game: GameConfig,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EngineConfig {
    pub name: String,
    pub author: String,
    pub version: String,
    pub description: String,
    pub window: WindowConfig,
    pub debug: DebugConfig,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WindowConfig {
    pub width: u32,
    pub height: u32,
    pub resizable: bool,
    pub fullscreen: bool,
    pub vsync: bool,
    #[serde(rename = "framerate-target")]
    pub framerate_target: u32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DebugConfig {
    pub show_fps: bool,
    pub show_colliders: bool,
    pub show_grid: bool,
    pub grid_size: u32,
    pub grid_color: u32, // 0x808080 のような16進数は数値としてパース可能
    pub grid_opacity: f32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GameConfig {
    pub player: PlayerConfig,
    pub camera: CameraConfig,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PlayerConfig {
    pub speed: f32,
    pub jump_force: f32,
    pub gravity: f32,
    pub max_health: u32,
    pub max_stamina: u32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CameraConfig {
    pub speed: f32,
}

impl Config {
    /// conf.yml を読み込む。ファイルが存在しなければデフォルト設定を作成して保存する。
    pub fn load_or_default(path: &str) -> Self {
        if Path::new(path).exists() {
            let content = fs::read_to_string(path).expect("Failed to read config file");
            serde_yaml::from_str(&content).expect("Failed to parse YAML config")
        } else {
            let default_config = Self::default_config();
            default_config.save(path);
            default_config
        }
    }

    /// conf.yml に現在の設定を書き込む
    pub fn save(&self, path: &str) {
        let content = serde_yaml::to_string(self).expect("Failed to serialize config");
        fs::write(path, content).expect("Failed to write config file");
    }

    /// デフォルトの設定値（ファイルが存在しない時のフォールバック）
    fn default_config() -> Self {
        Self {
            engine: EngineConfig {
                name: "Charon Engine".to_string(),
                author: "sofia-gros".to_string(),
                version: "1.0.0".to_string(),
                description: "Let's you create 2D sandbox games with ease.".to_string(),
                window: WindowConfig {
                    width: 1280,
                    height: 720,
                    resizable: false,
                    fullscreen: false,
                    vsync: true,
                    framerate_target: 144,
                },
                debug: DebugConfig {
                    show_fps: true,
                    show_colliders: true,
                    show_grid: true,
                    grid_size: 50,
                    grid_color: 0x808080,
                    grid_opacity: 0.5,
                },
            },
            game: GameConfig {
                player: PlayerConfig {
                    speed: 200.0,
                    jump_force: 500.0,
                    gravity: 9.8,
                    max_health: 100,
                    max_stamina: 100,
                },
                camera: CameraConfig { speed: 200.0 },
            },
        }
    }
}
