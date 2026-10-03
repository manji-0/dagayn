//! Manifest file loaders.
//!
//! JSON objects keep their document order: `scripts`, dependency tables,
//! and generator maps are walked in order, and that order decides edge
//! order and the order of lists in edge metadata. A duplicated key keeps
//! its first position and its last value, as Python's `json.loads` does.

use std::fmt;
use std::path::Path;

use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

use super::text::read_text_strict;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Json {
    Null,
    Bool(bool),
    Number(serde_json::Number),
    String(String),
    Array(Vec<Json>),
    Object(JsonObject),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct JsonObject(Vec<(String, Json)>);

impl JsonObject {
    pub fn get(&self, key: &str) -> Option<&Json> {
        self.0
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Json::as_str)
    }

    pub fn get_object(&self, key: &str) -> Option<&JsonObject> {
        self.get(key).and_then(Json::as_object)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Json)> {
        self.0.iter().map(|(key, value)| (key.as_str(), value))
    }

    pub fn values(&self) -> impl Iterator<Item = &Json> {
        self.0.iter().map(|(_, value)| value)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn insert(&mut self, key: String, value: Json) {
        match self.0.iter_mut().find(|(name, _)| *name == key) {
            Some(slot) => slot.1 = value,
            None => self.0.push((key, value)),
        }
    }
}

impl Json {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&JsonObject> {
        match self {
            Json::Object(object) => Some(object),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    /// Python truthiness of the decoded value.
    pub fn is_truthy(&self) -> bool {
        match self {
            Json::Null => false,
            Json::Bool(value) => *value,
            Json::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
            Json::String(value) => !value.is_empty(),
            Json::Array(items) => !items.is_empty(),
            Json::Object(object) => !object.is_empty(),
        }
    }

    /// Python's `str()` of the decoded value.
    pub fn py_str(&self) -> String {
        match self {
            Json::String(value) => value.clone(),
            other => other.py_repr(),
        }
    }

    fn py_repr(&self) -> String {
        match self {
            Json::Null => "None".to_string(),
            Json::Bool(true) => "True".to_string(),
            Json::Bool(false) => "False".to_string(),
            Json::Number(number) => number.to_string(),
            Json::String(value) => {
                format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
            }
            Json::Array(items) => {
                let items: Vec<String> = items.iter().map(Json::py_repr).collect();
                format!("[{}]", items.join(", "))
            }
            Json::Object(object) => {
                let items: Vec<String> = object
                    .iter()
                    .map(|(key, value)| {
                        format!(
                            "{}: {}",
                            Json::String(key.to_string()).py_repr(),
                            value.py_repr()
                        )
                    })
                    .collect();
                format!("{{{}}}", items.join(", "))
            }
        }
    }
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(JsonVisitor)
    }
}

struct JsonVisitor;

impl<'de> Visitor<'de> for JsonVisitor {
    type Value = Json;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_unit<E>(self) -> Result<Json, E> {
        Ok(Json::Null)
    }

    fn visit_bool<E>(self, value: bool) -> Result<Json, E> {
        Ok(Json::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Json, E> {
        Ok(Json::Number(value.into()))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Json, E> {
        Ok(Json::Number(value.into()))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Json, E> {
        Ok(serde_json::Number::from_f64(value).map_or(Json::Null, Json::Number))
    }

    fn visit_str<E>(self, value: &str) -> Result<Json, E> {
        Ok(Json::String(value.to_string()))
    }

    fn visit_string<E>(self, value: String) -> Result<Json, E> {
        Ok(Json::String(value))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Json, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Json::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Json, A::Error> {
        let mut object = JsonObject::default();
        while let Some((key, value)) = map.next_entry::<String, Json>()? {
            object.insert(key, value);
        }
        Ok(Json::Object(object))
    }
}

/// A JSON file whose top level is an object; `None` when unreadable,
/// invalid, or not an object.
pub(super) fn load_json(path: &Path) -> Option<JsonObject> {
    let text = read_text_strict(path)?;
    match serde_json::from_str::<Json>(&text).ok()? {
        Json::Object(object) => Some(object),
        _ => None,
    }
}

/// A TOML file; `None` when unreadable or invalid. Table order does not
/// matter to any TOML reader here (lookups and arrays only).
pub(super) fn load_toml(path: &Path) -> Option<toml::Table> {
    let bytes = std::fs::read(path).ok()?;
    let text = std::str::from_utf8(&bytes).ok()?;
    text.parse::<toml::Table>().ok()
}

pub(super) fn toml_table<'a>(table: &'a toml::Table, key: &str) -> Option<&'a toml::Table> {
    table.get(key).and_then(toml::Value::as_table)
}

pub(super) fn toml_str<'a>(table: &'a toml::Table, key: &str) -> Option<&'a str> {
    table.get(key).and_then(toml::Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn objects_keep_document_order_and_last_duplicate() {
        let Json::Object(object) =
            serde_json::from_str::<Json>(r#"{"b": 1, "a": 2, "b": 3}"#).unwrap()
        else {
            panic!("expected an object");
        };
        let keys: Vec<&str> = object.iter().map(|(key, _)| key).collect();
        assert_eq!(keys, ["b", "a"]);
        assert_eq!(object.get("b"), Some(&Json::Number(3.into())));
    }

    #[test]
    fn py_str_matches_python() {
        assert_eq!(Json::Bool(true).py_str(), "True");
        assert_eq!(Json::Number(5.into()).py_str(), "5");
        assert_eq!(Json::String("x".into()).py_str(), "x");
    }
}
