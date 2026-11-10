//! The properties every path either side of a transfer must satisfy, whatever the bytes.

use proptest::prelude::*;

use super::ManifestPath;
use crate::Error;

/// One component that is often fine and sometimes breaks a rule, so the accepted and the
/// rejected side of every rule both get exercised.
fn component() -> impl Strategy<Value = String> {
    prop_oneof![
        8 => "[a-z0-9\u{e9}\u{65e5}\u{672c}_-]{1,12}",
        2 => "[a-zA-Z0-9._ -]{1,12}",
        1 => "[.:\\\\ a]{0,3}",
        1 => "(con|nul|aux|prn|com[0-9\u{b9}]|lpt[0-9\u{b2}])(\\.[a-z]{1,3})?",
    ]
}

proptest! {
    #[test]
    fn an_accepted_path_is_made_of_accepted_components(parts in prop::collection::vec(component(), 1..6)) {
        let text = parts.join("/");
        if let Ok(path) = ManifestPath::parse(&text) {
            for part in path.components() {
                prop_assert!(ManifestPath::parse(part).is_ok(), "component {part:?} of {text:?}");
            }
            prop_assert_eq!(path.components().collect::<Vec<_>>().join("/"), text);
        }
    }

    #[test]
    fn a_path_of_the_awkward_characters_is_accepted_or_rejected(text in "[a./\\\\: ]{0,24}") {
        if let Ok(path) = ManifestPath::parse(&text) {
            for part in path.components() {
                prop_assert!(!part.is_empty() && part != "." && part != "..", "{text:?}");
            }
            prop_assert!(!text.contains(['\\', ':']), "{text:?}");
        }
    }

    #[test]
    fn any_text_gets_an_answer_and_a_short_error(text in any::<String>()) {
        if let Err(Error::Policy(message)) = ManifestPath::parse(&text) {
            prop_assert!(message.len() < 400, "{} bytes", message.len());
        }
    }
}
