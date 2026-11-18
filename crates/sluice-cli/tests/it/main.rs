//! The integration tests of the command line, built as one binary so the tool links once.

mod common;

mod completions;
mod conventions;
mod daemon;
mod daemon_transfer;
mod delta;
mod doctor;
mod help;
mod id;
mod include;
mod inspect;
#[cfg(unix)]
mod interrupt;
mod live;
mod mcp;
mod mcp_line;
mod paired;
mod pipes;
mod queue;
mod ranges;
mod receipt;
mod receipt_delivery;
mod receivers;
mod resume;
mod short_code;
mod transfer;
mod ui;
mod ui_guards;
mod version;
