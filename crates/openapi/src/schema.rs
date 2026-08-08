//! Hand-rolled access to an OpenAPI document.
//!
//! The document stays a [`serde_norway::Value`] instead of being modelled with
//! typed structs. Only a handful of fields are ever read, 3.0 and 3.1 disagree
//! in places none of them touch, and a strict model would reject documents the
//! importer can still make sense of. The YAML parser also reads JSON, so both
//! spellings arrive through the same door.

use anyhow::{Context as _, Result};
use serde_norway::Value;

/// Component sections a `$ref` is resolved against.
pub(crate) const SCHEMAS: &str = "schemas";
pub(crate) const PARAMETERS: &str = "parameters";
pub(crate) const REQUEST_BODIES: &str = "requestBodies";

pub(crate) struct Spec {
    root: Value,
}

impl Spec {
    pub(crate) fn parse(text: &str) -> Result<Self> {
        let root: Value =
            serde_norway::from_str(text).context("the document is not valid YAML or JSON")?;
        Ok(Self { root })
    }

    /// The declared `openapi` version, if the document carries one.
    pub(crate) fn version(&self) -> Option<&str> {
        as_str(&self.root, "openapi")
    }

    pub(crate) fn get(&self, key: &str) -> Option<&Value> {
        self.root.get(key)
    }

    fn component(&self, section: &str, name: &str) -> Option<&Value> {
        self.root.get("components")?.get(section)?.get(name)
    }

    /// Follows a `$ref` chain into `#/components/<section>/` until a concrete
    /// object is reached.
    ///
    /// Returns `None` when the value is a reference that points outside the
    /// section, names a component that does not exist, or is part of a cycle —
    /// the caller then leaves that piece of the request empty rather than
    /// recursing forever.
    pub(crate) fn resolve<'a>(&'a self, value: &'a Value, section: &str) -> Option<&'a Value> {
        let prefix = format!("#/components/{section}/");
        let mut current = value;
        let mut seen: Vec<&str> = Vec::new();
        loop {
            let Some(reference) = reference_of(current) else {
                return Some(current);
            };
            if seen.contains(&reference) {
                return None;
            }
            seen.push(reference);
            current = self.component(section, reference.strip_prefix(&prefix)?)?;
        }
    }
}

/// The `$ref` target of a value, when the value is a reference object.
pub(crate) fn reference_of(value: &Value) -> Option<&str> {
    as_str(value, "$ref")
}

/// A string field, treating an empty string as absent: OpenAPI documents in the
/// wild carry `summary: ""` and an empty request name is worse than none.
pub(crate) fn as_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    let text = value.get(key)?.as_str()?;
    (!text.is_empty()).then_some(text)
}

/// A boolean field, defaulting to `false` as the OpenAPI schema does.
pub(crate) fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool) == Some(true)
}

/// The string-keyed entries of a mapping, in document order. Anything that is
/// not a mapping yields nothing.
pub(crate) fn entries(value: &Value) -> impl Iterator<Item = (&str, &Value)> {
    value
        .as_mapping()
        .into_iter()
        .flatten()
        .filter_map(|(key, value)| Some((key.as_str()?, value)))
}

/// The elements of a sequence. Anything that is not a sequence yields nothing.
pub(crate) fn items(value: &Value) -> &[Value] {
    value.as_sequence().map_or(&[], Vec::as_slice)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(text: &str) -> Spec {
        Spec::parse(text).expect("the fixture parses")
    }

    #[test]
    fn reads_the_version_from_yaml_and_json() {
        assert_eq!(spec("openapi: 3.1.0\n").version(), Some("3.1.0"));
        assert_eq!(spec(r#"{"openapi": "3.0.3"}"#).version(), Some("3.0.3"));
        assert_eq!(spec("info:\n  title: x\n").version(), None);
    }

    #[test]
    fn resolves_a_chain_of_references() {
        let spec = spec(
            "components:\n  \
             parameters:\n    \
             Alias:\n      $ref: '#/components/parameters/Limit'\n    \
             Limit:\n      name: limit\n      in: query\n",
        );
        let alias = spec.get("components").unwrap().get("parameters").unwrap();
        let alias = alias.get("Alias").unwrap();
        let resolved = spec.resolve(alias, PARAMETERS).expect("the chain resolves");
        assert_eq!(as_str(resolved, "name"), Some("limit"));
    }

    #[test]
    fn a_reference_cycle_resolves_to_nothing() {
        let spec = spec(
            "components:\n  \
             parameters:\n    \
             A:\n      $ref: '#/components/parameters/B'\n    \
             B:\n      $ref: '#/components/parameters/A'\n",
        );
        let start = spec
            .get("components")
            .unwrap()
            .get("parameters")
            .unwrap()
            .get("A")
            .unwrap();
        assert!(spec.resolve(start, PARAMETERS).is_none());
    }

    #[test]
    fn a_reference_into_another_section_is_not_resolved() {
        let spec = spec(
            "components:\n  \
             schemas:\n    \
             Pet:\n      type: object\n",
        );
        let reference = serde_norway::from_str::<Value>("$ref: '#/components/schemas/Pet'\n")
            .expect("the fixture parses");
        assert!(spec.resolve(&reference, PARAMETERS).is_none());
        assert!(spec.resolve(&reference, SCHEMAS).is_some());
    }

    #[test]
    fn a_missing_component_resolves_to_nothing() {
        let spec = spec("components:\n  schemas: {}\n");
        let reference = serde_norway::from_str::<Value>("$ref: '#/components/schemas/Ghost'\n")
            .expect("the fixture parses");
        assert!(spec.resolve(&reference, SCHEMAS).is_none());
    }

    #[test]
    fn an_empty_string_field_counts_as_absent() {
        let value = serde_norway::from_str::<Value>("summary: ''\ntitle: x\n").unwrap();
        assert_eq!(as_str(&value, "summary"), None);
        assert_eq!(as_str(&value, "title"), Some("x"));
    }

    #[test]
    fn entries_follow_document_order() {
        let value = serde_norway::from_str::<Value>("zebra: 1\nalpha: 2\n").unwrap();
        let names: Vec<&str> = entries(&value).map(|(name, _)| name).collect();
        assert_eq!(names, ["zebra", "alpha"]);
    }
}
