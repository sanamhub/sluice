//! `sluice recv <ticket>`: decode, fetch, verify, write, and never ask.

use std::{io::BufRead, path::PathBuf};

use clap::Args;
use sluice_core::{
    Error,
    engine::{
        Rate, ReceiveOptions, Selection, receive, receive_live, receive_range, receive_stdout,
    },
    manifest::Limits,
    range::ByteRange,
    range::ByteSize,
    ticket::{Body, Format, Ticket},
};
use tokio::sync::mpsc::unbounded_channel;

use crate::{
    cli::{CongestionArg, PriorityArg},
    events,
    output::Output,
};

/// Receives the file a ticket names.
#[derive(Args)]
pub struct RecvArgs {
    /// The ticket the sender printed, or `-` to read it from standard input. Several tickets run
    /// through the queue, three at once by default
    #[arg(value_name = "TICKET", required = true)]
    pub tickets: Vec<String>,
    /// The folder to write into, `-` to write a single file to standard output, or for a live
    /// ticket the file to write
    #[arg(long, default_value = ".", value_name = "DIR")]
    pub out: PathBuf,
    /// Replace a path that is already there. A folder is never replaced by a file or a link, even
    /// with this flag
    #[arg(long)]
    pub force: bool,
    /// Do not restore the modification time
    #[arg(long)]
    pub no_times: bool,
    /// Create the symlinks the sender recorded (never followed outside the output)
    #[arg(long)]
    pub links: bool,
    /// Write only this byte range of a one-file transfer, such as `4G:4.1G`, with the units of
    /// `--max-size` (`KB` is 1000 bytes, `KiB` 1024); a folder is refused
    #[arg(long, value_name = "RANGE")]
    pub range: Option<String>,
    /// Accept a top-level name that starts with a dot, such as `.ssh`
    #[arg(long)]
    pub allow_hidden: bool,
    /// The congestion controller: cubic (default) or bbr3 (experimental)
    #[arg(long, value_enum, default_value_t = CongestionArg::Cubic, value_name = "NAME")]
    pub congestion: CongestionArg,
    /// Where several tickets start among themselves: high, normal (default) or low
    #[arg(long, value_enum, default_value_t = PriorityArg::Normal, value_name = "LEVEL")]
    pub priority: PriorityArg,
    /// Cap the bytes this command fetches per second, shared by its transfers, such as `50M` or
    /// `50MB`: `KB` is 1000 bytes and `KiB` 1024, and `K`, `M`, `G`, `T`, `P` alone are powers of 1024
    #[arg(long, value_name = "RATE")]
    pub limit_rate: Option<String>,
    /// Stop a live receive once the file would pass this size, such as `4G` or `4GB`; what arrived
    /// stays. The units are those of `--limit-rate`: `KB` is 1000 bytes and `KiB` 1024
    #[arg(long, value_name = "SIZE")]
    pub max_size: Option<String>,
    /// Take only the entries whose path matches this glob, such as `photos/**`; repeat for more.
    /// `*` matches inside one name and `**` crosses a `/`
    #[arg(long, value_name = "PATTERN")]
    pub include: Vec<String>,
}

pub fn run(args: &RecvArgs, output: &Output) -> Result<u8, Error> {
    let tickets = tickets_text(&args.tickets)?;
    let rate = args.limit_rate.as_deref().map(Rate::parse).transpose()?;
    let include = (!args.include.is_empty())
        .then(|| Selection::new(&args.include))
        .transpose()?;
    if tickets.len() > 1 {
        if args.max_size.is_some() {
            return Err(Error::Policy(
                "--max-size bounds one live stream, not a queue of transfers".to_string(),
            ));
        }
        return run_many(args, output, &tickets, rate, include);
    }
    let ticket = tickets.into_iter().next().unwrap_or_default();
    let decoded = Ticket::decode(&ticket)?;
    let range = args.range.as_deref().map(ByteRange::parse).transpose()?;
    refuse_include_without_entries(&decoded, args, &include)?;
    if args.max_size.is_some() && !matches!(decoded.body(), Body::Live { .. }) {
        return Err(Error::Policy(
            "--max-size bounds a live stream; a ticket for a fixed transfer already carries its \
             size"
                .to_string(),
        ));
    }
    if matches!(decoded.body(), Body::Live { .. }) {
        if args.out.as_path() == std::path::Path::new("-") {
            return Err(Error::Policy(
                "a live ticket writes a file, so --out - is not supported yet".to_string(),
            ));
        }
        if range.is_some() {
            return Err(Error::Policy(
                "--range works on a static transfer, not on a live one".to_string(),
            ));
        }
        return run_live(args, output, &ticket, rate);
    }
    if args.out.as_path() == std::path::Path::new("-") {
        if range.is_some() {
            return Err(Error::Policy(
                "--range writes a file, so it cannot be combined with --out -".to_string(),
            ));
        }
        return to_stdout(args, output, &ticket, rate);
    }
    if let Some(range) = range {
        return run_range(args, output, &ticket, range, rate);
    }
    receive_one(args, output, &ticket, rate, include)
}

