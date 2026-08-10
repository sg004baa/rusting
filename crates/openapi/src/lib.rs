//! Importing an OpenAPI 3.0/3.1 document into a collection.
//!
//! The import is a pure transformation: it reads the document and returns the
//! collection tree plus the environment variables the requests refer to.
//! Nothing is written to disk — the caller decides where, and whether, the
//! result is saved.

mod example;
mod schema;

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use serde_norway::Value;

use rusting_core::model::REQUEST_SUFFIX;
use rusting_core::{
    Auth, BodyContent, Collection, HttpMethod, KeyValue, PathParam, RequestModel, files, urls,
};

use crate::example::Examples;
use crate::schema::{PARAMETERS, PATH_ITEMS, REQUEST_BODIES, SCHEMAS, Spec};

/// The variable every imported URL is written against.
const BASE_URL: &str = "BASE_URL";

/// The result of reading an OpenAPI document.
#[derive(Debug)]
pub struct Imported {
    pub collection: Collection,
    /// Variables to write into a `.env` file. Holds `BASE_URL` and a
    /// placeholder for every supported security scheme the document declares.
    pub env: BTreeMap<String, String>,
}

/// Reads an OpenAPI 3.0 or 3.1 document, in YAML or JSON.
///
/// `output_dir` becomes the path of the returned collection and the prefix of
/// every request path, so the caller can hand each request straight to
/// `rusting_core::collection::save_request`. The directory does not need to
/// exist and is not created here.
pub fn import(spec_path: &Path, output_dir: &Path) -> Result<Imported> {
    let text = std::fs::read_to_string(spec_path)
        .with_context(|| format!("could not read {}", spec_path.display()))?;
    let spec =
        Spec::parse(&text).with_context(|| format!("could not read {}", spec_path.display()))?;

    match spec.version() {
        Some(version) if version.starts_with("3.0") || version.starts_with("3.1") => {}
        Some(version) => bail!(
            "Unsupported OpenAPI version {version:?} in {}. Only 3.0.x and 3.1.x are supported.",
            spec_path.display()
        ),
        None => bail!(
            "Unsupported OpenAPI version: {} has no `openapi` field. Only 3.0.x and 3.1.x are supported.",
            spec_path.display()
        ),
    }

    let root_path = std::path::absolute(output_dir)
        .with_context(|| format!("could not resolve {}", output_dir.display()))?;

    let mut env = BTreeMap::new();
    env.insert(BASE_URL.to_owned(), server_url(&spec));
    for (name, scheme) in security_schemes(&spec) {
        let Some(kind) = SchemeKind::of(scheme) else {
            continue;
        };
        for variable in kind.variables(name) {
            env.insert(variable, String::new());
        }
    }

    let mut nodes = vec![Node::new(Collection::new(&root_path))];
    let mut tag_nodes: BTreeMap<String, usize> = BTreeMap::new();
    let mut tag_directories: HashSet<String> = HashSet::new();

    for (path, path_item) in spec.get("paths").into_iter().flat_map(schema::entries) {
        let Some(path_item) = spec.resolve(path_item, PATH_ITEMS) else {
            continue;
        };
        let shared = parameter_list(&spec, path_item);
        for (key, operation) in schema::entries(path_item) {
            // Everything in a path item that is not a method — `parameters`,
            // `summary`, `servers` — is not an operation.
            let Some(method) = HttpMethod::parse(key) else {
                continue;
            };
            let parameters = merge_parameters(&shared, parameter_list(&spec, operation));
            let mut request = build_request(&spec, method, path, path_item, operation, &parameters);

            let index = match first_tag(operation) {
                Some(tag) => match tag_nodes.get(tag) {
                    Some(index) => *index,
                    None => {
                        let directory = tag_directory(tag, &mut tag_directories)?;
                        let mut collection = Collection::new(root_path.join(directory));
                        collection.name = tag.to_owned();
                        nodes.push(Node::new(collection));
                        let index = nodes.len() - 1;
                        tag_nodes.insert(tag.to_owned(), index);
                        index
                    }
                },
                None => 0,
            };

            let node = &mut nodes[index];
            let file_name = node.claim(file_name_for(&request.name, method, path));
            request.path = Some(node.collection.path.join(file_name));
            let at = node.collection.insertion_index(&request);
            node.collection.requests.insert(at, request);
        }
    }

    let mut nodes = nodes.into_iter();
    let mut collection = nodes
        .next()
        .expect("the root node is pushed before the walk")
        .collection;
    collection.children = nodes.map(|node| node.collection).collect();
    // Match the order a reload from disk would produce.
    collection.children.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(Imported { collection, env })
}

/// A collection under construction, with the request filenames already taken
/// in its directory.
struct Node {
    collection: Collection,
    used: HashSet<String>,
}

impl Node {
    fn new(collection: Collection) -> Self {
        Self {
            collection,
            used: HashSet::new(),
        }
    }

