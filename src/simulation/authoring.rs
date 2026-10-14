//! Typed, namespaced per-material data that remains inert until a registered system consumes it.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PropertyValue {
    Bool(bool),
    Integer(i64),
    FixedQ16(i64),
    Enum(String),
    Reference(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterialProperty {
    pub key: String,
    pub value: PropertyValue,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterialAttachment {
    pub kind: String,
    pub version: u16,
    pub values: Vec<MaterialProperty>,
}

/// Combustion data is content only; it has no effect until the M4 reaction/thermal phase exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flammability {
    pub ignition_energy: u32,
    pub fuel_ticks: u16,
    pub heat_per_tick: u16,
    pub exhaust_material: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MaterialAuthoring {
    pub properties: Vec<MaterialProperty>,
    pub attachments: Vec<MaterialAttachment>,
    pub flammability: Option<Flammability>,
}

impl MaterialAuthoring {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.properties.len() > 64 || self.attachments.len() > 32 {
            return Err("material property or attachment count exceeds its limit");
        }
        validate_properties(&self.properties)?;
        for attachment in &self.attachments {
            validate_key(&attachment.kind)?;
            if attachment.version == 0 {
                return Err("attachment version must be non-zero");
            }
            validate_properties(&attachment.values)?;
        }
        if let Some(flammability) = &self.flammability {
            if flammability.ignition_energy == 0 || flammability.fuel_ticks == 0 {
                return Err("flammable materials require positive ignition energy and fuel ticks");
            }
            if let Some(product) = &flammability.exhaust_material {
                validate_key(product)?;
            }
        }
        Ok(())
    }
}

pub(crate) fn validate_key(value: &str) -> Result<(), &'static str> {
    if let Some((namespace, name)) = value.split_once(':')
        && valid_part(namespace)
        && valid_part(name)
        && !name.contains(':')
    {
        return Ok(());
    }
    Err("expected a lowercase namespace:name key")
}

fn validate_properties(properties: &[MaterialProperty]) -> Result<(), &'static str> {
    if properties.len() > 64 {
        return Err("attachment property count exceeds 64");
    }
    use std::collections::HashSet;
    let mut keys = HashSet::new();
    keys.try_reserve(properties.len())
        .map_err(|_| "property validation allocation failed")?;
    for property in properties {
        validate_key(&property.key)?;
        if !keys.insert(property.key.as_str()) {
            return Err("duplicate property key");
        }
        if let PropertyValue::Enum(value) | PropertyValue::Reference(value) = &property.value
            && value.len() > 256
        {
            return Err("property string exceeds 256 bytes");
        }
        if let PropertyValue::Reference(value) = &property.value {
            validate_key(value)?;
        }
    }
    Ok(())
}

fn valid_part(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_properties_and_attachment_are_validated() {
        let authoring = MaterialAuthoring {
            properties: vec![MaterialProperty {
                key: "sample:mana".into(),
                value: PropertyValue::Integer(12),
            }],
            attachments: vec![MaterialAttachment {
                kind: "sample:receiver".into(),
                version: 1,
                values: vec![MaterialProperty {
                    key: "sample:channel".into(),
                    value: PropertyValue::Enum("blue".into()),
                }],
            }],
            flammability: Some(Flammability {
                ignition_energy: 800,
                fuel_ticks: 600,
                heat_per_tick: 8,
                exhaust_material: Some("core:air".into()),
            }),
        };
        assert_eq!(authoring.validate(), Ok(()));
    }

    #[test]
    fn invalid_duplicate_or_unbounded_property_data_is_rejected() {
        let duplicate = MaterialAuthoring {
            properties: vec![
                MaterialProperty {
                    key: "sample:value".into(),
                    value: PropertyValue::Bool(true),
                },
                MaterialProperty {
                    key: "sample:value".into(),
                    value: PropertyValue::Bool(false),
                },
            ],
            ..MaterialAuthoring::default()
        };
        assert_eq!(duplicate.validate(), Err("duplicate property key"));
        let invalid = MaterialAuthoring {
            properties: vec![MaterialProperty {
                key: "bad key".into(),
                value: PropertyValue::Integer(0),
            }],
            ..MaterialAuthoring::default()
        };
        assert!(invalid.validate().is_err());
    }
}
