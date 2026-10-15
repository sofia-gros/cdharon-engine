//! 新設計の実行・描画から独立したデータ基盤。
//!
//! ここではウィンドウ、音声、壁時計、既存のプレイヤー型を参照しません。
//! 作者の状態を安定キーと明示codecで管理し、変更は候補を検証してから公開します。
//! 既存の`core`/`simulation`とは保存形式を共有せず、移行の判断を明示できます。

pub mod codec;
pub mod component;
pub mod error;
pub mod grid;
pub mod id;
pub mod key;
pub mod random;
pub mod schema;
pub mod state;
pub mod transaction;
pub mod units;

pub use error::{EngineError, EngineResult, ErrorCode};
pub use key::StableKey;