    /// Reserves `file_name`, appending `-2`, `-3`, … until it is free.
    ///
    /// `files::unique_file_name` cannot be used: nothing has been written yet,
    /// so the collisions are between requests of this same import.
    fn claim(&mut self, file_name: String) -> String {
        if self.used.insert(file_name.clone()) {
            return file_name;
        }
        let stem = file_name
            .strip_suffix(REQUEST_SUFFIX)
            .unwrap_or(&file_name)
            .to_owned();
        for counter in 2u32.. {
            let candidate = format!("{stem}-{counter}{REQUEST_SUFFIX}");
            if self.used.insert(candidate.clone()) {
                return candidate;
            }
        }
        unreachable!("the counter range is unbounded")
    }
}

fn build_request(
    spec: &Spec,
    method: HttpMethod,
    path: &str,
    path_item: &Value,
    operation: &Value,
    parameters: &[&Value],
) -> RequestModel {
    let name = schema::as_str(operation, "summary")
        .or_else(|| schema::as_str(operation, "operationId"))
        .map_or_else(|| format!("{method} {path}"), str::to_owned);

    let mut request = RequestModel {
        name,
        description: schema::as_str(operation, "description")
            .unwrap_or("")
            .to_owned(),
        method,
        url: request_url(path, path_item, operation),
        auth: auth_for(spec, operation),
        ..RequestModel::default()
    };

    for parameter in parameters {
        let Some(name) = schema::as_str(parameter, "name") else {
            continue;
        };
        // A deprecated parameter is still worth listing, just not sending.
        let enabled = !schema::flag(parameter, "deprecated");
        match schema::as_str(parameter, "in") {
            Some("query") => request.params.push(KeyValue {
                name: name.to_owned(),
                value: String::new(),
                enabled,
            }),
            Some("header") => request.headers.push(KeyValue {
                name: name.to_owned(),
                value: String::new(),
                enabled,
            }),
            Some("path") => request.path_params.push(PathParam {
                name: name.to_owned(),
                value: String::new(),
            }),
            // Cookies are session state in this client, not part of a request
            // template, and an unknown location has nowhere to go.
            _ => {}
        }
    }

    // A placeholder the document templates into the path but never declares
    // still needs a slot, or the URL can never be completed.
    for name in urls::path_param_names(&request.url) {
        if !request.path_params.iter().any(|param| param.name == name) {
            request.path_params.push(PathParam {
                name,
                value: String::new(),
            });
        }
    }

    request.body = request_body(spec, operation);
    request
}

fn request_body(spec: &Spec, operation: &Value) -> Option<BodyContent> {
    let body = operation.get("requestBody")?;
    let content = spec.resolve(body, REQUEST_BODIES)?.get("content")?;

    let json = schema::entries(content)
        .find(|(content_type, _)| content_type.eq_ignore_ascii_case("application/json"))
        .or_else(|| {
            schema::entries(content)
                .find(|(content_type, _)| has_json_structured_suffix(content_type))
        });
    if let Some((content_type, media_type)) = json {
        return Some(BodyContent::Raw {
            content: Examples::new(spec).media_type(media_type),
            content_type: Some(content_type.to_owned()),
        });
    }

    for content_type in [
        BodyContent::FORM_CONTENT_TYPE,
        BodyContent::MULTIPART_CONTENT_TYPE,
    ] {
        if let Some(media_type) = content.get(content_type) {
            return Some(BodyContent::Form {
                form_data: form_fields(spec, media_type),
                content_type: Some(content_type.to_owned()),
            });
        }
    }
    None
}

fn has_json_structured_suffix(media_type: &str) -> bool {
    let essence = media_type.split(';').next().unwrap_or(media_type).trim();
    let Some((_, subtype)) = essence.split_once('/') else {
        return false;
    };
    subtype
        .as_bytes()
        .get(subtype.len().saturating_sub(5)..)
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case(b"+json"))
}

fn form_fields(spec: &Spec, media_type: &Value) -> Vec<KeyValue> {
    let Some(properties) = media_type
        .get("schema")
        .and_then(|schema| spec.resolve(schema, SCHEMAS))
        .and_then(|schema| schema.get("properties"))
    else {
        return Vec::new();
    };
    schema::entries(properties)
        .map(|(name, _)| KeyValue::new(name, ""))
        .collect()
}

/// The parameters an operation or path item declares, with references
/// resolved. A reference that leads nowhere drops the parameter rather than
/// inventing a nameless one.
fn parameter_list<'a>(spec: &'a Spec, owner: &'a Value) -> Vec<&'a Value> {
    owner
        .get("parameters")
        .map_or(&[][..], schema::items)
        .iter()
        .filter_map(|parameter| spec.resolve(parameter, PARAMETERS))
        .collect()
}

