use super::*;
use serde::de::{DeserializeSeed, Error, MapAccess, SeqAccess, Visitor};
use std::{cell::Cell, fmt};

struct RawArray<'a> {
    limits: &'a InputLimits,
    failure: &'a Cell<Option<BWErr>>,
}
impl<'de> Visitor<'de> for RawArray<'_> {
    type Value = Vec<&'de RawValue>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a JSON array")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
        let mut values = vec![];
        while let Some(value) = access.next_element::<&RawValue>()? {
            if let Err(error) = self.limits.values.container_header(values.len() + 1) {
                self.failure.set(Some(error));
                return Err(A::Error::custom("input resource limit"));
            }
            values.push(value);
        }
        Ok(values)
    }
}
impl<'de> DeserializeSeed<'de> for RawArray<'_> {
    type Value = Vec<&'de RawValue>;
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_seq(self)
    }
}

struct RawMap<'a> {
    limits: &'a InputLimits,
    root: bool,
    failure: &'a Cell<Option<BWErr>>,
}
impl<'de> Visitor<'de> for RawMap<'_> {
    type Value = BTreeMap<String, &'de RawValue>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a JSON object")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
        let mut values = BTreeMap::new();
        while let Some(key) = access.next_key::<String>()? {
            let result = self.limits.values.key_size(key.len()).and_then(|()| {
                if values.contains_key(&key) {
                    return Ok(());
                }
                if self.root {
                    if values.len() >= self.limits.variables {
                        return Err(BWErr::ResourceLimit {
                            resource: "input variables",
                            limit: self.limits.variables as u64,
                        });
                    }
                    Ok(())
                } else {
                    self.limits
                        .values
                        .container_header(values.len() + 1)
                        .map(|_| ())
                }
            });
            if let Err(error) = result {
                self.failure.set(Some(error));
                return Err(A::Error::custom("input resource limit"));
            }
            values.insert(key, access.next_value::<&RawValue>()?);
        }
        Ok(values)
    }
}
impl<'de> DeserializeSeed<'de> for RawMap<'_> {
    type Value = BTreeMap<String, &'de RawValue>;
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_map(self)
    }
}

pub(super) fn array<'a>(
    origin: &(impl std::fmt::Display + ?Sized),
    path: &str,
    text: &'a str,
    limits: &InputLimits,
) -> DiagnosticResult<Vec<&'a RawValue>> {
    let failure = Cell::new(None);
    let mut deserializer = serde_json::Deserializer::from_str(text);
    RawArray {
        limits,
        failure: &failure,
    }
    .deserialize(&mut deserializer)
    .map_err(|error| {
        failure.take().map_or_else(
            || invalid(origin, path, error),
            |error| resource(origin, error),
        )
    })
}

pub(super) fn map<'a>(
    origin: &(impl std::fmt::Display + ?Sized),
    path: &str,
    text: &'a str,
    limits: &InputLimits,
    root: bool,
) -> DiagnosticResult<BTreeMap<String, &'a RawValue>> {
    let failure = Cell::new(None);
    let mut deserializer = serde_json::Deserializer::from_str(text);
    RawMap {
        limits,
        root,
        failure: &failure,
    }
    .deserialize(&mut deserializer)
    .map_err(|error| {
        failure.take().map_or_else(
            || invalid(origin, path, error),
            |error| resource(origin, error),
        )
    })
}
