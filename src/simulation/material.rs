//! # マテリアル（Material）モジュール
//!
//! セルが持つ物質の種類、ゲーム質量、速度パラメータ、typed creator data、8x8テクスチャを定義します。
//!
//! 【データ駆動マテリアルレジストリ】
//! ハードコードされた固定配列を廃止し、予約IDを除く最大 65,534 種類（16-bit ID）のブロックを動的登録可能な
//! `MaterialRegistry` を提供します。これにより、テラリア規模の大量ブロック（数千〜1万個以上）や
//! クリエイターによる MOD 追加に対応し、O(1) の定数時間で物性取得と描画色計算を行います。

use crate::simulation::authoring::{MaterialAuthoring, PropertyValue};
use crate::simulation::determinism::Fnv64;
use std::collections::HashMap;

/// 空気マテリアル（何もない空間）
pub const AIR: u16 = 0;
/// 土マテリアル（破壊可能な固体ブロック）
pub const DIRT: u16 = 1;
/// 砂マテリアル（崩落する粉体ブロック）
pub const SAND: u16 = 2;
/// 石マテリアル（強固な岩盤ブロック）
pub const STONE: u16 = 3;
/// 水マテリアル（流動する液体）
pub const WATER: u16 = 4;
/// 未解決マテリアル参照を表す予約ID。空気として扱わない。
pub const MISSING: u16 = u16::MAX;
const MAX_REGISTERED_ID: usize = (u16::MAX - 1) as usize;
pub const MAX_MASS_PER_CELL: u32 = 1_000_000;

/// セル・オートマトンにおける物質の基本挙動タイプ
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Behavior {
    /// 何もない（自由移動の宛先となる）
    Empty,
    /// 固定された固体（4x4 以上の密集構造を持ち、動かない壁・床）
    Solid,
    /// 液体（下へ高速落下し、左右へ勢いよく水平に広がる。砂や小粒子を押し流す）
    Liquid,
    /// 粉体（下へ適度な速度で落下し、斜め下へ崩落して山を作る。水流に流されやすい）
    Powder,
}

/// マテリアルの物性および 8x8 テクスチャ定義
#[derive(Clone, Debug)]
pub struct MaterialDef {
    /// 永続名前空間付き識別名（例: "core:dirt", "example:mithril_ore"）
    pub name: String,
    /// 基本挙動
    pub behavior: Behavior,
    /// 1占有セルのゲーム質量。相対的な交換順位に用い、SI質量は保証しない。
    pub mass_per_cell: u32,
    /// 水平方向への拡散力（1 tick で横へ探査する最大セル数）
    pub dispersion: u8,
    /// 1 tick あたりの最大自由落下速度（セル数/tick）
    pub max_fall_speed: u8,
    /// 1 tick あたりの横方向流速（セル数/tick）
    pub flow_speed: u8,
    /// 水流に押し流されるかどうか（流砂・土砂崩れ判定）
    pub can_be_washed_away: bool,
    /// 8x8 ピクセルのテクスチャデータ（計 64 個の RGBA カラー配列）
    pub texture: [[u8; 4]; 64],
    /// Typed creator metadata. It is inert unless a simulation system consumes it.
    pub authoring: MaterialAuthoring,
}

/// クリエイターが直感的にマテリアルを定義するためのビルダー
#[allow(dead_code)]
pub struct MaterialBuilder {
    name: String,
    behavior: Behavior,
    mass_per_cell: u32,
    dispersion: u8,
    max_fall_speed: u8,
    flow_speed: u8,
    can_be_washed_away: bool,
    texture: [[u8; 4]; 64],
    authoring: MaterialAuthoring,
}

#[allow(dead_code)]
impl MaterialBuilder {
    /// 指定された名前と単色で新しいマテリアルビルダーを作成します。
    pub fn new(name: impl Into<String>, base_color: [u8; 4]) -> Self {
        Self {
            name: name.into(),
            behavior: Behavior::Solid,
            mass_per_cell: 1_000,
            dispersion: 0,
            max_fall_speed: 0,
            flow_speed: 0,
            can_be_washed_away: false,
            texture: [base_color; 64],
            authoring: MaterialAuthoring::default(),
        }
    }

    /// 基本挙動を設定します。
    pub fn behavior(mut self, behavior: Behavior) -> Self {
        self.behavior = behavior;
        self
    }

    /// 1セルあたりのゲーム質量を設定します。
    pub fn mass_per_cell(mut self, mass: u32) -> Self {
        self.mass_per_cell = mass;
        self
    }

    /// 旧API互換。値を1セルのゲーム質量として解釈します。
    #[deprecated(note = "use mass_per_cell; this is not physical density")]
    pub fn density(mut self, density: u8) -> Self {
        self.mass_per_cell = u32::from(density);
        self
    }

