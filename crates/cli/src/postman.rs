use anyhow::{Context as _, Result, bail};
use rusting_core::{
    collection::Collection,
    model::{Auth, AuthKind, BodyContent, KeyValue, RequestModel},
    urls,
    variables::{self, Variables},
};
use serde_json::{Value, json};

pub(crate) struct Export {
    pub document: Value,
    pub warnings: Vec<String>,
}

pub(crate) fn export(collection: &Collection) -> Result<Export> {
    let mut warnings = vec![
        "The export can contain literal credentials and other secrets. Review it before sharing."
            .to_owned(),
        "Rusting runtime options (timeout, proxy, SSL, redirects, and cookie behavior) and session cookies are not exported."
            .to_owned(),
    ];
    let items = collection_items(collection, &collection.name, &mut warnings)?;
    Ok(Export {
        document: json!({
            "info": {
                "name": collection.name,
                "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json",
            },
            "item": items,
        }),
        warnings,
    })
}

fn collection_items(
    collection: &Collection,
    context: &str,
    warnings: &mut Vec<String>,
) -> Result<Vec<Value>> {
    let mut items = Vec::with_capacity(collection.requests.len() + collection.children.len());
    for request in &collection.requests {
        let context = format!("{context}/{}", request.name);
        let item = request_item(request, &context, warnings)
            .with_context(|| format!("could not export request {context:?}"))?;
        items.push(item);
    }
    for child in &collection.children {
        let child_context = format!("{context}/{}", child.name);
        items.push(json!({
            "name": child.name,
            "item": collection_items(child, &child_context, warnings)?,
        }));
    }
    Ok(items)
}

fn request_item(
    request: &RequestModel,
    context: &str,
    warnings: &mut Vec<String>,
) -> Result<Value> {
    if !request.scripts.is_empty() {
        warnings.push(format!("Request {context:?}: script hooks are omitted."));
    }
    let mut headers: Vec<Value> = request.headers.iter().map(row).collect::<Result<_>>()?;
    let mut multipart = false;
    let body = match &request.body {
        None => None,
        Some(BodyContent::Raw { content, .. }) => Some(json!({
            "mode": "raw",
            "raw": template(content)?,
        })),
        Some(BodyContent::Form {
            form_data,
            content_type,
        }) => {
            let encoding = content_type
                .as_deref()
                .context(
                    "form body requires application/x-www-form-urlencoded or multipart/form-data",
                )?
                .split(';')
                .next()
                .unwrap_or_default()
                .trim();
            let mut fields: Vec<Value> = form_data.iter().map(row).collect::<Result<_>>()?;
            if encoding.eq_ignore_ascii_case(BodyContent::FORM_CONTENT_TYPE) {
                Some(json!({ "mode": "urlencoded", "urlencoded": fields }))
            } else if encoding.eq_ignore_ascii_case(BodyContent::MULTIPART_CONTENT_TYPE) {
                multipart = true;
                for field in &mut fields {
                    field["type"] = json!("text");
                }
                // Postman must generate its own boundary with the serialized form.
                headers.retain(|header| {
                    !header["key"]
                        .as_str()
                        .is_some_and(|name| name.eq_ignore_ascii_case("content-type"))
                });
                if request
                    .headers
                    .iter()
                    .any(|header| header.name.eq_ignore_ascii_case("content-type"))
                {
                    warnings.push(format!(
                        "Request {context:?}: explicit Content-Type headers are omitted so Postman can generate a multipart boundary."
                    ));
                }
                Some(json!({ "mode": "formdata", "formdata": fields }))
            } else {
                bail!(
                    "unsupported form encoding; expected application/x-www-form-urlencoded or multipart/form-data"
                );
            }
        }
    };
    if !multipart
        && !request.has_explicit_content_type()
        && let Some(content_type) = request.body.as_ref().and_then(BodyContent::content_type)
    {
        headers.push(json!({
            "key": "Content-Type",
            "value": template(content_type)?,
            "disabled": false,
        }));
    }
    let mut result = json!({
        "name": request.name,
        "request": {
            "method": request.method.as_str(),
            "description": request.description,
            "url": request_url(request, context, warnings)?,
            "header": headers,
            "auth": authentication(request.auth.as_ref())?,
        },
    });
    if let Some(body) = body {
        result["request"]["body"] = body;
    }
    Ok(result)
}

