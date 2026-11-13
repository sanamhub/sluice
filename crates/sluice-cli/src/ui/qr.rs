//! The ticket as an SVG QR code, from the QR crate `send --qr` draws with.

use qrcodegen::{QrCode, QrCodeEcc};

/// Modules of white round the code, which a reader needs.
const QUIET: i32 = 4;

/// The code for `text` as an SVG that is only paths, with no script and no link: black modules on
/// white, scalable. `None` when the text is too long for a code.
pub fn svg(text: &str) -> Option<String> {
    let code = QrCode::encode_text(text, QrCodeEcc::Medium).ok()?;
    let size = code.size();
    let total = size + QUIET * 2;
    let mut path = String::new();
    for y in 0..size {
        for x in 0..size {
            if code.get_module(x, y) {
                path.push_str(&format!("M{},{}h1v1h-1z", x + QUIET, y + QUIET));
            }
        }
    }
    Some(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {total} {total}\" \
         shape-rendering=\"crispEdges\"><rect width=\"{total}\" height=\"{total}\" fill=\"#fff\"/>\
         <path d=\"{path}\" fill=\"#000\"/></svg>"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ticket_makes_an_svg_with_paths_only() {
        let svg = svg("sluice1abcdefghijklmnopqrstuvwxyz").unwrap();
        assert!(svg.starts_with("<svg "));
        assert!(svg.ends_with("</svg>"));
        assert!(!svg.contains("script") && !svg.contains("href"));
        assert!(svg.contains("<path d=\"M"));
    }

    #[test]
    fn text_too_long_for_a_code_gives_none() {
        assert!(svg(&"x".repeat(5000)).is_none());
    }
}
