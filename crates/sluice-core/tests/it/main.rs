//! The integration tests of the home folder, built as one binary so the crate links once.

mod home;
#[cfg(unix)]
mod home_key_mode;
mod home_links;
