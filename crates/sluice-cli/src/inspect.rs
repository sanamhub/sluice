//! `sluice inspect <ticket>`: what a ticket offers, without the content. Only the manifest is
//! fetched, so a ticket can be looked at before a byte of it is downloaded.

use clap::Args;
use serde_json::{Value, json};
use sluice_core::{
    Error,
    engine::{Offered, Offering, inspect},
    manifest::{Kind, Limits, path::shown},
};

use crate::{
    output::{Outcome, summary},
    recv::ticket_text,
};

/// How many names the tree shows before it says how many are left. A terminal shows more than this
/// without scrolling, and a tree of a whole disk would bury the size.
pub const NAMES_SHOWN: usize = 50;

#[derive(Args)]
pub struct InspectArgs {
    /// The ticket to look at, or `-` to read it from standard input
    #[arg(value_name = "TICKET")]
    pub ticket: String,
    /// List every name, not only the first 50
    #[arg(long)]
    pub all: bool,
}

pub fn run(args: &InspectArgs) -> Result<Outcome, Error> {
    let text = ticket_text(&args.ticket)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Storage(format!("cannot start the transfer runtime: {e}")))?;
    // Dropping the future is what removes the throwaway store this made, and the interrupt has to
    // reach the end of the command so that the exit wait for removals runs. A runtime whose select
    // is skipped would leave the store under `home/tmp` until the next command sweeps it.
    let offering = runtime.block_on(crate::recv::until_interrupted(inspect(
        &text,
        Limits::default(),
    )))?;
    Ok(Outcome::Offering {
        offering,
        all: args.all,
    })
}

/// What was read, as text: the summary line, then the names as a tree with a folder's name ending
/// in `/` and a link's in `@`. A tree longer than [`NAMES_SHOWN`] names is cut, and the line after
/// it says how many names it left out unless every one was asked for.
pub fn text(offering: &Offering, all: bool) -> String {
    let offered = match offering {
        Offering::Live => {
            return "a live stream, whose names and size arrive with the stream itself\n"
                .to_string();
        }
        Offering::Static(offered) => offered,
    };
    let mut text = format!(
        "{}\n",
        summary(offered.files, offered.folders, offered.links, offered.bytes)
    );
    let shown_lines = if all {
        offered.entries.len()
    } else {
        offered.entries.len().min(NAMES_SHOWN)
    };
    for entry in &offered.entries[..shown_lines] {
        text.push_str(&line(&entry.path, entry.kind));
    }
    if shown_lines < offered.entries.len() {
        text.push_str(&format!(
            "... and {} more names (--all lists every one)\n",
            offered.entries.len() - shown_lines
        ));
    }
    text
}

/// One name, indented by its depth so a folder's entries sit under it.
fn line(path: &str, kind: Kind) -> String {
    let depth = path.matches('/').count();
    let name = path.rsplit('/').next().unwrap_or(path);
    let mark = match kind {
        Kind::File => "",
        Kind::Dir => "/",
        Kind::Symlink => "@",
    };
    format!("{}{}{mark}\n", "  ".repeat(depth), shown(name))
}

/// What was read, as one JSON object: the counts, the size and every entry whatever `--all` says,
/// because a line limit is about a terminal and not about a value.
pub fn json(offering: &Offering) -> String {
    let value = match offering {
        Offering::Live => json!({ "live": true }),
        Offering::Static(offered) => object(offered),
    };
    format!("{value}\n")
}

fn object(offered: &Offered) -> Value {
    let entries: Vec<Value> = offered
        .entries
        .iter()
        .map(|entry| json!({ "path": entry.path, "kind": kind(entry.kind), "size": entry.size }))
        .collect();
    json!({
        "files": offered.files,
        "folders": offered.folders,
        "links": offered.links,
        "bytes": offered.bytes,
        "entries": entries,
    })
}

fn kind(kind: Kind) -> &'static str {
    match kind {
        Kind::File => "file",
        Kind::Dir => "directory",
        Kind::Symlink => "link",
    }
}

