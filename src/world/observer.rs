//! cameraの表示領域を正規tickの活動要求へ変換します。
//!
//! Frameの補間cameraをWorldへ直接接続せず、captureした値を次の入力tickでapplyします。
//! 保存対象は解決後のlease regionです。複数cameraは異なるsourceを使い、要求和を取ります。

use super::{ChunkCoord, ChunkRegion, LeaseMode, RegionLease, World, WorldEdit, invalid};
use crate::{
    foundation::{EngineResult, StableKey, error::allocation},
    presentation::Camera2d,
};

/// 作者が選べるcamera周辺のmargin。単位は64×64cellのchunkです。
#[derive(Clone, Copy, Debug)]
pub struct ObserverMargins {
    /// 表示範囲に加えて正規更新するmargin。
    pub simulation: u32,
    /// 読込だけを維持するmargin。simulation以上が必要です。
    pub resident: u32,
}
impl Default for ObserverMargins {
    fn default() -> Self {
        Self {
            simulation: 1,
            resident: 2,
        }
    }
}
/// 一つのtickへ割り当てる不変の観測入力。OS時刻やcameraへの参照は保持しません。
#[derive(Clone, Copy, Debug)]
pub struct CameraObservation {
    source: StableKey,
    resident: Option<ChunkRegion>,
    simulation: Option<ChunkRegion>,
}
impl CameraObservation {
    /// 現在のcameraからWorld境界へclipした要求を作ります。回転とzoomを考慮します。
    pub fn capture(
        source: StableKey,
        camera: &Camera2d,
        world: &World,
        margins: ObserverMargins,
    ) -> EngineResult<Self> {
        camera.validate()?;
        if margins.simulation > margins.resident {
            return Err(invalid("observerのresident marginがsimulation未満"));
        }
        let half = [
            f64::from(camera.viewport.width) / (2.0 * camera.zoom),
            f64::from(camera.viewport.height) / (2.0 * camera.zoom),
        ];
        let (sin, cos) = camera.rotation.sin_cos();
        let extent = [
            half[0] * cos.abs() + half[1] * sin.abs(),
            half[0] * sin.abs() + half[1] * cos.abs(),
        ];
        let min = [
            camera.position[0] - extent[0],
            camera.position[1] - extent[1],
        ];
        let max = [
            camera.position[0] + extent[0],
            camera.position[1] + extent[1],
        ];
        if min.iter().chain(max.iter()).any(|value| !value.is_finite()) {
            return Err(invalid("observerのcamera範囲overflow"));
        }
        let make = |margin: u32| -> EngineResult<Option<ChunkRegion>> {
            let bounds = world.bounds();
            let domain_min = [bounds.min().x.div_euclid(64), bounds.min().y.div_euclid(64)];
            let domain_max = [
                (bounds.max()[0] + 63).div_euclid(64),
                (bounds.max()[1] + 63).div_euclid(64),
            ];
            let margin = f64::from(margin);
            let start = [
                (min[0] / 64.0).floor() - margin,
                (min[1] / 64.0).floor() - margin,
            ];
            let end = [
                (max[0] / 64.0).ceil() + margin,
                (max[1] / 64.0).ceil() + margin,
            ];
            let min = [
                start[0].max(f64::from(domain_min[0])),
                start[1].max(f64::from(domain_min[1])),
            ];
            let max = [
                end[0].min(domain_max[0] as f64),
                end[1].min(domain_max[1] as f64),
            ];
            if min[0] >= max[0] || min[1] >= max[1] {
                return Ok(None);
            }
            Ok(Some(ChunkRegion::new(
                ChunkCoord::new(min[0] as i32, min[1] as i32)?,
                max[0] as i64,
                max[1] as i64,
            )?))
        };
        Ok(Self {
            source,
            resident: make(margins.resident)?,
            simulation: make(margins.simulation)?,
        })
    }
    /// 自動Observerを無効化する入力。次のtickで同じsourceの要求を解放します。
    pub fn disabled(source: StableKey) -> Self {
        Self {
            source,
            resident: None,
            simulation: None,
        }
    }
    /// 先に準備する必要があるchunkを列挙します。保存済み区域と未生成区域はWorldで照会できます。
    pub fn missing_chunks(&self, world: &World, limit: usize) -> EngineResult<Vec<ChunkCoord>> {
        let mut missing = Vec::new();
        if let Some(region) = self.resident {
            for coord in region.coordinates(limit)? {
                if !world.chunks.contains_key(&coord) {
                    missing.try_reserve(1).map_err(allocation)?;
                    missing.push(coord);
                }
            }
        }
        Ok(missing)
    }
    /// 物理更新の要求範囲。画面内判定とは別の値です。
    pub fn simulation_region(&self) -> Option<ChunkRegion> {
        self.simulation
    }
    /// 読込保持の要求範囲。
    pub fn resident_region(&self) -> Option<ChunkRegion> {
        self.resident
    }
}
impl WorldEdit<'_> {
    /// 同じsourceが管理する要求を置換します。未準備・容量不足では旧要求を維持します。
    pub fn apply_camera_observation(&mut self, observation: CameraObservation) -> EngineResult<()> {
        self.run(|editor| {
            let mut old = Vec::new();
            old.try_reserve_exact(editor.world.leases.len())
                .map_err(allocation)?;
            for (id, lease) in editor.world.leases.iter() {
                if lease.source == observation.source {
                    old.push(id);
                }
            }
            for id in old {
                editor.world.leases.remove(id)?;
            }
            for (region, mode) in [
                (observation.resident, LeaseMode::LoadOnly),
                (observation.simulation, LeaseMode::Simulate),
            ] {
                if let Some(region) = region {
                    editor.world.leases.insert(RegionLease {
                        source: observation.source,
                        region,
                        mode,
                        expires: None,
                    })?;
                }
            }
            editor.world.resolve_activity()
        })
    }
}
