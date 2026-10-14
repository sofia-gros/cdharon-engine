//! # セル（Cell）モジュール
//!
//! Charon Engine の最小シミュレーション単位であるセル構造体を定義します。
//! CPU の L1 キャッシュ効率を最大化するため、4 バイト（32-bit）固定サイズで設計されています。

/// ワールド上の 1 ピクセルを表現するセル構造体。
///
/// 4 バイト（32-bit）固定サイズを持ち、最大 65,535 種類のマテリアルに対応します。
/// 1 つの CPU キャッシュライン（64 バイト）にジャスト 16 個格納可能です。
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
#[repr(C, align(4))]
pub struct Cell {
    /// マテリアル識別番号（16-bit: 0 〜 65,535。テラリア規模の大量ブロックやMODに対応）
    pub material: u16,
    /// シミュレーション用直交フラグ（8-bit）
    /// - Bit 0: 自由落下・崩落中フラグ
    /// - Bit 1: 燃焼中フラグ
    /// - Bit 2: 濡れフラグ
    /// - Bit 3〜7: 予約（0）
    pub flags: u8,
    /// バリエーション・コンテキストデータ（8-bit）
    /// - 8x8 テクスチャ内のサンプリングオフセット（0..64: 旧 shade）や追加物理パラメータ
    pub data: u8,
}

/// 自由落下・崩落中フラグマスク（構造強度を失い落下中: Bit 0）
pub const FLAG_FALLING: u8 = 1 << 0;
/// 燃焼中フラグマスク（Bit 1）
#[allow(dead_code)]
pub const FLAG_BURNING: u8 = 1 << 1;
/// 濡れフラグマスク（Bit 2）
#[allow(dead_code)]
pub const FLAG_WET: u8 = 1 << 2;

impl Cell {
    /// 指定されたマテリアル ID で空のセルを作成します。
    ///
    /// # 引数
    /// * `material` - マテリアル ID (u16)
    #[inline]
    pub const fn new(material: u16) -> Self {
        Self {
            material,
            flags: 0,
            data: 0,
        }
    }

    /// マテリアル ID とコンテキストデータ（テクスチャのインデックス等）を指定してセルを作成します。
    ///
    /// # 引数
    /// * `material` - マテリアル ID (u16)
    /// * `data` - バリエーションデータ値 (0..255)
    #[inline]
    pub const fn with_data(material: u16, data: u8) -> Self {
        Self {
            material,
            flags: 0,
            data,
        }
    }

    /// マテリアル ID と色の濃淡（shade）を指定してセルを作成します（with_data へのエイリアス）。
    #[inline]
    pub const fn with_shade(material: u16, shade: u8) -> Self {
        Self::with_data(material, shade)
    }

    /// テクスチャサンプリング用の濃淡オフセット（data フィールドの下位 6 ビット 0..64）を取得します。
    #[inline]
    pub const fn shade(&self) -> u8 {
        self.data
    }

    /// セルが構造強度を失って自由落下・崩落中かどうかを判定します。
    #[inline]
    pub fn is_falling(&self) -> bool {
        (self.flags & FLAG_FALLING) != 0
    }

    /// セルの自由落下・崩落状態を設定します。
    ///
    /// # 引数
    /// * `falling` - true: 落下中フラグを付与, false: 解除して静止
    #[inline]
    pub fn set_falling(&mut self, falling: bool) {
        if falling {
            self.flags |= FLAG_FALLING;
        } else {
            self.flags &= !FLAG_FALLING;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cell_size_and_alignment() {
        assert_eq!(
            std::mem::size_of::<Cell>(),
            4,
            "Cell は厳密に 4 バイトでなければなりません"
        );
        assert_eq!(
            std::mem::align_of::<Cell>(),
            4,
            "Cell は 4 バイトアライメントでなければなりません"
        );
    }

    #[test]
    fn test_falling_flag() {
        let mut cell = Cell::new(1);
        assert!(!cell.is_falling(), "初期状態は落下中ではないはず");
        cell.set_falling(true);
        assert!(cell.is_falling(), "set_falling(true) 後は落下中");
        assert_eq!(cell.flags, FLAG_FALLING, "fallingは契約どおりbit0");
        cell.set_falling(false);
        assert!(!cell.is_falling(), "set_falling(false) 後は静止状態に戻る");
    }
}