#[cfg(test)]
mod tests {
    use super::{NAMES_SHOWN, line, text};
    use sluice_core::{
        engine::{Offered, OfferedEntry, Offering},
        manifest::Kind,
    };

    /// An offering of the entries given, with the counts and the size worked out the way the manifest
    /// reader works them out.
    fn offered(entries: Vec<(&str, Kind, u64)>) -> Offering {
        let mut files = 0;
        let mut folders = 0;
        let mut links = 0;
        let mut bytes = 0;
        let entries: Vec<OfferedEntry> = entries
            .into_iter()
            .map(|(path, kind, size)| {
                match kind {
                    Kind::File => {
                        files += 1;
                        bytes += size;
                    }
                    Kind::Dir => folders += 1,
                    Kind::Symlink => links += 1,
                }
                OfferedEntry {
                    path: path.to_string(),
                    kind,
                    size,
                }
            })
            .collect();
        Offering::Static(Offered {
            files,
            folders,
            links,
            bytes,
            entries,
        })
    }

    /// The tree is indented by depth and marks a folder and a link, so a reader sees the shape of
    /// what is coming without reading the paths.
    #[test]
    fn the_tree_indents_each_name_under_its_folder() {
        let tree = text(
            &offered(vec![
                ("tree", Kind::Dir, 0),
                ("tree/a.txt", Kind::File, 1024),
                ("tree/sub", Kind::Dir, 0),
                ("tree/sub/link", Kind::Symlink, 0),
            ]),
            false,
        );
        assert_eq!(
            tree,
            "1 file, 2 folders, 1 link, 1.0 KiB\ntree/\n  a.txt\n  sub/\n    link@\n"
        );
    }

    /// A folder with more names than the terminal should take is cut, and the line after it says how
    /// many were left out; `--all` shows all of them.
    #[test]
    fn a_long_tree_is_cut_unless_every_name_is_asked_for() {
        // The folder's own line is one of the lines, so two files fewer than the limit plus three.
        let mut entries = vec![("big", Kind::Dir, 0)];
        let names: Vec<String> = (0..(NAMES_SHOWN + 2))
            .map(|index| format!("big/{index}.bin"))
            .collect();
        entries.extend(names.iter().map(|name| (name.as_str(), Kind::File, 1)));
        let offering = offered(entries);
        let cut = text(&offering, false);
        assert_eq!(cut.lines().count(), 1 + NAMES_SHOWN + 1, "{cut}");
        assert!(
            cut.ends_with("... and 3 more names (--all lists every one)\n"),
            "{cut}"
        );
        let every = text(&offering, true);
        assert_eq!(every.lines().count(), 1 + NAMES_SHOWN + 3, "{every}");
        assert!(!every.contains("more names"), "{every}");
    }

    /// A name cannot change what the terminal shows: a control or a bidirectional character is
    /// written as its escape instead, so it cannot reorder the line or clear the screen.
    #[test]
    fn a_name_cannot_change_what_the_terminal_shows() {
        assert_eq!(
            line("report\u{202e}fdp.exe", Kind::File),
            "report\\u{202e}fdp.exe\n"
        );
        assert_eq!(line("a\u{1b}[2Jb", Kind::File), "a\\u{1b}[2Jb\n");
        assert_eq!(line("two\nlines", Kind::File), "two\\u{a}lines\n");
        assert_eq!(line("tree/a\u{200e}b", Kind::File), "  a\\u{200e}b\n");
        let tree = text(
            &offered(vec![
                ("report\u{202e}fdp.exe", Kind::File, 0),
                ("a\u{1b}[2Jb", Kind::File, 0),
            ]),
            false,
        );
        assert_eq!(tree.lines().count(), 3, "{tree:?}");
        assert!(
            !tree.contains('\u{202e}') && !tree.contains('\u{1b}'),
            "{tree:?}"
        );
    }

    /// A live ticket has no manifest to read, and the line says so rather than showing nothing.
    #[test]
    fn a_live_ticket_says_what_it_can_offer() {
        assert_eq!(
            text(&Offering::Live, false),
            "a live stream, whose names and size arrive with the stream itself\n"
        );
    }
}
