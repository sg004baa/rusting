//! Static catalogue driving the header-name and header-value autocompletion.
//!
//! Scraped from MDN. Only the two things the UI actually shows are kept: the
//! canonical name, and whether the header is experimental (rendered in the
//! warning colour).

/// A request header the name autocompletion offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeaderName {
    pub name: &'static str,
    pub experimental: bool,
}

/// Canonical request header names, in MDN's grouping order.
pub const REQUEST_HEADERS: &[HeaderName] = &[
    HeaderName {
        name: "Authorization",
        experimental: false,
    },
    HeaderName {
        name: "Proxy-Authorization",
        experimental: false,
    },
    HeaderName {
        name: "Cache-Control",
        experimental: false,
    },
    HeaderName {
        name: "If-Match",
        experimental: false,
    },
    HeaderName {
        name: "If-None-Match",
        experimental: false,
    },
    HeaderName {
        name: "If-Modified-Since",
        experimental: false,
    },
    HeaderName {
        name: "If-Unmodified-Since",
        experimental: false,
    },
    HeaderName {
        name: "Connection",
        experimental: false,
    },
    HeaderName {
        name: "Keep-Alive",
        experimental: false,
    },
    HeaderName {
        name: "Accept",
        experimental: false,
    },
    HeaderName {
        name: "Accept-Encoding",
        experimental: false,
    },
    HeaderName {
        name: "Accept-Language",
        experimental: false,
    },
    HeaderName {
        name: "Expect",
        experimental: false,
    },
    HeaderName {
        name: "Max-Forwards",
        experimental: false,
    },
    HeaderName {
        name: "Cookie",
        experimental: false,
    },
    HeaderName {
        name: "Access-Control-Request-Headers",
        experimental: false,
    },
    HeaderName {
        name: "Access-Control-Request-Method",
        experimental: false,
    },
    HeaderName {
        name: "Origin",
        experimental: false,
    },
    HeaderName {
        name: "Content-Length",
        experimental: false,
    },
    HeaderName {
        name: "Content-Type",
        experimental: false,
    },
    HeaderName {
        name: "Content-Encoding",
        experimental: false,
    },
    HeaderName {
        name: "Content-Language",
        experimental: false,
    },
    HeaderName {
        name: "Content-Location",
        experimental: false,
    },
    HeaderName {
        name: "Forwarded",
        experimental: false,
    },
    HeaderName {
        name: "Via",
        experimental: false,
    },
    HeaderName {
        name: "From",
        experimental: false,
    },
    HeaderName {
        name: "Host",
        experimental: false,
    },
    HeaderName {
        name: "Referer",
        experimental: false,
    },
    HeaderName {
        name: "User-Agent",
        experimental: false,
    },
    HeaderName {
        name: "Range",
        experimental: false,
    },
    HeaderName {
        name: "If-Range",
        experimental: false,
    },
    HeaderName {
        name: "Upgrade-Insecure-Requests",
        experimental: false,
    },
    HeaderName {
        name: "Transfer-Encoding",
        experimental: false,
    },
    HeaderName {
        name: "TE",
        experimental: false,
    },
    HeaderName {
        name: "Trailer",
        experimental: false,
    },
    HeaderName {
        name: "Alt-Used",
        experimental: false,
    },
    HeaderName {
        name: "Date",
        experimental: false,
    },
    HeaderName {
        name: "Link",
        experimental: false,
    },
    HeaderName {
        name: "X-Forwarded-For",
        experimental: false,
    },
    HeaderName {
        name: "X-Forwarded-Host",
        experimental: false,
    },
    HeaderName {
        name: "X-Forwarded-Proto",
        experimental: false,
    },
    HeaderName {
        name: "Pragma",
        experimental: false,
    },
    HeaderName {
        name: "Origin-Isolation",
        experimental: true,
    },
    HeaderName {
        name: "Accept-Push-Policy",
        experimental: true,
    },
    HeaderName {
        name: "Accept-Signature",
        experimental: true,
    },
    HeaderName {
        name: "Early-Data",
        experimental: true,
    },
    HeaderName {
        name: "Signature",
        experimental: true,
    },
    HeaderName {
        name: "Signed-Headers",
        experimental: true,
    },
    HeaderName {
        name: "Sec-GPC",
        experimental: true,
    },
    HeaderName {
        name: "Accept-Charset",
        experimental: false,
    },
    HeaderName {
        name: "DNT",
        experimental: false,
    },
    HeaderName {
        name: "Upgrade",
        experimental: false,
    },
    HeaderName {
        name: "Sec-Fetch-Site",
        experimental: false,
    },
    HeaderName {
        name: "Sec-Fetch-Mode",
        experimental: false,
    },
    HeaderName {
        name: "Sec-Fetch-User",
        experimental: false,
    },
    HeaderName {
        name: "Sec-Fetch-Dest",
        experimental: false,
    },
    HeaderName {
        name: "Service-Worker-Navigation-Preload",
        experimental: false,
    },
];

