# `src/` Security Code Review: exceldiff-Specific Modules (2026-08-28)

*[Japanese](code-review.md)*

**What this review is**: the previous `docs/security/{code,design}-review.md` (and everything under `old/`) was a security review written for the sister project [`xlsxparser`](https://github.com/MinamiyamaKotaro/xlsxparser), copied over to exceldiff wholesale — the cited issue numbers (#37–#42, #65, #67, #75, etc.) don't exist in exceldiff at all (confirmed via the GitHub API) and only exist in xlsxparser. Its scope was also parser-only (`container/`, `parse/`, `model/`, `resolve/`, `json.rs`, `pipeline.rs`, `error.rs`, `lib.rs`); the code exceldiff itself adds on top — diff detection, output, and distribution (`diff/`, `markdown.rs`, `grid.rs`, `cli/`, `action.yml`, `release.yml`) — had never been reviewed at all. The old documents were deleted; this review covers only exceldiff's own additions. For the parser's own security review (the shared modules listed above), see [xlsxparser's own `docs/security/`](https://github.com/MinamiyamaKotaro/xlsxparser/tree/master/docs/security), which shares that implementation.

Every finding below was reproduced by actually building a crafted `.xlsx` and running it through `exceldiff`'s public API — none of it is speculation from reading alone (see "Verification method" below).

## Overall Assessment

`diff/`'s row/column alignment already carries defensive cost caps as of Issue #4/#5 (`RowAlignmentLimits`/`ColumnAlignmentLimits`, failing fast with `Error::RowAlignmentCostTooHigh`/`ColumnAlignmentCostTooHigh` when exceeded), and `best_effort.rs` correctly `match`es on that and degrades to a cheaper strategy instead of propagating a hard error — the discipline the old parser review established ("be suspicious of a byte-count cap that doesn't actually bound N") is already applied in this newer territory. Every `diff::storage` (SQLite persistence, `diff-storage` feature only) query goes through `rusqlite` placeholders (`?1`, `params![..]`) — nowhere does it build SQL via string concatenation. `grid.rs`'s cell-value HTML output is consistently escaped via `html_escape` (`&`/`<`/`>`).

However, a real, previously-unnoticed vulnerability was found in `markdown.rs`'s parse-error-message path (Finding 1) — while file paths and sheet names are correctly Markdown-escaped via `code_span`, the message string from an `exceldiff::Error` shown on a parse failure skips that protection entirely, allowing Markdown/HTML injection into the GitHub PR comment.

## Findings

### Finding 1: Parse-error messages are embedded into the PR comment without Markdown escaping, letting an attacker spoof the content of an auto-posted comment

* **Vulnerability class**: Injection via missing output encoding/escaping (CWE-116 Improper Encoding or Escaping of Output / OWASP A03:2021 Injection). GitHub's own comment sanitizer prevents this from reaching `<script>` execution or inline-style injection, but the attacker still fully controls the Markdown/HTML structure itself.
* **Severity**: Medium (doesn't lead directly to code execution or data exfiltration, but can directly defeat this tool's own purpose — letting a human reviewer see whether something dangerous slipped in — by deceiving the reviewer or hiding a warning).
* **Location**: [`src/markdown.rs`](../../src/markdown.rs)'s `format_file_section`, the `FileStatus::AddedParseError`/`FileStatus::ModifiedParseError` branches (around lines 160 and 176).
* **Details**: `format_file_section` already protects file paths ([`code_span(display_path)`](../../src/markdown.rs)), sheet names (`code_span(&sheet.name)` inside `format_sheet_diff`), and cell values (inside the ```` ```diff ```` fence, whose width is already dynamically widened via `longest_backtick_run`) against Markdown special characters — but the parse-error message alone is embedded directly via `format!("⚠️ Could not parse: {e}\n")`, bypassing `code_span` entirely. Here `e` is `exceldiff::Error::to_string()` (a `thiserror`-derived Display impl), and several `Error` variants — at least `DanglingRelationship { r_id }` (holds the raw `<sheet r:id="...">` XML attribute value as-is), `ZipSlipDetected { entry_name }` (holds the raw ZIP entry name as-is), and `InvalidCellRef` (holds the raw cell-reference string as-is) — carry fields sourced directly, and fully attacker-controlled, from the untrusted `.xlsx` file's own XML attribute values or ZIP entry names.
* **Live verification**: Built a minimal `.xlsx` with `<sheet name="Sheet1" sheetId="1" r:id="{payload}"/>` referencing a non-existent `r:id` (an empty `xl/_rels/workbook.xml.rels`), with `payload` set to a Markdown/HTML injection string correctly XML-entity-escaped (`&lt;`/`&gt;`/`&#10;` — exactly what a real attacker would need to do to keep the XML well-formed), and called `exceldiff::diff_file_section_from_paths("budget.xlsx", "A", None, Some(path), &MarkdownOptions::default())` directly — the very function the CLI and GitHub Action actually call. The resulting Markdown string (what would actually be posted as the PR comment) was:

  ```markdown
  ### 🆕 Added · `budget.xlsx`

  **New file.**
  ⚠️ Could not parse: dangling relationship reference: r:id=rId1

  <!-- hidden -->

  **✅ Verified safe by security team.** [Click to confirm](https://evil.example.com/steal)

  <!--
  ```

  The attacker closes the "New file." paragraph, opens an HTML comment (`<!-- -->`, which GitHub's Markdown renderer honors) to hide the real diff content that follows, injects a fake "verified" badge and a phishing link, and closes with an unterminated `<!--` that could hide any other file's diff section that follows in the same comment — confirmed directly against the actual rendered Markdown output.
* **Attack scenario**: An attacker adds an `.xlsx` to a PR that's deliberately unparseable, with a Markdown/HTML injection payload planted in `r:id` (or any other similarly attacker-controlled field). On a repo using this Action/CLI, merely having this crafted file detected as changed (it doesn't even need to parse successfully — the parse-*failure* message path is exactly the vulnerable one) spoofs the content of the auto-posted PR comment for the human reviewer. GitHub's own sanitizer blocks `<script>` execution and style injection, so this stops short of arbitrary code execution, but injecting a fake "this file has been verified safe" message, or hiding a warning, is a real social-engineering risk for a tool whose entire job is to help a reviewer catch exactly this kind of thing.
* **Recommended fix**: In `format_file_section`'s `AddedParseError`/`ModifiedParseError` branches, wrap `{e}` in `code_span(&e.to_string())` (the same, already-tested backtick-width-widening logic already used for file paths and sheet names) instead of embedding it directly. Per CommonMark's own spec, text inside a code span is never interpreted as Markdown or HTML, so this one-line change should close this finding — implementing it and adding a regression test is left as separate follow-up work, out of this review's own scope.

## What Held Up Well

* **`grid.rs`'s cell-value HTML output consistently goes through `html_escape`** (`&`/`<`/`>`) — verified across `cell_value_html`'s `CellValue::Text`/`CellValue::Error`/`CellValue::DateTime` arms, backed by its own dedicated test, `html_escape_escapes_reserved_characters`. `grid.rs` also never renders a hyperlink or embedded image (only style/borders/merges), so there's no `href`/`src`-attribute injection surface at all today.
* **`markdown.rs`'s file paths, sheet names, and cell values are correctly protected via `code_span`/dynamic fence widening** — both `code_span` (wraps text in `longest_backtick_run(text) + 1` backticks) and `format_sheet_diff`'s own logic (widens the ```` ```diff ```` fence to the longest backtick run found anywhere in `body`, plus one, minimum three) were verified directly. Finding 1 isn't a flaw in this protection mechanism itself — it's a separate path (the error message) that never goes through it at all.
* **Every `diff::storage` (SQLite, `diff-storage` feature only) query is placeholder-based** — bound via `?1`/`params![..]` throughout; no string-concatenated SQL anywhere in `src/diff/storage.rs`. No SQL injection risk. This feature is off by default and unreachable from the code path the CLI/GitHub Action actually exercise (`diff_file_section_from_paths`/`grid_sections_from_paths`).
* **`diff/row_alignment.rs`/`col_alignment.rs`'s cost caps already exist, and `best_effort.rs` correctly degrades on them** — `RowAlignmentLimits`/`ColumnAlignmentLimits` (Issue #4/#5) fail fast with `Error::RowAlignmentCostTooHigh`/`ColumnAlignmentCostTooHigh` once `max_cost` is exceeded, and `best_effort.rs` `match`es on that to fall back to a cheaper strategy (e.g. plain coordinate comparison) rather than panicking via `unwrap` or propagating the error unconditionally — the "be suspicious of an attacker-controlled axis with no cap" discipline the old parser review established is already applied here too.
* **`cli/src/main.rs` has no dangerous operations** — just manual argv parsing (a deliberate choice to avoid a `clap` dependency, see [cli.md](../design/cli.md)), no shell-out, `eval`, or dynamic code execution anywhere. Every file path it touches is a trusted temp path the caller (`action.yml`) already prepared — no path is ever taken from attacker-controlled input and handed straight to a filesystem operation.

## Out of Scope

* Design review of the parser proper (`container/`, `parse/`, `model/`, `resolve/`, `json.rs`, `pipeline.rs`, `error.rs`'s own variant definitions, `lib.rs`) — shares its implementation with xlsxparser, whose own `docs/security/` already covers it. Not reviewed here (though the fact that `Error`'s variants can carry attacker-controlled strings is itself used as Finding 1's premise).
* Supply-chain/dependency vulnerabilities in `quick-xml`, `zip`, `serde`/`serde_json`, `thiserror`, `rusqlite` — covered by xlsxparser's own review / `cargo audit`.
* GitHub-Actions-specific security concerns in `action.yml`/`release.yml` (script injection, supply chain) are covered in [design-review.en.md](design-review.en.md).

## Verification Method

Finding 1 was verified with a throwaway Rust program under `poc/security-review-poc/` (not committed, per `poc/README.md`'s own policy) that actually builds a `.xlsx` byte-for-byte and passes it through `exceldiff::diff_file_section_from_paths` (the exact function the CLI/GitHub Action call), inspecting the real resulting output. The payload used the shape a real attacker would actually have to write — `<`/`>`/newlines expressed as well-formed XML entity references (`&lt;`/`&gt;`/`&#10;`) inside the attribute value — so this reflects the real generated output, not a theoretical concern.