/// Refuses a selection on a ticket that lists no entries to choose between: a live stream carries no
/// names, a raw blob has one and no tree, and a byte range or standard output names one file. A
/// ticket that turns out to hold one file is refused after its manifest is read, where the file
/// count is known.
fn refuse_include_without_entries(
    ticket: &Ticket,
    args: &RecvArgs,
    include: &Option<Selection>,
) -> Result<(), Error> {
    if include.is_none() {
        return Ok(());
    }
    let why = match ticket.body() {
        Body::Live { .. } => "a live ticket has no names to choose from",
        Body::Static {
            format: Format::Raw,
            ..
        } => "a raw ticket has no names to choose from",
        Body::Static { .. } => {
            if args.out.as_path() == std::path::Path::new("-") {
                "standard output takes one file, not a tree to choose from"
            } else if args.range.is_some() {
                "a byte range takes one file, not a tree to choose from"
            } else {
                return Ok(());
            }
        }
    };
    Err(Error::Policy(format!(
        "--include selects a folder ticket's entries, and {why}"
    )))
}

/// Runs a transfer until it finishes or the user interrupts it. An interrupt drops the future, which
/// is what removes the private temporary folders the receive made; the partial store beside the
/// output is left, so the transfer can be continued.
pub(crate) async fn until_interrupted<T>(
    work: impl std::future::Future<Output = Result<T, Error>>,
) -> Result<T, Error> {
    tokio::select! {
        result = work => result,
        _ = tokio::signal::ctrl_c() => Err(Error::Cancelled),
    }
}

/// One static ticket into the output folder, with the usual events and final line.
fn receive_one(
    args: &RecvArgs,
    output: &Output,
    ticket: &str,
    rate: Option<std::sync::Arc<Rate>>,
    include: Option<Selection>,
) -> Result<u8, Error> {
    let out = std::path::absolute(&args.out)
        .map_err(|e| Error::Policy(format!("cannot resolve {}: {e}", args.out.display())))?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Storage(format!("cannot start the transfer runtime: {e}")))?;
    runtime.block_on(async {
        let (tx, rx) = unbounded_channel();
        let printer = events::printer(*output, rx, "receiving");
        let options = options(args, out, rate, include)?;
        let result = until_interrupted(receive(ticket, &options, &tx)).await;
        drop(tx);
        let printed = printer
            .await
            .map_err(|e| Error::Bug(format!("the event printer stopped: {e}")))?;
        printed?;
        let received = result?;
        if matches!(output.format(), crate::output::Format::Human) {
            if received.files == 1 {
                output.write_stderr(&format!("wrote {}\n", received.path.display()));
            } else {
                output.write_stderr(&format!(
                    "wrote {} files under {}\n",
                    received.files,
                    received.path.display()
                ));
            }
        }
        Ok(0)
    })
}

/// Several tickets in one command. They run through the queue, so three start at once, a high one
/// starts before a normal one and a normal one before a low, and a network failure waits and tries
/// the same ticket again with the partial store the last attempt left.
fn run_many(
    args: &RecvArgs,
    output: &Output,
    tickets: &[String],
    rate: Option<std::sync::Arc<Rate>>,
    include: Option<Selection>,
) -> Result<u8, Error> {
    if args.out.as_path() == std::path::Path::new("-") {
        return Err(Error::Policy(
            "several tickets need a folder, so --out - receives one ticket at a time".to_string(),
        ));
    }
    if args.range.is_some() {
        return Err(Error::Policy(
            "--range receives one byte range of one ticket, not several tickets".to_string(),
        ));
    }
    let out = std::path::absolute(&args.out)
        .map_err(|e| Error::Policy(format!("cannot resolve {}: {e}", args.out.display())))?;
    let mut options = options(args, out, rate, include)?;
    // The queue owns retries for a command of several tickets: one failure comes back to it at
    // once, and its backoff schedules the next attempt from the store the last one left.
    options.max_network_failures = 1;
    crate::queue::run(output, tickets, args.priority.into(), options)
}