/// Path-item parameters apply to every operation under it, and an operation
/// may override one by repeating its name and location.
fn merge_parameters<'a>(shared: &[&'a Value], own: Vec<&'a Value>) -> Vec<&'a Value> {
    let mut merged = shared.to_vec();
    for parameter in own {
        let identity = parameter_identity(parameter);
        match merged
            .iter()
            .position(|existing| parameter_identity(existing) == identity)
        {
            Some(index) => merged[index] = parameter,
            None => merged.push(parameter),
        }
    }
    merged
}

fn parameter_identity(parameter: &Value) -> (Option<&str>, Option<&str>) {
    (
        schema::as_str(parameter, "name"),
        schema::as_str(parameter, "in"),
    )
}

/// Rewrites OpenAPI's `{name}` path placeholders into the collection's `:name`
/// form. A malformed placeholder is left exactly as it was written.
fn path_template(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut rest = path;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}') else {
            break;
        };
        let name = &rest[open + 1..open + close];
        if name.is_empty() || name.contains(['{', '/']) {
            out.push_str(&rest[..=open]);
            rest = &rest[open + 1..];
            continue;
        }
        out.push_str(&rest[..open]);
        out.push(':');
        out.push_str(name);
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    out
}

fn first_tag(operation: &Value) -> Option<&str> {
    schema::items(operation.get("tags")?)
        .iter()
        .filter_map(Value::as_str)
        .find(|tag| !tag.is_empty())
}

/// A directory name for a tag, unique within the import.
fn tag_directory(tag: &str, used: &mut HashSet<String>) -> Result<String> {
    let stem = files::generate_file_stem(tag);
    if stem.is_empty() {
        bail!("the tag {tag:?} does not yield a usable directory name");
    }
    if used.insert(stem.clone()) {
        return Ok(stem);
    }
    for counter in 2u32.. {
        let candidate = format!("{stem}-{counter}");
        if used.insert(candidate.clone()) {
            return Ok(candidate);
        }
    }
    unreachable!("the counter range is unbounded")
}

fn file_name_for(name: &str, method: HttpMethod, path: &str) -> String {
    let stem = files::generate_file_stem(name);
    if stem.is_empty() {
        // A summary of punctuation alone leaves nothing to name the file
        // after; the method and path always do.
        return files::generate_file_name(&format!("{method} {path}"));
    }
    format!("{stem}{REQUEST_SUFFIX}")
}

fn request_url(path: &str, path_item: &Value, operation: &Value) -> String {
    let base = first_server_url(operation.get("servers"))
        .or_else(|| first_server_url(path_item.get("servers")))
        .unwrap_or_else(|| format!("${{{BASE_URL}}}"));
    format!("{base}{}", path_template(path))
}

/// The first root server's URL, with its `{variable}` placeholders replaced by
/// the declared defaults. A document without servers yields an empty value:
/// root requests still go through `${BASE_URL}`, and the user fills it in.
fn server_url(spec: &Spec) -> String {
    first_server_url(spec.get("servers")).unwrap_or_default()
}

fn first_server_url(servers: Option<&Value>) -> Option<String> {
    let server = servers.map(schema::items).and_then(<[Value]>::first)?;
    let mut url = schema::as_str(server, "url")?.to_owned();
    for (name, variable) in server
        .get("variables")
        .into_iter()
        .flat_map(schema::entries)
    {
        let Some(default) = variable.get("default").and_then(scalar_text) else {
            continue;
        };
        url = url.replace(&format!("{{{name}}}"), &default);
    }
    Some(url)
}

/// The text of a scalar, for the few places a document may spell a value as a
/// number or a boolean where a string is meant.
fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn security_schemes(spec: &Spec) -> impl Iterator<Item = (&str, &Value)> {
    spec.get("components")
        .and_then(|components| components.get("securitySchemes"))
        .into_iter()
        .flat_map(schema::entries)
}

/// The security schemes this client can fill in. Everything else — API keys,
/// OAuth flows, OpenID Connect — has no equivalent in the request model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SchemeKind {
    Basic,
    Bearer,
}

impl SchemeKind {
    fn of(scheme: &Value) -> Option<Self> {
        if schema::as_str(scheme, "type")? != "http" {
            return None;
        }
        let name = schema::as_str(scheme, "scheme")?;
        if name.eq_ignore_ascii_case("basic") {
            Some(SchemeKind::Basic)
        } else if name.eq_ignore_ascii_case("bearer") {
            Some(SchemeKind::Bearer)
        } else {
            None
        }
    }

    fn variables(self, name: &str) -> Vec<String> {
        let prefix = variable_prefix(name);
        match self {
            SchemeKind::Basic => vec![format!("{prefix}_USERNAME"), format!("{prefix}_PASSWORD")],
            SchemeKind::Bearer => vec![format!("{prefix}_BEARER_TOKEN")],
        }
    }

