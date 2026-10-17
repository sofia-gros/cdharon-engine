//! E05の物質追従速度channelへ、標準係数または作者providerの力を原子的に適用する。
//!
//! 力の積分とDDA輸送を分離し、作者の独自則は全ての候補を検査したWorld取引の
//! 内側で評価する。保存はMaterialMotion channelが担い、provider自身の状態は
//! 作者が独立したschema/実行profileで管理する。

use super::{
    CellQuery, ChannelQuery, LayerId, MaterialBase, MaterialId, MaterialMotion, WorldEdit, invalid,
};
use crate::foundation::{EngineError, EngineResult, StableKey, error::allocation, grid::CellPoint};

const SCALE: i128 = 65536;

/// 一つの活動cellの参照情報。materialはContent内で版管理した定義です。
#[derive(Clone, Copy, Debug)]
pub struct MaterialForceInput {
    pub point: CellPoint,
    pub tick: u64,
    pub material: StableKey,
    pub base: MaterialBase,
    pub mass_per_cell: u32,
    /// phase開始時点の速度・端数。providerは速度成分だけを返します。
    pub motion: MaterialMotion,
}

/// 作者の速度計算を置換する読み取り専用provider。
/// 戻り値は二軸のQ16 cell/tick速度で、端数は引き継ぎます。
/// 結果は同じWorld取引で公開され、失敗時は全候補を巻き戻します。
pub trait MaterialForceProvider: Sync {
    fn evaluate(&self, input: MaterialForceInput) -> EngineResult<[i64; 2]>;
}

/// 均一重力・気体の浮力・抗力・粉体の横摩擦による簡易参照provider。
/// 熱・圧力の局所場を参照する標準物性solverは別契約です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UniformMaterialForces {
    /// 正のYへ加える重力加速度（Q16）。
    pub gravity_q16: i64,
    /// 気体から引く上向きの一様浮力加速度（Q16）。
    pub gas_buoyancy_q16: i64,
    /// 一tickの速度減衰率Q16。0..=65536。
    pub drag_q16: u32,
    /// 粉体の水平速度から0へ近づける固定Q16量。
    pub powder_friction_q16: u32,
    /// 計算後の速度絶対値上限。超えたら丸めずに拒否します。
    pub maximum_velocity_q16: i64,
}

impl UniformMaterialForces {
    pub fn validate(self) -> EngineResult<()> {
        if self.gravity_q16 < 0
            || self.gas_buoyancy_q16 < 0
            || self.drag_q16 > 65536
            || self.maximum_velocity_q16 <= 0
        {
            return Err(invalid("一様物質力の係数範囲"));
        }
        Ok(())
    }
}

impl MaterialForceProvider for UniformMaterialForces {
    fn evaluate(&self, input: MaterialForceInput) -> EngineResult<[i64; 2]> {
        self.validate()?;
        if input.base == MaterialBase::Solid {
            return Ok([0, 0]);
        }
        let y_force = if input.base == MaterialBase::Gas {
            i128::from(self.gravity_q16) - i128::from(self.gas_buoyancy_q16)
        } else {
            i128::from(self.gravity_q16)
        };
        let accelerations = [0, y_force];
        let mut velocities = [0i64; 2];
        for (axis, value) in velocities.iter_mut().enumerate() {
            let with_force = i128::from(input.motion.velocity_q16[axis])
                .checked_add(accelerations[axis])
                .ok_or_else(|| invalid("物質力の速度加算overflow"))?;
            let attenuated = with_force
                .checked_mul(i128::from(65536 - self.drag_q16))
                .ok_or_else(|| invalid("物質抗力の積overflow"))?
                / SCALE;
            let friction = if input.base == MaterialBase::Powder && axis == 0 {
                i128::from(self.powder_friction_q16)
            } else {
                0
            };
            let updated = if attenuated > 0 {
                attenuated.saturating_sub(friction).max(0)
            } else {
                attenuated.saturating_add(friction).min(0)
            };
            if updated.unsigned_abs() > self.maximum_velocity_q16 as u128 {
                return Err(EngineError::capacity(
                    "一様物質力の速度上限",
                    updated.unsigned_abs().min(u64::MAX as u128) as u64,
                    self.maximum_velocity_q16 as u64,
                ));
            }
            *value = i64::try_from(updated).map_err(|_| invalid("物質力の速度表現範囲"))?;
        }
        Ok(velocities)
    }
}

/// 力を適用したcell数と、速度変更を伴ったcell数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MaterialForceReport {
    pub candidates: usize,
    pub updated: usize,
}

impl WorldEdit<'_> {
    /// 活動・非block物質のMaterialMotionへ作者providerが返した速度を適用します。
    ///
    /// 同じWorld::edit内でtransport_material_motionを呼ぶと、速度・輸送の
    /// いずれかが失敗しても全phaseがrollbackします。物性の参照式とprovider識別子は
    /// 作者が実行profileへ固定してください。非活動chunkは更新しません。
    pub fn apply_material_forces<P: MaterialForceProvider + ?Sized>(
        &mut self,
        layer: LayerId,
        channel: StableKey,
        provider: &P,
        candidate_limit: usize,
    ) -> EngineResult<MaterialForceReport> {
        self.run(|editor| {
            let layer_key = editor.world.content.layer(layer)?.key;
            if editor
                .world
                .channel_material_layer::<MaterialMotion>(channel)?
                != Some(layer_key)
            {
                return Err(invalid(
                    "物質力の速度channelは対象layerに追従する必要がある",
                ));
            }
            let origin = editor
                .world
                .channel_layout::<MaterialMotion>(channel)?
                .origin;
            let points = editor.material_candidates(layer, candidate_limit)?;
            let mut report = MaterialForceReport {
                candidates: points.len(),
                updated: 0,
            };
            for point in points {
                let cell = match editor.world.cell(layer, point)? {
                    CellQuery::Loaded(value) => value,
                    _ => return Err(invalid("力更新の活動cellが未読込")),
                };
                let def = editor
                    .world
                    .content
                    .material(cell.core.material)?
                    .ok_or_else(|| invalid("力更新の対象素材が空"))?;
                if cell.core.material == MaterialId::AIR {
                    return Err(invalid("力更新の候補が空"));
                }
                let coordinate = CellPoint {
                    x: point
                        .x
                        .checked_sub(origin.x)
                        .ok_or_else(|| invalid("力channel座標x"))?,
                    y: point
                        .y
                        .checked_sub(origin.y)
                        .ok_or_else(|| invalid("力channel座標y"))?,
                };
                let motion = match editor
                    .world
                    .channel_value::<MaterialMotion>(channel, coordinate)?
                {
                    ChannelQuery::Loaded(value) => value,
                    _ => return Err(invalid("力channelのcoverage")),
                };
                let updated = provider.evaluate(MaterialForceInput {
                    point,
                    tick: editor.world.tick(),
                    material: def.key(),
                    base: def.base(),
                    mass_per_cell: def.mass_per_cell(),
                    motion,
                })?;
                if motion.velocity_q16 != updated {
                    editor.set_channel(
                        channel,
                        coordinate,
                        MaterialMotion {
                            velocity_q16: updated,
                            ..motion
                        },
                    )?;
                    report.updated += 1;
                }
            }
            Ok(report)
        })
    }
}