    pub fn property(
        mut self,
        key: impl Into<String>,
        value: crate::simulation::authoring::PropertyValue,
    ) -> Self {
        self.authoring
            .properties
            .push(crate::simulation::authoring::MaterialProperty {
                key: key.into(),
                value,
            });
        self
    }

    pub fn attach(mut self, attachment: crate::simulation::authoring::MaterialAttachment) -> Self {
        self.authoring.attachments.push(attachment);
        self
    }

    pub fn flammability(
        mut self,
        flammability: crate::simulation::authoring::Flammability,
    ) -> Self {
        self.authoring.flammability = Some(flammability);
        self
    }

    /// 水平拡散力を設定します。
    pub fn dispersion(mut self, dispersion: u8) -> Self {
        self.dispersion = dispersion;
        self
    }

    /// 最大落下速度を設定します。
    pub fn max_fall_speed(mut self, speed: u8) -> Self {
        self.max_fall_speed = speed;
        self
    }

    /// 横方向流速を設定します。
    pub fn flow_speed(mut self, speed: u8) -> Self {
        self.flow_speed = speed;
        self
    }

    /// 水流押し流しフラグを設定します。
    pub fn can_be_washed_away(mut self, washable: bool) -> Self {
        self.can_be_washed_away = washable;
        self
    }

    /// 8x8 テクスチャ配列を設定します。
    pub fn texture(mut self, texture: [[u8; 4]; 64]) -> Self {
        self.texture = texture;
        self
    }

    /// ビルドして MaterialDef を生成します。
    pub fn build(self) -> MaterialDef {
        MaterialDef {
            name: self.name,
            behavior: self.behavior,
            mass_per_cell: self.mass_per_cell,
            dispersion: self.dispersion,
            max_fall_speed: self.max_fall_speed,
            flow_speed: self.flow_speed,
            can_be_washed_away: self.can_be_washed_away,
            texture: self.texture,
            authoring: self.authoring,
        }
    }
}

/// 動的マテリアルレジストリ
///
/// 予約IDを除く最大 65,534 種類（u16）の作者マテリアルを動的に登録・管理します。
/// 連続メモリ（Vec）による O(1) 配列引きにより、1 万個登録しても処理速度は低下しません。
#[derive(Clone, Debug)]
pub struct MaterialRegistry {
    materials: Vec<MaterialDef>,
    name_to_id: HashMap<String, u16>,
    missing: MaterialDef,
    frozen: bool,
}

/// マテリアル登録時に発生する検証エラー。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MaterialRegistryError {
    InvalidName(String),
    DuplicateName(String),
    ReservedName(String),
    CapacityExceeded,
    Frozen,
    InvalidAuthoring(String, &'static str),
    UnknownMaterialReference(String),
}

impl std::fmt::Display for MaterialRegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidName(name) => write!(
                f,
                "invalid material name `{name}`; expected namespace:name using lowercase ASCII, digits, '_' or '-'"
            ),
            Self::DuplicateName(name) => write!(f, "material name `{name}` is already registered"),
            Self::ReservedName(name) => {
                write!(f, "material name `{name}` is reserved by the engine")
            }
            Self::CapacityExceeded => write!(
                f,
                "material registry capacity exceeded (maximum ID is {})",
                u16::MAX - 1
            ),
            Self::Frozen => write!(f, "material registry is frozen"),
            Self::InvalidAuthoring(name, reason) => {
                write!(f, "invalid creator data for material `{name}`: {reason}")
            }
            Self::UnknownMaterialReference(name) => {
                write!(f, "material reference `{name}` does not exist")
            }
        }
    }
}

impl std::error::Error for MaterialRegistryError {}

impl Default for MaterialRegistry {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[allow(dead_code)]
impl MaterialRegistry {
    /// 空気とmissingの予約定義を含むレジストリを作成します。
    pub fn new() -> Self {
        let air = MaterialDef {
            name: "core:air".to_string(),
            behavior: Behavior::Empty,
            mass_per_cell: 0,
            dispersion: 0,
            max_fall_speed: 0,
            flow_speed: 0,
            can_be_washed_away: false,
            texture: [[135, 206, 235, 255]; 64],
            authoring: MaterialAuthoring::default(),
        };
        let mut name_to_id = HashMap::with_capacity(64);
        name_to_id.insert(air.name.clone(), AIR);
        Self {
            materials: vec![air],
            name_to_id,
            frozen: false,
            missing: MaterialDef {
                name: "engine:missing".to_string(),
                behavior: Behavior::Solid,
                mass_per_cell: u32::MAX,
                dispersion: 0,
                max_fall_speed: 0,
                flow_speed: 0,
                can_be_washed_away: false,
                texture: [[255, 0, 255, 255]; 64],
                authoring: MaterialAuthoring::default(),
            },
        }
    }

