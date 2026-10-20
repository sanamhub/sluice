# Test vectors

These files say what a Sluice implementation must accept and reject. `SPEC.md` section 12 lists the
kinds. An implementation that passes every vector here follows the same rules as ours at the points
the vectors cover. They do not cover the whole wire.

Run them against this repository with `cargo xtask conformance`. It exits non-zero if one case
disagrees, and `cargo nextest run` runs the same check.

## Layout

```
tests/vectors/
  paths/        manifest path rules, SPEC.md section 8
  tickets/      not written yet
  manifests/    not written yet
  packs/        not written yet
  live/         not written yet
  delta/        not written yet
```

One directory per row of the table in section 12. A directory holds one or more `.json` files. The
runner reads every `.json` file in a directory it knows, in name order, and reports a directory it
does not know as skipped, never as passed. A directory named here but missing from the tree has no
vectors yet.

## File format

Every file is one JSON object, UTF-8, with two fields:

| Field | Type | Meaning |
|---|---|---|
| `description` | string | What the cases in the file have in common. A runner prints it next to a failing case |
| `cases` | array | The cases. Never empty. Their order carries no meaning |

The shape of a case depends on the kind. A reader must ignore fields it does not know, so a later
release can add a field without breaking an older runner.

## `paths/`

A case is one manifest path and the verdict a receiver must reach:

```json
{ "path": "a/../b", "valid": false, "rule": 4 }
{ "path": "photos/IMG_0001.jpg", "valid": true }
```

| Field | Type | Meaning |
|---|---|---|
| `path` | string | The path exactly as a manifest carries it |
| `valid` | boolean | `true` if a receiver MUST accept it, `false` if it MUST reject it |
| `rule` | integer | Present when `valid` is `false`. The number of the rule in `SPEC.md` section 8 that the path breaks |

To check an implementation, give every `path` to its receiver check and compare the verdict with
`valid`. That is all the runner in this repository compares.

If a path breaks more than one rule, `rule` is the lowest number. The file `lowest-rule.json` holds
such paths. Compare `rule` only if your implementation reports the rule it stopped at, and then
expect the lowest, not the first one met when reading left to right.

Some points a reader cannot see from the rules alone:

- The vectors test the text of a path and nothing on disk. A symlink that already exists in the
  output folder is the writer's concern.
- Rule 1 and rule 5 count bytes of UTF-8, not characters. A component of 128 `é` is 256 bytes and
  breaks rule 5.
- Rule 2 is a check. An implementation must not normalize a path and then accept it.
- Rule 8 takes the name from the text before the first `.`, ignores spaces before that dot, and
  compares U+0131 as `I`. So `con.tar.gz` and `CON .txt` break rule 8, and `a.con` does not.
- Rule 6 rejects the C1 and bidirectional controls and accepts the zero-width characters U+200B,
  U+200C, U+200D and U+FEFF. A UI that shows a name has to escape them.
- A path with control characters, decomposed accents, ligatures or look-alike characters is written
  with `\u` escapes, so the file shows what it holds. Read the JSON with a parser, not by eye.
- A path is a JSON string, so it is valid Unicode. A manifest decoder rejects bytes that are not
  UTF-8 before it asks the path rules anything, and no vector covers that.
- Each file is named for the rule it tests. It holds the paths that break that rule, and next to
  them the closest paths that must pass, such as `..a` for rule 4 and `CONSOLE` for rule 8.
  `accepted.json` holds ordinary paths.
