//! A token, a bearer token or a URL's query never turns up in a message, wherever it is put.

use crate::check_config;

const SECRET: &str = "s3cr3t-T0KEN";

/// Configs that are refused, each with the secret in a place where toml's own message would
/// quote it.
fn refused_with_secret() -> Vec<(String, bool)> {
    let s = SECRET;
    [
        // Closed, so --open is refused after the parse.
        (format!("[access]\nshared_token = [\"{s}\"]\n"), true),
        (
            format!("[access.http]\nurl = \"https://a.example/?key={s}\"\n"),
            true,
        ),
        (
            format!("[access.http]\nurl = \"https://a.example\"\nbearer_token = \"{s}\"\n"),
            true,
        ),
        // The wrong type, so toml's message holds the value.
        (format!("[access]\nshared_token = \"{s}\"\n"), false),
        (
            format!("[access.http]\nurl = \"https://a.example\"\nbearer_token = [\"{s}\"]\n"),
            false,
        ),
        (
            format!("[access.http]\nurl = [\"https://a.example/?key={s}\"]\n"),
            false,
        ),
        (format!("[access]\nallowlist = [\"{s}\"]\n"), false),
        (format!("[access]\nallowlist = \"{s}\"\n"), false),
        (format!("access = \"{s}\"\n"), false),
        (
            format!("[limits.client.rx]\nbytes_per_second = \"{s}\"\n"),
            true,
        ),
        (format!("[tls]\ncert_mode = \"{s}\"\n"), false),
        (
            format!("[tls]\ncert_mode = \"Manual\"\nhostname = {{ a = \"{s}\" }}\n"),
            false,
        ),
        (format!("enable_relay = \"{s}\"\n"), false),
        // Beside an unknown key and in text that is not TOML.
        (
            format!("[access]\nshared_token = [\"{s}\"]\nextra = \"{s}\"\n"),
            false,
        ),
        (format!("[access]\nshared_token = [\"{s}\n"), false),
        (
            format!("[access]\nshared_token = [\"{s}\"] \"{s}\"\n"),
            false,
        ),
    ]
    .into()
}

#[test]
fn no_refusal_prints_a_token_a_bearer_token_or_a_query() {
    for (text, open) in refused_with_secret() {
        let message = check_config(&text, open).expect_err(&text).to_string();
        assert!(
            !message.contains(SECRET),
            "{text:?} printed the secret: {message}"
        );
        assert!(
            !message.contains("s3cr3t"),
            "{text:?} printed part of it: {message}"
        );
    }
}

#[test]
fn the_token_itself_is_what_toml_would_have_quoted() {
    // Shows the test above tests something: toml's own message quotes the value.
    let err = toml::from_str::<crate::schema::Config>(&format!("access = \"{SECRET}\"\n"));
    assert!(err.unwrap_err().message().contains(SECRET));
}