    fn auth(self, name: &str) -> Auth {
        let prefix = variable_prefix(name);
        match self {
            SchemeKind::Basic => Auth::basic(
                format!("${{{prefix}_USERNAME}}"),
                format!("${{{prefix}_PASSWORD}}"),
            ),
            SchemeKind::Bearer => Auth::bearer(format!("${{{prefix}_BEARER_TOKEN}}")),
        }
    }
}

/// A scheme name turned into the leading part of an environment variable.
/// Anything that cannot appear in a variable name becomes an underscore. A
/// leading digit gets an underscore prefix because core variable names are
/// shell identifiers. This keeps the generated `${…}` reference usable and in
/// step with the `.env` key.
fn variable_prefix(name: &str) -> String {
    let mut prefix = String::with_capacity(name.len() + 1);
    if matches!(name.as_bytes().first(), Some(first) if first.is_ascii_digit()) {
        prefix.push('_');
    }
    prefix.extend(name.chars().map(|character| {
        if character.is_ascii_alphanumeric() {
            character.to_ascii_uppercase()
        } else {
            '_'
        }
    }));
    prefix
}

/// The auth block for an operation: its own `security`, or the document-wide
/// default. The first requirement naming a scheme this client understands wins.
fn auth_for(spec: &Spec, operation: &Value) -> Option<Auth> {
    let requirements = operation.get("security").or_else(|| spec.get("security"))?;
    for requirement in schema::items(requirements) {
        for (name, _scopes) in schema::entries(requirement) {
            let kind = security_schemes(spec)
                .find(|(scheme_name, _)| *scheme_name == name)
                .and_then(|(_, scheme)| SchemeKind::of(scheme));
            if let Some(kind) = kind {
                return Some(kind.auth(name));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use rusting_core::AuthKind;

    use super::*;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A scratch directory for one test. `tempfile` is not a dependency of
    /// this crate and the import only needs somewhere to read a file from.
    struct Scratch {
        path: PathBuf,
    }

    impl Scratch {
        fn new() -> Self {
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("rusting-openapi-{}-{unique}", std::process::id()));
            std::fs::create_dir_all(&path).expect("the scratch directory is creatable");
            Self { path }
        }

        fn write(&self, file_name: &str, text: &str) -> PathBuf {
            let path = self.path.join(file_name);
            std::fs::write(&path, text).expect("the spec is writable");
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// Imports `text` as `spec.yaml`, with the collection rooted at
    /// `<scratch>/collection`.
    fn import_text(text: &str) -> Result<Imported> {
        let scratch = Scratch::new();
        let spec_path = scratch.write("spec.yaml", text);
        import(&spec_path, &scratch.path.join("collection"))
    }

    fn imported(text: &str) -> Imported {
        import_text(text).expect("the document imports")
    }

    fn file_names(collection: &Collection) -> Vec<String> {
        collection
            .requests
            .iter()
            .map(|request| {
                request
                    .path
                    .as_ref()
                    .expect("every imported request has a path")
                    .file_name()
                    .expect("the path ends in a file name")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    #[test]
    fn imports_a_3_1_document() {
        let result = imported(
            r#"
openapi: 3.1.0
info: { title: Test, version: "1.0" }
paths:
  /:
    get:
      parameters:
        - { name: page, in: query }
        - { name: account_id, in: header, deprecated: true }
      responses: { "200": { description: OK } }
      security:
        - bearerAuth: []
components:
  securitySchemes:
    bearerAuth: { type: http, scheme: bearer }
"#,
        );

        let collection = result.collection;
        assert_eq!(collection.requests.len(), 1);
        let request = &collection.requests[0];
        assert_eq!(request.url, "${BASE_URL}/");
        assert_eq!(request.method, HttpMethod::Get);
        assert_eq!(request.name, "GET /");

        assert_eq!(request.params, [KeyValue::new("page", "")]);
        assert_eq!(
            request.headers,
            [KeyValue {
                name: "account_id".to_owned(),
                value: String::new(),
                enabled: false,
            }]
        );

        let auth = request.auth.as_ref().expect("the operation is secured");
        assert_eq!(auth.kind, Some(AuthKind::BearerToken));
        assert_eq!(
            auth.bearer_token
                .as_ref()
                .map(|bearer| bearer.token.as_str()),
            Some("${BEARERAUTH_BEARER_TOKEN}")
        );

        assert_eq!(
            result
                .env
                .get("BEARERAUTH_BEARER_TOKEN")
                .map(String::as_str),
            Some("")
        );
    }

    #[test]
    fn imports_a_3_0_document_with_tags_and_a_referenced_schema() {
        let result = imported(
            r#"
openapi: 3.0.3
info: { title: Test 3.0, version: "1.0" }
paths:
  /pets:
    get:
      summary: List pets
      tags: [Pets]
      parameters:
        - { name: limit, in: query, schema: { type: integer } }
      security:
        - basicAuth: []
    post:
      summary: Create pet
      tags: [Pets]
      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: '#/components/schemas/Pet' }
  /health:
    get:
      summary: Health check
components:
  schemas:
    Pet:
      type: object
      properties:
        name: { type: string }
        tag: { type: string }
  securitySchemes:
    basicAuth: { type: http, scheme: basic }
"#,
        );

        let collection = result.collection;
        // /health carries no tag, so it stays at the root.
        assert_eq!(collection.requests.len(), 1);
        assert_eq!(collection.requests[0].url, "${BASE_URL}/health");

        assert_eq!(collection.children.len(), 1);
        let pets = &collection.children[0];
        assert_eq!(pets.name, "Pets");
        assert_eq!(pets.path, collection.path.join("pets"));
        assert_eq!(pets.requests.len(), 2);

        let get = &pets.requests[0];
        assert_eq!(get.method, HttpMethod::Get);
        assert_eq!(get.url, "${BASE_URL}/pets");
        assert_eq!(get.params, [KeyValue::new("limit", "")]);
        let auth = get.auth.as_ref().expect("the operation is secured");
        assert_eq!(auth.kind, Some(AuthKind::Basic));
        let basic = auth.basic.as_ref().expect("basic credentials are filled");
        assert_eq!(basic.username, "${BASICAUTH_USERNAME}");
        assert_eq!(basic.password, "${BASICAUTH_PASSWORD}");

        let post = &pets.requests[1];
        assert_eq!(post.method, HttpMethod::Post);
        assert_eq!(
            post.body,
            Some(BodyContent::Raw {
                content: "{\n  \"name\": \"\",\n  \"tag\": \"\"\n}".to_owned(),
                content_type: Some("application/json".to_owned()),
            })
        );

        assert_eq!(
            result.env.keys().map(String::as_str).collect::<Vec<_>>(),
            ["BASE_URL", "BASICAUTH_PASSWORD", "BASICAUTH_USERNAME"]
        );
    }

    #[test]
    fn rejects_an_unsupported_version() {
        let error = import_text("openapi: '2.0'\ninfo: { title: T, version: '1' }\npaths: {}\n")
            .expect_err("2.0 is not supported");
        assert!(
            format!("{error:#}").contains("Unsupported OpenAPI version"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn rejects_a_document_without_a_version() {
        let error =
            import_text("info: { title: T, version: '1' }\n").expect_err("the version is required");
        assert!(
            format!("{error:#}").contains("Unsupported OpenAPI version"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn resolves_parameter_references() {
        let result = imported(
            r#"
openapi: 3.0.3
info: { title: Parameter refs, version: "1.0" }
paths:
  /pets:
    get:
      parameters:
        - { $ref: '#/components/parameters/Limit' }
        - { $ref: '#/components/parameters/TraceId' }
        - { $ref: '#/components/parameters/Missing' }
components:
  parameters:
    Limit: { name: limit, in: query, schema: { type: integer } }
    TraceId: { name: X-Trace-Id, in: header, schema: { type: string } }
"#,
        );

        let request = &result.collection.requests[0];
        assert_eq!(
            request
                .params
                .iter()
                .map(|param| param.name.as_str())
                .collect::<Vec<_>>(),
            ["limit"]
        );
        assert_eq!(
            request
                .headers
                .iter()
                .map(|header| header.name.as_str())
                .collect::<Vec<_>>(),
            ["X-Trace-Id"]
        );
    }

    #[test]
    fn resolves_local_path_item_references_before_importing_operations() {
        let result = imported(
            r#"
openapi: 3.1.0
info: { title: Path item refs, version: "1.0" }
paths:
  /pets: { $ref: '#/components/pathItems/Pets' }
  /wrong: { $ref: '#/components/schemas/Pets' }
  /cycle: { $ref: '#/components/pathItems/CycleA' }
components:
  pathItems:
    Pets:
      parameters:
        - { name: trace, in: header }
      get: { summary: List pets }
    CycleA: { $ref: '#/components/pathItems/CycleB' }
    CycleB: { $ref: '#/components/pathItems/CycleA' }
  schemas:
    Pets:
      get: { summary: Not a path item component }
"#,
        );

        assert_eq!(result.collection.requests.len(), 1);
        let request = &result.collection.requests[0];
        assert_eq!(request.name, "List pets");
        assert_eq!(request.url, "${BASE_URL}/pets");
        assert_eq!(request.headers, [KeyValue::new("trace", "")]);
    }

    #[test]
    fn resolves_a_request_body_reference_into_a_schema_reference() {
        let result = imported(
            r#"
openapi: 3.0.3
info: { title: Request body refs, version: "1.0" }
paths:
  /pets:
    post:
      requestBody: { $ref: '#/components/requestBodies/PetBody' }
components:
  schemas:
    Pet:
      type: object
      properties:
        name: { type: string }
        age: { type: integer }
  requestBodies:
    PetBody:
      required: true
      content:
        application/json:
          schema: { $ref: '#/components/schemas/Pet' }
"#,
        );

        let request = &result.collection.requests[0];
        let Some(BodyContent::Raw { content, .. }) = &request.body else {
            panic!("expected a raw JSON body, got {:?}", request.body);
        };
        assert_eq!(content, "{\n  \"name\": \"\",\n  \"age\": 0\n}");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(content).expect("the body is valid JSON"),
            serde_json::json!({ "name": "", "age": 0 })
        );
    }

    #[test]
    fn imports_problem_and_vendor_json_media_types_as_raw_bodies() {
        let result = imported(
            r#"
openapi: 3.1.0
info: { title: JSON suffixes, version: "1.0" }
paths:
  /problem:
    post:
      summary: Problem
      requestBody:
        content:
          application/problem+json:
            schema: { type: object, properties: { detail: { type: string } } }
  /vendor:
    post:
      summary: Vendor
      requestBody:
        content:
          application/vnd.example+JSON:
            schema: { type: object, properties: { id: { type: integer } } }
  /unsupported:
    post:
      summary: Unsupported
      requestBody:
        content:
          text/plain:
            schema: { type: string }
"#,
        );

        let by_name: BTreeMap<&str, &RequestModel> = result
            .collection
            .requests
            .iter()
            .map(|request| (request.name.as_str(), request))
            .collect();
        assert_eq!(
            by_name["Problem"].body,
            Some(BodyContent::Raw {
                content: "{\n  \"detail\": \"\"\n}".to_owned(),
                content_type: Some("application/problem+json".to_owned()),
            })
        );
        assert_eq!(
            by_name["Vendor"].body,
            Some(BodyContent::Raw {
                content: "{\n  \"id\": 0\n}".to_owned(),
                content_type: Some("application/vnd.example+JSON".to_owned()),
            })
        );
        assert_eq!(by_name["Unsupported"].body, None);
    }

    #[test]
    fn a_recursive_schema_does_not_loop_forever() {
        let result = imported(
            r#"
openapi: 3.1.0
info: { title: Recursive, version: "1.0" }
paths:
  /nodes:
    post:
      requestBody:
        content:
          application/json:
            schema: { $ref: '#/components/schemas/Node' }
components:
  schemas:
    Node:
      type: object
      properties:
        name: { type: string }
        parent: { $ref: '#/components/schemas/Node' }
"#,
        );

        let Some(BodyContent::Raw { content, .. }) = &result.collection.requests[0].body else {
            panic!("expected a raw JSON body");
        };
        assert_eq!(content, "{\n  \"name\": \"\",\n  \"parent\": null\n}");
    }

    #[test]
    fn a_cyclic_parameter_reference_drops_the_parameter() {
        let result = imported(
            r#"
openapi: 3.1.0
info: { title: Cyclic parameters, version: "1.0" }
paths:
  /pets:
    get:
      parameters:
        - { $ref: '#/components/parameters/A' }
        - { name: ok, in: query }
components:
  parameters:
    A: { $ref: '#/components/parameters/B' }
    B: { $ref: '#/components/parameters/A' }
"#,
        );

        let request = &result.collection.requests[0];
        assert_eq!(request.params, [KeyValue::new("ok", "")]);
    }

    #[test]
    fn the_first_server_becomes_the_base_url() {
        let result = imported(
            r#"
openapi: 3.0.3
info: { title: No components, version: "1.0" }
servers:
  - url: https://{region}.api.example.com/{version}
    description: Production
    variables:
      region: { default: eu, enum: [eu, us] }
      version: { default: v2 }
  - url: https://staging.example.com
paths:
  /health:
    get:
      summary: Health check
"#,
        );

        assert_eq!(
            result.env.get("BASE_URL").map(String::as_str),
            Some("https://eu.api.example.com/v2")
        );
        assert_eq!(result.collection.requests[0].url, "${BASE_URL}/health");
    }

    #[test]
    fn operation_and_path_servers_override_the_root_with_declared_defaults() {
        let result = imported(
            r#"
openapi: 3.1.0
info: { title: Server precedence, version: "1.0" }
servers:
  - url: https://root.example.com
paths:
  /root:
    get: { summary: Root }
  /path:
    servers:
      - url: https://{region}.path.example.com/{version}
        variables:
          region: { default: eu }
          version: { default: v2 }
    get: { summary: Path }
  /operation:
    servers:
      - url: https://path.example.com
    get:
      summary: Operation
      servers:
        - url: https://{tenant}.operation.example.com
          variables:
            tenant: { default: acme }
"#,
        );

        let by_name: BTreeMap<&str, &RequestModel> = result
            .collection
            .requests
            .iter()
            .map(|request| (request.name.as_str(), request))
            .collect();
        assert_eq!(by_name["Root"].url, "${BASE_URL}/root");
        assert_eq!(by_name["Path"].url, "https://eu.path.example.com/v2/path");
        assert_eq!(
            by_name["Operation"].url,
            "https://acme.operation.example.com/operation"
        );
    }

    #[test]
    fn a_document_without_servers_still_declares_the_base_url() {
        let result = imported(
            "openapi: 3.1.0\ninfo: { title: T, version: '1' }\npaths:\n  /a:\n    get: {}\n",
        );
        assert_eq!(result.env.get("BASE_URL").map(String::as_str), Some(""));
    }

    #[test]
    fn path_placeholders_become_colon_parameters() {
        let result = imported(
            r#"
openapi: 3.1.0
info: { title: Paths, version: "1.0" }
paths:
  /pets/{petId}/toys/{toyId}:
    parameters:
      - { name: petId, in: path, required: true }
    get:
      summary: Get toy
"#,
        );

        let request = &result.collection.requests[0];
        assert_eq!(request.url, "${BASE_URL}/pets/:petId/toys/:toyId");
        assert_eq!(
            request
                .path_params
                .iter()
                .map(|param| param.name.as_str())
                .collect::<Vec<_>>(),
            ["petId", "toyId"]
        );
        assert!(
            request
                .path_params
                .iter()
                .all(|param| param.value.is_empty()),
            "path parameter values are left for the user"
        );
    }

    #[test]
    fn an_operation_parameter_overrides_the_path_item_parameter() {
        let result = imported(
            r#"
openapi: 3.1.0
info: { title: Overrides, version: "1.0" }
paths:
  /pets:
    parameters:
      - { name: limit, in: query }
      - { name: shared, in: query }
    get:
      parameters:
        - { name: limit, in: query, deprecated: true }
"#,
        );

        let request = &result.collection.requests[0];
        assert_eq!(
            request.params,
            [
                KeyValue {
                    name: "limit".to_owned(),
                    value: String::new(),
                    enabled: false,
                },
                KeyValue::new("shared", ""),
            ]
        );
    }

    #[test]
    fn names_fall_back_from_summary_to_operation_id_to_the_route() {
        let result = imported(
            r#"
openapi: 3.1.0
info: { title: Names, version: "1.0" }
paths:
  /a:
    get: { summary: Read A, operationId: readA }
  /b:
    get: { operationId: readB }
  /c:
    get: {}
"#,
        );

        let names: Vec<&str> = result
            .collection
            .requests
            .iter()
            .map(|request| request.name.as_str())
            .collect();
        assert_eq!(names, ["GET /c", "Read A", "readB"]);
        assert_eq!(
            file_names(&result.collection),
            [
                "get-c.posting.yaml",
                "read-a.posting.yaml",
                "readb.posting.yaml"
            ]
        );
    }

    #[test]
    fn colliding_names_are_numbered_per_directory() {
        let result = imported(
            r#"
openapi: 3.1.0
info: { title: Collisions, version: "1.0" }
paths:
  /a:
    get: { summary: Read it }
  /b:
    get: { summary: read it }
  /c:
    get: { summary: "Read  it!" }
  /d:
    get: { summary: Read it, tags: [Other] }
"#,
        );

        // Filenames are claimed in operation traversal order, then requests
        // are stored in Collection's `(method rank, name)` order.
        assert_eq!(
            file_names(&result.collection),
            [
                "read-it-3.posting.yaml",
                "read-it.posting.yaml",
                "read-it-2.posting.yaml"
            ]
        );
        // A different directory starts counting again.
        assert_eq!(
            file_names(&result.collection.children[0]),
            ["read-it.posting.yaml"]
        );
    }

    #[test]
    fn request_paths_sit_under_the_output_directory() {
        let scratch = Scratch::new();
        let spec_path = scratch.write(
            "spec.yaml",
            "openapi: 3.1.0\npaths:\n  /a:\n    get: { summary: Read A, tags: [Group] }\n",
        );
        let output = scratch.path.join("nested").join("collection");
        let result = import(&spec_path, &output).expect("the document imports");

        assert_eq!(result.collection.path, output);
        assert_eq!(
            result.collection.children[0].requests[0].path,
            Some(output.join("group").join("read-a.posting.yaml"))
        );
    }

    #[test]
    fn a_relative_output_directory_becomes_absolute() {
        let scratch = Scratch::new();
        let spec_path = scratch.write(
            "spec.yaml",
            "openapi: 3.1.0\npaths:\n  /a:\n    get: { summary: Read A }\n",
        );
        let result = import(&spec_path, Path::new("some/collection")).expect("it imports");
        assert!(
            result.collection.path.is_absolute(),
            "got {}",
            result.collection.path.display()
        );
        assert!(
            result.collection.requests[0]
                .path
                .as_ref()
                .is_some_and(|path| path.is_absolute())
        );
    }

    #[test]
    fn form_bodies_list_their_fields() {
        let result = imported(
            r#"
openapi: 3.1.0
info: { title: Forms, version: "1.0" }
paths:
  /login:
    post:
      requestBody:
        content:
          application/x-www-form-urlencoded:
            schema:
              type: object
              properties:
                username: { type: string }
                password: { type: string }
"#,
        );

        assert_eq!(
            result.collection.requests[0].body,
            Some(BodyContent::Form {
                form_data: vec![KeyValue::new("username", ""), KeyValue::new("password", "")],
                content_type: Some(BodyContent::FORM_CONTENT_TYPE.to_owned()),
            })
        );
    }

    #[test]
    fn multipart_bodies_are_imported_as_forms() {
        let result = imported(
            r#"
openapi: 3.1.0
info: { title: Multipart forms, version: "1.0" }
paths:
  /upload:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              type: object
              properties:
                caption: { type: string }
                file: { type: string, format: binary }
"#,
        );

        assert_eq!(
            result.collection.requests[0].body,
            Some(BodyContent::Form {
                form_data: vec![KeyValue::new("caption", ""), KeyValue::new("file", "")],
                content_type: Some(BodyContent::MULTIPART_CONTENT_TYPE.to_owned()),
            })
        );
    }

    #[test]
    fn document_wide_security_applies_when_an_operation_declares_none() {
        let result = imported(
            r#"
openapi: 3.1.0
info: { title: Security, version: "1.0" }
security:
  - apiKeyAuth: []
  - basicAuth: []
paths:
  /a:
    get: {}
  /b:
    get:
      security: []
components:
  securitySchemes:
    apiKeyAuth: { type: apiKey, name: X-Key, in: header }
    basicAuth: { type: http, scheme: basic }
"#,
        );

        let by_url: BTreeMap<&str, &RequestModel> = result
            .collection
            .requests
            .iter()
            .map(|request| (request.url.as_str(), request))
            .collect();
        // The API-key scheme has no equivalent, so the next requirement wins.
        assert_eq!(
            by_url["${BASE_URL}/a"]
                .auth
                .as_ref()
                .and_then(|auth| auth.kind),
            Some(AuthKind::Basic)
        );
        // An empty `security` on the operation opts out.
        assert_eq!(by_url["${BASE_URL}/b"].auth, None);
        assert!(result.env.contains_key("BASICAUTH_USERNAME"));
        assert!(!result.env.keys().any(|key| key.starts_with("APIKEYAUTH")));
    }

    #[test]
    fn security_scheme_names_produce_valid_variable_names() {
        assert_eq!(variable_prefix("client-auth"), "CLIENT_AUTH");
        assert_eq!(variable_prefix("123auth"), "_123AUTH");
    }

    #[test]
    fn tag_directories_are_distinct_even_when_the_tags_slug_alike() {
        let result = imported(
            r#"
openapi: 3.1.0
info: { title: Tags, version: "1.0" }
paths:
  /a:
    get: { summary: A, tags: ["Pet Store"] }
  /b:
    get: { summary: B, tags: ["pet-store"] }
"#,
        );

        let children = &result.collection.children;
        assert_eq!(children.len(), 2);
        let mut directories: Vec<&str> = children
            .iter()
            .map(|child| {
                child
                    .path
                    .file_name()
                    .expect("a tag directory has a name")
                    .to_str()
                    .expect("the name is UTF-8")
            })
            .collect();
        directories.sort_unstable();
        assert_eq!(directories, ["pet-store", "pet-store-2"]);
        let mut names: Vec<&str> = children.iter().map(|child| child.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["Pet Store", "pet-store"]);
    }

    #[test]
    fn json_documents_are_read_too() {
        let result =
            imported(r#"{"openapi": "3.0.3", "paths": {"/a": {"get": {"summary": "Read A"}}}}"#);
        assert_eq!(result.collection.requests[0].name, "Read A");
    }

    #[test]
    fn non_method_keys_in_a_path_item_are_not_operations() {
        let result = imported(
            r#"
openapi: 3.1.0
info: { title: Path items, version: "1.0" }
paths:
  /a:
    summary: A route
    description: Not an operation
    servers: [{ url: https://example.com }]
    parameters: [{ name: q, in: query }]
    get: {}
    trace: {}
"#,
        );
        assert_eq!(result.collection.requests.len(), 1);
        assert_eq!(result.collection.requests[0].method, HttpMethod::Get);
    }

    #[test]
    fn a_missing_spec_file_reports_the_path() {
        let error = import(Path::new("/nonexistent/spec.yaml"), Path::new("/tmp/out"))
            .expect_err("the file does not exist");
        assert!(
            format!("{error:#}").contains("/nonexistent/spec.yaml"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn path_templates_survive_odd_input() {
        assert_eq!(path_template("/pets/{id}"), "/pets/:id");
        assert_eq!(path_template("/pets/{}"), "/pets/{}");
        assert_eq!(path_template("/pets/{id"), "/pets/{id");
        assert_eq!(path_template("/a/{x}/b/{y}/c"), "/a/:x/b/:y/c");
        assert_eq!(path_template("/plain"), "/plain");
    }
}
