//! 素材が選択する配置条件と作者のvalidator。
//!
//! 支持条件は任意です。エンジンが全素材へ特定ゲームの建築規則を強制しません。
//! validatorは不変Worldを読み、未知regionを空とみなさず、配置前にエラーを返します。

use super::{CellQuery, ContentRegistry, LayerId, MaterialDef, Orientation, World, invalid};
use crate::foundation::{
    EngineError, EngineResult, ErrorCode, StableKey,
    codec::{Encoder, StateCodec},
};

/// 下の支持cellを一つでも必要とするか、全幅で必要とするか。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupportMode {
    /// 底辺に一つ以上の固体が必要。
    Any,
    /// 底辺の全cellが固体である必要。
    All,
}
/// 素材ごとの任意の配置条件。
#[derive(Clone, Debug)]
pub enum AnchorRule {
    /// 支持等を要求しない。
    Free,
    /// 回転後footprintの直下を、指定layerの固体で支持する。
    Below {
        /// 支持判定のlayer。配置先と異なるlayerも指定できます。
        layer: StableKey,
        /// 必要な支持の範囲。
        mode: SupportMode,
    },
    /// 作者の条件。設定codecと実行validatorを同じproviderへ登録します。
    Custom {
        /// 条件実装の安定キー。
        provider: StableKey,
        /// 条件とrecordの版。
        version: u32,
        /// 登録済みcodecが検証する設定。
        data: Vec<u8>,
    },
}
/// 作者の条件が読む不変の配置入力。候補を書き換える権限は含みません。
pub struct PlacementContext<'a> {
    /// 直前までの候補地形。公開前の同一取引の変更も反映されています。
    pub world: &'a World,
    /// 配置先layer。
    pub layer: LayerId,
    /// 配置先anchor。
    pub anchor: crate::foundation::grid::CellPoint,
    /// 配置向き。
    pub orientation: Orientation,
    /// 解決済み素材。
    pub material: &'a MaterialDef,
}
type Validate = fn(&PlacementContext<'_>, &[u8]) -> EngineResult<()>;
pub(crate) struct AnchorProvider {
    pub(crate) key: StableKey,
    pub(crate) version: u32,
    schema: crate::foundation::schema::Schema,
    validate: Validate,
}
impl ContentRegistry {
    /// 独自条件の設定codecと実行処理を登録します。未登録の条件を初回配置まで持ち越しません。
    pub fn anchor_provider(
        &mut self,
        key: StableKey,
        version: u32,
        schema: crate::foundation::schema::Schema,
        validate: Validate,
    ) -> EngineResult<()> {
        if version == 0 {
            return Err(invalid("配置providerの版"));
        }
        if self.anchors.iter().any(|old| old.key == key) {
            return Err(EngineError::new(
                ErrorCode::DuplicateKey,
                "配置providerキー",
            ));
        }
        self.anchors
            .try_reserve(1)
            .map_err(crate::foundation::error::allocation)?;
        self.anchors.push(AnchorProvider {
            key,
            version,
            schema,
            validate,
        });
        Ok(())
    }
}
impl AnchorRule {
    pub(crate) fn validate_definition(&self, registry: &ContentRegistry) -> EngineResult<()> {
        match self {
            Self::Free => Ok(()),
            Self::Below { layer, .. } => {
                if !registry.has_layer(*layer) {
                    return Err(EngineError::new(
                        ErrorCode::MissingDependency,
                        "配置支持のlayer",
                    ));
                }
                Ok(())
            }
            Self::Custom {
                provider,
                version,
                data,
            } => {
                let definition = registry
                    .anchors
                    .iter()
                    .find(|value| value.key == *provider && value.version == *version)
                    .ok_or_else(|| {
                        EngineError::new(ErrorCode::MissingDependency, "配置providerの実装・版")
                    })?;
                definition.schema.validate(data)
            }
        }
    }
    pub(crate) fn encode_definition(&self, out: &mut Encoder) -> EngineResult<()> {
        match self {
            Self::Free => out.u8(0),
            Self::Below { layer, mode } => {
                out.u8(1)?;
                layer.encode(out)?;
                out.u8(if *mode == SupportMode::Any { 0 } else { 1 })
            }
            Self::Custom {
                provider,
                version,
                data,
            } => {
                out.u8(2)?;
                provider.encode(out)?;
                out.u32(*version)?;
                out.bytes(data)
            }
        }
    }
}
impl World {
    pub(crate) fn validate_placement(&self, context: &PlacementContext<'_>) -> EngineResult<()> {
        match context.material.anchor_rule() {
            AnchorRule::Free => Ok(()),
            AnchorRule::Custom {
                provider,
                version,
                data,
            } => {
                let definition = self
                    .content
                    .anchors
                    .iter()
                    .find(|value| value.key == *provider && value.version == *version)
                    .ok_or_else(|| {
                        EngineError::new(ErrorCode::MissingDependency, "配置providerの実装・版")
                    })?;
                (definition.validate)(context, data)
            }
            AnchorRule::Below { layer, mode } => {
                let layer = self.content.layer_id(*layer)?;
                let size = context.material.block_size();
                let [width, height] = match context.orientation {
                    Orientation::Original | Orientation::Down => size,
                    Orientation::Right | Orientation::Left => [size[1], size[0]],
                };
                let y = i32::try_from(i64::from(context.anchor.y) + i64::from(height))
                    .map_err(|_| invalid("支持判定のy境界"))?;
                let mut supported = 0usize;
                for x in 0..width {
                    let x = i32::try_from(i64::from(context.anchor.x) + i64::from(x))
                        .map_err(|_| invalid("支持判定のx境界"))?;
                    match self.cell(layer, crate::foundation::grid::CellPoint { x, y })? {
                        CellQuery::Loaded(value) => {
                            if self
                                .content
                                .material(value.core.material)?
                                .is_some_and(|value| value.base() == super::MaterialBase::Solid)
                            {
                                supported += 1;
                            }
                        }
                        CellQuery::Unavailable(_) => {
                            return Err(EngineError::new(
                                ErrorCode::InvalidState,
                                "支持判定の区域が未読込",
                            ));
                        }
                        CellQuery::WorldBoundary => (),
                    }
                }
                if supported == 0 || (*mode == SupportMode::All && supported != usize::from(width))
                {
                    return Err(EngineError::new(
                        ErrorCode::Conflict,
                        "素材の支持条件を満たさない",
                    ));
                }
                Ok(())
            }
        }
    }
}