    /// デフォルトの基本マテリアル（AIR, DIRT, STONE, WATER, SAND）が登録されたレジストリを作成します。
    pub fn with_defaults() -> Self {
        let mut registry = Self::with_default_materials_builder();
        registry
            .freeze()
            .expect("built-in material references are valid");
        registry
    }

    /// 標準物質を含み、作者物質を追加登録できるbuilderを作成します。
    pub fn with_default_materials_builder() -> Self {
        let mut reg = Self::new();

        // 1: DIRT
        reg.register(MaterialDef {
            name: "core:dirt".to_string(),
            behavior: Behavior::Solid,
            mass_per_cell: 2_000,
            dispersion: 0,
            max_fall_speed: 3,
            flow_speed: 1,
            can_be_washed_away: true,
            texture: [
                [120, 85, 45, 255],
                [130, 95, 50, 255],
                [115, 80, 40, 255],
                [135, 100, 55, 255],
                [125, 90, 48, 255],
                [118, 82, 42, 255],
                [130, 92, 50, 255],
                [122, 86, 46, 255],
                [105, 72, 36, 255],
                [98, 66, 32, 255],
                [112, 78, 40, 255],
                [102, 70, 35, 255],
                [95, 64, 30, 255],
                [108, 75, 38, 255],
                [115, 80, 42, 255],
                [100, 68, 34, 255],
                [92, 62, 30, 255],
                [105, 72, 38, 255],
                [96, 65, 33, 255],
                [88, 58, 28, 255],
                [104, 71, 37, 255],
                [110, 76, 40, 255],
                [95, 64, 31, 255],
                [90, 60, 29, 255],
                [96, 65, 32, 255],
                [90, 60, 28, 255],
                [102, 70, 36, 255],
                [108, 75, 39, 255],
                [94, 63, 30, 255],
                [86, 56, 26, 255],
                [100, 68, 35, 255],
                [95, 64, 31, 255],
                [88, 58, 28, 255],
                [102, 70, 36, 255],
                [94, 63, 31, 255],
                [90, 60, 29, 255],
                [98, 66, 33, 255],
                [105, 72, 37, 255],
                [92, 61, 29, 255],
                [85, 55, 25, 255],
                [94, 63, 31, 255],
                [88, 58, 27, 255],
                [100, 68, 35, 255],
                [96, 64, 32, 255],
                [90, 60, 28, 255],
                [92, 62, 30, 255],
                [104, 71, 36, 255],
                [98, 66, 33, 255],
                [86, 56, 26, 255],
                [95, 64, 32, 255],
                [90, 60, 29, 255],
                [84, 54, 25, 255],
                [98, 66, 34, 255],
                [102, 70, 36, 255],
                [92, 61, 30, 255],
                [88, 58, 28, 255],
                [82, 53, 24, 255],
                [88, 58, 27, 255],
                [85, 55, 25, 255],
                [80, 52, 23, 255],
                [90, 60, 28, 255],
                [86, 56, 26, 255],
                [84, 54, 24, 255],
                [78, 50, 22, 255],
            ],
            authoring: MaterialAuthoring::default(),
        })
        .expect("built-in material definitions are valid");

        // core:stone (stable ID after freeze: 3)
        reg.register(MaterialDef {
            name: "core:stone".to_string(),
            behavior: Behavior::Solid,
            mass_per_cell: 2_500,
            dispersion: 0,
            max_fall_speed: 4,
            flow_speed: 1,
            can_be_washed_away: false,
            texture: [
                [130, 130, 130, 255],
                [140, 140, 140, 255],
                [135, 135, 135, 255],
                [125, 125, 125, 255],
                [145, 145, 145, 255],
                [138, 138, 138, 255],
                [130, 130, 130, 255],
                [120, 120, 120, 255],
                [125, 125, 125, 255],
                [115, 115, 115, 255],
                [105, 105, 105, 255],
                [120, 120, 120, 255],
                [135, 135, 135, 255],
                [128, 128, 128, 255],
                [118, 118, 118, 255],
                [112, 112, 112, 255],
                [120, 120, 120, 255],
                [108, 108, 108, 255],
                [95, 95, 95, 255],
                [110, 110, 110, 255],
                [122, 122, 122, 255],
                [115, 115, 115, 255],
                [128, 128, 128, 255],
                [118, 118, 118, 255],
                [115, 115, 115, 255],
                [122, 122, 122, 255],
                [110, 110, 110, 255],
                [100, 100, 100, 255],
                [92, 92, 92, 255],
                [105, 105, 105, 255],
                [118, 118, 118, 255],
                [110, 110, 110, 255],
                [128, 128, 128, 255],
                [118, 118, 118, 255],
                [125, 125, 125, 255],
                [112, 112, 112, 255],
                [104, 104, 104, 255],
                [115, 115, 115, 255],
                [122, 122, 122, 255],
                [116, 116, 116, 255],
                [110, 110, 110, 255],
                [102, 102, 102, 255],
                [115, 115, 115, 255],
                [120, 120, 120, 255],
                [112, 112, 112, 255],
                [108, 108, 108, 255],
                [114, 114, 114, 255],
                [105, 105, 105, 255],
                [105, 105, 105, 255],
                [112, 112, 112, 255],
                [108, 108, 108, 255],
                [100, 100, 100, 255],
                [96, 96, 96, 255],
                [104, 104, 104, 255],
                [110, 110, 110, 255],
                [102, 102, 102, 255],
                [95, 95, 95, 255],
                [100, 100, 100, 255],
                [92, 92, 92, 255],
                [88, 88, 88, 255],
                [98, 98, 98, 255],
                [102, 102, 102, 255],
                [96, 96, 96, 255],
                [90, 90, 90, 255],
            ],
            authoring: MaterialAuthoring::default(),
        })
        .expect("built-in material definitions are valid");

        // core:water (stable ID after freeze: 4)
        reg.register(MaterialDef {
            name: "core:water".to_string(),
            behavior: Behavior::Liquid,
            mass_per_cell: 1_000,
            dispersion: 4,
            max_fall_speed: 5,
            flow_speed: 3,
            can_be_washed_away: false,
            texture: [
                [220, 240, 255, 230],
                [210, 235, 255, 225],
                [230, 245, 255, 235],
                [200, 230, 255, 220],
                [225, 242, 255, 230],
                [215, 238, 255, 225],
                [205, 232, 255, 220],
                [220, 240, 255, 230],
                [70, 130, 230, 210],
                [65, 125, 225, 205],
                [75, 135, 235, 215],
                [60, 120, 220, 200],
                [72, 132, 232, 210],
                [68, 128, 228, 205],
                [62, 122, 222, 200],
                [70, 130, 230, 210],
                [50, 110, 210, 210],
                [55, 115, 215, 215],
                [48, 108, 208, 205],
                [52, 112, 212, 210],
                [58, 118, 218, 215],
                [45, 105, 205, 200],
                [53, 113, 213, 210],
                [50, 110, 210, 210],
                [40, 95, 195, 215],
                [42, 98, 198, 215],
                [38, 92, 192, 210],
                [45, 100, 200, 220],
                [35, 90, 190, 210],
                [44, 99, 199, 215],
                [40, 95, 195, 215],
                [38, 92, 192, 210],
                [30, 80, 180, 220],
                [32, 82, 182, 220],
                [28, 78, 178, 215],
                [35, 85, 185, 225],
                [34, 84, 184, 220],
                [26, 76, 176, 215],
                [30, 80, 180, 220],
                [32, 82, 182, 220],
                [25, 70, 170, 225],
                [28, 72, 172, 225],
                [22, 68, 168, 220],
                [30, 75, 175, 230],
                [26, 71, 171, 225],
                [24, 69, 169, 220],
                [27, 73, 173, 225],
                [25, 70, 170, 225],
                [20, 60, 160, 230],
                [22, 62, 162, 230],
                [18, 58, 158, 225],
                [25, 65, 165, 235],
                [21, 61, 161, 230],
                [19, 59, 159, 225],
                [23, 63, 163, 230],
                [20, 60, 160, 230],
                [15, 50, 150, 235],
                [17, 52, 152, 235],
                [14, 48, 148, 230],
                [20, 55, 155, 240],
                [16, 51, 151, 235],
                [15, 50, 150, 235],
                [18, 53, 153, 235],
                [15, 50, 150, 235],
            ],
            authoring: MaterialAuthoring::default(),
        })
        .expect("built-in material definitions are valid");

        // core:sand (stable ID after freeze: 2)
        reg.register(MaterialDef {
            name: "core:sand".to_string(),
            behavior: Behavior::Powder,
            mass_per_cell: 1_500,
            dispersion: 0,
            max_fall_speed: 4,
            flow_speed: 1,
            can_be_washed_away: true,
            texture: [
                [235, 210, 140, 255],
                [225, 200, 130, 255],
                [240, 215, 145, 255],
                [218, 192, 122, 255],
                [230, 205, 135, 255],
                [238, 212, 142, 255],
                [222, 198, 128, 255],
                [215, 190, 120, 255],
                [228, 202, 132, 255],
                [218, 192, 122, 255],
                [232, 208, 138, 255],
                [225, 200, 130, 255],
                [216, 190, 120, 255],
                [235, 210, 140, 255],
                [226, 201, 131, 255],
                [220, 195, 125, 255],
                [220, 195, 125, 255],
                [230, 205, 135, 255],
                [215, 190, 120, 255],
                [238, 212, 142, 255],
                [224, 198, 128, 255],
                [218, 192, 122, 255],
                [230, 204, 134, 255],
                [222, 196, 126, 255],
                [232, 208, 138, 255],
                [222, 198, 128, 255],
                [228, 202, 132, 255],
                [216, 190, 120, 255],
                [235, 210, 140, 255],
                [225, 200, 130, 255],
                [218, 192, 122, 255],
                [230, 205, 135, 255],
                [225, 200, 130, 255],
                [215, 190, 120, 255],
                [235, 210, 140, 255],
                [228, 202, 132, 255],
                [218, 192, 122, 255],
                [232, 208, 138, 255],
                [222, 198, 128, 255],
                [216, 190, 120, 255],
                [218, 192, 122, 255],
                [232, 208, 138, 255],
                [222, 198, 128, 255],
                [215, 190, 120, 255],
                [230, 205, 135, 255],
                [220, 195, 125, 255],
                [236, 211, 141, 255],
                [224, 198, 128, 255],
                [230, 205, 135, 255],
                [220, 195, 125, 255],
                [228, 202, 132, 255],
                [235, 210, 140, 255],
                [222, 198, 128, 255],
                [216, 190, 120, 255],
                [226, 201, 131, 255],
                [218, 192, 122, 255],
                [222, 198, 128, 255],
                [228, 202, 132, 255],
                [216, 190, 120, 255],
                [220, 195, 125, 255],
                [234, 208, 138, 255],
                [224, 198, 128, 255],
                [218, 192, 122, 255],
                [212, 186, 116, 255],
            ],
            authoring: MaterialAuthoring::default(),
        })
        .expect("built-in material definitions are valid");

        reg
    }

