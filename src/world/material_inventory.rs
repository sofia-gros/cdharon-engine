//! 読込済みcellと常駐正規粒子の素材別token・質量の診断集計。
//!
//! 退避区域のpayloadはWorldの外部保存先が所有します。参照できない物質を
//! ゼロとして扱うと収支が偽装されるため、外部保存が存在することを明示します。
//! 保存した全表現の監査とreceiptの突合はE05の後続の統合処理が担当します。

use super::{MaterialId, World, invalid};
use crate::foundation::{EngineResult, StableKey, error::allocation};

/// 現在のresident領域の一素材について、cellと粒子が所有する量です。
/// massはContentへ登録した1cellあたりのゲーム質量で換算します。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResidentMaterialAmount {
    /// Contentで確定した素材の安定キー。
    pub material: StableKey,
    /// 全layerのresident cell内にあるcarrier token数。block個体の占有cellも含みます。
    pub cell_tokens: u64,
    /// resident正規粒子が所有するtoken数。1粒子につき1tokenです。
    pub particle_tokens: u64,
    /// cell_tokensとparticle_tokensの合計。
    pub total_tokens: u64,
    /// total_tokensとContentのmass_per_cellの積による名目mass。混合成分の実質量ではありません。
    pub mass: u128,
}

/// 読込済みWorldにある物質量の診断結果。素材は安定キー順で、ゼロ個も列挙します。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResidentMaterialInventory {
    /// 計数した素材別の量。複数layerの同一素材は合算します。
    pub materials: Vec<ResidentMaterialAmount>,
    /// 集計外のarchive chunk数。0以外ならWorld全体の総量ではありません。
    pub archived_chunks: usize,
    /// payloadが退避されている正規粒子の数。常駐粒子へ重複計上しません。
    pub archived_particles: usize,
}

impl World {
    /// 読込済みの地形と正規物質粒子を走査し、素材別token・massを返します。
    ///
    /// inactiveなresident chunkは含み、生成baseは重複計上しません。格子の
    /// 値や混合組成を追加の物質tokenとして数えません。退避chunk・退避粒子は
    /// 外部archiveの読込が必要なため、件数のみを結果へ残します。
    ///
    /// 混合cellはcarrier素材として計上し、成分別の分数数量は含みません。
    /// 診断目的の全resident cell走査です。tickごとのhot pathでは利用せず、
    /// World全体の収支確定にはarchiveと将来のbody/reservoirを合わせてください。
    pub fn resident_material_inventory(&self) -> EngineResult<ResidentMaterialInventory> {
        let mut materials = Vec::new();
        materials
            .try_reserve_exact(self.content.materials.len())
            .map_err(allocation)?;
        for (_, definition) in self.content.materials() {
            materials.push(ResidentMaterialAmount {
                material: definition.key(),
                cell_tokens: 0,
                particle_tokens: 0,
                total_tokens: 0,
                mass: 0,
            });
        }
        for chunk in self.chunks.values() {
            for page in &chunk.layers {
                for cell in &page.cells {
                    increase(&mut materials, cell.material, false)?;
                }
            }
        }
        let mut archived_particles = 0usize;
        for (_, particle) in self.particles.iter() {
            if particle.is_archived() {
                archived_particles = archived_particles
                    .checked_add(1)
                    .ok_or_else(|| invalid("退避粒子数overflow"))?;
            } else {
                increase(&mut materials, particle.core().material, true)?;
            }
        }
        for (record, (_, definition)) in materials.iter_mut().zip(self.content.materials()) {
            record.total_tokens = record
                .cell_tokens
                .checked_add(record.particle_tokens)
                .ok_or_else(|| invalid("素材token合計overflow"))?;
            record.mass = u128::from(record.total_tokens) * u128::from(definition.mass_per_cell());
        }
        Ok(ResidentMaterialInventory {
            materials,
            archived_chunks: self.archived.len(),
            archived_particles,
        })
    }
}


/// 保存した全区域を含む、素材ごとの計数結果。

/// 物質として所有するcarrier tokenを数えます。混合の有理成分量、itemや
/// 後工程のbody/reservoirは別の台帳であり、この型だけではQ04の完全監査になりません。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorldMaterialAmount {
    /// Contentの安定素材キー。
    pub material: StableKey,
    /// 読込済み地形が所有するtoken。
    pub resident_cell_tokens: u64,
    /// 常駐粒子が所有するtoken。
    pub resident_particle_tokens: u64,
    /// 永続archive内の地形が所有するtoken。
    pub archived_cell_tokens: u64,
    /// 永続archive内の物質粒子が所有するtoken。
    pub archived_particle_tokens: u64,
    /// 全四表現の合計token。epoch remap後の素材で集計します。
    pub total_tokens: u64,
    /// Content mass_per_cellによる名目mass。混合成分の保存証明ではありません。
    pub nominal_mass: u128,
}

/// 外部archiveを実際に読み、checksumと粒子metadataを検証した診断結果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldMaterialInventory {
    /// 素材キー順。空素材を除き、未配置素材もゼロで列挙します。
    pub materials: Vec<WorldMaterialAmount>,
    /// 検証して集計したarchive region group数。
    pub archived_groups: usize,
}

/// 空cellはtokenを持ちません。不正な素材IDは暗黙に空へ変換せず失敗します。
fn increase(
    materials: &mut [ResidentMaterialAmount],
    material: MaterialId,
    particle: bool,
) -> EngineResult<()> {
    if material == MaterialId::AIR {
        return Ok(());
    }
    let index = usize::try_from(material.index() - 1).map_err(|_| invalid("素材IDが範囲外"))?;
    let amount = materials
        .get_mut(index)
        .ok_or_else(|| invalid("集計対象の素材IDが不正"))?;
    let count = if particle {
        &mut amount.particle_tokens
    } else {
        &mut amount.cell_tokens
    };
    *count = count
        .checked_add(1)
        .ok_or_else(|| invalid("素材token数overflow"))?;
    Ok(())
}
