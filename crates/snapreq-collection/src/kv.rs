//! Key/value tables (`[headers]`, `[query]`, `[body.fields]`).
//!
//! In files they are TOML tables, keeping their written order:
//! `Name = "v"`, `Name = ["a", "b"]` (repeated), `Name = { value = "v", enabled = false }`.

use std::fmt;

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeStruct};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use snapreq_core::model::KeyValue;

/// An ordered list of name/value entries; names may repeat and entries may be disabled.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyValues(pub Vec<KeyValue>);

impl KeyValues {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<KeyValue>> for KeyValues {
    fn from(v: Vec<KeyValue>) -> Self {
        Self(v)
    }
}

const EXPECTED: &str = "a string, a list of strings, or { value = \"...\", enabled = false }";

/// One value: a scalar (strings, and numbers or booleans for convenience) or `{ value, enabled }`.
struct Single(String, bool);

struct SingleVisitor;

impl<'de> Visitor<'de> for SingleVisitor {
    type Value = Single;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(EXPECTED)
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Single, E> {
        Ok(Single(v.to_owned(), true))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Single, E> {
        Ok(Single(v.to_string(), true))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Single, E> {
        Ok(Single(v.to_string(), true))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Single, E> {
        Ok(Single(v.to_string(), true))
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Single, E> {
        Ok(Single(v.to_string(), true))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Single, A::Error> {
        let mut value = None;
        let mut enabled = true;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "value" => value = Some(map.next_value::<ScalarString>()?.0),
                "enabled" => enabled = map.next_value()?,
                other => {
                    return Err(de::Error::unknown_field(other, &["value", "enabled"]));
                }
            }
        }
        let value = value.ok_or_else(|| de::Error::missing_field("value"))?;
        Ok(Single(value, enabled))
    }
}

impl<'de> Deserialize<'de> for Single {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(SingleVisitor)
    }
}

/// A scalar converted to a string.
struct ScalarString(String);

impl<'de> Deserialize<'de> for ScalarString {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = ScalarString;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a string")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<ScalarString, E> {
                Ok(ScalarString(v.to_owned()))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<ScalarString, E> {
                Ok(ScalarString(v.to_string()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<ScalarString, E> {
                Ok(ScalarString(v.to_string()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<ScalarString, E> {
                Ok(ScalarString(v.to_string()))
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<ScalarString, E> {
                Ok(ScalarString(v.to_string()))
            }
        }
        d.deserialize_any(V)
    }
}

/// One table value: a single entry or a list of entries.
struct Spec(Vec<Single>);

impl<'de> Deserialize<'de> for Spec {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Spec;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(EXPECTED)
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Spec, E> {
                SingleVisitor.visit_str(v).map(|s| Spec(vec![s]))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Spec, E> {
                SingleVisitor.visit_i64(v).map(|s| Spec(vec![s]))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Spec, E> {
                SingleVisitor.visit_u64(v).map(|s| Spec(vec![s]))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Spec, E> {
                SingleVisitor.visit_f64(v).map(|s| Spec(vec![s]))
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Spec, E> {
                SingleVisitor.visit_bool(v).map(|s| Spec(vec![s]))
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Spec, A::Error> {
                SingleVisitor.visit_map(map).map(|s| Spec(vec![s]))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Spec, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element::<Single>()? {
                    items.push(item);
                }
                Ok(Spec(items))
            }
        }
        d.deserialize_any(V)
    }
}

impl<'de> Deserialize<'de> for KeyValues {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = KeyValues;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a table of names and values")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<KeyValues, A::Error> {
                let mut entries = Vec::new();
                while let Some(name) = map.next_key::<String>()? {
                    for Single(value, enabled) in map.next_value::<Spec>()?.0 {
                        entries.push(KeyValue {
                            name: name.clone(),
                            value,
                            enabled,
                        });
                    }
                }
                Ok(KeyValues(entries))
            }
        }
        d.deserialize_map(V)
    }
}

/// Serializes one entry: a plain string when enabled, else `{ value, enabled = false }`.
struct SerSingle<'a>(&'a KeyValue);

impl Serialize for SerSingle<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.0.enabled {
            s.serialize_str(&self.0.value)
        } else {
            let mut st = s.serialize_struct("Entry", 2)?;
            st.serialize_field("value", &self.0.value)?;
            st.serialize_field("enabled", &false)?;
            st.end()
        }
    }
}

impl Serialize for KeyValues {
    /// Entries with the same name are grouped into a list at the position of the first one.
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut groups: Vec<(&str, Vec<&KeyValue>)> = Vec::new();
        for kv in &self.0 {
            match groups.iter_mut().find(|(n, _)| *n == kv.name) {
                Some((_, list)) => list.push(kv),
                None => groups.push((&kv.name, vec![kv])),
            }
        }
        let mut map = s.serialize_map(Some(groups.len()))?;
        for (name, list) in groups {
            if let [one] = list.as_slice() {
                map.serialize_entry(name, &SerSingle(one))?;
            } else {
                let items: Vec<SerSingle<'_>> = list.into_iter().map(SerSingle).collect();
                map.serialize_entry(name, &items)?;
            }
        }
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Deserialize, Serialize, PartialEq)]
    struct Doc {
        headers: KeyValues,
    }

    fn parse(toml_text: &str) -> Result<KeyValues, String> {
        toml::from_str::<Doc>(toml_text)
            .map(|d| d.headers)
            .map_err(|e| e.message().to_owned())
    }

    #[test]
    fn all_value_forms_in_order() {
        let kv = parse(
            r#"
            [headers]
            Zeta = "1"
            Multi = ["a", { value = "b", enabled = false }]
            Off = { value = "x", enabled = false }
            page = 2
            debug = true
            "#,
        )
        .unwrap();
        assert_eq!(
            kv.0,
            [
                KeyValue::new("Zeta", "1"),
                KeyValue::new("Multi", "a"),
                KeyValue::new("Multi", "b").disabled(),
                KeyValue::new("Off", "x").disabled(),
                KeyValue::new("page", "2"),
                KeyValue::new("debug", "true"),
            ]
        );
    }

    #[test]
    fn round_trip_through_toml() {
        let doc = Doc {
            headers: KeyValues(vec![
                KeyValue::new("B", "1"),
                KeyValue::new("A", "2"),
                KeyValue::new("B", "3"),
                KeyValue::new("C", "4").disabled(),
            ]),
        };
        let text = crate::save::to_toml(&doc).unwrap();
        assert_eq!(
            text,
            "[headers]\nB = [\"1\", \"3\"]\nA = \"2\"\nC = { value = \"4\", enabled = false }\n"
        );
        // Grouping moves the second `B` next to the first; the set of entries is unchanged.
        let back: Doc = toml::from_str(&text).unwrap();
        assert_eq!(back.headers.0.len(), 4);
    }

    #[test]
    fn helpful_errors() {
        let err = parse("[headers]\nX = { valu = \"1\" }\n").unwrap_err();
        assert!(err.contains("unknown field `valu`"), "{err}");
        let err = parse("[headers]\nX = { enabled = false }\n").unwrap_err();
        assert!(err.contains("missing field `value`"), "{err}");
        let err = parse("[headers]\nX = [[1]]\n").unwrap_err();
        assert!(err.contains("a string, a list of strings"), "{err}");
    }
}