    /// 新しいマテリアルを検証して登録します。ID 0と65535は予約されています。
    pub fn register(&mut self, def: MaterialDef) -> Result<u16, MaterialRegistryError> {
        if self.frozen {
            return Err(MaterialRegistryError::Frozen);
        }
        validate_material_name(&def.name)?;
        if def.name == "engine:missing" {
            return Err(MaterialRegistryError::ReservedName(def.name));
        }
        if self.name_to_id.contains_key(&def.name) {
            return Err(MaterialRegistryError::DuplicateName(def.name));
        }
        def.authoring
            .validate()
            .map_err(|reason| MaterialRegistryError::InvalidAuthoring(def.name.clone(), reason))?;
        if def.name != "core:air"
            && (def.mass_per_cell == 0 || def.mass_per_cell > MAX_MASS_PER_CELL)
        {
            return Err(MaterialRegistryError::InvalidAuthoring(
                def.name,
                "mass_per_cell must be within 1..=1,000,000",
            ));
        }
        if self.materials.len() > MAX_REGISTERED_ID {
            return Err(MaterialRegistryError::CapacityExceeded);
        }
        let id = self.materials.len() as u16;
        self.name_to_id.insert(def.name.clone(), id);
        self.materials.push(def);
        Ok(id)
    }

    /// ビルダーからマテリアルを登録します。
    pub fn register_builder(
        &mut self,
        builder: MaterialBuilder,
    ) -> Result<u16, MaterialRegistryError> {
        self.register(builder.build())
    }

