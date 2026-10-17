//! 保存可能なMaterialMotion channelに対する標準・独自の力積分入口。
//!
//! 速度更新とDDA輸送を一つのWorld取引で実行できます。作者providerはWorldの
//! 読取専用状態を参照して独自式へ完全置換でき、標準式を暗黙に二重適用しません。

use super::{
    CellQuery, ChannelQuery, LayerId, MaterialBase, MaterialDef, MaterialMotion,
    MaterialMotionConfig, MaterialMotionReport, World, WorldEdit, invalid,
};
use crate::foundation::{
    EngineError, EngineResult, StableKey,
    error::allocation,
    grid::{CellPoint, GridValue},
};

const ONE: i128 = 65536;

/// 力の対象cellの重力側へ隣接する物質。未読込は別の待機として扱います。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialForceNeighbor {
    /// 隣接素材の標準分類。
    pub base: MaterialBase,
    /// 隣接素材の1cell当たりのゲーム質量。
    pub mass_per_cell: u32,
    /// block個体の一部を占有しているか。
    pub block: bool,
}

/// providerへ渡す不変の入力。必要に応じて保存済みWorld作者状態も読めます。
pub struct MaterialForceInput<'a> {
    /// 候補開始時点の読み取り専用World。
    pub world: &'a World,
    /// Worldの対象layer。
    pub layer: LayerId,
    /// 候補のcell位置。
    pub point: CellPoint,
    /// 素材定義。標準以外の物性もContentに保持できます。
    pub material: &'a MaterialDef,
    /// 移動channelが所有する現在の速度と端数。
    pub motion: MaterialMotion,
    /// Q16 cell/tick²の重力ベクトル。符号は右・下が正。
    pub gravity_q16: [i64; 2],
    /// 重力側にある隣接素材。World境界はNoneです。
    pub neighbor: Option<MaterialForceNeighbor>,
}

/// 書換え可能な力学処理。キー・versionを作者のsim設定とともに保存してください。
///
/// callbackはWorldの参照だけを受け取り、結果を返します。追加の作者状態は
/// World StateStoreなどへ保存し、callback自身の隠れた時間状態に依存させません。
pub trait MaterialForceProvider: Send + Sync {
    /// 正規の実装キー。
    fn key(&self) -> StableKey;
    /// 係数・アルゴリズム・読み取りstencilを変更したら更新する版。
    fn version(&self) -> u32;
    /// 一候補のQ16速度と端数の更新。別のcellを書き換えません。
    fn integrate(&self, input: MaterialForceInput<'_>) -> EngineResult<MaterialMotion>;
}

/// 標準Q16参照式。独自providerに置き換えれば標準式は適用されません。
pub struct StandardMaterialForces;
impl MaterialForceProvider for StandardMaterialForces {
    fn key(&self) -> StableKey {
        StableKey::parse("engine:standard-material-forces").expect("固定された標準providerキー")
    }
    fn version(&self) -> u32 {
        1
    }
    fn integrate(&self, input: MaterialForceInput<'_>) -> EngineResult<MaterialMotion> {
        let properties = input.material.motion_properties();
        properties.validate()?;
        let mut updated = input.motion;
        let mut velocity = input.motion.velocity_q16.map(i128::from);
        for (axis, component) in velocity.iter_mut().enumerate() {
            // 抗力は速度の既存符号を変えず、0方向への整数除算で丸めます。
            *component = *component * (ONE - i128::from(properties.drag_q16)) / ONE;
            *component += i128::from(input.gravity_q16[axis])
                * i128::from(properties.gravity_scale_q16)
                / ONE;
        }
        let gravity_axis =
            usize::from(input.gravity_q16[1].unsigned_abs() >= input.gravity_q16[0].unsigned_abs());
        if let Some(neighbor) = input.neighbor {
            if matches!(neighbor.base, MaterialBase::Liquid | MaterialBase::Gas)
                && neighbor.mass_per_cell > input.material.mass_per_cell()
            {
                // 密度差の比を有理のまま計算し、重力と逆向きの浮力へ換算します。
                let difference =
                    i128::from(neighbor.mass_per_cell - input.material.mass_per_cell());
                let buoyancy = i128::from(input.gravity_q16[gravity_axis])
                    * i128::from(properties.buoyancy_scale_q16)
                    * difference
                    / (ONE * i128::from(neighbor.mass_per_cell));
                velocity[gravity_axis] -= buoyancy;
            }
            if neighbor.base == MaterialBase::Solid || neighbor.block {
                let tangent = 1 - gravity_axis;
                velocity[tangent] =
                    velocity[tangent] * (ONE - i128::from(properties.friction_q16)) / ONE;
            }
        }
        for (axis, component) in velocity.into_iter().enumerate() {
            updated.velocity_q16[axis] =
                i64::try_from(component).map_err(|_| invalid("物性速度Q16の値域"))?;
        }
        updated.validate()?;
        Ok(updated)
    }
}

/// 標準・独自providerを共通の候補順で実行するための設定。
#[derive(Clone, Copy, Debug)]
pub struct MaterialForceConfig {
    /// 対象layerに所有された1対1のMaterialMotion channel。
    pub channel: StableKey,
    /// 重力ベクトル。方向も作者が決めます。
    pub gravity_q16: [i64; 2],
    /// 対象tickに列挙する活動物質の最大数。
    pub candidate_limit: usize,
    /// 絶対Q16速度の上限。飽和させず、超過すると全World取引を失敗させます。
    pub max_velocity_q16: i64,
}

/// 力積分の診断。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MaterialForceReport {
    /// 処理対象の非空・非blockセル数。
    pub candidates: usize,
    /// 重力側の未読込regionにより次tickへ延期した件数。
    pub waiting_for_region: usize,
    /// MaterialMotionが変化した件数。
    pub changed: usize,
}

