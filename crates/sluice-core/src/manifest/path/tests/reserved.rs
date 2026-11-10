//! Rules 7 to 9: the names a file system reads differently from the bytes a sender wrote, and the
//! names a receive keeps for itself.

use super::{accepts, rejects};

#[test]
fn rule_7_trailing_dot_or_space() {
    rejects(
        7,
        &[
            "a.", "a ", "dir./a", "dir /a", "a/b.", "a/b ", "...", "a. ", " ",
        ],
    );
    accepts(&["a.b", " a", "a b", "a/ b", "a/.b"]);
}

#[test]
fn rule_8_reserved_device_names() {
    rejects(
        8,
        &[
            "con",
            "CON",
            "Con",
            "prn",
            "AUX",
            "nul",
            "Nul.txt",
            "CONIN$",
            "conout$",
            "ConIn$.log",
            "com0",
            "COM1",
            "com9",
            "LPT0",
            "lpt9",
            "COM\u{b9}",
            "LPT\u{b2}.log",
            "com\u{b3}",
            "LPT\u{b9}.log",
            "a/con",
            "con/a",
            "dir/NUL.tar.gz",
            "CON .txt",
            "con  .txt",
            "CON.tar.gz",
            // U+0131 folds to `I`.
            "CONIN$".replace('I', "\u{131}").as_str(),
        ],
    );
    accepts(&[
        "CONSOLE",
        "con_1",
        "CON_1",
        "COM",
        "COM10",
        "COM1a",
        "LPT",
        "LPT10",
        "COMx",
        "AUXILIARY",
        "NULL",
        "PRN1",
        "CONIN",
        "CONOUT",
        "CONIN$$",
        "xcon",
        "con-",
        "a.con",
        "a.nul",
        ".con",
        "CON\u{e9}",
        "COM\u{b9}\u{b9}",
    ]);
}

/// Rule 9: the names a receive gives the store beside the output and a folder it writes through.
/// A manifest that could name one would let a sender write into, or plant, the folder a later
/// receive resumes from.
#[test]
fn rule_9_reserved_store_names() {
    rejects(
        9,
        &[
            ".sluice-partial-0123456789abcdef",
            ".sluice-tmp-0123456789abcdef",
            // The prefix itself names a store too, so it is reserved as well.
            ".sluice-partial-",
            ".sluice-tmp-",
            "a/.sluice-partial-x",
            ".sluice-partial-x/b",
            "proj/.sluice-tmp-x/c.txt",
            // Windows and macOS resolve a name whatever its case, so the same folder has these
            // spellings too.
            ".SLUICE-PARTIAL-X",
            ".Sluice-Partial-0123456789ABCDEF",
            ".SLUICE-TMP-X",
            ".SlUiCe-TmP-",
            "proj/.SLUICE-PARTIAL-x/c.txt",
        ],
    );
    accepts(&[
        ".sluice-partial",
        ".sluice-tmp",
        "sluice-partial-x",
        ".sluice-partials-x",
        ".sluice-tmpx",
        ".SLUICE-PARTIALS-X",
    ]);
}
/// Rule 9 folds the way a file system may, not only ASCII case: APFS folds a long s to `s`, and a
/// fullwidth letter or dot is the plain one after NFKC. Rejecting only the ASCII spellings would let
/// a sender name a folder that a later receive would resume from.
#[test]
fn rule_9_folds_unicode_spellings_of_a_reserved_prefix() {
    rejects(
        9,
        &[
            ".ſluice-partial-x",
            ".ſLUICE-TMP-X",
            "．sluice-partial-x",
            ".Ｓluice-TMP-x",
            "tree/.ſluice-partial-x/b",
        ],
    );
    accepts(&[
        "ſluice-partials-x",
        "é.sluice-partial-x",
        "．sluice-partials-x",
    ]);
}

/// Two spellings rule 9 has to read as the prefix although neither NFKC nor case folding changes
/// them: NTFS upcases the dotless `ı` to `I`, and HFS+ ignores the code points a name may not show,
/// so a name that reaches `.sluice-partial-` only after those are gone still names the store.
#[test]
fn rule_9_reads_the_dotless_i_and_the_ignorable_code_points() {
    rejects(
        9,
        &[
            ".sluıce-partial-x",
            ".SLUICE-PART\u{131}AL-X",
            "tree/.sluıce-tmp-x/b",
            ".\u{200b}sluice-partial-x",
            ".slu\u{200d}ice-partial-x",
            ".slu\u{2060}ice-tmp-x",
            ".\u{feff}sluice-partial-x",
            ".\u{ad}sluice-partial-x",
        ],
    );
    accepts(&[
        ".sluıce-partials-x",
        ".slu\u{131}ce",
        "a\u{200b}b.txt",
        "ſluıce.txt",
    ]);
}
