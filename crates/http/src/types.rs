//! Types crossing the send boundary. The TUI never sees `reqwest`.

use std::time::Duration;

use rusting_core::KeyValue;

/// The phases the timing strip and the Timings tab report.
///
/// This is a deliberate simplification of httpx's 21 trace events: these are
/// the phases that can actually be measured through `reqwest`, and the ones a
/// user acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    /// Hostname resolution.
    Dns,
    /// TCP connect.
    Connect,
    /// TLS handshake. Absent for plain HTTP.
    Tls,
    /// Request sent, waiting for the first response byte.
    TimeToFirstByte,
    /// Streaming the response body to completion.
    Download,
}

impl Phase {
    pub const ALL: [Phase; 5] = [
        Phase::Dns,
        Phase::Connect,
        Phase::Tls,
        Phase::TimeToFirstByte,
        Phase::Download,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Phase::Dns => "DNS",
            Phase::Connect => "Connect",
            Phase::Tls => "TLS",
            Phase::TimeToFirstByte => "TTFB",
            Phase::Download => "Download",
        }
    }
}

/// What is known about one phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PhaseOutcome {
    /// The phase did not run. A reused connection skips DNS/Connect/TLS, and
    /// plain HTTP always skips TLS.
    #[default]
    Skipped,
    /// Started but not finished — the request is still in flight, or it failed
    /// during this phase.
    Started,
    Completed(Duration),
    Failed,
}

/// Per-phase timings for one request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Timings {
    phases: [PhaseOutcome; 5],
    /// Wall time from the start of the send to the last body byte.
    pub total: Option<Duration>,
}

impl Timings {
    pub fn outcome(&self, phase: Phase) -> PhaseOutcome {
        self.phases[phase as usize]
    }

    pub fn set(&mut self, phase: Phase, outcome: PhaseOutcome) {
        self.phases[phase as usize] = outcome;
    }

    pub fn iter(&self) -> impl Iterator<Item = (Phase, PhaseOutcome)> + '_ {
        Phase::ALL.into_iter().map(|phase| (phase, self.outcome(phase)))
    }

    /// True when nothing has been recorded yet, so the strip stays hidden.
    pub fn is_empty(&self) -> bool {
        self.total.is_none()
            && self
                .phases
                .iter()
                .all(|outcome| *outcome == PhaseOutcome::Skipped)
    }
}

/// The request exactly as it went on the wire, for the Sent Request tab.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SentRequest {
    pub method: String,
    /// Final URL including the merged query string.
    pub url: String,
    pub headers: Vec<KeyValue>,
    /// Decoded body, lossily if it is not UTF-8. `None` for a bodyless request.
    pub body: Option<String>,
}

/// A completed response, fully buffered.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub reason: String,
    /// The final URL after any redirects.
    pub url: String,
    pub headers: Vec<KeyValue>,
    /// Cookies the response set, to be folded into the session jar.
    pub cookies: Vec<KeyValue>,
    /// Raw body bytes. Kept as bytes so a binary response does not lose data.
    pub body: Vec<u8>,
    pub timings: Timings,
    /// The request that produced this response.
    pub sent: SentRequest,
}

impl Response {
    /// The body decoded as UTF-8, replacing invalid sequences.
    pub fn text(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|header| header.name.eq_ignore_ascii_case(name))
            .map(|header| header.value.as_str())
    }

    /// The syntax language implied by `content-type`.
    ///
    /// Defaults to JSON: an API client shows JSON far more often than anything
    /// else, and a server that omits or mislabels the header is common.
    pub fn language(&self) -> Option<&'static str> {
        let content_type = self
            .header("content-type")
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        match content_type.as_str() {
            "text/plain" => None,
            "text/html" | "application/xhtml+xml" | "application/xml" | "text/xml" => Some("html"),
            "text/css" => Some("css"),
            _ => Some("json"),
        }
    }
}

/// Why a send failed. The variants exist so the notification can be specific;
/// a bare string would make "your timeout is too low" indistinguishable from
/// "your proxy URL is malformed".
#[derive(Debug, thiserror::Error)]
pub enum SendError {
    #[error("Couldn't resolve or connect to the host: {0}")]
    Connect(String),
    #[error("Timed out after {}s.", .0.as_secs_f64())]
    Timeout(Duration),
    #[error("TLS handshake failed: {0}")]
    Tls(String),
    #[error("The proxy URL is not usable: {0}")]
    Proxy(String),
    #[error("The request could not be built: {0}")]
    InvalidRequest(String),
    #[error("Could not read the TLS material: {0}")]
    Certificate(String),
    #[error("{0}")]
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response_with_content_type(value: Option<&str>) -> Response {
        Response {
            status: 200,
            reason: "OK".into(),
            url: "http://x".into(),
            headers: value
                .map(|v| vec![KeyValue::new("Content-Type", v)])
                .unwrap_or_default(),
            cookies: Vec::new(),
            body: Vec::new(),
            timings: Timings::default(),
            sent: SentRequest::default(),
        }
    }

    #[test]
    fn language_maps_content_types_and_defaults_to_json() {
        assert_eq!(
            response_with_content_type(Some("application/json; charset=utf-8")).language(),
            Some("json")
        );
        assert_eq!(
            response_with_content_type(Some("text/html")).language(),
            Some("html")
        );
        assert_eq!(
            response_with_content_type(Some("TEXT/CSS")).language(),
            Some("css")
        );
        assert_eq!(
            response_with_content_type(Some("text/plain")).language(),
            None
        );
        assert_eq!(response_with_content_type(None).language(), Some("json"));
        assert_eq!(
            response_with_content_type(Some("application/octet-stream")).language(),
            Some("json"),
            "an unknown type still gets the JSON viewer"
        );
    }

    #[test]
    fn header_lookup_is_case_insensitive() {
        let response = response_with_content_type(Some("application/json"));
        assert_eq!(
            response.header("CONTENT-TYPE"),
            Some("application/json")
        );
        assert_eq!(response.header("missing"), None);
    }

    #[test]
    fn timings_start_empty_and_record_per_phase() {
        let mut timings = Timings::default();
        assert!(timings.is_empty());
        timings.set(Phase::Dns, PhaseOutcome::Completed(Duration::from_millis(3)));
        assert!(!timings.is_empty());
        assert_eq!(
            timings.outcome(Phase::Dns),
            PhaseOutcome::Completed(Duration::from_millis(3))
        );
        assert_eq!(timings.outcome(Phase::Tls), PhaseOutcome::Skipped);
        assert_eq!(timings.iter().count(), Phase::ALL.len());
    }

    #[test]
    fn lossy_text_never_panics_on_invalid_utf8() {
        let mut response = response_with_content_type(None);
        response.body = vec![0xFF, b'a'];
        assert!(response.text().contains('a'));
    }
}
