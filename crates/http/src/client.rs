//! Per-request `reqwest` client construction.

use std::fs;
use std::time::Duration;

use reqwest::{Certificate, Client, Identity, Proxy, redirect};
use rusting_core::{RequestModel, config::SslSettings};

use crate::SendError;

/// Builds a client for one request.
///
/// Redirect, verification, proxy, and timeout settings are request-scoped, so
/// callers must not cache the returned client across sends.
pub(crate) fn build(request: &RequestModel, settings: &SslSettings) -> Result<Client, SendError> {
    let timeout = validated_timeout(request.options.timeout)?;
    let mut builder =
        Client::builder()
            .timeout(timeout)
            .redirect(if request.options.follow_redirects {
                redirect::Policy::limited(10)
            } else {
                redirect::Policy::none()
            });

    if !request.options.verify_ssl {
        builder = builder.danger_accept_invalid_certs(true);
    } else if let Some(path) = &settings.ca_bundle {
        let pem = read_tls_file(path, "CA bundle")?;
        let certificates = Certificate::from_pem_bundle(&pem).map_err(|error| {
            SendError::Certificate(format!(
                "CA bundle {} is not valid PEM: {error}",
                path.display()
            ))
        })?;
        if certificates.is_empty() {
            return Err(SendError::Certificate(format!(
                "CA bundle {} contains no certificates",
                path.display()
            )));
        }
        for certificate in certificates {
            builder = builder.add_root_certificate(certificate);
        }
    }

    match (&settings.certificate_path, &settings.key_file) {
        (Some(certificate_path), Some(key_path)) => {
            let certificate = read_tls_file(certificate_path, "client certificate")?;
            let key = read_tls_file(key_path, "client private key")?;
            reject_encrypted_key(key_path, &key)?;

            let mut identity_pem = Vec::with_capacity(certificate.len() + key.len() + 1);
            identity_pem.extend_from_slice(&certificate);
            if !certificate.ends_with(b"\n") {
                identity_pem.push(b'\n');
            }
            identity_pem.extend_from_slice(&key);
            let identity = Identity::from_pem(&identity_pem).map_err(|error| {
                SendError::Certificate(format!(
                    "client certificate {} and private key {} are not a valid PEM identity: {error}",
                    certificate_path.display(),
                    key_path.display()
                ))
            })?;
            builder = builder.identity(identity);
        }
        (Some(path), None) => {
            return Err(SendError::Certificate(format!(
                "client certificate {} was configured without a private key",
                path.display()
            )));
        }
        (None, Some(path)) => {
            return Err(SendError::Certificate(format!(
                "client private key {} was configured without a certificate",
                path.display()
            )));
        }
        (None, None) => {}
    }

    if !request.options.proxy_url.is_empty() {
        let proxy = Proxy::all(&request.options.proxy_url)
            .map_err(|error| SendError::Proxy(error.to_string()))?;
        builder = builder.proxy(proxy);
    }

    builder.build().map_err(|error| {
        let message = error.to_string();
        if looks_like_tls_error(&message) {
            SendError::Tls(message)
        } else {
            SendError::Other(message)
        }
    })
}

pub(crate) fn validated_timeout(seconds: f64) -> Result<Duration, SendError> {
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err(SendError::InvalidRequest(
            "timeout must be a finite number greater than zero".into(),
        ));
    }
    Duration::try_from_secs_f64(seconds).map_err(|error| {
        SendError::InvalidRequest(format!("timeout {seconds} is out of range: {error}"))
    })
}

pub(crate) fn looks_like_tls_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "certificate",
        "tls",
        "ssl",
        "handshake",
        "rustls",
        "unknown issuer",
        "invalid peer",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

fn read_tls_file(path: &std::path::Path, description: &str) -> Result<Vec<u8>, SendError> {
    fs::read(path).map_err(|error| {
        SendError::Certificate(format!(
            "could not read {description} {}: {error}",
            path.display()
        ))
    })
}

fn reject_encrypted_key(path: &std::path::Path, pem: &[u8]) -> Result<(), SendError> {
    let text = String::from_utf8_lossy(pem).to_ascii_uppercase();
    if text.contains("ENCRYPTED PRIVATE KEY") || text.contains("PROC-TYPE: 4,ENCRYPTED") {
        return Err(SendError::Certificate(format!(
            "encrypted client private key {} is not supported",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rusting_core::config::SslSettings;

    use super::*;

    #[test]
    fn rejects_non_positive_and_non_finite_timeouts() {
        for timeout in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(matches!(
                validated_timeout(timeout),
                Err(SendError::InvalidRequest(_))
            ));
        }
    }

    #[test]
    fn rejects_unpaired_client_certificate_paths() {
        let request = RequestModel::default();
        let settings = SslSettings {
            certificate_path: Some(PathBuf::from("client.pem")),
            key_file: None,
            ca_bundle: None,
        };
        assert!(matches!(
            build(&request, &settings),
            Err(SendError::Certificate(_))
        ));
    }
}
