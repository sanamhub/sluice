#![no_main]

use libfuzzer_sys::fuzz_target;
use sluice_core::manifest::path::ManifestPath;

// A manifest decoder rejects bytes that are not UTF-8 before the path rules run, so only text
// reaches `parse`. Whatever it accepts must be free of the components that leave the output root.
fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(path) = ManifestPath::parse(text) else {
        return;
    };
    assert_eq!(path.as_str(), text);
    for component in path.components() {
        assert!(!component.is_empty(), "empty component in {text:?}");
        assert!(component != "..", "`..` component in {text:?}");
        assert!(!component.contains('\\'), "backslash in {text:?}");
        assert!(!component.contains(':'), "colon in {text:?}");
    }
});
