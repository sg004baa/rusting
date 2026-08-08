//! JSON request-body examples derived from a schema.
//!
//! The body an import produces is a starting point for editing, not a valid
//! payload: every field is present, filled with its declared example or the
//! empty value for its type.

use serde_norway::Value;

use crate::schema::{self, SCHEMAS, Spec};

/// A JSON document under construction.
///
/// `serde_json::Value` is not used here because its object map sorts keys,
/// and the example has to list properties in the order the schema declares
/// them — an alphabetised body reads nothing like the API's documentation.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Json {
    Null,
    Bool(bool),
    /// Already rendered as a JSON number literal.
    Number(String),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    /// Renders with two-space indentation, matching what the body editor shows
    /// after prettifying.
    pub(crate) fn render(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out
    }

    fn write(&self, out: &mut String, depth: usize) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(true) => out.push_str("true"),
            Json::Bool(false) => out.push_str("false"),
            Json::Number(literal) => out.push_str(literal),
            Json::String(text) => write_string(out, text),
            Json::Array(items) if items.is_empty() => out.push_str("[]"),
            Json::Array(items) => {
                out.push_str("[\n");
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push_str(",\n");
                    }
                    indent(out, depth + 1);
                    item.write(out, depth + 1);
                }
                out.push('\n');
                indent(out, depth);
                out.push(']');
            }
            Json::Object(fields) if fields.is_empty() => out.push_str("{}"),
            Json::Object(fields) => {
                out.push_str("{\n");
                for (index, (name, value)) in fields.iter().enumerate() {
                    if index > 0 {
                        out.push_str(",\n");
                    }
                    indent(out, depth + 1);
                    write_string(out, name);
                    out.push_str(": ");
                    value.write(out, depth + 1);
                }
                out.push('\n');
                indent(out, depth);
                out.push('}');
            }
        }
    }
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn write_string(out: &mut String, text: &str) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            control if control < ' ' => {
                out.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

/// Converts a literal `example` or `default` taken from the document.
///
/// Returns `None` for the few YAML values JSON cannot express — a non-finite
/// number, a non-string mapping key, a tagged node — so the caller falls back
/// to the empty value for the declared type instead of emitting broken JSON.
pub(crate) fn from_document(value: &Value) -> Option<Json> {
    Some(match value {
        Value::Null => Json::Null,
        Value::Bool(flag) => Json::Bool(*flag),
        Value::Number(number) => Json::Number(number_literal(number)?),
        Value::String(text) => Json::String(text.clone()),
        Value::Sequence(items) => {
            Json::Array(items.iter().map(from_document).collect::<Option<_>>()?)
        }
        Value::Mapping(fields) => Json::Object(
            fields
                .iter()
                .map(|(name, value)| Some((name.as_str()?.to_owned(), from_document(value)?)))
                .collect::<Option<_>>()?,
        ),
        Value::Tagged(_) => return None,
    })
}

fn number_literal(number: &serde_norway::Number) -> Option<String> {
    if let Some(value) = number.as_i64() {
        return Some(value.to_string());
    }
    if let Some(value) = number.as_u64() {
        return Some(value.to_string());
    }
    let value = number.as_f64()?;
    value.is_finite().then(|| value.to_string())
}

/// Walks schemas to build example bodies, remembering which resolved schemas
/// are currently being expanded so a recursive `$ref` terminates at its first
/// re-entry.
pub(crate) struct Examples<'a> {
    spec: &'a Spec,
    active: Vec<&'a Value>,
}

impl<'a> Examples<'a> {
    pub(crate) fn new(spec: &'a Spec) -> Self {
        Self {
            spec,
            active: Vec::new(),
        }
    }

    /// The body text for one entry of a `content` mapping. A media type whose
    /// schema says nothing usable becomes an empty object, which is still a
    /// sensible thing to start editing.
    pub(crate) fn media_type(&mut self, media_type: &'a Value) -> String {
        let generated = match media_type.get("example").and_then(from_document) {
            Some(example) => Some(example),
            None => media_type
                .get("schema")
                .and_then(|schema| self.schema(schema)),
        };
        generated.unwrap_or(Json::Object(Vec::new())).render()
    }