    /// 永続name順にIDを確定し、以降の登録を禁止します。
    pub fn freeze(&mut self) -> Result<(), MaterialRegistryError> {
        self.try_freeze()
    }

    pub fn try_freeze(&mut self) -> Result<(), MaterialRegistryError> {
        if self.frozen {
            return Ok(());
        }
        for material in &self.materials {
            if let Some(product) = material
                .authoring
                .flammability
                .as_ref()
                .and_then(|flammability| flammability.exhaust_material.as_ref())
                && !self.name_to_id.contains_key(product)
            {
                return Err(MaterialRegistryError::UnknownMaterialReference(
                    product.clone(),
                ));
            }
        }
        self.materials
            .sort_by(|left, right| left.name.cmp(&right.name));
        self.name_to_id.clear();
        for (index, material) in self.materials.iter().enumerate() {
            self.name_to_id.insert(material.name.clone(), index as u16);
        }
        self.frozen = true;
        Ok(())
    }

    #[inline]
    pub fn is_frozen(&self) -> bool {
        self.frozen
    }

    /// マテリアル ID から物性定義を高速 O(1) で取得します。
    /// 未定義IDと予約済みmissing IDは不透明なmissing定義として扱います。
    #[inline]
    pub fn get(&self, id: u16) -> &MaterialDef {
        if id == MISSING {
            &self.missing
        } else {
            self.materials.get(id as usize).unwrap_or(&self.missing)
        }
    }

    /// マテリアル名から ID を取得します。
    pub fn get_id_by_name(&self, name: &str) -> Option<u16> {
        self.name_to_id.get(name).copied()
    }