fn template(text: &str) -> Result<String> {
    if variables::is_literal(text) {
        return Ok(text.to_owned());
    }
    let mut references = Variables::new();
    for token in variables::find_variables(text) {
        references
            .entry(token.name)
            .or_insert_with_key(|name| format!("{{{{{name}}}}}"));
    }
    variables::substitute(text, &references).context("invalid variable reference")
}

fn row(row: &KeyValue) -> Result<Value> {
    Ok(json!({
        "key": template(&row.name)?,
        "value": template(&row.value)?,
        "disabled": !row.enabled,
    }))
}

fn authentication(auth: Option<&Auth>) -> Result<Value> {
    let Some(auth) = auth else {
        return Ok(json!({ "type": "noauth" }));
    };
    let (kind, credentials) = match auth.kind {
        None => return Ok(json!({ "type": "noauth" })),
        Some(AuthKind::Basic) => (
            "basic",
            auth.basic
                .as_ref()
                .context("basic auth is selected but has no credentials")?,
        ),
        Some(AuthKind::Digest) => (
            "digest",
            auth.digest
                .as_ref()
                .context("digest auth is selected but has no credentials")?,
        ),
        Some(AuthKind::BearerToken) => {
            let bearer = auth
                .bearer_token
                .as_ref()
                .context("bearer auth is selected but has no token")?;
            return Ok(json!({
                "type": "bearer",
                "bearer": [{ "key": "token", "value": template(&bearer.token)?, "type": "string" }],
            }));
        }
    };
    let mut value = json!({ "type": kind });
    value[kind] = json!([
        { "key": "username", "value": template(&credentials.username)?, "type": "string" },
        { "key": "password", "value": template(&credentials.password)?, "type": "string" },
    ]);
    Ok(value)
}

struct QueryParam {
    key: String,
    value: Option<String>,
    disabled: bool,
}

