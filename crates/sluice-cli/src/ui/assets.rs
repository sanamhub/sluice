//! The page, embedded in the binary at build time. It loads nothing from anywhere else, and each of
//! its answers carries the headers of ADR 0036 section 5.

/// The page and what it loads, by the path it is served at: the content type and the text.
pub const ASSETS: [(&str, &str, &str); 5] = [
    (
        "/",
        "text/html; charset=utf-8",
        include_str!("../../ui/index.html"),
    ),
    (
        "/app.css",
        "text/css; charset=utf-8",
        include_str!("../../ui/app.css"),
    ),
    (
        "/api.js",
        "text/javascript; charset=utf-8",
        include_str!("../../ui/api.js"),
    ),
    (
        "/app.js",
        "text/javascript; charset=utf-8",
        include_str!("../../ui/app.js"),
    ),
    (
        "/favicon.svg",
        "image/svg+xml",
        include_str!("../../ui/favicon.svg"),
    ),
];

/// The Content-Security-Policy of every answer, as the ADR states it.
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; connect-src 'self'; img-src 'self' \
data:; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

/// The headers every answer carries, whatever it is. None of them is a CORS header, and there is no
/// CORS header on any answer.
pub const HEADERS: [(&str, &str); 4] = [
    ("content-security-policy", CONTENT_SECURITY_POLICY),
    ("x-content-type-options", "nosniff"),
    ("referrer-policy", "no-referrer"),
    ("cache-control", "no-store"),
];

/// The embedded file served at `path`, if there is one.
pub fn find(path: &str) -> Option<(&'static str, &'static str)> {
    ASSETS
        .iter()
        .find(|(at, _, _)| *at == path)
        .map(|(_, kind, text)| (*kind, *text))
}