    /// 指定IDがレジストリ内の通常マテリアルかを返します。
    #[inline]
    pub fn is_registered(&self, id: u16) -> bool {
        (id as usize) < self.materials.len()
    }

    /// 登録順に内容をFNV diagnostic fingerprintへ加えます（現在のIDも登録順）。
    pub(crate) fn write_fingerprint(&self, hash: &mut Fnv64) {
        for material in &self.materials {
            hash.write(material.name.as_bytes());
            hash.write(&[0]);
            hash.write(&[match material.behavior {
                Behavior::Empty => 0,
                Behavior::Solid => 1,
                Behavior::Liquid => 2,
                Behavior::Powder => 3,
            }]);
            hash.write(&[
                material.dispersion,
                material.max_fall_speed,
                material.flow_speed,
                u8::from(material.can_be_washed_away),
            ]);
            hash.write_u32(material.mass_per_cell);
            write_authoring_fingerprint(&material.authoring, hash);
            for rgba in material.texture {
                hash.write(&rgba);
            }
        }
    }

    /// 登録されている総マテリアル数を取得します。
    #[inline]
    pub fn len(&self) -> usize {
        self.materials.len()
    }

    /// レジストリが空かどうかを取得します。
    #[inline]
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.materials.is_empty()
    }

    /// 周囲のコンテキストおよびブロック結合状態を考慮した適応的レンダリング色を高速 O(1) で取得します。
    #[inline]
    pub fn get_adaptive_color(
        &self,
        material_id: u16,
        world_x: u32,
        world_y: u32,
        is_surface: bool,
        stored_data: u8,
        is_falling: bool,
    ) -> [u8; 4] {
        let mat = self.get(material_id);

        // 水（WATER）の適応的レンダリング（水面ハイライト）
        if material_id == WATER {
            if is_surface {
                let tx = (world_x % 8) as usize;
                return mat.texture[tx];
            } else {
                let tx = (world_x % 8) as usize;
                let ty = ((world_y % 7) + 1) as usize;
                return mat.texture[ty * 8 + tx];
            }
        }

        // 自由落下・崩落中の粒子（土・石・砂）：ブロック構造から切り離されているため、
        // ワールド座標でタイリングせず、粒子固有のテクスチャ色（stored_data）でレンダリング
        if is_falling || mat.behavior == Behavior::Powder {
            let idx = (stored_data as usize) % 64;
            return mat.texture[idx];
        }

        // 静止した固体（DIRT, STONE 等）：ブロックとして連続しているため、現在のワールド座標に基づくタイリングで描画
        if mat.behavior == Behavior::Solid {
            let tx = (world_x % 8) as usize;
            let ty = (world_y % 8) as usize;
            return mat.texture[ty * 8 + tx];
        }

        // その他のマテリアル（空など）
        let idx = (stored_data as usize) % 64;
        mat.texture[idx]
    }
}

fn validate_material_name(name: &str) -> Result<(), MaterialRegistryError> {
    let invalid = || MaterialRegistryError::InvalidName(name.to_string());
    let mut parts = name.split(':');
    let (Some(namespace), Some(material), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(invalid());
    };
    let valid_part = |part: &str| {
        !part.is_empty()
            && part.len() <= 64
            && part.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
            })
    };
    if valid_part(namespace) && valid_part(material) {
        Ok(())
    } else {
        Err(invalid())
    }
}

