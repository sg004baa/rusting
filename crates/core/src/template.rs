//! Applying variable substitution to a request, immediately before it is sent.
//!
//! The order matters and is load-bearing: path-param *values* are resolved
//! first so that a variable can supply a placeholder's value, then the
//! placeholders are substituted into the URL, and only then is the scheme
//! inferred.

use crate::model::{Auth, BodyContent, RequestModel};
use crate::urls;
use crate::variables::{SubstitutionError, Variables, is_literal, substitute};

/// Which field failed, so the error can point at the right editor.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{field}: {source}")]
pub struct TemplateError {
    pub field: String,
    #[source]
    pub source: SubstitutionError,
}

/// Resolves every variable reference in `request`, in place.
///
/// On success the request is ready to send: the URL carries a scheme and no
/// placeholders remain for which a value was supplied.
pub fn apply(request: &mut RequestModel, variables: &Variables) -> Result<(), TemplateError> {
    for (index, param) in request.path_params.iter_mut().enumerate() {
        expand(&mut param.value, variables, || {
            format!("path param '{}'", param_label(index, &param.name))
        })?;
    }

    expand(&mut request.url, variables, || "url".to_owned())?;
    expand(&mut request.description, variables, || {
        "description".to_owned()
    })?;
    expand(&mut request.options.proxy_url, variables, || {
        "proxy url".to_owned()
    })?;

    match &mut request.body {
        Some(BodyContent::Raw { content, .. }) => {
            expand(content, variables, || "body".to_owned())?;
        }
        Some(BodyContent::Form { form_data, .. }) => {
            for (index, item) in form_data.iter_mut().enumerate() {
                expand(&mut item.name, variables, || format!("form name {index}"))?;
                expand(&mut item.value, variables, || format!("form value {index}"))?;
            }
        }
        None => {}
    }

    for (index, header) in request.headers.iter_mut().enumerate() {
        expand(&mut header.name, variables, || {
            format!("header name {index}")
        })?;
        expand(&mut header.value, variables, || {
            format!("header value {index}")
        })?;
    }
    for (index, param) in request.params.iter_mut().enumerate() {
        expand(&mut param.name, variables, || format!("query name {index}"))?;
        expand(&mut param.value, variables, || {
            format!("query value {index}")
        })?;
    }

    if let Some(auth) = request.auth.as_mut() {
        expand_auth(auth, variables)?;
    }

    request.url = urls::substitute_path_params(&request.url, request.path_params.as_slice());
    request.url = urls::ensure_protocol(&request.url);
    Ok(())
}

fn expand_auth(auth: &mut Auth, variables: &Variables) -> Result<(), TemplateError> {
    if let Some(basic) = auth.basic.as_mut() {
        expand(&mut basic.username, variables, || {
            "basic username".to_owned()
        })?;
        expand(&mut basic.password, variables, || {
            "basic password".to_owned()
        })?;
    }
    if let Some(digest) = auth.digest.as_mut() {
        expand(&mut digest.username, variables, || {
            "digest username".to_owned()
        })?;
        expand(&mut digest.password, variables, || {
            "digest password".to_owned()
        })?;
    }
    if let Some(bearer) = auth.bearer_token.as_mut() {
        expand(&mut bearer.token, variables, || "bearer token".to_owned())?;
    }
    Ok(())
}

fn expand(
    target: &mut String,
    variables: &Variables,
    field: impl FnOnce() -> String,
) -> Result<(), TemplateError> {
    if is_literal(target) {
        return Ok(());
    }
    match substitute(target, variables) {
        Ok(resolved) => {
            *target = resolved;
            Ok(())
        }
        Err(source) => Err(TemplateError {
            field: field(),
            source,
        }),
    }
}

fn param_label(index: usize, name: &str) -> String {
    if name.is_empty() {
        index.to_string()
    } else {
        name.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{KeyValue, PathParam};

    fn vars(pairs: &[(&str, &str)]) -> Variables {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn resolves_url_then_path_params_then_infers_scheme() {
        let mut request = RequestModel {
            url: "$HOST/posts/:id".into(),
            path_params: vec![PathParam {
                name: "id".into(),
                value: "$POST_ID".into(),
            }],
            ..Default::default()
        };
        apply(
            &mut request,
            &vars(&[("HOST", "example.com"), ("POST_ID", "7")]),
        )
        .unwrap();
        assert_eq!(request.url, "http://example.com/posts/7");
    }

    #[test]
    fn existing_scheme_is_kept() {
        let mut request = RequestModel {
            url: "https://example.com".into(),
            ..Default::default()
        };
        apply(&mut request, &vars(&[])).unwrap();
        assert_eq!(request.url, "https://example.com");
    }

    #[test]
    fn substitutes_headers_params_body_and_auth() {
        let mut request = RequestModel {
            url: "http://x".into(),
            headers: vec![KeyValue::new("X-$HK", "$HV")],
            params: vec![KeyValue::new("$PK", "$PV")],
            body: Some(BodyContent::Raw {
                content: "{\"t\": \"$TOKEN\"}".into(),
                content_type: None,
            }),
            auth: Some(Auth::basic("$USER", "$PASS")),
            ..Default::default()
        };
        apply(
            &mut request,
            &vars(&[
                ("HK", "H"),
                ("HV", "v"),
                ("PK", "p"),
                ("PV", "1"),
                ("TOKEN", "abc"),
                ("USER", "u"),
                ("PASS", "s"),
            ]),
        )
        .unwrap();
        assert_eq!(request.headers[0].name, "X-H");
        assert_eq!(request.headers[0].value, "v");
        assert_eq!(request.params[0].name, "p");
        let Some(BodyContent::Raw { ref content, .. }) = request.body else {
            panic!("expected a raw body");
        };
        assert_eq!(content, "{\"t\": \"abc\"}");
        assert_eq!(
            request
                .auth
                .as_ref()
                .unwrap()
                .basic
                .as_ref()
                .unwrap()
                .username,
            "u"
        );
    }

    #[test]
    fn an_undefined_variable_names_the_field() {
        let mut request = RequestModel {
            url: "http://x".into(),
            headers: vec![KeyValue::new("A", "$NOPE")],
            ..Default::default()
        };
        let error = apply(&mut request, &vars(&[])).unwrap_err();
        assert_eq!(error.field, "header value 0");
        assert_eq!(error.source, SubstitutionError::Undefined("NOPE".into()));
    }

    #[test]
    fn name_and_scripts_are_not_templated() {
        let mut request = RequestModel {
            name: "$NOT_A_VAR".into(),
            url: "http://x".into(),
            scripts: crate::model::Scripts {
                setup: Some("$NOT_A_VAR.js".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        apply(&mut request, &vars(&[])).unwrap();
        assert_eq!(request.name, "$NOT_A_VAR");
        assert_eq!(request.scripts.setup.as_deref(), Some("$NOT_A_VAR.js"));
    }

    #[test]
    fn unfilled_placeholder_survives_and_still_gets_a_scheme() {
        let mut request = RequestModel {
            url: "example.com/:missing".into(),
            ..Default::default()
        };
        apply(&mut request, &vars(&[])).unwrap();
        assert_eq!(request.url, "http://example.com/:missing");
    }
}
