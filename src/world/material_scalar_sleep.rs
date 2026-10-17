//! 標準の粉体・液体・気体scalar輸送に対する保存可能な静止証明。
//!
//! 両tick parityを保守的に満たす「一つも交換候補がない」状態だけをsleepし、
//! 作者ruleや別phaseの進行を勝手に省略しません。

use super::{
    CellQuery, ChunkCoord, LayerId, MaterialBase, MaterialId, MaterialTransportReport, World,
    invalid,
};
use crate::foundation::{
    EngineError, EngineResult, StableKey,
    codec::{Decoder, Encoder, StateCodec},
    error::allocation,
    grid::CellPoint,
    id::WorldId,
};

/// 標準scalarの静止証明。正規Worldやchannelの所有物にはなりません。
#[derive(Clone, Debug)]
struct ScalarSleepProof {
    world: WorldId,
    content: [u8; 32],
    epoch: u64,
    revision: u64,
    layer: StableKey,
    candidate_limit: usize,
    candidates: usize,
    active: Vec<(ChunkCoord, u64)>,
}

/// 標準scalar専用の派生sleep/wakeトラッカー。
///
/// 新たなWorldへrestoreした証明は未認証として扱い、必ず参照stepを実行して
/// 地形・密度・活性域を再検証します。独自Systemからもwakeを明示できます。
#[derive(Clone, Debug, Default)]
pub struct ScalarMaterialSleepTracker {
    proof: Option<ScalarSleepProof>,
    verified: bool,
}

/// 標準scalar輸送と同じ変更・候補数、および実際のsleep有無。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScalarMaterialSleepReport {
    /// 省略しなかった場合の参照輸送結果と同一。
    pub reference: MaterialTransportReport,
    /// 静止証明によって全cell走査を省略できたか。
    pub sleeping: bool,
    /// 直前の証明が外部変更または設定変更によって失効したか。
    pub woke: bool,
}

impl ScalarMaterialSleepTracker {
    /// 外部の作者ruleや未申告の依存関係から手動で起こす場合にも使用します。
    pub fn wake(&mut self) {
        self.proof = None;
        self.verified = false;
    }

    /// 以前の評価で証明された候補があるか。復元後の未認証状態も含みます。
    pub fn has_proof(&self) -> bool {
        self.proof.is_some()
    }

    fn matching(&self, world: &World, layer: LayerId, limit: usize) -> bool {
        if !self.verified {
            return false;
        }
        let Some(proof) = &self.proof else {
            return false;
        };
        if proof.world != world.id
            || proof.content != world.content.fingerprint()
            || proof.epoch != world.remaps.epoch()
            || proof.candidate_limit != limit
            || world.content.layer(layer).is_err()
            || world
                .content
                .layer(layer)
                .is_ok_and(|definition| definition.key != proof.layer)
        {
            return false;
        }
        match world.changes_after(proof.revision) {
            super::ChangeQuery::Current => {}
            super::ChangeQuery::Delta(delta) if delta.chunks.is_empty() => {}
            _ => return false,
        }
        let mut current = world.active_chunks();
        for &(coord, revision) in &proof.active {
            if current.next() != Some(coord)
                || !world
                    .chunks
                    .get(&coord)
                    .is_some_and(|chunk| chunk.active && chunk.revision == revision)
            {
                return false;
            }
        }
        current.next().is_none()
    }