fn write_authoring_fingerprint(authoring: &MaterialAuthoring, hash: &mut Fnv64) {
    let write_string = |value: &str, hash: &mut Fnv64| {
        hash.write_u64(value.len() as u64);
        hash.write(value.as_bytes());
    };
    let write_properties = |properties: &[crate::simulation::authoring::MaterialProperty],
                            hash: &mut Fnv64| {
        hash.write_u64(properties.len() as u64);
        for property in properties {
            write_string(&property.key, hash);
            match &property.value {
                PropertyValue::Bool(value) => hash.write(&[0, u8::from(*value)]),
                PropertyValue::Integer(value) => {
                    hash.write(&[1]);
                    hash.write(&value.to_le_bytes());
                }
                PropertyValue::FixedQ16(value) => {
                    hash.write(&[2]);
                    hash.write(&value.to_le_bytes());
                }
                PropertyValue::Enum(value) => {
                    hash.write(&[3]);
                    write_string(value, hash);
                }
                PropertyValue::Reference(value) => {
                    hash.write(&[4]);
                    write_string(value, hash);
                }
            }
        }
    };

    write_properties(&authoring.properties, hash);
    hash.write_u64(authoring.attachments.len() as u64);
    for attachment in &authoring.attachments {
        write_string(&attachment.kind, hash);
        hash.write_u16(attachment.version);
        write_properties(&attachment.values, hash);
    }
    if let Some(flammability) = &authoring.flammability {
        hash.write(&[1]);
        hash.write_u32(flammability.ignition_energy);
        hash.write_u16(flammability.fuel_ticks);
        hash.write_u16(flammability.heat_per_tick);
        if let Some(product) = &flammability.exhaust_material {
            hash.write(&[1]);
            write_string(product, hash);
        } else {
            hash.write(&[0]);
        }
    } else {
        hash.write(&[0]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_registry() {
        let reg = MaterialRegistry::with_defaults();
        assert_eq!(reg.len(), 5, "デフォルトは 5 種類");
        assert_eq!(reg.get(AIR).name, "core:air");
        assert_eq!(reg.get(DIRT).name, "core:dirt");
        assert_eq!(reg.get(STONE).name, "core:stone");
        assert_eq!(reg.get(WATER).name, "core:water");
        assert_eq!(reg.get(SAND).name, "core:sand");

        // 未定義 ID のフォールバック
        assert_eq!(
            reg.get(9999).name,
            "engine:missing",
            "未登録 ID は空気にせずmissingとして扱う"
        );
        assert_eq!(reg.get(MISSING).behavior, Behavior::Solid);
    }

    #[test]
    fn test_builder_and_custom_registration() {
        let mut reg = MaterialRegistry::with_default_materials_builder();

        // クリエイターによる独自ブロック登録
        reg.register_builder(
            MaterialBuilder::new("example:ruby_block", [220, 20, 60, 255])
                .behavior(Behavior::Solid)
                .mass_per_cell(2_500),
        )
        .unwrap();

        reg.freeze().unwrap();
        let ruby_id = reg.get_id_by_name("example:ruby_block").unwrap();

        assert_eq!(ruby_id, 5, "新マテリアルの ID は 5");
        assert_eq!(reg.len(), 6);
        assert_eq!(reg.get(ruby_id).name, "example:ruby_block");
        assert_eq!(reg.get_id_by_name("example:ruby_block"), Some(5));
    }

    #[test]
    fn freeze_assigns_ids_by_name_and_blocks_registration() {
        let mut reg = MaterialRegistry::new();
        let make = |name: &str| MaterialDef {
            name: name.to_string(),
            behavior: Behavior::Solid,
            mass_per_cell: 1,
            dispersion: 0,
            max_fall_speed: 0,
            flow_speed: 0,
            can_be_washed_away: false,
            texture: [[0; 4]; 64],
            authoring: MaterialAuthoring::default(),
        };
        reg.register(make("example:zeta")).unwrap();
        reg.register(make("example:alpha")).unwrap();
        reg.freeze().unwrap();

        assert_eq!(reg.get_id_by_name("core:air"), Some(AIR));
        assert_eq!(reg.get_id_by_name("example:alpha"), Some(1));
        assert_eq!(reg.get_id_by_name("example:zeta"), Some(2));
        assert!(reg.is_frozen());
        assert_eq!(
            reg.register(make("example:late")),
            Err(MaterialRegistryError::Frozen)
        );
    }

    #[test]
    fn empty_registry_freezes_and_material_id_capacity_is_checked() {
        let mut empty = MaterialRegistry::new();
        empty.try_freeze().unwrap();
        assert_eq!(empty.len(), 1);
        assert_eq!(empty.get_id_by_name("core:air"), Some(AIR));

        let mut registry = MaterialRegistry::new();
        let make = |index: usize| MaterialDef {
            name: format!("test:m{index:05}"),
            behavior: Behavior::Solid,
            mass_per_cell: 1,
            dispersion: 0,
            max_fall_speed: 0,
            flow_speed: 0,
            can_be_washed_away: false,
            texture: [[0; 4]; 64],
            authoring: MaterialAuthoring::default(),
        };
        for index in 0..=u16::MAX as usize - 2 {
            registry.register(make(index)).unwrap();
        }
        assert_eq!(registry.len(), u16::MAX as usize);
        assert_eq!(
            registry.register(make(u16::MAX as usize)),
            Err(MaterialRegistryError::CapacityExceeded)
        );
    }

    #[test]
    fn duplicate_and_invalid_names_fail_without_changing_registry() {
        let mut registry = MaterialRegistry::new();
        let make = |name: &str| MaterialDef {
            name: name.to_string(),
            behavior: Behavior::Solid,
            mass_per_cell: 1,
            dispersion: 0,
            max_fall_speed: 0,
            flow_speed: 0,
            can_be_washed_away: false,
            texture: [[0; 4]; 64],
            authoring: MaterialAuthoring::default(),
        };
        registry.register(make("test:valid")).unwrap();
        let before = registry.len();
        assert_eq!(
            registry.register(make("test:valid")),
            Err(MaterialRegistryError::DuplicateName("test:valid".into()))
        );
        assert!(matches!(
            registry.register(make("Invalid Name")),
            Err(MaterialRegistryError::InvalidName(_))
        ));
        assert_eq!(registry.len(), before);
    }

    #[test]
    fn creator_mass_typed_properties_attachments_and_flammability_register() {
        use crate::simulation::authoring::{Flammability, MaterialAttachment, PropertyValue};
        let mut registry = MaterialRegistry::with_default_materials_builder();
        registry
            .register_builder(
                MaterialBuilder::new("sample:light_wood", [120, 80, 40, 255])
                    .mass_per_cell(500)
                    .property("sample:mana", PropertyValue::Integer(12))
                    .attach(MaterialAttachment {
                        kind: "sample:mana_receiver".into(),
                        version: 1,
                        values: vec![crate::simulation::authoring::MaterialProperty {
                            key: "sample:channel".into(),
                            value: PropertyValue::Enum("blue".into()),
                        }],
                    })
                    .flammability(Flammability {
                        ignition_energy: 800,
                        fuel_ticks: 600,
                        heat_per_tick: 8,
                        exhaust_material: Some("core:air".into()),
                    }),
            )
            .unwrap();
        registry.try_freeze().unwrap();
        let wood = registry.get(registry.get_id_by_name("sample:light_wood").unwrap());
        assert_eq!(wood.mass_per_cell, 500);
        assert_eq!(wood.authoring.properties[0].key, "sample:mana");
        assert_eq!(wood.authoring.attachments[0].version, 1);
        assert_eq!(
            wood.authoring.flammability.as_ref().unwrap().fuel_ticks,
            600
        );
    }

    #[test]
    fn freeze_rejects_unknown_combustion_product() {
        use crate::simulation::authoring::Flammability;
        let mut registry = MaterialRegistry::with_default_materials_builder();
        registry
            .register_builder(
                MaterialBuilder::new("sample:dry_wood", [120, 80, 40, 255]).flammability(
                    Flammability {
                        ignition_energy: 10,
                        fuel_ticks: 30,
                        heat_per_tick: 1,
                        exhaust_material: Some("sample:unknown_ash".into()),
                    },
                ),
            )
            .unwrap();
        assert_eq!(
            registry.try_freeze(),
            Err(MaterialRegistryError::UnknownMaterialReference(
                "sample:unknown_ash".into()
            ))
        );
        assert!(!registry.is_frozen());
    }

    #[test]
    fn creator_mass_outside_schema_range_is_rejected_at_registration() {
        let mut registry = MaterialRegistry::new();
        let result = registry.register_builder(
            MaterialBuilder::new("sample:too_heavy", [1, 2, 3, 255])
                .mass_per_cell(MAX_MASS_PER_CELL + 1),
        );
        assert!(matches!(
            result,
            Err(MaterialRegistryError::InvalidAuthoring(_, _))
        ));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn material_authoring_data_changes_diagnostic_fingerprint() {
        use crate::simulation::authoring::PropertyValue;
        let mut first_registry = MaterialRegistry::with_default_materials_builder();
        first_registry
            .register_builder(
                MaterialBuilder::new("sample:crystal", [10, 20, 30, 255])
                    .mass_per_cell(900)
                    .property("sample:charge", PropertyValue::Integer(1)),
            )
            .unwrap();
        let mut second_registry = MaterialRegistry::with_default_materials_builder();
        second_registry
            .register_builder(
                MaterialBuilder::new("sample:crystal", [10, 20, 30, 255])
                    .mass_per_cell(900)
                    .property("sample:charge", PropertyValue::Integer(2)),
            )
            .unwrap();

        let first = crate::simulation::world::World::with_registry(8, 8, first_registry).unwrap();
        let second = crate::simulation::world::World::with_registry(8, 8, second_registry).unwrap();
        assert_ne!(first.state_fingerprint(), second.state_fingerprint());
    }

    #[test]
    fn test_large_scale_registration_perf() {
        let mut reg = MaterialRegistry::new();
        // 10,000 個のマテリアルを高速動的登録
        for i in 0..10_000 {
            reg.register(MaterialDef {
                name: format!("example:mat_{}", i),
                behavior: Behavior::Solid,
                mass_per_cell: ((i % 256) + 1) as u32,
                dispersion: 0,
                max_fall_speed: 0,
                flow_speed: 0,
                can_be_washed_away: false,
                texture: [[(i % 256) as u8; 4]; 64],
                authoring: MaterialAuthoring::default(),
            })
            .unwrap();
        }

        assert_eq!(
            reg.len(),
            10_001,
            "空気に加えて1万個のマテリアルが登録されている"
        );
        // O(1) ランダムアクセス
        assert_eq!(reg.get(7778).name, "example:mat_7777");
    }
}
