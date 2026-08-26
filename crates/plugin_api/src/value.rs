use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{PluginError, PluginResult};

/// JSON-like values exchanged between a plugin engine and its host.
///
/// Keeping this type independent from `mlua::Value` is what lets Native use
/// mlua while Web uses a different Lua implementation without duplicating the
/// `ctx.*` protocol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PluginValue {
    Null,
    Bool(bool),
    Integer(i64),
    Number(f64),
    String(String),
    Array(Vec<PluginValue>),
    Object(BTreeMap<String, PluginValue>),
}

impl PluginValue {
    pub fn object() -> BTreeMap<String, PluginValue> {
        BTreeMap::new()
    }

    pub fn to_json(&self) -> PluginResult<serde_json::Value> {
        Ok(match self {
            Self::Null => serde_json::Value::Null,
            Self::Bool(value) => serde_json::Value::Bool(*value),
            Self::Integer(value) => serde_json::Value::Number((*value).into()),
            Self::Number(value) => serde_json::Number::from_f64(*value)
                .map(serde_json::Value::Number)
                .ok_or_else(|| PluginError::InvalidValue("number is not finite".to_owned()))?,
            Self::String(value) => serde_json::Value::String(value.clone()),
            Self::Array(values) => serde_json::Value::Array(
                values
                    .iter()
                    .map(Self::to_json)
                    .collect::<PluginResult<Vec<_>>>()?,
            ),
            Self::Object(values) => serde_json::Value::Object(
                values
                    .iter()
                    .map(|(key, value)| Ok((key.clone(), value.to_json()?)))
                    .collect::<PluginResult<_>>()?,
            ),
        })
    }

    pub fn from_json(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(value) => Self::Bool(*value),
            serde_json::Value::Number(value) => value
                .as_i64()
                .map(Self::Integer)
                .or_else(|| value.as_f64().map(Self::Number))
                .unwrap_or(Self::Null),
            serde_json::Value::String(value) => Self::String(value.clone()),
            serde_json::Value::Array(values) => {
                Self::Array(values.iter().map(Self::from_json).collect())
            }
            serde_json::Value::Object(values) => Self::Object(
                values
                    .iter()
                    .map(|(key, value)| (key.clone(), Self::from_json(value)))
                    .collect(),
            ),
        }
    }
}

impl From<serde_json::Value> for PluginValue {
    fn from(value: serde_json::Value) -> Self {
        Self::from_json(&value)
    }
}

impl TryFrom<PluginValue> for serde_json::Value {
    type Error = PluginError;

    fn try_from(value: PluginValue) -> Result<Self, Self::Error> {
        value.to_json()
    }
}

#[cfg(test)]
mod tests {
    use super::PluginValue;

    #[test]
    fn json_round_trip_keeps_integer_and_object_shape() {
        let value = PluginValue::from_json(&serde_json::json!({
            "id": 7,
            "enabled": true,
            "items": ["a", null]
        }));

        assert_eq!(
            value.to_json().unwrap(),
            serde_json::json!({
                "id": 7,
                "enabled": true,
                "items": ["a", null]
            })
        );
    }

    #[test]
    fn non_finite_number_is_rejected_at_host_boundary() {
        assert!(PluginValue::Number(f64::NAN).to_json().is_err());
    }
}