/// 力の更新と連続DDA輸送の同一tick統合診断。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialForceMotionReport {
    /// 力積分。
    pub force: MaterialForceReport,
    /// 速度による経路輸送。
    pub motion: MaterialMotionReport,
}

impl WorldEdit<'_> {
    /// 明示されたproviderだけを、活動素材の速度channelへ適用します。
    ///
    /// 複数更新は同じ候補へまとめ、providerがエラーを返した場合や速度の上限を
    /// 超えた場合は先行更新まで巻き戻します。未読込の重力側は空と仮定せず待機。
    /// 物質channelの追従・DDA・粒子stepはこの入口では実行しません。
    pub fn integrate_material_forces(
        &mut self,
        layer: LayerId,
        config: MaterialForceConfig,
        provider: &dyn MaterialForceProvider,
    ) -> EngineResult<MaterialForceReport> {
        self.run(|editor| {
            if provider.version() == 0 || config.max_velocity_q16 <= 0 {
                return Err(invalid("物性provider版・速度上限"));
            }
            let layer_key = editor.world.content.layer(layer)?.key;
            if editor
                .world
                .channel_material_layer::<MaterialMotion>(config.channel)?
                != Some(layer_key)
            {
                return Err(invalid("物性速度channelの所有layer"));
            }
            let origin = editor
                .world
                .channel_layout::<MaterialMotion>(config.channel)?
                .origin;
            let points = editor.material_candidates(layer, config.candidate_limit)?;
            let mut report = MaterialForceReport {
                candidates: points.len(),
                ..Default::default()
            };
            // 全候補のprovider評価を同一の読取りスナップショットで終えてから
            // 更新を公開します。候補順による隣接速度の参照ぶれを防ぎます。
            let mut updates = Vec::new();
            updates
                .try_reserve_exact(points.len())
                .map_err(allocation)?;
            for point in points {
                let cell = match editor.world.cell(layer, point)? {
                    CellQuery::Loaded(value) => value,
                    _ => return Err(invalid("物性候補のcoverage")),
                };
                let material = editor
                    .world
                    .content
                    .material(cell.core.material)?
                    .ok_or_else(|| invalid("物性候補の空素材"))?;
                let coord = CellPoint {
                    x: point
                        .x
                        .checked_sub(origin.x)
                        .ok_or_else(|| invalid("物性channel水平座標"))?,
                    y: point
                        .y
                        .checked_sub(origin.y)
                        .ok_or_else(|| invalid("物性channel垂直座標"))?,
                };
                let previous = match editor
                    .world
                    .channel_value::<MaterialMotion>(config.channel, coord)?
                {
                    ChannelQuery::Loaded(value) => value,
                    _ => return Err(invalid("物性速度channelのcoverage")),
                };
                let axis = usize::from(
                    config.gravity_q16[1].unsigned_abs() >= config.gravity_q16[0].unsigned_abs(),
                );
                let sign = config.gravity_q16[axis].signum() as i32;
                let neighbor_point = if config.gravity_q16 == [0, 0] {
                    None
                } else if axis == 0 {
                    point
                        .x
                        .checked_add(sign)
                        .map(|x| CellPoint { x, y: point.y })
                } else {
                    point
                        .y
                        .checked_add(sign)
                        .map(|y| CellPoint { x: point.x, y })
                };
                let neighbor = match neighbor_point {
                    None => None,
                    Some(position) => match editor.world.cell(layer, position)? {
                        CellQuery::WorldBoundary => None,
                        CellQuery::Loaded(value) => editor
                            .world
                            .content
                            .material(value.core.material)?
                            .map(|material| MaterialForceNeighbor {
                                base: material.base(),
                                mass_per_cell: material.mass_per_cell(),
                                block: value.block.is_some(),
                            }),
                        _ => {
                            report.waiting_for_region += 1;
                            continue;
                        }
                    },
                };
                let updated = provider.integrate(MaterialForceInput {
                    world: editor.world,
                    layer,
                    point,
                    material,
                    motion: previous,
                    gravity_q16: config.gravity_q16,
                    neighbor,
                })?;
                updated.validate()?;
                if updated
                    .velocity_q16
                    .iter()
                    .any(|speed| speed.unsigned_abs() > config.max_velocity_q16 as u64)
                {
                    return Err(EngineError::capacity(
                        "物性速度上限",
                        updated
                            .velocity_q16
                            .iter()
                            .map(|v| v.unsigned_abs())
                            .max()
                            .unwrap_or(0),
                        config.max_velocity_q16 as u64,
                    ));
                }
                if updated != previous {
                    updates.push((coord, updated));
                    report.changed += 1;
                }
            }
            for (coord, updated) in updates {
                editor.set_channel(config.channel, coord, updated)?;
            }
            Ok(report)
        })
    }

    /// 力を一度だけ積分してから既存DDAを一度実行します。
    ///
    /// 一体化した取引で失敗をrollbackし、物質の全channel追従を既存の正規経路
    /// に任せます。広域移動Job、dirty/sleep、正規粒子への力は別の実装単位です。
    pub fn step_material_force_motion(
        &mut self,
        layer: LayerId,
        force: MaterialForceConfig,
        motion: MaterialMotionConfig,
        provider: &dyn MaterialForceProvider,
    ) -> EngineResult<MaterialForceMotionReport> {
        self.run(|editor| {
            if force.channel != motion.channel {
                return Err(invalid("物性とDDAの速度channelが一致しない"));
            }
            let force = editor.integrate_material_forces(layer, force, provider)?;
            let motion = editor.transport_material_motion(layer, motion)?;
            Ok(MaterialForceMotionReport { force, motion })
        })
    }
}
