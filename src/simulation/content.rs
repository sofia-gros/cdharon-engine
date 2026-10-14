//! Bounded TOML material authoring schema, normalized into frozen runtime definitions.

use serde::Deserialize;

use crate::simulation::authoring::{
    Flammability, MaterialAttachment, MaterialAuthoring, MaterialProperty, PropertyValue,
};
use crate::simulation::material::{
    Behavior, MaterialBuilder, MaterialDef, MaterialRegistry, MaterialRegistryError,
};

pub const MATERIAL_SCHEMA_VERSION: u16 = 1;
pub const MAX_MATERIAL_DOCUMENT_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContentError {
    TooLarge,
    Parse(String),
    UnsupportedSchema(u16),
    InvalidField(&'static str),
    InvalidProperty(&'static str),
    Registry(MaterialRegistryError),
}
impl std::fmt::Display for ContentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "material content error: {self:?}")
    }
}
impl std::error::Error for ContentError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDocument {
    schema_version: u16,
    name: String,
    behavior: String,
    mass_per_cell: u32,
    color: [u8; 4],
    #[serde(default)]
    dispersion: u8,
    #[serde(default)]
    max_fall_speed: u8,
    #[serde(default)]
    flow_speed: u8,
    #[serde(default)]
    can_be_washed_away: bool,
    #[serde(default)]
    properties: Vec<RawProperty>,
    #[serde(default)]
    attachments: Vec<RawAttachment>,
    flammability: Option<RawFlammability>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProperty {
    key: String,
    kind: String,
    boolean: Option<bool>,
    integer: Option<i64>,
    fixed_q16: Option<i64>,
    string: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAttachment {
    kind: String,
    version: u16,
    #[serde(default)]
    values: Vec<RawProperty>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFlammability {
    ignition_energy: u32,
    fuel_ticks: u16,
    heat_per_tick: u16,
    exhaust_material: Option<String>,
}

pub fn parse_material_toml(source: &str) -> Result<MaterialDef, ContentError> {
    if source.len() > MAX_MATERIAL_DOCUMENT_BYTES {
        return Err(ContentError::TooLarge);
    }
    let raw: RawDocument =
        toml::from_str(source).map_err(|error| ContentError::Parse(error.to_string()))?;
    if raw.schema_version != MATERIAL_SCHEMA_VERSION {
        return Err(ContentError::UnsupportedSchema(raw.schema_version));
    }
    let behavior = match raw.behavior.as_str() {
        "solid" => Behavior::Solid,
        "powder" => Behavior::Powder,
        "liquid" => Behavior::Liquid,
        "empty" => Behavior::Empty,
        _ => return Err(ContentError::InvalidField("behavior")),
    };
    let mut authoring = MaterialAuthoring::default();
    for property in raw.properties {
        authoring.properties.push(parse_property(property)?);
    }
    for attachment in raw.attachments {
        let mut values = Vec::new();
        values
            .try_reserve_exact(attachment.values.len())
            .map_err(|_| ContentError::InvalidField("allocation"))?;
        for value in attachment.values {
            values.push(parse_property(value)?);
        }
        authoring.attachments.push(MaterialAttachment {
            kind: attachment.kind,
            version: attachment.version,
            values,
        });
    }
    authoring.flammability = raw.flammability.map(|value| Flammability {
        ignition_energy: value.ignition_energy,
        fuel_ticks: value.fuel_ticks,
        heat_per_tick: value.heat_per_tick,
        exhaust_material: value.exhaust_material,
    });
    authoring.validate().map_err(ContentError::InvalidField)?;
    let mut builder = MaterialBuilder::new(raw.name, raw.color)
        .behavior(behavior)
        .mass_per_cell(raw.mass_per_cell)
        .dispersion(raw.dispersion)
        .max_fall_speed(raw.max_fall_speed)
        .flow_speed(raw.flow_speed)
        .can_be_washed_away(raw.can_be_washed_away);
    for property in authoring.properties.iter().cloned() {
        builder = builder.property(property.key, property.value);
    }
    for attachment in authoring.attachments.iter().cloned() {
        builder = builder.attach(attachment);
    }
    if let Some(flammability) = authoring.flammability {
        builder = builder.flammability(flammability);
    }
    let definition = builder.build();
    // Run runtime registration validation on a candidate registry before returning user data.
    let mut validation = MaterialRegistry::with_default_materials_builder();
    validation
        .register(definition.clone())
        .map_err(ContentError::Registry)?;
    Ok(definition)
}

pub fn register_material_toml(
    registry: &mut MaterialRegistry,
    source: &str,
) -> Result<u16, ContentError> {
    register_material_pack(registry, [source])?
        .into_iter()
        .next()
        .ok_or(ContentError::InvalidField("empty pack"))
}

/// Register a complete content pack in a candidate registry; partial packs are never published.
pub fn register_material_pack<'a>(
    registry: &mut MaterialRegistry,
    sources: impl IntoIterator<Item = &'a str>,
) -> Result<Vec<u16>, ContentError> {
    let mut definitions = Vec::new();
    for source in sources {
        if definitions.len() >= 65_534 {
            return Err(ContentError::InvalidField("pack material limit"));
        }
        definitions
            .try_reserve(1)
            .map_err(|_| ContentError::InvalidField("allocation"))?;
        definitions.push(parse_material_toml(source)?);
    }
    definitions.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    let mut candidate = registry.clone();
    let mut ids = Vec::new();
    ids.try_reserve_exact(definitions.len())
        .map_err(|_| ContentError::InvalidField("allocation"))?;
    for definition in definitions {
        ids.push(
            candidate
                .register(definition)
                .map_err(ContentError::Registry)?,
        );
    }
    *registry = candidate;
    Ok(ids)
}

fn parse_property(raw: RawProperty) -> Result<MaterialProperty, ContentError> {
    let count = usize::from(raw.boolean.is_some())
        + usize::from(raw.integer.is_some())
        + usize::from(raw.fixed_q16.is_some())
        + usize::from(raw.string.is_some());
    if count != 1 {
        return Err(ContentError::InvalidProperty(
            "exactly one value field is required",
        ));
    }
    let value = match raw.kind.as_str() {
        "bool"
            if raw.boolean.is_some()
                && raw.integer.is_none()
                && raw.fixed_q16.is_none()
                && raw.string.is_none() =>
        {
            PropertyValue::Bool(raw.boolean.unwrap_or(false))
        }
        "integer"
            if raw.integer.is_some()
                && raw.boolean.is_none()
                && raw.fixed_q16.is_none()
                && raw.string.is_none() =>
        {
            PropertyValue::Integer(raw.integer.unwrap_or_default())
        }
        "fixed_q16"
            if raw.fixed_q16.is_some()
                && raw.boolean.is_none()
                && raw.integer.is_none()
                && raw.string.is_none() =>
        {
            PropertyValue::FixedQ16(raw.fixed_q16.unwrap_or_default())
        }
        "enum"
            if raw.string.is_some()
                && raw.boolean.is_none()
                && raw.integer.is_none()
                && raw.fixed_q16.is_none() =>
        {
            PropertyValue::Enum(raw.string.unwrap_or_default())
        }
        "reference"
            if raw.string.is_some()
                && raw.boolean.is_none()
                && raw.integer.is_none()
                && raw.fixed_q16.is_none() =>
        {
            PropertyValue::Reference(raw.string.unwrap_or_default())
        }
        _ => return Err(ContentError::InvalidProperty("kind/value mismatch")),
    };
    Ok(MaterialProperty {
        key: raw.key,
        value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"
schema_version = 1
name = "sample:light_wood"
behavior = "solid"
mass_per_cell = 450
color = [130, 82, 42, 255]

[[properties]]
key = "sample:buoyant"
kind = "bool"
boolean = true

[[properties]]
key = "sample:hardness"
kind = "fixed_q16"
fixed_q16 = 8192

[flammability]
ignition_energy = 700
fuel_ticks = 240
heat_per_tick = 6
exhaust_material = "core:air"
"#;

    #[test]
    fn toml_defines_mass_flammability_and_typed_properties() {
        let material = parse_material_toml(VALID).unwrap();
        assert_eq!(material.mass_per_cell, 450);
        assert_eq!(material.authoring.properties.len(), 2);
        assert_eq!(
            material.authoring.flammability.as_ref().unwrap().fuel_ticks,
            240
        );
        let mut registry = MaterialRegistry::with_default_materials_builder();
        register_material_toml(&mut registry, VALID).unwrap();
        assert!(registry.try_freeze().is_ok());
        let sample =
            parse_material_toml(include_str!("../../content/examples/light_wood.toml")).unwrap();
        assert_eq!(sample.name, "sample:light_wood");
        assert_eq!(sample.authoring.attachments.len(), 1);
    }

    #[test]
    fn malformed_unknown_and_oversized_content_is_rejected() {
        assert!(matches!(
            parse_material_toml("schema_version=1\nunknown=2"),
            Err(ContentError::Parse(_))
        ));
        assert!(matches!(
            parse_material_toml(&"x".repeat(MAX_MATERIAL_DOCUMENT_BYTES + 1)),
            Err(ContentError::TooLarge)
        ));
        let invalid = VALID.replace(
            "kind = \"bool\"\nboolean = true",
            "kind = \"bool\"\ninteger = 2",
        );
        assert!(matches!(
            parse_material_toml(&invalid),
            Err(ContentError::InvalidProperty(_))
        ));
        let bad_version = VALID.replace("schema_version = 1", "schema_version = 2");
        assert!(matches!(
            parse_material_toml(&bad_version),
            Err(ContentError::UnsupportedSchema(2))
        ));
    }

    #[test]
    fn pack_resolves_cross_material_products_and_publishes_atomically() {
        let wood = VALID.replace("core:air", "sample:ash");
        let ash = r#"
schema_version = 1
name = "sample:ash"
behavior = "powder"
mass_per_cell = 300
color = [90, 90, 90, 255]
"#;
        let mut registry = MaterialRegistry::with_default_materials_builder();
        let ids = register_material_pack(&mut registry, [&wood, ash]).unwrap();
        assert_eq!(ids.len(), 2);
        registry.try_freeze().unwrap();

        let mut unchanged = MaterialRegistry::with_default_materials_builder();
        let before = unchanged.len();
        assert!(register_material_pack(&mut unchanged, [VALID, VALID]).is_err());
        assert_eq!(unchanged.len(), before);
    }
}
