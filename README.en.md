# exceldiff

*[日本語](README.md)*

[![Rust CI](https://github.com/MinamiyamaKotaro/exceldiff/actions/workflows/rust-ci.yml/badge.svg)](https://github.com/MinamiyamaKotaro/exceldiff/actions/workflows/rust-ci.yml)
[![Docs](https://github.com/MinamiyamaKotaro/exceldiff/actions/workflows/docs.yml/badge.svg)](https://github.com/MinamiyamaKotaro/exceldiff/actions/workflows/docs.yml)
[![exceldiff on crates.io](https://img.shields.io/crates/v/exceldiff.svg)](https://crates.io/crates/exceldiff)
[![codecov](https://codecov.io/gh/MinamiyamaKotaro/exceldiff/branch/master/graph/badge.svg)](https://codecov.io/gh/MinamiyamaKotaro/exceldiff)
[![License](https://img.shields.io/github/license/MinamiyamaKotaro/exceldiff)](LICENSE)

A lightweight, high-performance `.xlsx` (OOXML) parser library written in Rust. It's also usable as a [GitHub Action](#using-it-as-a-github-action) that posts an automatic diff-preview comment for any `.xlsx` file changed in a pull request.

## Motivation

`.xlsx` files are routinely tracked in git and reviewed in pull requests at
Japanese business systems, but `git diff` tells you nothing useful about
them — a `.xlsx` is a ZIP archive of XML parts, so even a single changed
cell can shuffle the shared-string table or the ZIP's compressed bytes
wholesale, leaving nothing but an opaque binary diff.

`exceldiff` closes that gap. A diff engine parses the before/after `.xlsx`
into two `Workbook`s and compares them cell by cell, and a CLI plus GitHub
Action summarize the result as a PR comment — Markdown text, or an
Excel-like grid HTML view. The files this targets are the kind common in
Japanese business systems: sheets with an extreme number of rows/columns
("方眼紙Excel", "grid-paper Excel") and heavy use of merged cells. That the
underlying parser handles both without ever building a full in-memory 2D
grid isn't just about the parser's own footprint — it's what keeps the
diff itself fast and accurate, including row/column alignment detection so
that inserting a single row or column doesn't get misreported as a wall of
`Modified` cells.

The underlying parser is based on the same design and implementation as a
sibling project, [`xlsxparser`](https://github.com/MinamiyamaKotaro/xlsxparser).
The detailed parsing architecture, OOXML coverage, and parsing-performance
benchmarks (vs. `calamine`, etc.) live in `xlsxparser`'s own README — see
that instead.

## Status

Core implementation complete — every module in the planned architecture
below is implemented and tested against the design in `docs/design/`. The
public API (`parse_workbook`, `parse_workbook_reader`, `to_json_string`,
`to_json_writer`, `resolve_color`) is wired up in `src/lib.rs`.

```rust
let workbook = exceldiff::parse_workbook("book.xlsx")?;
let json = exceldiff::to_json_string(&workbook)?;
```

- [docs/requirement/requirements.en.md](docs/requirement/requirements.en.md) —
  this project's own functional requirements around diff detection,
  output, and distribution (the underlying parser's own requirements live
  in the sister project `xlsxparser`; also available in
  [Japanese](docs/requirement/requirements.md)).
- [docs/design/architecture.en.md](docs/design/architecture.en.md) — the
  overall `src/` directory layout, module responsibilities, and design
  principles (also available in [Japanese](docs/design/architecture.md)).
  It links out to a per-module design doc for every file, covering
  responsibility/scope, key types and function signatures, dependencies,
  error handling policy, testing strategy, and open questions — each doc
  written in both Japanese and English (`*.md` / `*.en.md`). Where
  implementation diverged from a design doc's draft (an external API
  detail settled differently than planned, a bug found while writing
  tests, etc.), the doc was updated in place to record what changed and why.

## Using it as a GitHub Action

Use it as a GitHub Action (composite action) that automatically posts a per-sheet summary of what changed in any `.xlsx` file a pull request touches.

### Preconditions

Unlike an ordinary workflow job, a composite action cannot declare or perform two things on its own — the caller's own workflow needs to supply them:

- `actions/checkout@v4` with `fetch-depth: 0` — the diff step reads both the PR's base and head revisions via `git show`, so a shallow checkout (which only has the merge commit) won't work.
- `permissions: contents: read` — always required, regardless of `comment`/`visual`. Writing any `permissions:` block at all sets every scope you don't list to `none` (not the repo's default), so a workflow with only `pull-requests: write` silently loses `contents` access and `actions/checkout` fails with a confusing "repository not found" — a real bug hit while verifying this action from an external repo.
- `permissions: pull-requests: write` — needed to post a comment while `comment` is left at its default `true` (set `comment: false`/`job-summary: true` to skip needing this permission). `visual: true` needs no extra permission — see below.

### Example usage

```yaml
name: xlsx diff preview

on:
  pull_request:

permissions:
  contents: read
  pull-requests: write

jobs:
  xlsx-diff:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - uses: MinamiyamaKotaro/exceldiff@v1
```

To also see an Excel-like grid view, set `visual: true`. No extra `permissions:` are needed — the grid isn't embedded directly in the comment, it's attached as one standalone HTML page per changed file (every changed sheet combined onto that same page) in a workflow artifact, with a download link posted in the comment (only people with read access to this repository can download it; this is deliberate, so the grid stays reliably viewable on private repos too — see the comment at the top of [action.yml](action.yml) and [Issue #47](https://github.com/MinamiyamaKotaro/exceldiff/issues/47) for why). Being HTML rather than a screenshot image means a large sheet doesn't get shrunk into an illegible blur — it scrolls and zooms like any other web page:

```yaml
permissions:
  contents: read
  pull-requests: write

steps:
  - uses: actions/checkout@v4
    with:
      fetch-depth: 0
  - uses: MinamiyamaKotaro/exceldiff@v1
    with:
      visual: 'true'
```

### Inputs

| input | type/default | what it does |
|---|---|---|
| `github-token` | string, `${{ github.token }}` | token used to post the comment |
| `files` | string, `'*.xlsx'` | a **git pathspec** passed straight to `git diff -- <files>` — not a shell glob |
| `comment` | bool string, `'true'` | post/update a PR comment |
| `job-summary` | bool string, `'false'` | also write to `$GITHUB_STEP_SUMMARY`. A caller without `pull-requests: write` (e.g. a fork PR) can set `comment: false`/`job-summary: true` to see the diff without hitting a permission error |
| `max-rows-per-sheet` | numeric string, `'30'` | caps the number of cell-change hunks rendered per sheet |
| `diff-mode` | `auto` \| `coordinate`, `'auto'` | `auto` (default) auto-picks whichever of coordinate/row-aligned/column-aligned diffing reports the fewest changes per sheet. `coordinate` forces plain coordinate comparison, skipping alignment detection |
| `diff-scope` | `pr` \| `commit`, `'pr'` | `pr` (default) diffs the PR's cumulative changes as one section per file, same as always. `commit` instead breaks each commit the PR introduces into its own subsection, diffed against its immediate parent — so a file added and then modified within the same PR shows both instead of only a final "Added". With `visual: true`, the grid HTML is also downloadable separately per commit |
| `visual` | bool string, `'false'` | render an Excel-like Before/After grid view (HTML) for each changed sheet and attach them to the comment as a downloadable workflow artifact. No extra `permissions:` needed |

### Outputs

| output | type | what it is |
|---|---|---|
| `has-changes` | bool string | whether any file matching `files` changed in the PR |
| `changed-files-count` | numeric string | how many files matching `files` changed |

See [docs/design/action.en.md](docs/design/action.en.md) for the full design and tradeoffs.

## Using it as a CLI

The GitHub Action above builds and runs `cli/` (package `xlsxdiff`, not published to crates.io) once per changed `.xlsx` file in a PR. That binary can also be built and run standalone — it prints one file's git diff as a GitHub-flavored Markdown section.

### Building it

```bash
git clone https://github.com/MinamiyamaKotaro/exceldiff.git
cd exceldiff
cargo build --release -p xlsxdiff
```

### Usage

```text
xlsxdiff [--max-rows-per-sheet <N>] [--diff-mode <auto|coordinate>] [--grid-html-dir <dir>] <display_path> <A|M|D> [base_file] [head_file]
```

- `display_path`: the path shown in the Markdown heading (the file's path in the repo).
- `A`/`M`/`D`: the git status (added/modified/deleted).
- `base_file`/`head_file`: the actual filesystem paths of the before/after revisions. `A` has no `base_file`, `D` has no `head_file` — omit it or pass an empty string.
- `--max-rows-per-sheet <N>` (default `30`): caps the number of cell-change hunks rendered per sheet.
- `--diff-mode <auto|coordinate>` (default `auto`): `auto` auto-picks alignment, `coordinate` forces plain coordinate comparison.
- `--grid-html-dir <dir>`: when given, additionally writes every changed sheet combined onto one standalone HTML page (`<dir>/grid.html`) plus a `<dir>/manifest.tsv` listing each sheet's name against that same path (`sheet_name\thtml_path`) — `action.yml`'s `visual: true` attaches this page directly to its workflow artifact.

Example (`M` — both the before and after file exist):

```bash
xlsxdiff "budget.xlsx" M /tmp/base/budget.xlsx /tmp/head/budget.xlsx
```

A parse error or an unrecognized git status is rendered as an error section inside the Markdown output rather than aborting the process — one file's problem never stops the rest of the diff from being shown. See [docs/design/cli.en.md](docs/design/cli.en.md) for details.

## Embedded images

`images` is a sheet-level array of cell-anchored embedded images
(`xl/drawings/drawingN.xml`) — real output, from
`tests/fixtures/complex/embedded_image.xlsx` (one image anchored `B2:E9`
with a hyperlink; `cells`/`columns` omitted below for brevity):

```json
{
  "images": [
    {
      "anchor": {
        "type": "twoCell",
        "from": { "row": 2, "col": 2, "colOff": 10000, "rowOff": 20000 },
        "to": { "row": 9, "col": 5, "colOff": 0, "rowOff": 0 }
      },
      "target": "xl/media/image1.png",
      "hyperlink": "https://example.com/sample-image"
    }
  ]
}
```

- `anchor` is tagged by `type`: `"twoCell"` (stretches between two cell
  corners, `from`/`to`) or `"oneCell"` (`from` plus an `ext: {"cx", "cy"}`
  size in EMU — a `oneCell` anchor has no `to` marker, since its size is
  independent of any cell boundary). `row`/`col` are 1-based, matching
  every other cell coordinate this crate emits; `colOff`/`rowOff` are the
  EMU-unit offset *within* that cell (kept rather than rounded away, so a
  diff can distinguish an image nudged a few pixels from one that hasn't
  moved).
- `target` is the embedded media part's resolved path (e.g.
  `"xl/media/image1.png"`) — never the image's own bytes, which stay
  entirely out of scope (a diff-oriented tool has no use for pixel data,
  and reading it would scale memory use with image count rather than
  cell count).
- `hyperlink` is the image's own hyperlink (`a:hlinkClick`), distinct from
  a cell hyperlink (a `JsonCell`-level field — see above). Omitted when the
  image carries none. An `Internal` (in-package) target resolves to a
  ZIP-entry-name-equivalent path the same way `target` does; an
  `External` one (a URL, as above) is kept verbatim.
- Grouped images (`<xdr:grpSp>`) resolve each contained `<xdr:pic>`'s
  anchor relative to its enclosing group, flattened into this same
  per-sheet `images` array — no separate group structure is exposed.

## Resolving display colors

`fillFgColor`/`fillBgColor` above are kept raw because exceldiff's
primary purpose is diffing, not rendering — but when a caller does need
to know the actual color a cell displays as (not just whether it
changed), `resolve_color` converts any of the three `ColorRef` forms
(`rgb` / `theme`+`tint` / `indexed`) into a real `Rgb { r, g, b }` value
on demand:

```rust
use exceldiff::{parse_workbook, resolve_color, CellRef};

let workbook = parse_workbook("book.xlsx")?;
let sheet = &workbook.sheets()[0];
let cell = sheet.get(CellRef { row: 1, col: 1 }).unwrap();

if let Some(color_ref) = cell.style.as_ref().and_then(|s| s.fill_fg_color.as_ref()) {
    let rgb = resolve_color(color_ref, workbook.theme());
    // e.g. Some(Rgb { r: 0x4F, g: 0x81, b: 0xBD })
}
```

- `theme`+`tint` references resolve against the workbook's
  `xl/theme/theme{N}.xml` `<clrScheme>` (`Workbook::theme()`), applying
  ECMA-376's tint luminance correction, and return `None` if the
  workbook has no theme part at all or the referenced slot index is out
  of range.
- `indexed` references resolve against the legacy ECMA-376 64-color
  palette; `indexed=64`/`65` (the "system foreground"/"system
  background" special values) resolve to fixed `#000000`/`#FFFFFF`,
  independent of any OS system palette (this crate runs headless).
- `resolve_color` never panics on malformed input (an out-of-range theme
  index, a non-finite `tint`, malformed hex) — it returns `None` instead.
- `xl/theme/theme{N}.xml` is read and parsed only if the workbook's
  stylesheet actually references a theme color at all
  ("pay-for-what-you-use") — a workbook that never uses one pays zero
  added I/O or CPU cost for this feature, even when the part is present
  in the file.

## Architecture

1. **Relationship resolution** — parse `_rels` parts to build a routing map
   from sheet `r:id` to worksheet file path, then discard the intermediate
   data immediately.
2. **Sanitization** — guard against zip bombs, zip-slip path traversal, and
   XXE before any untrusted content is parsed.
3. **Streaming parse** — a SAX-style reader processes `<sheetData>` one
   `<row>` at a time, without holding the sheet's full XML DOM in memory.
4. **Resolution** — shared strings (`t="s"`) and cell styles are resolved
   against the SST/stylesheet, and `<mergeCells>` ranges are resolved
   against the collected cells after the stream pass completes.
5. **JSON output** — the resolved data model is serialized to structured
   JSON (including `row_span`/`col_span` for merged cells) for downstream
   consumption, as a separate step from the primary `Workbook`-returning API.

Core requirements driving the design:

- **Sparse storage** — cells are kept in a coordinate-keyed map, never a
  dense 2D array, so sparse "grid-paper" sheets stay cheap to hold in memory.
- **Merge-cell transparency** — any coordinate inside a merged range
  resolves (via an O(1) bounding-box pre-check plus a geometric containment
  scan over the sheet's merged regions) to the same value and merge
  metadata as the range's anchor cell.
- **I/O and domain logic stay separated** — XML/ZIP handling (`container/`,
  `parse/`) never mixes with the resolution logic (`resolve/`), which
  operates purely on in-memory data and needs no I/O to unit test.

The module layout (see [docs/design/architecture.en.md](docs/design/architecture.en.md)
for the core 5-phase pipeline's full breakdown, and
[docs/design/diff/mod.en.md](docs/design/diff/mod.en.md) /
[docs/design/markdown.en.md](docs/design/markdown.en.md) /
[docs/design/grid.en.md](docs/design/grid.en.md) for the diff-computation,
Markdown-formatting, and grid-rendering layer built on top):

```text
src/
  lib.rs        # public API entry point (parse_workbook, diff_workbooks_best_effort, diff_file_section_from_paths, ...)
  error.rs      # crate-wide error type
  pipeline.rs   # orchestrates the 5-phase pipeline and resource lifetimes

  container/    # ZIP (OPC) extraction, zip-bomb/zip-slip guarding
  parse/        # XML parsing (quick-xml usage is confined here), XXE mitigation
  model/        # pure data structures (Workbook, Sheet, Cell, CellValue, ...)
  resolve/      # shared-string/style/merge-cell resolution + on-demand color resolution, I/O-independent
  json.rs       # serializes a resolved Workbook to JSON

  diff/         # the diff engine that compares two Workbooks (coordinate/row-aligned/column-aligned/best-effort auto-pick)
  markdown.rs   # formats a WorkbookDiff into GitHub-flavored Markdown for a PR comment (the CLI's entry point, diff_file_section_from_paths)
  grid.rs       # renders a changed sheet as an Excel-like grid HTML page (what action.yml's visual: true mode attaches to its artifact)
```

## OOXML parts covered

- `xl/_rels/workbook.xml.rels`
- `xl/workbook.xml` (including `<workbookPr date1904="...">`, needed to
  resolve a date/time cell's serial value under the 1900 vs. 1904 date
  system)
- `xl/sharedStrings.xml` (rich-text run concatenation, `xml:space="preserve"`
  handling, CDATA runs, and the `_x000D_` escape Excel uses for a literal CR)
- `xl/styles.xml` (font size/bold, horizontal alignment, wrap text,
  number format — both the built-in numFmtId table (ECMA-376 §18.8.30) and
  custom `<numFmt>` codes — fill color, kept in its raw `rgb`/
  `theme`+`tint`/`indexed` form (see
  [Resolving display colors](#resolving-display-colors) for converting it
  to a real RGB value), and border presence per side — line style/weight/
  color and `<diagonal>` are not read)
- `xl/theme/theme{N}.xml` (`<clrScheme>`'s 12 colors — read only when a
  style actually references a theme color; see
  [Resolving display colors](#resolving-display-colors))
- `xl/worksheets/sheetX.xml` (`<sheetData>` — including `t="d"` ISO 8601
  date cells alongside the numeric-serial dates every other date/time
  cell uses, both unified into the same `"dateTime"` output —
  `<mergeCells>`, and `<hyperlinks>`, kept raw/unresolved — see the
  `hyperlink` field above)
- `xl/worksheets/_rels/sheetX.xml.rels` (resolves a `<hyperlink r:id="...">`
  to its raw Target string — read only when the sheet declares at least
  one hyperlink with an `r:id`; a `location`-only internal hyperlink never
  triggers this read)
- `xl/drawings/drawingN.xml` and its own `_rels` (cell-anchored embedded
  images — anchor geometry, the embedded media's resolved path, and the
  image's own hyperlink, including images nested in `<xdr:grpSp>` groups;
  see [Embedded images](#embedded-images) above)

`[Content_Types].xml` is not read at all — the workbook part's actual path
is resolved via `_rels/.rels`'s `officeDocument` relationship rather than
assumed to be the conventional `xl/workbook.xml` (Issue #55), but that
resolution never cross-checks a part's declared Content-Type against
`[Content_Types].xml` (see
[pipeline.en.md Open Question 3](docs/design/pipeline.en.md) for the
rationale and the strict-OPC-conformance tradeoff this makes).

## Benchmarks

Parsing performance itself (memory use on a sparse "grid-paper Excel", the
cost on a merge-heavy file, the comparison against `calamine`, etc.) lives
in [`xlsxparser`'s README Benchmarks section](https://github.com/MinamiyamaKotaro/xlsxparser#benchmarks),
which the parser underneath this crate is shared with. Benchmarks specific
to `exceldiff` itself — diff computation, Markdown formatting, grid
rendering — will be added here once there's a need for them.

## Security notes

- **Zip Bomb / Zip Slip / XXE**: guarded against at parse time (see
  [Architecture](#architecture) above and
  [docs/security/design-review.md](docs/security/design-review.md) for the
  full analysis).
- **CSV / formula injection**: cell string values (including formula-computed
  result strings) pass through unchanged, with no escaping at any stage —
  this is safe as JSON output, but callers who re-export parsed values into
  CSV or another spreadsheet format are responsible for their own
  formula-injection mitigations (e.g. escaping a value that starts with `=`,
  `+`, `-`, or `@`), since a `.xlsx` input is untrusted and this library
  performs no rewriting of cell content.

## License

This project is licensed under the GNU Affero General Public License v3.0 (AGPL-3.0). See the [LICENSE](LICENSE) file for details.

### Using it as an Action, and AGPL

Simply invoking this Action unmodified via `uses: MinamiyamaKotaro/exceldiff@<tag>` doesn't place any new AGPL-3.0 obligation on the calling repository or workflow. A composite action just builds and runs this repository's own source on the caller's CI runner — the caller isn't *modifying* the software. AGPL-3.0 §13's network-copyleft clause is an obligation that arises from making a *modified* version available to users over a network; the Action's own Corresponding Source is already available, as this same public repository it's built from.

If you fork and modify this software and then offer *your modified version* as your own Action or service over a network, AGPL-3.0 §13 does require you to make that modified source available to your users — that's AGPL working as intended, not an extra condition specific to this project.

This is a general summary, not legal advice. Consult a lawyer if you need a definitive answer. If you'd rather avoid AGPL-3.0's obligations entirely, see the commercial license below.

### Commercial Licensing

`exceldiff` is dual-licensed: the AGPL-3.0 terms above apply by default, but if you wish to use this software in a closed-source / proprietary system, or otherwise without the copyleft and network-source-disclosure obligations of the AGPL-3.0, a separate commercial license is available.

See [COMMERCIAL-LICENSE.md](COMMERCIAL-LICENSE.md) for what a commercial license covers and how to request one.
