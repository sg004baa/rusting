//! Conversion between Rust request/response models and script-facing JSON values.

use std::collections::BTreeMap;

use rusting_core::{Auth, AuthKind, BodyContent, KeyValue, PathParam, RequestModel, Scripts};
use rusting_http::Response;
use serde_json::{Map, Value, json};

/// Convert a request into the complete plain-object shape exposed to JavaScript.
///
/// Unlike the on-disk serializer, this includes default-valued fields so scripts
/// can inspect and mutate arrays and option objects without first creating them.
pub fn request_to_value(request: &RequestModel) -> Value {
    json!({
        "name": request.name,
        "description": request.description,
        "method": request.method.as_str(),
        "url": request.url,
        "body": request.body.as_ref().map(body_to_value),
        "auth": request.auth.as_ref().map(auth_to_value),
        "headers": request.headers.iter().map(key_value_to_value).collect::<Vec<_>>(),
        "params": request.params.iter().map(key_value_to_value).collect::<Vec<_>>(),
        "path_params": request.path_params.iter().map(path_param_to_value).collect::<Vec<_>>(),
        "scripts": scripts_to_value(&request.scripts),
        "options": {
            "follow_redirects": request.options.follow_redirects,
            "verify_ssl": request.options.verify_ssl,
            "attach_cookies": request.options.attach_cookies,
            "proxy_url": request.options.proxy_url,
            "timeout": request.options.timeout,
        },
    })
}

/// Convert a script-mutated request object back into a complete model.
///
/// The conversion constructs a new model and restores the two session-only Rust
/// fields only after full deserialization succeeds. Callers can therefore leave
/// the original model untouched on every conversion error.
pub fn request_from_value(
    value: Value,
    original: &RequestModel,
) -> serde_json::Result<RequestModel> {
    let mut converted: RequestModel = serde_json::from_value(value)?;
    converted.path = original.path.clone();
    converted.cookies = original.cookies.clone();
    Ok(converted)
}

/// Convert a response into the script-facing read-only object shape.
/// Read-only enforcement belongs to the QuickJS boundary in [`crate::api`].
pub fn response_to_value(response: &Response) -> Value {
    let mut headers = BTreeMap::new();
    for header in &response.headers {
        headers.insert(header.name.to_ascii_lowercase(), header.value.clone());
    }

    json!({
        "status": response.status,
        "reason": response.reason,
        "url": response.url,
        "headers": headers,
        "body": response.text(),
        "elapsed_ms": response.timings.total.map(|elapsed| elapsed.as_secs_f64() * 1_000.0),
    })
}

fn key_value_to_value(item: &KeyValue) -> Value {
    json!({
        "name": item.name,
        "value": item.value,
        "enabled": item.enabled,
    })
}

fn path_param_to_value(item: &PathParam) -> Value {
    json!({
        "name": item.name,
        "value": item.value,
    })
}

fn body_to_value(body: &BodyContent) -> Value {
    match body {
        BodyContent::Raw {
            content,
            content_type,
        } => json!({
            "content": content,
            "content_type": content_type,
        }),
        BodyContent::Form {
            form_data,
            content_type,
        } => json!({
            "form_data": form_data.iter().map(key_value_to_value).collect::<Vec<_>>(),
            "content_type": content_type,
        }),
    }
}

fn auth_to_value(auth: &Auth) -> Value {
    let mut object = Map::new();
    object.insert(
        "type".into(),
        auth.kind
            .map(|kind| {
                Value::String(
                    match kind {
                        AuthKind::Basic => "basic",
                        AuthKind::Digest => "digest",
                        AuthKind::BearerToken => "bearer_token",
                    }
                    .into(),
                )
            })
            .unwrap_or(Value::Null),
    );
    object.insert(
        "basic".into(),
        auth.basic
            .as_ref()
            .map(|value| json!({ "username": value.username, "password": value.password }))
            .unwrap_or(Value::Null),
    );
    object.insert(
        "digest".into(),
        auth.digest
            .as_ref()
            .map(|value| json!({ "username": value.username, "password": value.password }))
            .unwrap_or(Value::Null),
    );
    object.insert(
        "bearer_token".into(),
        auth.bearer_token
            .as_ref()
            .map(|value| json!({ "token": value.token }))
            .unwrap_or(Value::Null),
    );
    Value::Object(object)
}

fn scripts_to_value(scripts: &Scripts) -> Value {
    json!({
        "setup": scripts.setup,
        "on_request": scripts.on_request,
        "on_response": scripts.on_response,
    })
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, time::Duration};

    use rusting_core::{Auth, BodyContent, HttpMethod, KeyValue};
    use rusting_http::{SentRequest, Timings};

    use super::*;

    #[test]
    fn request_round_trip_is_complete_and_preserves_session_fields() {
        let request = RequestModel {
            method: HttpMethod::Post,
            url: "https://example.test".into(),
            body: Some(BodyContent::Raw {
                content: "{}".into(),
                content_type: Some("application/json".into()),
            }),
            auth: Some(Auth::basic("user", "pass")),
            headers: vec![KeyValue::new("X-Test", "yes")],
            path: Some(PathBuf::from("saved.posting.yaml")),
            cookies: vec![KeyValue::new("session", "abc")],
            ..RequestModel::default()
        };

        let value = request_to_value(&request);
        assert_eq!(value["headers"][0]["enabled"], true);
        assert!(value["params"].is_array());
        assert_eq!(value["options"]["timeout"], 5.0);

        let converted = request_from_value(value, &request).expect("valid request object");
        assert_eq!(converted, request);
    }

    #[test]
    fn invalid_request_conversion_does_not_mutate_the_source() {
        let request = RequestModel {
            name: "original".into(),
            ..RequestModel::default()
        };
        let before = request.clone();
        let mut value = request_to_value(&request);
        value["name"] = json!(17);

        assert!(request_from_value(value, &request).is_err());
        assert_eq!(request, before);
    }

    #[test]
    fn response_shape_lowercases_headers_and_reports_elapsed_time() {
        let mut timings = Timings::default();
        timings.total = Some(Duration::from_millis(125));
        let response = Response {
            status: 201,
            reason: "Created".into(),
            url: "https://example.test/items/1".into(),
            headers: vec![KeyValue::new("Content-Type", "application/json")],
            cookies: Vec::new(),
            body: br#"{"id":1}"#.to_vec(),
            timings,
            sent: SentRequest::default(),
        };

        let value = response_to_value(&response);
        assert_eq!(value["headers"]["content-type"], "application/json");
        assert_eq!(value["body"], r#"{"id":1}"#);
        assert_eq!(value["elapsed_ms"], 125.0);
    }
}