    /// 静止の証明は走査順と無関係な全候補の局所交換可能性を検査します。
    ///
    /// 候補が一つでも移動できる場合はどちらのparityで最後に動くかを
    /// 推測せず参照輸送へ戻すため、sleepしたtickの並び替えが結果を変えません。
    fn prove(
        world: &World,
        layer: LayerId,
        limit: usize,
        report: MaterialTransportReport,
    ) -> EngineResult<Option<ScalarSleepProof>> {
        if report.exchanges != 0 || !world.content.layer(layer)?.simulated {
            return Ok(None);
        }
        let mut active = Vec::new();
        active
            .try_reserve_exact(world.active_chunks().count())
            .map_err(allocation)?;
        let mut candidates = 0usize;
        for (coord, chunk) in &world.chunks {
            if !chunk.active {
                continue;
            }
            active.push((*coord, chunk.revision));
            let origin = coord.origin()?;
            let page = &chunk.layers[layer.0 as usize];
            for (index, cell) in page.cells.iter().enumerate() {
                if cell.material == MaterialId::AIR
                    || page
                        .blocks
                        .as_ref()
                        .is_some_and(|blocks| blocks[index].is_some())
                {
                    continue;
                }
                candidates += 1;
                if candidates > limit {
                    return Ok(None);
                }
                let material = world
                    .content
                    .material(cell.material)?
                    .ok_or_else(|| invalid("scalar sleepの未定義素材"))?;
                let position = CellPoint {
                    x: origin.x + (index % 64) as i32,
                    y: origin.y + (index / 64) as i32,
                };
                let choices: &[(i32, i32)] = match material.base() {
                    MaterialBase::Solid => &[],
                    MaterialBase::Powder => &[(0, 1), (-1, 1), (1, 1)],
                    MaterialBase::Liquid => &[(0, 1), (-1, 0), (1, 0)],
                    MaterialBase::Gas => &[(0, -1), (-1, -1), (1, -1), (-1, 0), (1, 0)],
                };
                for &(dx, dy) in choices {
                    let Some(target) = add(position, dx, dy) else {
                        continue;
                    };
                    let Some(dst) = resident_active(world, layer, target)? else {
                        continue;
                    };
                    if dst.block.is_some() {
                        continue;
                    }
                    let can_swap = if dst.core.material == MaterialId::AIR {
                        true
                    } else {
                        let other = world
                            .content
                            .material(dst.core.material)?
                            .ok_or_else(|| invalid("scalar sleepの未定義交換先"))?;
                        matches!(other.base(), MaterialBase::Liquid | MaterialBase::Gas)
                            && dst.core.material != cell.material
                            && ((dy > 0 && material.mass_per_cell() > other.mass_per_cell())
                                || (dy < 0 && material.mass_per_cell() < other.mass_per_cell()))
                    };
                    if !can_swap {
                        continue;
                    }
                    if dx != 0
                        && dy != 0
                        && !free_corner(world, layer, add(position, dx, 0))?
                        && !free_corner(world, layer, add(position, 0, dy))?
                    {
                        continue;
                    }
                    // 一候補でも交換できればこの活動集合はまだ休止できません。
                    return Ok(None);
                }
            }
        }
        if candidates != report.candidates {
            return Ok(None);
        }
        Ok(Some(ScalarSleepProof {
            world: world.id,
            content: world.content.fingerprint(),
            epoch: world.remaps.epoch(),
            revision: world.revision(),
            layer: world.content.layer(layer)?.key,
            candidate_limit: limit,
            candidates,
            active,
        }))
    }
}

fn add(point: CellPoint, dx: i32, dy: i32) -> Option<CellPoint> {
    Some(CellPoint {
        x: point.x.checked_add(dx)?,
        y: point.y.checked_add(dy)?,
    })
}

fn resident_active(
    world: &World,
    layer: LayerId,
    point: CellPoint,
) -> EngineResult<Option<super::CellView>> {
    if !world
        .chunks
        .get(&ChunkCoord::at(point))
        .is_some_and(|chunk| chunk.active)
    {
        return Ok(None);
    }
    match world.cell(layer, point)? {
        CellQuery::Loaded(value) => Ok(Some(value)),
        _ => Ok(None),
    }
}

fn free_corner(world: &World, layer: LayerId, point: Option<CellPoint>) -> EngineResult<bool> {
    let Some(point) = point else {
        return Ok(false);
    };
    Ok(matches!(
        resident_active(world, layer, point)?,
        Some(value) if value.block.is_none() && value.core.material == MaterialId::AIR
    ))
}

