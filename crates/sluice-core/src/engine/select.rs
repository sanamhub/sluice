//! Which entries of a manifest a receive takes: the patterns of `--include`, matched against the
//! paths in the manifest. A pattern is never matched against an output path and can select nothing
//! that the manifest does not list, so a selection cannot reach outside the transfer.

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use crate::Error;

/// The entries of a manifest a receive takes, as a set of globs. No pattern takes every entry, which
/// is what a receive without `--include` does.
#[derive(Clone, Debug, Default)]
pub struct Selection {
    set: GlobSet,
    patterns: Vec<String>,
}

impl Selection {
    /// The entries whose path matches at least one pattern. `*` matches inside one component and
    /// `**` crosses a `/`, with `/` as the only separator and the match case-sensitive, as a
    /// manifest path is. A pattern that does not parse is refused here, before anything is fetched.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::engine::Selection;
    ///
    /// let selection = Selection::new(&["photos/**".to_string()])?;
    /// assert!(selection.selects("photos/2026/a.jpg"));
    /// assert!(!selection.selects("notes/a.txt"));
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn new(patterns: &[String]) -> Result<Self, Error> {
        let mut builder = GlobSetBuilder::new();
        for pattern in patterns {
            let glob = GlobBuilder::new(pattern)
                // `*` stops at a `/`; `**` is how a pattern reaches into a folder. Without this a
                // pattern over a tree would match across components and take more than it says.
                .literal_separator(true)
                .build()
                .map_err(|e| Error::Policy(format!("{pattern} is not a pattern: {e}")))?;
            builder.add(glob);
        }
        let set = builder
            .build()
            .map_err(|e| Error::Policy(format!("the patterns cannot be used: {e}")))?;
        Ok(Self {
            set,
            patterns: patterns.to_vec(),
        })
    }

    /// Whether any pattern was given. None takes every entry of the manifest.
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Whether this selection takes the manifest path `path`.
    pub fn selects(&self, path: &str) -> bool {
        self.is_empty() || self.set.is_match(path)
    }

    /// The patterns, in the order they were given.
    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }

    /// A short name for this selection that does not change between runs, such as `b3:0f…`, and
    /// `all` for no patterns. A partial store's marker keeps it, so a receive with another selection
    /// does not continue a store made for a different set of entries.
    pub(crate) fn key(&self) -> String {
        if self.patterns.is_empty() {
            return "all".to_string();
        }
        let mut sorted: Vec<&str> = self.patterns.iter().map(String::as_str).collect();
        sorted.sort_unstable();
        sorted.dedup();
        // Each pattern is fed with its length in front, so two selections that differ only in
        // where a newline falls cannot hash the same bytes: joining the patterns would make
        // `a\nb` and `c` one key with `a`, `b` and `c`. A marker written before this change holds
        // a key made from the join, so it names a selection this one is not, and the store it
        // holds starts over; `all` and a marker with no key still mean the whole ticket.
        let mut feed = Vec::new();
        for pattern in &sorted {
            feed.extend_from_slice(&(pattern.len() as u64).to_le_bytes());
            feed.extend_from_slice(pattern.as_bytes());
        }
        let digest = blake3::hash(&feed);
        format!("b3:{}", digest.to_hex())
    }
}

#[cfg(test)]
mod tests {
    use super::Selection;

    fn selection(patterns: &[&str]) -> Selection {
        Selection::new(&patterns.iter().map(|p| p.to_string()).collect::<Vec<_>>()).unwrap()
    }

    /// One star stops at a separator, two cross it, and a pattern is case-sensitive: a user who
    /// wrote a path gets the paths that spell it that way.
    #[test]
    fn a_star_stops_at_a_separator_and_two_cross_it() {
        let one = selection(&["tree/*.txt"]);
        assert!(one.selects("tree/a.txt"));
        assert!(!one.selects("tree/sub/a.txt"));
        let two = selection(&["tree/**/*.txt"]);
        assert!(two.selects("tree/sub/a.txt"));
        assert!(!two.selects("other/a.txt"));
        let exact = selection(&["Tree/a.txt"]);
        assert!(!exact.selects("tree/a.txt"));
    }

    /// A name with a space and a name that is only a prefix of another are matched as written.
    #[test]
    fn a_name_is_matched_as_it_reads() {
        let selection = selection(&["my notes/a b.txt"]);
        assert!(selection.selects("my notes/a b.txt"));
        assert!(!selection.selects("my notes/a b.txt.bak"));
    }

    /// A pattern that steps above the tree, or starts at a root, is a pattern like any other and
    /// matches nothing: a manifest path is relative and never holds `..`, so a selection cannot
    /// reach outside what the sender listed.
    #[test]
    fn a_path_above_the_tree_matches_nothing() {
        for pattern in ["../**", "/etc/**", "a/../../**"] {
            let selection = selection(&[pattern]);
            for path in ["a.txt", "a/b.txt", "etc/passwd"] {
                assert!(!selection.selects(path), "{pattern} took {path}");
            }
        }
    }

    /// Two selections that name the same patterns in another order are one selection, and a
    /// selection with no patterns is the whole manifest.
    #[test]
    fn the_key_names_the_patterns_and_not_their_order() {
        assert_eq!(
            selection(&["a/**", "b"]).key(),
            selection(&["b", "a/**"]).key()
        );
        assert_eq!(Selection::default().key(), "all");
        assert_ne!(selection(&["a"]).key(), selection(&["b"]).key());
    }

    /// A pattern may hold any character, so a newline inside one is not where the next pattern
    /// starts: two patterns, one of them holding a newline, are not three patterns.
    #[test]
    fn a_newline_inside_a_pattern_is_not_a_separator() {
        assert_ne!(
            selection(&["a\nb", "c"]).key(),
            selection(&["a", "b", "c"]).key()
        );
        assert_ne!(selection(&["ab", "c"]).key(), selection(&["a", "bc"]).key());
    }

    /// A pattern that does not parse is refused where the command line is read.
    #[test]
    fn a_pattern_that_does_not_parse_is_refused() {
        let err = Selection::new(&["a/[".to_string()]).unwrap_err();
        assert!(matches!(err, crate::Error::Policy(_)), "{err}");
        assert!(err.to_string().contains("a/["), "{err}");
    }
}