fn request_url(request: &RequestModel, context: &str, warnings: &mut Vec<String>) -> Result<Value> {
    let (without_fragment, fragment) = request
        .url
        .split_once('#')
        .map_or((request.url.as_str(), None), |(url, fragment)| {
            (url, Some(fragment))
        });
    let (base, inline_query) = without_fragment
        .split_once('?')
        .map_or((without_fragment, None), |(url, query)| (url, Some(query)));
    let mut query = Vec::with_capacity(request.params.len());
    if let Some(inline_query) = inline_query {
        for part in inline_query.split('&') {
            let (key, value) = part
                .split_once('=')
                .map_or((part, None), |(key, value)| (key, Some(value)));
            query.push(QueryParam {
                key: template(key)?,
                value: value.map(template).transpose()?,
                disabled: false,
            });
        }
    }
    if request.params.iter().any(|param| {
        !variables::find_variables(&param.name).is_empty()
            || !variables::find_variables(&param.value).is_empty()
    }) {
        warnings.push(format!(
            "Request {context:?}: Postman query variables may need URL-encoded values; use separate encoded variables for query fields when shared elsewhere."
        ));
    }
    for param in &request.params {
        query.push(QueryParam {
            key: encoded_query_component(&template(&param.name)?),
            value: Some(encoded_query_component(&template(&param.value)?)),
            disabled: !param.enabled,
        });
    }

    let path_tokens = urls::find_path_params(base);
    let mut variable = Vec::with_capacity(path_tokens.len());
    for (index, token) in path_tokens.iter().enumerate() {
        if path_tokens[..index]
            .iter()
            .any(|previous| previous.name == token.name)
        {
            continue;
        }
        let value = request
            .path_params
            .iter()
            .find(|param| param.name == token.name)
            .map_or("", |param| param.value.as_str());
        variable.push(json!({ "key": token.name, "value": template(value)? }));
    }
    let after_scheme = base.find("://").map_or(0, |index| index + 3);
    let path_start = base[after_scheme..]
        .find('/')
        .map_or(base.len(), |index| after_scheme + index);
    let mut path_url = String::with_capacity(base.len());
    path_url.push_str(&base[..path_start]);
    let mut cursor = path_start;
    for token in path_tokens {
        push_path_literal(&mut path_url, &base[cursor..token.start]);
        let segment_start = token.start > 0 && base.as_bytes()[token.start - 1] == b'/';
        let segment_end = matches!(base.as_bytes().get(token.end), None | Some(b'/' | b'.'));
        let configured = request
            .path_params
            .iter()
            .find(|param| param.name == token.name);
        if segment_start && segment_end && configured.is_none_or(|param| !param.value.is_empty()) {
            path_url.push_str(&base[token.start..token.end]);
        } else if let Some(param) = configured {
            path_url.push_str(&param.value);
        } else {
            path_url.push_str(&base[token.start..token.end]);
            warnings.push(format!(
                "Request {context:?}: an embedded path placeholder has no configured value and may require editing in Postman."
            ));
        }
        cursor = token.end;
    }
    push_path_literal(&mut path_url, &base[cursor..]);
    let path_url = template(&path_url)?;
    let leading_variable = variables::find_variables(base)
        .first()
        .is_some_and(|token| {
            token.start == 0 && matches!(base.as_bytes().get(token.end), None | Some(b'/'))
        });
    let base = if path_url.contains("://") || leading_variable {
        path_url
    } else {
        urls::ensure_protocol(&path_url)
    };
    let (protocol, authority_path) = base
        .split_once("://")
        .map_or((None, base.as_str()), |(protocol, rest)| {
            (Some(protocol), rest)
        });
    let (authority, path) = authority_path
        .split_once('/')
        .map_or((authority_path, ""), |(authority, _)| {
            (authority, &authority_path[authority.len()..])
        });
    let mut result = json!({
        "path": path,
        "variable": variable,
    });
    if let Some(protocol) = protocol {
        result["protocol"] = json!(protocol);
    }
    // Splitting the port after the IPv6 closing bracket also preserves templated ports.
    let host_start = authority.rfind('@').map_or(0, |index| index + 1);
    let host = &authority[host_start..];
    let port = host
        .rsplit_once(':')
        .filter(|(host, _)| !host.contains('[') || host.ends_with(']'));
    if let Some((host, port)) = port {
        result["host"] = json!(host);
        result["port"] = json!(port);
    } else {
        result["host"] = json!(host);
    }
    if host_start != 0 {
        let user_info = &authority[..host_start - 1];
        let (user, password) = user_info
            .split_once(':')
            .map_or((user_info, None), |(user, password)| (user, Some(password)));
        let mut auth = json!({ "user": user });
        if let Some(password) = password {
            auth["password"] = json!(password);
        }
        result["auth"] = auth;
    }

    let mut raw = base;
    let mut enabled = query.iter().filter(|param| !param.disabled).peekable();
    if enabled.peek().is_some() {
        raw.push('?');
        for (index, param) in enabled.enumerate() {
            if index != 0 {
                raw.push('&');
            }
            raw.push_str(&param.key);
            if let Some(value) = &param.value {
                raw.push('=');
                raw.push_str(value);
            }
        }
    }
    if let Some(fragment) = fragment {
        let fragment = template(fragment)?;
        raw.push('#');
        raw.push_str(&fragment);
        result["hash"] = json!(fragment);
    }
    result["raw"] = json!(raw);
    result["query"] = json!(
        query
            .into_iter()
            .map(|param| json!({
                "key": param.key,
                "value": param.value,
                "disabled": param.disabled,
            }))
            .collect::<Vec<_>>()
    );
    Ok(result)
}

fn push_path_literal(output: &mut String, text: &str) {
    for (index, part) in text.split("::").enumerate() {
        if index != 0 {
            output.push_str("%3A");
        }
        output.push_str(part);
    }
}