/// The ticket arguments as text. One ticket may be `-`, read from standard input; several are
/// used as they are, and `-` is refused because a pipe carries one ticket.
fn tickets_text(arguments: &[String]) -> Result<Vec<String>, Error> {
    if arguments.len() == 1 {
        return Ok(vec![ticket_text(&arguments[0])?]);
    }
    if arguments.iter().any(|text| text == "-") {
        return Err(Error::Policy(
            "standard input carries one ticket, so read it in a command of its own".to_string(),
        ));
    }
    Ok(arguments.to_vec())
}

/// Follows a live stream into the file `--out` names. The ticket carries no name, so the flag has
/// to; a ticket with the default `--out` meets a folder and is refused with that advice.
fn run_live(
    args: &RecvArgs,
    output: &Output,
    ticket: &str,
    rate: Option<std::sync::Arc<Rate>>,
) -> Result<u8, Error> {
    let out = std::path::absolute(&args.out)
        .map_err(|e| Error::Policy(format!("cannot resolve {}: {e}", args.out.display())))?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Storage(format!("cannot start the transfer runtime: {e}")))?;
    runtime.block_on(async {
        let (tx, rx) = unbounded_channel();
        let printer = events::printer(*output, rx, "receiving");
        let options = options(args, out, rate, None)?;
        let store = sluice_core::Home::temporary_store()?;
        let result = until_interrupted(receive_live(ticket, &options, &store, &tx)).await;
        drop(tx);
        let printed = printer
            .await
            .map_err(|e| Error::Bug(format!("the event printer stopped: {e}")))?;
        printed?;
        let received = result?;
        if matches!(output.format(), crate::output::Format::Human) {
            output.write_stderr(&format!("wrote {}\n", received.path.display()));
        }
        Ok(0)
    })
}

/// Writes the content to standard output, where nothing else may go.
fn to_stdout(
    args: &RecvArgs,
    output: &Output,
    ticket: &str,
    rate: Option<std::sync::Arc<Rate>>,
) -> Result<u8, Error> {
    if matches!(output.format(), crate::output::Format::Json) {
        return Err(Error::Policy(
            "--json writes events to standard output, so --out - cannot share it".to_string(),
        ));
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Storage(format!("cannot start the transfer runtime: {e}")))?;
    runtime.block_on(async {
        let (tx, _rx) = unbounded_channel();
        let options = options(args, PathBuf::from("."), rate, None)?;
        let store = sluice_core::Home::temporary_store()?;
        let bytes = until_interrupted(receive_stdout(ticket, &options, &store, &tx)).await?;
        output.write_stderr(&format!("wrote {bytes} bytes to standard output\n"));
        Ok(0)
    })
}

/// Writes the requested range of a one-file transfer to its usual name in the output folder.
fn run_range(
    args: &RecvArgs,
    output: &Output,
    ticket: &str,
    range: ByteRange,
    rate: Option<std::sync::Arc<Rate>>,
) -> Result<u8, Error> {
    let out = std::path::absolute(&args.out)
        .map_err(|e| Error::Policy(format!("cannot resolve {}: {e}", args.out.display())))?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Storage(format!("cannot start the transfer runtime: {e}")))?;
    runtime.block_on(async {
        let (tx, _rx) = unbounded_channel();
        let options = options(args, out, rate, None)?;
        let store = sluice_core::Home::temporary_store()?;
        let bytes = until_interrupted(receive_range(ticket, range, &options, &store, &tx)).await?;
        output.write_stderr(&format!("wrote {bytes} bytes from the range\n"));
        Ok(0)
    })
}
fn options(
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
    })
}

/// The ticket as given, or the one line on standard input when the argument is `-`. A ticket on
/// the command line is visible in the process list and the shell history, so a caller that cares
/// pipes it instead. `inspect` reads its ticket the same way.
pub(crate) fn ticket_text(argument: &str) -> Result<String, Error> {
    if argument != "-" {
        return Ok(argument.to_string());
    }
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| Error::Storage(format!("cannot read the ticket from standard input: {e}")))?;
    let text = line.trim().to_string();
    if text.is_empty() {
        return Err(Error::Policy(
            "no ticket arrived on standard input".to_string(),
        ));
    }
    Ok(text)
}