    /// The example for one schema, following a leading `$ref`.
    pub(crate) fn schema(&mut self, schema: &'a Value) -> Option<Json> {
        let resolved = self.spec.resolve(schema, SCHEMAS)?;
        if self
            .active
            .iter()
            .any(|active| std::ptr::eq(*active, resolved))
        {
            return None;
        }

        self.active.push(resolved);
        let generated = self.resolved(resolved);
        self.active.pop();
        generated
    }

    fn resolved(&mut self, schema: &'a Value) -> Option<Json> {
        if let Some(example) = schema.get("example").and_then(from_document) {
            return Some(example);
        }
        if let Some(default) = schema.get("default").and_then(from_document) {
            return Some(default);
        }
        match type_name(schema) {
            Some("string") => Some(Json::String(String::new())),
            Some("number" | "integer") => Some(Json::Number("0".to_owned())),
            Some("boolean") => Some(Json::Bool(false)),
            Some("null") => Some(Json::Null),
            Some("array") => Some(Json::Array(Vec::new())),
            Some("object") => Some(self.object(schema)),
            // A typeless schema carrying properties is an object in all but
            // the declaration; anything else has no example worth inventing.
            None if schema.get("properties").is_some() => Some(self.object(schema)),
            Some(_) | None => None,
        }
    }

    fn object(&mut self, schema: &'a Value) -> Json {
        let mut fields = Vec::new();
        if let Some(properties) = schema.get("properties") {
            for (name, property) in schema::entries(properties) {
                // A property whose schema is unusable — a cycle, a dangling
                // `$ref`, a type we cannot invent a value for — is still worth
                // listing so the field is visible in the editor.
                let value = self.schema(property).unwrap_or(Json::Null);
                fields.push((name.to_owned(), value));
            }
        }
        Json::Object(fields)
    }
}