impl World {
    /// 通常scalarで両側を調べても交換できないと証明されたtickだけ休止します。
    ///
    /// 全active chunkのrevision、World/Content、remap、対象layerと上限を
    /// 保守的に照合し、失効時は何も推測せず参照solverを一度実行します。
    /// 睡眠時も通常World::edit一回分の公開revisionを進めます。
    pub fn step_scalar_materials_sleeping(
        &mut self,
        tracker: &mut ScalarMaterialSleepTracker,
        layer: LayerId,
        candidate_limit: usize,
    ) -> EngineResult<ScalarMaterialSleepReport> {
        let previously_asleep = tracker.has_proof();
        if tracker.matching(self, layer, candidate_limit) {
            let candidates = tracker.proof.as_ref().expect("検証済み").candidates;
            self.edit(|_| Ok(()))?;
            tracker.proof.as_mut().expect("検証済み").revision = self.revision();
            return Ok(ScalarMaterialSleepReport {
                reference: MaterialTransportReport {
                    candidates,
                    exchanges: 0,
                },
                sleeping: true,
                woke: false,
            });
        }
        tracker.wake();
        let reference = self.edit(|edit| edit.transport_materials(layer, candidate_limit))?;
        tracker.proof = ScalarMaterialSleepTracker::prove(self, layer, candidate_limit, reference)
            .ok()
            .flatten();
        tracker.verified = tracker.proof.is_some();
        Ok(ScalarMaterialSleepReport {
            reference,
            sleeping: false,
            woke: previously_asleep,
        })
    }
}

impl StateCodec for ScalarMaterialSleepTracker {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.raw(b"CHSLSCR1")?;
        let Some(p) = &self.proof else {
            return out.u8(0);
        };
        out.u8(1)?;
        p.world.encode(out)?;
        out.raw(&p.content)?;
        out.u64(p.epoch)?;
        out.u64(p.revision)?;
        p.layer.encode(out)?;
        out.u64(u64::try_from(p.candidate_limit).map_err(|_| invalid("scalar sleep候補容量"))?)?;
        out.u64(u64::try_from(p.candidates).map_err(|_| invalid("scalar sleep候補数"))?)?;
        out.u32(u32::try_from(p.active.len()).map_err(|_| invalid("scalar sleep活動数"))?)?;
        for &(coord, revision) in &p.active {
            coord.encode(out)?;
            out.u64(revision)?;
        }
        Ok(())
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        if input.raw(8)? != b"CHSLSCR1" {
            return Err(invalid("scalar sleep保存形式"));
        }
        match input.u8()? {
            0 => return Ok(Self::default()),
            1 => {}
            _ => return Err(invalid("scalar sleep存在bit")),
        }
        let world = WorldId::decode(input)?;
        let content = input
            .raw(32)?
            .try_into()
            .map_err(|_| invalid("scalar sleep Content"))?;
        let epoch = input.u64()?;
        let revision = input.u64()?;
        let layer = StableKey::decode(input)?;
        let candidate_limit =
            usize::try_from(input.u64()?).map_err(|_| invalid("scalar sleep容量"))?;
        let candidates =
            usize::try_from(input.u64()?).map_err(|_| invalid("scalar sleep候補数"))?;
        let count = input.u32()? as usize;
        if candidates > candidate_limit || count > input.remaining() / 16 {
            return Err(EngineError::capacity(
                "scalar sleep保存レコード",
                count as u64,
                input.remaining() as u64 / 16,
            ));
        }
        let mut active = Vec::new();
        active.try_reserve_exact(count).map_err(allocation)?;
        for _ in 0..count {
            active.push((ChunkCoord::decode(input)?, input.u64()?));
        }
        if active.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
            return Err(invalid("scalar sleep活動chunk保存順"));
        }
        // 復元したcacheは未検証。world正規状態の走査後だけ再びsleepできる。
        Ok(Self {
            proof: Some(ScalarSleepProof {
                world,
                content,
                epoch,
                revision,
                layer,
                candidate_limit,
                candidates,
                active,
            }),
            verified: false,
        })
    }
}
