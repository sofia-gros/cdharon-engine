//! E05の複数標準material処理を単一World候補へ統合する公開入口。
//!
//! 各処理を個々のWorld::editで公開すると、後段の圧力・移動・粒子の容量失敗
//! に先行する移動だけが残る。このAPIは一つのWorldEditで全結果を確定する。
//! 呼出側のRuntime SystemはPhase::Materialsで一度だけ提出できる。

use super::{
    LayerId, MaterialForceConfig, MaterialForceMotionReport, MaterialForceProvider,
    MaterialMotionConfig, MaterialParticleForceConfig, MaterialParticleForceStepReport,
    MaterialTransportReport, PressureRelaxation, PressureRelaxationReport, WorldEdit, invalid,
};
use crate::foundation::EngineResult;

/// E05の標準処理を一つのWorld取引内で実行する設定。
///
/// 実行順はpressure→cell force+DDA→scalar→canonical particle force+DDA。
/// 独自Providerの状態は別途保存済みWorld状態から評価し、任意の外部副作用を
/// 持つcallbackは許可しない。各段階を無効にしたとき隠れた既定処理はしない。
#[derive(Clone, Copy, Debug)]
pub struct MaterialPhaseConfig {
    /// 対象となるWorld内の登録済みシミュレーションlayer。
    pub layer: LayerId,
    /// 位置固定圧力channelの標準4回等のJacobi緩和。Noneなら実施しない。
    pub pressure: Option<PressureRelaxation>,
    /// cell用Q16力積分とDDA輸送。Noneなら実施しない。
    pub motion: Option<(MaterialForceConfig, MaterialMotionConfig)>,
    /// 粉・液・気の標準scalar輸送。Noneなら実施しない。
    pub scalar_candidate_limit: Option<usize>,
    /// 正規物質粒子の力積分と整数DDAの上限。Noneなら実施しない。
    pub particle: Option<(MaterialParticleForceConfig, usize)>,
}

/// 省略・適用された処理を区別する結果。既存ソルバーの診断値をそのまま公開。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MaterialPhaseReport {
    /// 圧力緩和の実行結果。
    pub pressure: Option<PressureRelaxationReport>,
    /// cell Q16力学の実行結果。
    pub motion: Option<MaterialForceMotionReport>,
    /// スカラー輸送の実行結果。
    pub scalar: Option<MaterialTransportReport>,
    /// 正規粒子力学の実行結果。
    pub particle: Option<MaterialParticleForceStepReport>,
}

impl WorldEdit<'_> {
    /// E05の標準処理を原子的に実行し、成功なら全reportを返します。
    ///
    /// 外側のWorld::edit、World::edit_material_accounted、またはRuntimeの
    /// WorldAccess::edit_material_accountedから呼びます。後段が失敗した場合は
    /// `WorldEdit::run` が失敗状態を覚え、最終World候補と台帳を破棄します。
    /// このAPI自体はRegionのIOを実行せず、ロード完了済みの入力だけを読む。
    pub fn step_material_phase(
        &mut self,
        config: MaterialPhaseConfig,
        provider: &dyn MaterialForceProvider,
    ) -> EngineResult<MaterialPhaseReport> {
        self.run(|editor| {
            if !editor.world.content.layer(config.layer)?.simulated {
                return Err(invalid("Materials phase対象はsimulated layerのみ"));
            }
            let pressure = config
                .pressure
                .map(|setting| editor.relax_pressure(config.layer, setting))
                .transpose()?;
            // cell force+DDAとscalarがある場合だけ共有予約scratchを確保。
            // pressure/粒子だけのphaseで活動chunk数に比例した余計な確保をしない。
            let mut reservations =
                if config.motion.is_some() || config.scalar_candidate_limit.is_some() {
                    Some(super::transport::CellReservations::new(editor.world)?)
                } else {
                    None
                };
            let motion = config
                .motion
                .map(|(force, motion)| {
                    if force.channel != motion.channel {
                        return Err(invalid("力積分とDDAの速度channel不一致"));
                    }
                    let force_report =
                        editor.integrate_material_forces(config.layer, force, provider)?;
                    let movement = editor.transport_material_motion_reserved(
                        config.layer,
                        motion,
                        reservations.as_mut().expect("motionには予約scratchが必要"),
                    )?;
                    Ok(MaterialForceMotionReport {
                        force: force_report,
                        motion: movement,
                    })
                })
                .transpose()?;
            let scalar = config
                .scalar_candidate_limit
                .map(|limit| {
                    editor.transport_materials_reserved(
                        config.layer,
                        limit,
                        reservations.as_mut().expect("scalarには予約scratchが必要"),
                    )
                })
                .transpose()?;
            let particle = config
                .particle
                .map(|(forces, dda_steps)| {
                    editor.step_material_particle_forces_in_layer(
                        config.layer,
                        forces,
                        provider,
                        dda_steps,
                    )
                })
                .transpose()?;
            Ok(MaterialPhaseReport {
                pressure,
                motion,
                scalar,
                particle,
            })
        })
    }
}
