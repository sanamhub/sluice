//! Writes one Unicode property's code point ranges as Rust source.
//!
//! The property is one line per range in `DerivedCoreProperties.txt`, and a hand copy of it is a
//! code point away from a receiver that accepts a manifest that another volume reads as two names.
//! The database file is the definition, so the table is generated from it, and the version and the
//! header travel with the table:
//!
//! `cargo xtask unicode-ignorable DerivedCoreProperties-18.0.0.txt \
//!     crates/sluice-core/src/manifest/path/ignorable.rs`

use std::{fs, path::Path};

use anyhow::{Context, Result, bail};

/// The property whose ranges are written.
const PROPERTY: &str = "Default_Ignorable_Code_Point";

pub fn run(source: &Path, out: &Path) -> Result<()> {
    let text =
        fs::read_to_string(source).with_context(|| format!("cannot read {}", source.display()))?;
    let (version, header, date) = describe(&text)?;
    let ranges = ranges(&text)?;
    let total: u32 = ranges.iter().map(|(first, last)| last - first + 1).sum();
    let table = render(version, header, date, &ranges, total);
    fs::write(out, table).with_context(|| format!("cannot write {}", out.display()))?;
    println!(
        "{}: {} ranges, {total} code points",
        out.display(),
        ranges.len()
    );
    Ok(())
}

/// The version, the file's first line and its date line, which the table keeps as a comment.
fn describe(text: &str) -> Result<(&str, &str, &str)> {
    let mut lines = text.lines();
    let header = lines.next().context("the file has no header line")?;
    let date = lines
        .next()
        .filter(|line| line.starts_with("# Date:"))
        .context("the file has no date line")?;
    let name = header.trim_start_matches("# ").trim_end_matches(".txt");
    let version = name
        .rsplit('-')
        .next()
        .context("the header line does not name a version")?;
    if !version.split('.').all(|part| part.parse::<u16>().is_ok()) {
        bail!("{version} in {header} is not a version");
    }
    Ok((version, header, date))
}

/// The property's ranges in the file, sorted and with adjacent or overlapping ranges merged.
fn ranges(text: &str) -> Result<Vec<(u32, u32)>> {
    let mut found = Vec::new();
    for line in text.lines() {
        let Some((codes, rest)) = line.split_once(';') else {
            continue;
        };
        let property = rest.split('#').next().unwrap_or_default().trim();
        if property != PROPERTY {
            continue;
        }
        let (first, last) = match codes.trim().split_once("..") {
            Some((first, last)) => (code(first)?, code(last)?),
            None => {
                let one = code(codes.trim())?;
                (one, one)
            }
        };
        if last < first {
            bail!("{} is a range that ends before it starts", codes.trim());
        }
        found.push((first, last));
    }
    if found.is_empty() {
        bail!("the file has no {PROPERTY} ranges");
    }
    found.sort_unstable();
    Ok(merge(found))
}

fn code(text: &str) -> Result<u32> {
    u32::from_str_radix(text, 16).with_context(|| format!("{text} is not a hexadecimal code point"))
}

fn merge(sorted: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    let mut merged: Vec<(u32, u32)> = Vec::with_capacity(sorted.len());
    for (first, last) in sorted {
        match merged.last_mut() {
            Some((_, end)) if first <= end.saturating_add(1) => *end = (*end).max(last),
            _ => merged.push((first, last)),
        }
    }
    merged
}

fn render(version: &str, header: &str, date: &str, ranges: &[(u32, u32)], total: u32) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "//! The `Default_Ignorable_Code_Point` property of Unicode {version}, written by\n\
         //! `cargo xtask unicode-ignorable` from the Unicode Character Database.\n\
         //!\n\
         //! {header}\n\
         //! {date}\n\
         //!\n\
         //! {total} code points in {} ranges, adjacent code points merged.\n\n",
        ranges.len()
    ));
    out.push_str(&format!(
        "/// The ranges, sorted, with the first and the last code point of each.\n\
         pub(super) const RANGES: [(u32, u32); {}] = [\n",
        ranges.len()
    ));
    for (first, last) in ranges {
        out.push_str(&format!("    (0x{first:04X}, 0x{last:04X}),\n"));
    }
    out.push_str("];\n\n");
    out.push_str(concat!(
        "/// Whether `c` carries the property: a name may keep one and a file system may compare it\n",
        "/// as though it were not there.\n",
        "pub(super) fn is_ignorable(c: char) -> bool {\n",
        "    let code = u32::from(c);\n",
        "    RANGES\n",
        "        .binary_search_by(|&(first, last)| {\n",
        "            if last < code {\n",
        "                std::cmp::Ordering::Less\n",
        "            } else if first > code {\n",
        "                std::cmp::Ordering::Greater\n",
        "            } else {\n",
        "                std::cmp::Ordering::Equal\n",
        "            }\n",
        "        })\n",
        "        .is_ok()\n",
        "}\n",
    ));
    out
}