/// The schema's type. OpenAPI 3.1 allows a list of types; the first one that is
/// not `null` describes the value best.
fn type_name(schema: &Value) -> Option<&str> {
    let declared = schema.get("type")?;
    if let Some(name) = declared.as_str() {
        return Some(name);
    }
    let names = schema::items(declared);
    let mut strings = names.iter().filter_map(Value::as_str);
    let first = strings.next()?;
    if first == "null" {
        return Some(strings.next().unwrap_or("null"));
    }
    Some(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(text: &str) -> Spec {
        Spec::parse(text).expect("the fixture parses")
    }

    /// Generates the body for `components.schemas.Root`.
    fn body(spec: &Spec) -> String {
        let root = spec
            .get("components")
            .and_then(|components| components.get("schemas"))
            .and_then(|schemas| schemas.get("Root"))
            .expect("the fixture defines Root");
        Examples::new(spec).schema(root).map_or_else(
            || Json::Object(Vec::new()).render(),
            |generated| generated.render(),
        )
    }

    #[test]
    fn scalar_types_get_their_empty_value() {
        let spec = spec(
            "components:\n  schemas:\n    Root:\n      type: object\n      properties:\n        \
             text: { type: string }\n        \
             count: { type: integer }\n        \
             ratio: { type: number }\n        \
             flag: { type: boolean }\n        \
             list: { type: array, items: { type: string } }\n        \
             nested: { type: object, properties: { inner: { type: string } } }\n",
        );
        assert_eq!(
            body(&spec),
            "{\n  \"text\": \"\",\n  \"count\": 0,\n  \"ratio\": 0,\n  \"flag\": false,\n  \
             \"list\": [],\n  \"nested\": {\n    \"inner\": \"\"\n  }\n}"
        );
    }

    #[test]
    fn an_example_beats_a_default_which_beats_the_type() {
        let spec = spec(
            "components:\n  schemas:\n    Root:\n      type: object\n      properties:\n        \
             a: { type: string, example: hello, default: ignored }\n        \
             b: { type: integer, default: 7 }\n        \
             c: { type: string }\n",
        );
        assert_eq!(
            body(&spec),
            "{\n  \"a\": \"hello\",\n  \"b\": 7,\n  \"c\": \"\"\n}"
        );
    }

    #[test]
    fn an_array_without_items_is_empty() {
        let spec = spec("components:\n  schemas:\n    Root:\n      type: array\n");
        assert_eq!(body(&spec), "[]");
    }

    #[test]
    fn a_self_referential_schema_terminates() {
        let spec = spec(
            "components:\n  schemas:\n    Root:\n      type: object\n      properties:\n        \
             name: { type: string }\n        \
             parent: { $ref: '#/components/schemas/Root' }\n        \
             children:\n          type: array\n          items: { $ref: '#/components/schemas/Root' }\n",
        );
        assert_eq!(
            body(&spec),
            "{\n  \"name\": \"\",\n  \"parent\": null,\n  \"children\": []\n}"
        );
    }

    #[test]
    fn mutually_referential_schemas_terminate() {
        let spec = spec(
            "components:\n  schemas:\n    Root:\n      type: object\n      properties:\n        \
             child: { $ref: '#/components/schemas/Child' }\n    \
             Child:\n      type: object\n      properties:\n        \
             parent: { $ref: '#/components/schemas/Root' }\n",
        );
        assert_eq!(
            body(&spec),
            "{\n  \"child\": {\n    \"parent\": null\n  }\n}"
        );
    }

    #[test]
    fn the_same_schema_may_appear_twice_side_by_side() {
        let spec = spec(
            "components:\n  schemas:\n    Root:\n      type: object\n      properties:\n        \
             first: { $ref: '#/components/schemas/Leaf' }\n        \
             second: { $ref: '#/components/schemas/Leaf' }\n    \
             Leaf: { type: string }\n",
        );
        assert_eq!(body(&spec), "{\n  \"first\": \"\",\n  \"second\": \"\"\n}");
    }

    #[test]
    fn a_nullable_union_type_uses_the_concrete_member() {
        let spec = spec(
            "components:\n  schemas:\n    Root:\n      type: object\n      properties:\n        \
             a:\n          type: [null, string]\n        \
             b:\n          type: [null]\n",
        );
        assert_eq!(body(&spec), "{\n  \"a\": \"\",\n  \"b\": null\n}");
    }

    #[test]
    fn a_typeless_schema_with_properties_is_an_object() {
        let spec = spec(
            "components:\n  schemas:\n    Root:\n      properties:\n        a: { type: string }\n",
        );
        assert_eq!(body(&spec), "{\n  \"a\": \"\"\n}");
    }

    #[test]
    fn a_media_type_example_wins_over_the_schema() {
        let spec = spec("components:\n  schemas:\n    Root: { type: string }\n");
        let media: Value = serde_norway::from_str(
            "example:\n  id: 3\nschema:\n  $ref: '#/components/schemas/Root'\n",
        )
        .expect("the fixture parses");
        assert_eq!(Examples::new(&spec).media_type(&media), "{\n  \"id\": 3\n}");
    }

    #[test]
    fn a_media_type_with_nothing_usable_is_an_empty_object() {
        let spec = spec("openapi: 3.1.0\n");
        let media: Value = serde_norway::from_str("schema: { type: gibberish }\n").unwrap();
        assert_eq!(Examples::new(&spec).media_type(&media), "{}");
    }

    #[test]
    fn strings_are_escaped() {
        let mut out = String::new();
        write_string(&mut out, "a\"b\\c\nd\u{1}");
        assert_eq!(out, r#""a\"b\\c\nd\u0001""#);
    }

    #[test]
    fn non_finite_numbers_are_not_usable_as_examples() {
        let value: Value = serde_norway::from_str(".inf").expect("the fixture parses");
        assert_eq!(from_document(&value), None);
    }
}