fn encoded_query_component(mut text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    while let Some(start) = text.find("{{") {
        let Some(end) = text[start + 2..].find("}}").map(|end| start + 2 + end + 2) else {
            break;
        };
        encoded.extend(url::form_urlencoded::byte_serialize(
            &text.as_bytes()[..start],
        ));
        encoded.push_str(&text[start..end]);
        text = &text[end..];
    }
    encoded.extend(url::form_urlencoded::byte_serialize(text.as_bytes()));
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusting_core::model::{BearerTokenAuth, HttpMethod, PathParam, Scripts, UserPassAuth};

    fn one(request: RequestModel) -> Result<Export> {
        export(&Collection {
            name: "API".to_owned(),
            requests: vec![request],
            ..Collection::default()
        })
    }

    fn request() -> RequestModel {
        RequestModel {
            name: "Example".to_owned(),
            url: "https://example.test".to_owned(),
            ..RequestModel::default()
        }
    }

    #[test]
    fn preserves_collection_hierarchy_names_descriptions_and_methods() {
        let collection = Collection {
            name: "API".to_owned(),
            children: vec![Collection {
                name: "Users".to_owned(),
                children: vec![Collection {
                    name: "Admin".to_owned(),
                    requests: vec![RequestModel {
                        description: "Find an administrator".to_owned(),
                        method: HttpMethod::Patch,
                        ..request()
                    }],
                    ..Collection::default()
                }],
                ..Collection::default()
            }],
            ..Collection::default()
        };
        let exported = export(&collection).unwrap();
        assert_eq!(exported.document["info"]["name"], "API");
        assert_eq!(exported.document["item"][0]["name"], "Users");
        assert_eq!(exported.document["item"][0]["item"][0]["name"], "Admin");
        let item = &exported.document["item"][0]["item"][0]["item"][0];
        assert_eq!(item["name"], "Example");
        assert_eq!(item["request"]["description"], "Find an administrator");
        assert_eq!(item["request"]["method"], "PATCH");
        assert_eq!(
            export(&Collection::default()).unwrap().document["item"],
            json!([])
        );
    }

    #[test]
    fn exports_only_active_auth_credentials_and_explicit_noauth() {
        for kind in [
            None,
            Some(AuthKind::Basic),
            Some(AuthKind::Digest),
            Some(AuthKind::BearerToken),
        ] {
            let auth = Auth {
                kind,
                basic: Some(UserPassAuth {
                    username: "basic-user".into(),
                    password: "$BASIC_PASSWORD".into(),
                }),
                digest: Some(UserPassAuth {
                    username: "digest-user".into(),
                    password: "literal-digest-password".into(),
                }),
                bearer_token: Some(BearerTokenAuth {
                    token: "${TOKEN}".into(),
                }),
            };
            let exported = one(RequestModel {
                auth: Some(auth),
                ..request()
            })
            .unwrap();
            let result = &exported.document["item"][0]["request"]["auth"];
            let serialized = result.to_string();
            match kind {
                None => assert_eq!(result, &json!({ "type": "noauth" })),
                Some(AuthKind::Basic) => {
                    assert_eq!(result["type"], "basic");
                    assert_eq!(result["basic"][0]["value"], "basic-user");
                    assert_eq!(result["basic"][1]["value"], "{{BASIC_PASSWORD}}");
                }
                Some(AuthKind::Digest) => {
                    assert_eq!(result["type"], "digest");
                    assert_eq!(result["digest"][1]["value"], "literal-digest-password");
                }
                Some(AuthKind::BearerToken) => {
                    assert_eq!(result["type"], "bearer");
                    assert_eq!(result["bearer"][0]["value"], "{{TOKEN}}");
                }
            }
            assert_eq!(
                serialized.contains("basic-user"),
                kind == Some(AuthKind::Basic)
            );
            assert_eq!(
                serialized.contains("literal-digest-password"),
                kind == Some(AuthKind::Digest)
            );
            assert_eq!(
                serialized.contains("{{TOKEN}}"),
                kind == Some(AuthKind::BearerToken)
            );
            assert!(
                !exported
                    .warnings
                    .join("\n")
                    .contains("literal-digest-password")
            );
        }
        let exported = one(request()).unwrap();
        assert_eq!(
            exported.document["item"][0]["request"]["auth"]["type"],
            "noauth"
        );
        for kind in [AuthKind::Basic, AuthKind::Digest, AuthKind::BearerToken] {
            assert!(
                one(RequestModel {
                    auth: Some(Auth {
                        kind: Some(kind),
                        ..Auth::default()
                    }),
                    ..request()
                })
                .is_err()
            );
        }
    }

    #[test]
    fn exports_raw_body_and_respects_enabled_explicit_content_type() {
        let exported = one(RequestModel {
            body: Some(BodyContent::Raw {
                content: "日本語 $BODY ${BODY} $$".into(),
                content_type: Some("application/json".into()),
            }),
            headers: vec![
                KeyValue::new("content-TYPE", "text/plain"),
                KeyValue {
                    name: "X-Token".into(),
                    value: "$TOKEN".into(),
                    enabled: false,
                },
            ],
            ..request()
        })
        .unwrap();
        let converted = &exported.document["item"][0]["request"];
        assert_eq!(converted["body"]["mode"], "raw");
        assert_eq!(converted["body"]["raw"], "日本語 {{BODY}} {{BODY}} $");
        assert_eq!(converted["header"].as_array().unwrap().len(), 2);
        assert_eq!(converted["header"][0]["value"], "text/plain");
        assert_eq!(
            converted["header"][1],
            json!({ "key": "X-Token", "value": "{{TOKEN}}", "disabled": true })
        );
        let exported = one(RequestModel {
            headers: vec![KeyValue {
                name: "Content-Type".into(),
                value: "text/plain".into(),
                enabled: false,
            }],
            body: Some(BodyContent::Raw {
                content: "".into(),
                content_type: Some("application/json".into()),
            }),
            ..request()
        })
        .unwrap();
        let headers = &exported.document["item"][0]["request"]["header"];
        assert_eq!(headers[0]["disabled"], true);
        assert_eq!(headers[1]["value"], "application/json");
        assert_eq!(headers[1]["disabled"], false);
    }

    #[test]
    fn maps_both_form_encodings_and_rejects_missing_or_unsupported_encoding() {
        for (content_type, mode) in [
            (
                "APPLICATION/X-WWW-FORM-URLENCODED; charset=UTF-8",
                "urlencoded",
            ),
            ("Multipart/Form-Data; boundary=obsolete", "formdata"),
        ] {
            let exported = one(RequestModel {
                body: Some(BodyContent::Form {
                    form_data: vec![
                        KeyValue::new("$FIELD", "${VALUE}"),
                        KeyValue {
                            name: "off".into(),
                            value: "stored".into(),
                            enabled: false,
                        },
                    ],
                    content_type: Some(content_type.into()),
                }),
                headers: vec![KeyValue::new(
                    "Content-Type",
                    "multipart/form-data; boundary=do-not-copy",
                )],
                ..request()
            })
            .unwrap();
            let request = &exported.document["item"][0]["request"];
            assert_eq!(request["body"]["mode"], mode);
            assert_eq!(request["body"][mode][0]["key"], "{{FIELD}}");
            assert_eq!(request["body"][mode][0]["value"], "{{VALUE}}");
            assert_eq!(request["body"][mode][1]["disabled"], true);
            if mode == "formdata" {
                assert_eq!(request["body"][mode][0]["type"], "text");
                assert_eq!(request["header"], json!([]));
                assert!(
                    exported
                        .warnings
                        .iter()
                        .any(|warning| warning.contains("API/Example")
                            && warning.contains("boundary"))
                );
                assert!(!exported.warnings.join("\n").contains("do-not-copy"));
            } else {
                assert_eq!(
                    request["header"][0]["value"],
                    "multipart/form-data; boundary=do-not-copy"
                );
            }
        }
        for content_type in [None, Some("text/plain".into())] {
            assert!(
                one(RequestModel {
                    body: Some(BodyContent::Form {
                        form_data: vec![],
                        content_type
                    }),
                    ..request()
                })
                .is_err()
            );
        }
        let exported = one(RequestModel {
            body: Some(BodyContent::Form {
                form_data: vec![],
                content_type: Some(BodyContent::FORM_CONTENT_TYPE.into()),
            }),
            ..request()
        })
        .unwrap();
        assert_eq!(
            exported.document["item"][0]["request"]["header"][0]["value"],
            BodyContent::FORM_CONTENT_TYPE
        );
    }

    #[test]
    fn composes_inline_and_explicit_queries_without_enabling_disabled_duplicates() {
        let exported = one(RequestModel {
            url: "https://example.test:8443/users/:id?q=one&q=${SEARCH}&flag&encoded=a%26b#${FRAGMENT}".into(),
            params: vec![KeyValue::new("q", "$SEARCH"), KeyValue::new("a&b", "two words & more"), KeyValue { name: "q".into(), value: "disabled".into(), enabled: false }],
            path_params: vec![PathParam { name: "id".into(), value: "${USER_ID}".into() }],
            ..request()
        }).unwrap();
        let url = &exported.document["item"][0]["request"]["url"];
        assert_eq!(
            url["raw"],
            "https://example.test:8443/users/:id?q=one&q={{SEARCH}}&flag&encoded=a%26b&q={{SEARCH}}&a%26b=two+words+%26+more#{{FRAGMENT}}"
        );
        assert_eq!(url["protocol"], "https");
        assert_eq!(url["host"], "example.test");
        assert_eq!(url["port"], "8443");
        assert_eq!(url["path"], "/users/:id");
        assert_eq!(url["hash"], "{{FRAGMENT}}");
        assert_eq!(
            url["variable"],
            json!([{ "key": "id", "value": "{{USER_ID}}" }])
        );
        assert_eq!(url["query"].as_array().unwrap().len(), 7);
        assert_eq!(url["query"][2]["value"], Value::Null);
        assert_eq!(url["query"][6]["disabled"], true);
        assert_eq!(url["query"][6]["value"], "disabled");
        assert!(exported.warnings.iter().any(|warning| {
            warning.contains("API/Example") && warning.contains("query variables")
        }));
        assert!(!exported.warnings.join("\n").contains("two words & more"));
    }

    #[test]
    fn preserves_base_variables_path_escapes_and_embedded_values() {
        let exported = one(RequestModel {
            url: "${BASE_URL}/::id/:id/prefix-:id/:id.json".into(),
            path_params: vec![PathParam {
                name: "id".into(),
                value: "${ID}".into(),
            }],
            ..request()
        })
        .unwrap();
        let url = &exported.document["item"][0]["request"]["url"];
        assert_eq!(url["raw"], "{{BASE_URL}}/%3Aid/:id/prefix-{{ID}}/:id.json");
        assert_eq!(url["protocol"], Value::Null);
        assert_eq!(url["host"], "{{BASE_URL}}");
        assert_eq!(url["path"], "/%3Aid/:id/prefix-{{ID}}/:id.json");
        assert_eq!(url["variable"], json!([{ "key": "id", "value": "{{ID}}" }]));
        let exported = one(RequestModel {
            url: "localhost:8000/path".into(),
            ..request()
        })
        .unwrap();
        assert_eq!(
            exported.document["item"][0]["request"]["url"]["raw"],
            "http://localhost:8000/path"
        );
        let exported = one(RequestModel {
            url: "http://[::1]:8000/::id".into(),
            ..request()
        })
        .unwrap();
        assert_eq!(
            exported.document["item"][0]["request"]["url"]["raw"],
            "http://[::1]:8000/%3Aid"
        );
        let exported = one(RequestModel {
            url: "https://example.test/:id/end".into(),
            path_params: vec![PathParam {
                name: "id".into(),
                value: String::new(),
            }],
            ..request()
        })
        .unwrap();
        assert_eq!(
            exported.document["item"][0]["request"]["url"]["raw"],
            "https://example.test//end"
        );
    }

    #[test]
    fn preserves_variable_schemes_hosts_and_host_only_ports() {
        for (source, protocol, host, port, raw) in [
            (
                "${SCHEME}://example.test/items",
                "{{SCHEME}}",
                "example.test",
                None,
                "{{SCHEME}}://example.test/items",
            ),
            (
                "https://${HOST}/items",
                "https",
                "{{HOST}}",
                None,
                "https://{{HOST}}/items",
            ),
            (
                "${HOST}:8080/items",
                "http",
                "{{HOST}}",
                Some("8080"),
                "http://{{HOST}}:8080/items",
            ),
        ] {
            let exported = one(RequestModel {
                url: source.into(),
                ..request()
            })
            .unwrap();
            let url = &exported.document["item"][0]["request"]["url"];
            assert_eq!(url["protocol"], protocol);
            assert_eq!(url["host"], host);
            assert_eq!(url["port"], json!(port));
            assert_eq!(url["raw"], raw);
        }
    }

    #[test]
    fn converts_variable_escapes_unicode_and_repeated_tokens_without_resolving_values() {
        let exported = one(RequestModel {
            body: Some(BodyContent::Raw {
                content: "日本語 $$NAME $NAME ${NAME} $$$NAME $$$$ $_x1".into(),
                content_type: None,
            }),
            ..request()
        })
        .unwrap();
        assert_eq!(
            exported.document["item"][0]["request"]["body"]["raw"],
            "日本語 $NAME {{NAME}} {{NAME}} ${{NAME}} $$ {{_x1}}"
        );
        for invalid in ["$", "${NAME", "${日本語}", "$1"] {
            let error = one(RequestModel {
                body: Some(BodyContent::Raw {
                    content: invalid.into(),
                    content_type: None,
                }),
                ..request()
            })
            .err()
            .unwrap();
            assert!(format!("{error:#}").contains("invalid variable reference"));
        }
    }

    #[test]
    fn warns_about_hooks_without_exporting_scripts_or_session_state() {
        let exported = one(RequestModel {
            scripts: Scripts {
                setup: Some("secret-script.js".into()),
                ..Scripts::default()
            },
            cookies: vec![KeyValue::new("session", "secret-cookie")],
            ..request()
        })
        .unwrap();
        let warnings = exported.warnings.join("\n");
        assert!(warnings.contains("API/Example"));
        assert!(warnings.contains("script hooks are omitted"));
        assert!(warnings.contains("runtime options"));
        assert!(warnings.contains("session cookies"));
        assert!(!warnings.contains("secret-script"));
        assert!(!exported.document.to_string().contains("secret-script"));
        assert!(!exported.document.to_string().contains("secret-cookie"));
        assert_eq!(exported.document["item"][0]["event"], Value::Null);
    }
}
