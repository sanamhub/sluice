//! The options one static receive runs with, and the refusals a receipt brings to the modes that do
//! not take a whole ticket.

use std::path::PathBuf;

use sluice_core::{
    Error,
    engine::{Rate, ReceiptTarget, ReceiveOptions, Selection},
    manifest::Limits,
    range::ByteSize,
    ticket::{Body, Ticket},
};

use super::RecvArgs;

/// Refuses a receipt for anything that is not one whole static transfer: a receipt names the ticket's
/// whole root and length, and the modes below take part of it or no file at all.
pub(super) fn refuse_receipt_without_a_whole_transfer(
    args: &RecvArgs,
    ticket: &Ticket,
    has_range: bool,
    include: &Option<Selection>,
) -> Result<(), Error> {
    if args.receipt.is_none() {
        return Ok(());
    }
    let why = match ticket.body() {
        Body::Live { .. } => Some("a live ticket has no root for a receipt to name"),
        Body::Static { .. } if args.out.as_path() == std::path::Path::new("-") => {
            Some("standard output carries the content, and a receipt goes to a file")
        }
        Body::Static { .. } if has_range => {
            Some("a byte range takes part of the content, and a receipt names all of it")
        }
        Body::Static { .. } if include.is_some() => {
            Some("a selection takes part of the content, and a receipt names all of it")
        }
        Body::Static { .. } => None,
    };
    match why {
        Some(why) => Err(Error::Policy(format!(
            "--receipt needs the whole transfer: {why}"
        ))),
        None => Ok(()),
    }
}

/// The options of a receive, with the receipt path made absolute once.
pub(super) fn options(
    args: &RecvArgs,
    out: PathBuf,
    rate: Option<std::sync::Arc<Rate>>,
    include: Option<Selection>,
) -> Result<ReceiveOptions, Error> {
    let max_size = args
        .max_size
        .as_deref()
        .map(ByteSize::parse)
        .transpose()?
        .map(ByteSize::bytes);
    Ok(ReceiveOptions {
        out,
        force: args.force,
        no_times: args.no_times,
        links: args.links,
        allow_hidden: args.allow_hidden,
        limits: Limits::default(),
        free_bytes: None,
        max_size,
        congestion: args.congestion.into(),
        rate,
        write_limit: None,
        max_network_failures: sluice_core::queue::MAX_NETWORK_FAILURES,
        include,
        receipt: args
            .receipt
            .as_deref()
            .map(|path| {
                std::path::absolute(path)
                    .map(|path| ReceiptTarget { path })
                    .map_err(|e| Error::Policy(format!("cannot resolve {}: {e}", path.display())))
            })
            .transpose()?,
    })
}
