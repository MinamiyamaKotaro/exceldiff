# `.xlsx` Diff Tool Requirements Specification

*[Japanese](requirements.md)*

## 0. Implementation Language

Rust

## 1. Project Overview

`.xlsx` files routinely end up under git management and PR review in Japanese business systems, but `git diff` returns nothing meaningful for them — `.xlsx` is a collection of ZIP-compressed XML parts, so even a single-cell edit can reorder the shared-string table and change the entire ZIP compression output, leaving git with nothing but an opaque binary diff.

This project closes that gap: it parses the before/after `.xlsx` files into two `Workbook`s, diffs them cell by cell (added/modified/deleted), and consistently delivers the result — as Markdown text or an Excel-like HTML grid view — through a library, a CLI, and a GitHub Action, all summarized into a PR comment. The target is the shapes that come up constantly in Japanese business systems: "grid-paper Excel" (sheets with an extreme number of rows/columns) and files that make heavy use of merged cells.

## 2. Relationship to the Underlying Parser

This project's OOXML parsing (ZIP extraction, sanitization, streaming parse, shared-string/style resolution, merged-cell resolution) is built on the same design and implementation as the sister project [`xlsxparser`](https://github.com/MinamiyamaKotaro/xlsxparser). The detailed requirements for parsing itself (the 5-phase pipeline, sparse-matrix memory optimization, transparent merged-cell access, etc.) live in `xlsxparser`'s own requirements specification — this document defines only what's specific to this project, built on top of that: diff detection, output, and distribution.

The parser's low memory footprint and speed matter to this project for more than just being a lightweight parser in its own right — **they translate directly into the diff engine's own speed and accuracy**. The time and memory complexity of comparing two versions of a cell-heavy sheet depends heavily on how sparse the data structure the parser hands back actually is.

## 3. Diff Engine Requirements

### 3.1 Cell-Level Diff Detection

- Compare two `Workbook`s (before/after) and, per sheet, detect added, modified, and deleted cells.
- Plain coordinate-based comparison (matching the same `(row, col)` on both sides) is the baseline strategy.

### 3.2 Row/Column Alignment Detection (Avoiding False Positives)

- Coordinate-based comparison alone means a single row or column inserted partway through a sheet shifts every subsequent cell's coordinates, falsely reporting all of them as modified.
- Row-alignment and column-alignment detection identify inserted/deleted rows and columns from content-similarity, so cells that didn't actually change aren't reported as `Modified`.
- A "best-effort" mode — auto-selecting whichever of coordinate/row-aligned/column-aligned comparison reports the fewest changes — is the default, with a switch to plain coordinate comparison (skipping alignment detection) also available.

### 3.3 Change Kinds Covered

Beyond cell values, diff detection also covers:

- Cell styling (font, alignment, number format, fill color, borders)
- Merged-cell ranges and state
- Embedded images (anchor position, hyperlink)
- Cell hyperlinks
- Column width / row height
- Sheet visibility (visible/hidden/very-hidden)

## 4. Output Format Requirements

### 4.1 Markdown Text Diff (for GitHub PR comments)

- Output as GitHub-Flavored Markdown, pastable directly into a PR comment.
- Contains no decorative HTML/CSS at all, so nothing is lost when it passes through GitHub's comment sanitizer.
- The number of cell-change hunks rendered per sheet must be cappable — so one enormous change doesn't blow up the comment itself.

### 4.2 Excel-Like Grid View (HTML)

- For each changed sheet, generate an Excel-like grid as HTML, with Before and After laid out side by side.
- Rendered as real HTML rather than a screenshot image, so large sheets don't get shrunk into illegibility — the result scrolls and zooms like any other web page in a browser.

## 5. Distribution/Packaging Requirements

### 5.1 CLI

- Given one changed file's git status (added/modified/deleted) and the actual before/after file paths, write that file's diff to stdout as a Markdown string.
- One file's parse error must not stop the rest of the files' diffs from being shown — the error itself is emitted as part of the diff output.

### 5.2 GitHub Action

- For every `.xlsx` file a PR changes, automatically post (or update) a single PR comment summarizing the above CLI's output for all of them.
- Let the caller customize which `.xlsx` files are targeted, whether to post a comment, the per-sheet display cap, and the diff strategy (plain coordinate vs. best-effort auto).
- Beyond building from source on every invocation, leave room for a faster path via downloading a pre-built binary.
- Beyond the PR's cumulative diff (the default), allow switching to a per-commit breakdown of the diffs that make up the PR.

## 6. Security Requirements

- Assume the incoming `.xlsx` is untrusted; the parser's own Zip Bomb/Zip Slip/XXE countermeasures are inherited as-is during parsing.
- A cell's string value (including a formula's computed result string) passes through unescaped at every stage of the diff output — safe for JSON/Markdown output, but a caller that re-emits the diff result into CSV or another spreadsheet format is responsible for its own formula-injection countermeasures.
