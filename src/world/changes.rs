//! 派生cacheの更新通知。通知を取り逃した場合は正確なqueryで再構築します。
//!
//! 通知自体を保存の正やgameplay判定へ使いません。復元後はFullRefreshRequiredを返し、
//! collision/navigation/light等が古いcacheを現行revisionだと誤認することを防ぎます。

use super::{CellQuery, ChunkCoord, LayerId, World};
use crate::foundation::{EngineResult, error::allocation, grid::CellPoint};
use std::collections::BTreeSet;

/// 一回のWorld公開で影響したchunk。複数layerのcacheは自身のqueryで詳細を確認できます。
pub struct WorldChanges {
    /// 公開前のWorld revision。
    pub from: u64,
    /// 公開後のWorld revision。
    pub to: u64,
    /// 読込・退避・地形・活動状態が変わったchunkの安定順集合。
    pub chunks: Vec<ChunkCoord>,
}
/// 消費済みrevisionに対する通知のcoverage。
pub enum ChangeQuery<'a> {
    /// 既に現行revisionを使用しています。
    Current,
    /// この差分を適用して現行revisionへ進めます。
    Delta(&'a WorldChanges),
    /// 複数公開を取り逃した、復元後、または別のrevisionです。全体の再照合が必要です。
    FullRefreshRequired,
}
impl WorldChanges {
    pub(crate) fn empty() -> Self {
        Self {
            from: 0,
            to: 0,
            chunks: Vec::new(),
        }
    }
    pub(crate) fn try_copy(&self) -> EngineResult<Self> {
        Ok(Self {
            from: self.from,
            to: self.to,
            chunks: super::terrain::copy_slice(&self.chunks)?,
        })
    }
    pub(crate) fn between(old: &World, new: &World) -> EngineResult<Self> {
        let coords: BTreeSet<_> = old
            .chunks
            .keys()
            .chain(new.chunks.keys())
            .copied()
            .collect();
        let mut chunks = Vec::new();
        chunks.try_reserve_exact(coords.len()).map_err(allocation)?;
        for coord in coords {
            let changed = match (old.chunks.get(&coord), new.chunks.get(&coord)) {
                (Some(a), Some(b)) => {
                    a.revision != b.revision
                        || a.active != b.active
                        || a.epoch != b.epoch
                        || a.layers
                            .iter()
                            .zip(&b.layers)
                            .any(|(a, b)| !std::sync::Arc::ptr_eq(a, b))
                }
                _ => true,
            };
            if changed {
                chunks.push(coord);
            }
        }
        Ok(Self {
            from: old.revision,
            to: new.revision,
            chunks,
        })
    }
}
impl World {
    /// collision、light、navigation、描画等が共有する公開revision通知。
    pub fn changes_after(&self, revision: u64) -> ChangeQuery<'_> {
        if revision == self.revision {
            return ChangeQuery::Current;
        }
        if revision == self.changes.from && self.changes.to == self.revision {
            return ChangeQuery::Delta(&self.changes);
        }
        ChangeQuery::FullRefreshRequired
    }
    /// 接続規則・見た目・支持確認等のため、上・右上・右・右下・下・左下・左・左上を返します。
    /// 未読込とWorld境界を保持し、端の座標をoverflowで反対側へ巻き戻しません。
    pub fn neighbors(&self, layer: LayerId, point: CellPoint) -> EngineResult<[CellQuery; 8]> {
        self.content.layer(layer)?;
        let mut neighbors = [CellQuery::WorldBoundary; 8];
        for (index, (dx, dy)) in [
            (0, -1),
            (1, -1),
            (1, 0),
            (1, 1),
            (0, 1),
            (-1, 1),
            (-1, 0),
            (-1, -1),
        ]
        .into_iter()
        .enumerate()
        {
            if let (Some(x), Some(y)) = (point.x.checked_add(dx), point.y.checked_add(dy)) {
                neighbors[index] = self.cell(layer, CellPoint { x, y })?;
            }
        }
        Ok(neighbors)
    }
}