/// Well-known values for a given header name, keyed by lowercase name.
const HEADER_VALUES: &[(&str, &[&str])] = &[
    (
        "accept",
        &[
            "application/",
            "audio/",
            "font/",
            "image/",
            "text/",
            "video/",
            "multipart/",
            "*",
            "*/*",
            "image/*",
            "audio/*",
            "video/*",
            "application/json",
            "application/xml",
            "application/x-www-form-urlencoded",
            "application/javascript",
            "application/pdf",
            "application/zip",
            "application/octet-stream",
            "application/graphql",
            "application/msgpack",
            "text/plain",
            "text/html",
            "text/css",
            "text/csv",
            "text/markdown",
            "text/yaml",
        ],
    ),
    (
        "accept-encoding",
        &[
            "gzip",
            "deflate",
            "br",
            "compress",
            "identity",
            "*",
            "gzip, deflate",
            "gzip, deflate, br",
        ],
    ),
    (
        "accept-language",
        &[
            "en", "en-US", "en-GB", "es", "es-ES", "fr", "fr-FR", "de", "de-DE", "it", "ja", "ko",
            "zh", "zh-CN", "zh-TW", "*",
        ],
    ),
    (
        "authorization",
        &["Bearer ", "Basic ", "Digest ", "OAuth ", "JWT ", "ApiKey "],
    ),
    (
        "cache-control",
        &[
            "no-cache",
            "no-store",
            "no-transform",
            "private",
            "public",
            "must-revalidate",
            "proxy-revalidate",
            "max-age=0",
            "no-cache, no-store",
            "private, no-cache",
            "no-cache, must-revalidate",
            "max-age=3600",
            "max-age=86400",
            "max-age=604800",
        ],
    ),
    ("connection", &["keep-alive", "close", "upgrade"]),
    (
        "content-type",
        &[
            "application/",
            "audio/",
            "font/",
            "image/",
            "text/",
            "video/",
            "multipart/",
            "application/json",
            "application/xml",
            "application/x-www-form-urlencoded",
            "application/javascript",
            "application/pdf",
            "application/zip",
            "application/octet-stream",
            "application/graphql",
            "application/msgpack",
            "text/plain",
            "text/html",
            "text/css",
            "text/csv",
            "text/markdown",
            "text/yaml",
            "multipart/form-data",
            "multipart/mixed",
            "multipart/alternative",
            "image/jpeg",
            "image/png",
            "image/gif",
            "image/webp",
            "image/svg+xml",
            "image/avif",
            "audio/mpeg",
            "audio/ogg",
            "audio/wav",
            "video/mp4",
            "video/webm",
            "video/ogg",
        ],
    ),
    ("if-match", &["*", "W/"]),
    ("if-none-match", &["*", "W/"]),
    ("pragma", &["no-cache"]),
    (
        "range",
        &[
            "bytes=",
            "bytes=0-",
            "bytes=0-499",
            "bytes=-500",
            "bytes=500-999",
            "bytes=0-499,500-999",
        ],
    ),
];

/// Suggested values for a header, or an empty slice when the header has none.
/// The lookup is case-insensitive because users type `Content-Type` and
/// `content-type` interchangeably.
pub fn values_for(header_name: &str) -> &'static [&'static str] {
    HEADER_VALUES
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(header_name))
        .map_or(&[], |(_, values)| *values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_is_populated_and_unique() {
        assert!(REQUEST_HEADERS.len() > 40);
        let mut lowered: Vec<String> = REQUEST_HEADERS
            .iter()
            .map(|h| h.name.to_lowercase())
            .collect();
        lowered.sort();
        let count = lowered.len();
        lowered.dedup();
        assert_eq!(lowered.len(), count, "duplicate header names");
    }

    #[test]
    fn some_headers_are_flagged_experimental() {
        assert!(REQUEST_HEADERS.iter().any(|h| h.experimental));
        assert!(REQUEST_HEADERS.iter().any(|h| !h.experimental));
    }

    #[test]
    fn value_lookup_is_case_insensitive() {
        assert!(values_for("Content-Type").contains(&"application/json"));
        assert_eq!(values_for("content-type"), values_for("CONTENT-TYPE"));
        assert!(values_for("X-Nonexistent").is_empty());
    }

    #[test]
    fn header_values_keys_are_lowercase() {
        for (name, values) in HEADER_VALUES {
            assert_eq!(*name, name.to_lowercase(), "{name}");
            assert!(!values.is_empty(), "{name} has no values");
        }
    }
}
