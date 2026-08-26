// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only

//! Formats a [`WorkbookDiff`] (plus the enclosing file's git status) as a
//! GitHub-flavored Markdown section, for the `.xlsx` diff preview
//! `xlsx-diff.yml` posts to a PR (Issue #22, #31). Extracted out of the
//! CLI (`examples/xlsx_diff_cli.rs`), which used to build this same
//! Markdown by writing directly to stdout via `println!` — the only way
//! to check that output was to run the compiled binary and capture
//! stdout. Every function here instead returns a `String`, so the
//! diff→Markdown mapping is unit-testable on its own (`tests` below), no
//! process spawn needed.
//!
//! Per-cell changes render as a ` ```diff ` fence (`format_sheet_diff`) —
//! one `@@ <A1 coord> @@` hunk per change, `-`/`+` lines for the old/new
//! value — rather than a Markdown table. GitHub applies the same
//! red/green backgrounds a real `git diff` gets to a `-`/`+` line inside
//! a ` ```diff ` fence; it does not apply background colors to Markdown
//! table cells at all (GitHub's sanitizer strips a `style=` attribute
//! from PR-comment HTML), so a table could never carry this coloring no
//! matter how it was built. A merged-region change (Issue #8's
//! `SheetDiff::merges`) gets its own hunk in the same fence
//! (`format_merge_hunk`), tagged `(merge)` on the header line to keep it
//! visually distinct from a cell-value hunk.

use crate::diff::{
    diff_workbooks, diff_workbooks_best_effort, CellDiff, CellPos, ColumnAlignmentLimits,
    DiffStatus, MergeDiff, RowAlignmentLimits, SheetDiff, WorkbookDiff,
};
use crate::json::JsonCellValue;
use crate::model::CellRef;
use crate::parse_workbook;

/// Which diffing strategy [`diff_file_section_from_paths`] uses for a
/// modified file (Issue #24, an `action.yml` `diff-mode` input). See
/// [`diff_workbooks_best_effort`]'s and [`diff_workbooks`]'s own doc
/// comments for what each algorithm actually does — this only selects
/// between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiffMode {
    /// Per sheet, picks whichever of coordinate-based, row-aligned, or
    /// column-aligned diffing reports the fewest changes
    /// ([`diff_workbooks_best_effort`], Issue #25). The default — matches
    /// every caller's behavior from before this field existed.
    #[default]
    Auto,
    /// Plain coordinate-based diffing ([`diff_workbooks`]), skipping
    /// row/column alignment detection entirely. Cheaper, and avoids
    /// alignment's own false-positive risk, at the cost of the cascading
    /// diff noise a shifted row/column produces (see
    /// [`diff_workbooks_best_effort`]'s doc comment for that cascade).
    Coordinate,
}

/// Tunables for [`format_file_section`]. `max_rows_per_sheet` was a
/// hardcoded `MAX_ROWS_PER_SHEET` constant in the CLI; pulling it out as
/// a field lets a caller (e.g. a future action.yml input, Issue #24)
/// change the cap without a code change.
#[derive(Debug, Clone, Copy)]
pub struct MarkdownOptions {
    /// Caps the number of cell hunks rendered per sheet, so one huge
    /// fixture rewrite can't blow up the rendered output size. Never caps
    /// merge hunks — see [`format_sheet_diff`]'s doc comment for why.
    pub max_rows_per_sheet: usize,
    /// Which diffing strategy to use for a modified file. See [`DiffMode`].
    pub diff_mode: DiffMode,
}

impl Default for MarkdownOptions {
    fn default() -> Self {
        Self {
            max_rows_per_sheet: 30,
            diff_mode: DiffMode::default(),
        }
    }
}

/// Sheet/cell counts for a newly-added file. Computed by the caller from
/// the parsed head-revision `Workbook` (via [`crate::parse_workbook`]) —
/// this module only ever formats already-computed diff/summary data, it
/// never parses a file itself, so it has no dependency on the pipeline
/// beyond the [`crate::diff`] types it renders.
#[derive(Debug, Clone, Copy)]
pub struct AddedSummary {
    pub sheet_count: usize,
    pub cell_count: usize,
}

/// Which revision failed to parse, for [`FileStatus::ModifiedParseError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionSide {
    Base,
    Head,
}

/// What happened to one file between two revisions, as needed to render
/// its Markdown section. Mirrors [`CellDiff::old_value`]/`new_value`'s
/// "carry the outcome, including the error case, as data" convention
/// rather than a plain `Result`, since [`FileStatus::Unrecognized`] is a
/// fourth, non-error case the original CLI also handled (an unrecognized
/// git status letter).
pub enum FileStatus<'a> {
    /// Git status `A`. `Some` mirrors a successfully-parsed head revision
    /// (counts known); `None` mirrors the CLI's `head_path`-absent early
    /// return (no file content available to summarize at all).
    Added(Option<AddedSummary>),
    /// Git status `A`, but the head revision failed to parse.
    AddedParseError(&'a str),
    /// Git status `D`.
    Deleted,
    /// Git status `M`, both revisions parsed and diffed.
    Modified(&'a WorkbookDiff),
    /// Git status `M`, but base and/or head content was unavailable.
    ModifiedMissingContent,
    /// Git status `M`, but one revision failed to parse.
    ModifiedParseError(RevisionSide, &'a str),
    /// Any other git status letter.
    Unrecognized(&'a str),
}

/// A short, scannable status label for the file heading. Without this, a
/// bare `### \`path\`` heading forces the reader to read the body text
/// below it to learn whether the file was added, changed, or removed —
/// noticeable once a PR touches several `.xlsx` files at once, since
/// nothing about the path itself says what happened to it.
fn file_status_badge(status: &FileStatus) -> &'static str {
    match status {
        FileStatus::Added(_) | FileStatus::AddedParseError(_) => "🆕 Added",
        FileStatus::Deleted => "🗑️ Deleted",
        FileStatus::Modified(_)
        | FileStatus::ModifiedMissingContent
        | FileStatus::ModifiedParseError(..) => "✏️ Modified",
        FileStatus::Unrecognized(_) => "❓ Unrecognized",
    }
}

/// Renders one file's Markdown section: the `### <badge> · \`{display_path}\``
/// heading plus a status-dependent body. `display_path` is the path shown
/// in the heading (the file's path in the repo, not necessarily a real
/// filesystem path — the caller controls this, e.g. to relativize it).
pub fn format_file_section(
    display_path: &str,
    status: &FileStatus,
    options: &MarkdownOptions,
) -> String {
    let badge = file_status_badge(status);
    let mut out = format!("### {badge} · {}\n\n", code_span(display_path));
    match status {
        FileStatus::Added(summary) => {
            out.push_str("**New file.**\n");
            if let Some(s) = summary {
                out.push_str(&format!(
                    "{} sheet(s), {} total cell(s).\n",
                    s.sheet_count, s.cell_count
                ));
            }
        }
        FileStatus::AddedParseError(e) => {
            out.push_str("**New file.**\n");
            out.push_str(&format!("⚠️ Could not parse: {e}\n"));
        }
        FileStatus::Deleted => {
            out.push_str("**File removed.**\n");
        }
        FileStatus::Modified(diff) => {
            out.push_str(&format_workbook_diff(diff, options));
        }
        FileStatus::ModifiedMissingContent => {
            out.push_str("_Missing before/after content, skipped._\n");
        }
        FileStatus::ModifiedParseError(side, e) => {
            let label = match side {
                RevisionSide::Base => "the previous version",
                RevisionSide::Head => "the new version",
            };
            out.push_str(&format!("⚠️ Could not parse {label}: {e}\n"));
        }
        FileStatus::Unrecognized(other) => {
            out.push_str(&format!("_Unrecognized git status `{other}`, skipped._\n"));
        }
    }
    out
}

/// Renders a full [`WorkbookDiff`] as Markdown — one section per changed
/// sheet (`WorkbookDiff::sheets` already omits any sheet with nothing to
/// report, per [`SheetDiff`]'s own doc comment), in order.
pub fn format_workbook_diff(diff: &WorkbookDiff, options: &MarkdownOptions) -> String {
    let mut out = String::new();
    if diff.sheets.is_empty() {
        out.push_str("_No differences detected._\n");
        return out;
    }
    for sheet in &diff.sheets {
        out.push_str(&format_sheet_diff(sheet, options));
    }
    out
}

/// Renders one sheet's changes: a `**Sheet \`name\`**` summary line, then
/// (if there's anything to show) a single ` ```diff ` fence holding one
/// `@@ <coord> @@` hunk per cell change followed by one per merge change.
/// Cell hunks are capped at `options.max_rows_per_sheet` (a full-sheet
/// rewrite can produce thousands); merge hunks never are — a sheet
/// carries at most a handful of merged regions even in a heavily-merged
/// grid-paper form, nowhere near the volume a cell edit list can reach.
fn format_sheet_diff(sheet: &SheetDiff, options: &MarkdownOptions) -> String {
    let mut out = String::new();

    let added = count(sheet, DiffStatus::Added);
    // A merged region appearing, disappearing, or resizing is a
    // structural edit to the sheet, not a cell value edit — there's no
    // separate "merges changed" slot in this summary line, so it's
    // lumped into "modified" rather than split into "added"/"deleted" by
    // the merge's own status: a merge that vanished didn't remove any
    // *cell*, it changed how existing cells are grouped, which reads as
    // a modification either way you look at it.
    let modified = count(sheet, DiffStatus::Modified) + sheet.merges.len();
    let deleted = count(sheet, DiffStatus::Deleted);

    let sheet_note = match sheet.status {
        DiffStatus::Added => " (sheet added)",
        DiffStatus::Deleted => " (sheet removed)",
        DiffStatus::Modified => "",
    };
    out.push_str(&format!(
        "**Sheet {}{sheet_note}** — {added} added, {modified} modified, {deleted} deleted\n",
        code_span(&sheet.name)
    ));
    if let (Some(old_v), Some(new_v)) = (sheet.old_visibility, sheet.new_visibility) {
        out.push_str(&format!("_Visibility: `{old_v}` → `{new_v}`_\n"));
    }
    out.push('\n');

    if sheet.cells.is_empty() && sheet.merges.is_empty() {
        return out;
    }

    let mut body = String::new();
    for cell in sheet.cells.iter().take(options.max_rows_per_sheet) {
        body.push_str(&format_cell_hunk(cell));
    }
    for merge in &sheet.merges {
        body.push_str(&format_merge_hunk(merge));
    }
    // A cell's rendered value can itself contain a run of backticks (an
    // `Error` value is wrapped in single backticks; `Text` isn't escaped
    // for backticks at all, only for `\n`). A hardcoded ` ```diff ` fence
    // would let such a run terminate the fence early once it's at least
    // as long as the fence itself, corrupting everything rendered after
    // it in the PR comment. Widening the fence past the longest backtick
    // run actually present in `body` closes this off, the same way
    // `code_span` widens an inline span's delimiter.
    let fence = "`".repeat((longest_backtick_run(&body) + 1).max(3));
    out.push_str(&fence);
    out.push_str("diff\n");
    out.push_str(&body);
    out.push_str(&fence);
    out.push('\n');
    if sheet.cells.len() > options.max_rows_per_sheet {
        out.push_str(&format!(
            "_...and {} more change(s) in this sheet._\n",
            sheet.cells.len() - options.max_rows_per_sheet
        ));
    }
    out.push('\n');
    out
}

fn format_cell_hunk(cell: &CellDiff) -> String {
    let coord = CellRef {
        row: cell.row,
        col: cell.col,
    }
    .to_a1();
    let mut out = format!("@@ {coord} @@\n");
    if let Some(old) = cell.old_value.as_ref() {
        out.push_str(&format!("- {}\n", format_value(old)));
    }
    if let Some(new) = cell.new_value.as_ref() {
        out.push_str(&format!("+ {}\n", format_value(new)));
    }
    out
}

/// Renders one merged-region change as a diff hunk, matching
/// `format_cell_hunk`'s `@@ coord @@` / `-`/`+` shape so it picks up the
/// same GitHub diff-fence coloring. The `(merge)` tag on the hunk header
/// is the only thing distinguishing it from a cell-value hunk at a
/// glance — `@@ B1:F1 @@` alone could otherwise read as some kind of
/// cell-range notation rather than a structural change.
fn format_merge_hunk(merge: &MergeDiff) -> String {
    let start = CellRef {
        row: merge.start.row,
        col: merge.start.col,
    }
    .to_a1();
    match merge.status {
        DiffStatus::Added => {
            let end = merge.new_end.map(merge_pos_to_a1).unwrap_or_default();
            format!("@@ {start}:{end} (merge) @@\n+ merged\n")
        }
        DiffStatus::Deleted => {
            let end = merge.old_end.map(merge_pos_to_a1).unwrap_or_default();
            format!("@@ {start}:{end} (merge) @@\n- merged\n")
        }
        DiffStatus::Modified => {
            let mut out = format!("@@ {start} (merge) @@\n");
            if let Some(old_end) = merge.old_end.map(merge_pos_to_a1) {
                out.push_str(&format!("- merged {start}:{old_end}\n"));
            }
            if let Some(new_end) = merge.new_end.map(merge_pos_to_a1) {
                out.push_str(&format!("+ merged {start}:{new_end}\n"));
            }
            out
        }
    }
}

fn merge_pos_to_a1(pos: CellPos) -> String {
    CellRef {
        row: pos.row,
        col: pos.col,
    }
    .to_a1()
}

fn count(sheet: &SheetDiff, status: DiffStatus) -> usize {
    sheet.cells.iter().filter(|c| c.status == status).count()
}

/// Renders one cell value for a `-`/`+` diff line — escaping a `\n` or
/// `\r` embedded in a text value, either of which some Markdown renderers
/// treat as a line break on its own, which would otherwise spill the
/// value across multiple lines and desync the diff fence's
/// one-line-per-value shape. `|` needs no escaping here (unlike a
/// Markdown table cell), since a diff line isn't Markdown table syntax.
fn format_value(v: &JsonCellValue) -> String {
    match v {
        JsonCellValue::Number(n) => n.to_string(),
        JsonCellValue::Boolean(b) => b.to_string(),
        JsonCellValue::DateTime(s) => s.clone(),
        JsonCellValue::Error(e) => format!("`{e}`"),
        JsonCellValue::Empty => "_(empty)_".to_string(),
        JsonCellValue::Text(s) => format!("\"{}\"", escape_diff_value(s)),
    }
}

fn escape_diff_value(s: &str) -> String {
    s.replace('\r', "\\r").replace('\n', "\\n")
}

/// Wraps `text` in Markdown inline-code backticks, safe for text that may
/// itself contain backticks (a file path, a sheet name — both effectively
/// caller/user controlled). Per CommonMark's code span rule, the delimiter
/// must use more consecutive backticks than the longest backtick run found
/// in `text`, and a padding space on each side is required whenever `text`
/// starts or ends with a backtick so the delimiter doesn't fuse with it.
fn code_span(text: &str) -> String {
    let fence = "`".repeat(longest_backtick_run(text) + 1);
    if text.starts_with('`') || text.ends_with('`') {
        format!("{fence} {text} {fence}")
    } else {
        format!("{fence}{text}{fence}")
    }
}

/// The length of the longest run of consecutive backticks in `text`, or 0
/// if it contains none. Shared by [`code_span`] (an inline span's own
/// delimiter) and [`format_sheet_diff`] (the ` ```diff ` block fence
/// wrapping a sheet's rendered hunks) — both need a fence strictly longer
/// than anything it's meant to contain, or the fence closes early.
fn longest_backtick_run(text: &str) -> usize {
    let mut longest_run = 0usize;
    let mut current_run = 0usize;
    for ch in text.chars() {
        if ch == '`' {
            current_run += 1;
            longest_run = longest_run.max(current_run);
        } else {
            current_run = 0;
        }
    }
    longest_run
}

/// High-level, path-based entry point for the `.xlsx` diff CLI (Issue
/// #32): given a git status letter and the file paths a caller has
/// already resolved (e.g. `.github/workflows/xlsx-diff.yml`, extracting
/// the base/head git revisions via `git show` into temp files before
/// invoking the `cli/` crate's `xlsxdiff` binary), does the
/// parsing (`parse_workbook`), diffing (`diff_workbooks_best_effort`,
/// Issue #25), and Markdown rendering (`format_file_section`) in one
/// call. This is the one place that orchestration lives now — the CLI
/// itself only turns argv into these five arguments and writes the
/// returned `String` to stdout, no process spawn needed to exercise it
/// (see `tests` below).
pub fn diff_file_section_from_paths(
    display_path: &str,
    git_status: &str,
    base_path: Option<&str>,
    head_path: Option<&str>,
    options: &MarkdownOptions,
) -> String {
    match git_status {
        "A" => {
            let Some(head_path) = head_path else {
                return format_file_section(display_path, &FileStatus::Added(None), options);
            };
            match parse_workbook(head_path) {
                Ok(wb) => {
                    let summary = AddedSummary {
                        sheet_count: wb.sheets().len(),
                        cell_count: wb.sheets().iter().map(|s| s.iter_cells().count()).sum(),
                    };
                    format_file_section(display_path, &FileStatus::Added(Some(summary)), options)
                }
                Err(e) => {
                    let message = e.to_string();
                    format_file_section(
                        display_path,
                        &FileStatus::AddedParseError(&message),
                        options,
                    )
                }
            }
        }
        "D" => format_file_section(display_path, &FileStatus::Deleted, options),
        "M" => {
            let (Some(base_path), Some(head_path)) = (base_path, head_path) else {
                return format_file_section(
                    display_path,
                    &FileStatus::ModifiedMissingContent,
                    options,
                );
            };
            let base = match parse_workbook(base_path) {
                Ok(wb) => wb,
                Err(e) => {
                    let message = e.to_string();
                    return format_file_section(
                        display_path,
                        &FileStatus::ModifiedParseError(RevisionSide::Base, &message),
                        options,
                    );
                }
            };
            let head = match parse_workbook(head_path) {
                Ok(wb) => wb,
                Err(e) => {
                    let message = e.to_string();
                    return format_file_section(
                        display_path,
                        &FileStatus::ModifiedParseError(RevisionSide::Head, &message),
                        options,
                    );
                }
            };
            let diff = match options.diff_mode {
                DiffMode::Auto => diff_workbooks_best_effort(
                    &base,
                    &head,
                    RowAlignmentLimits::default(),
                    ColumnAlignmentLimits::default(),
                ),
                DiffMode::Coordinate => diff_workbooks(&base, &head),
            };
            format_file_section(display_path, &FileStatus::Modified(&diff), options)
        }
        other => format_file_section(display_path, &FileStatus::Unrecognized(other), options),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_diff() -> WorkbookDiff {
        WorkbookDiff {
            sheets: vec![SheetDiff {
                name: "Sheet1".to_string(),
                status: DiffStatus::Modified,
                old_visibility: None,
                new_visibility: None,
                merges: Vec::new(),
                cells: vec![CellDiff {
                    row: 1,
                    col: 1,
                    status: DiffStatus::Modified,
                    old_col: None,
                    old_row: None,
                    old_value: Some(JsonCellValue::Number(1.0)),
                    new_value: Some(JsonCellValue::Number(2.0)),
                    old_style: None,
                    new_style: None,
                }],
            }],
        }
    }

    // No process spawn, no stdout capture, no fixture .xlsx file — this
    // is exactly the unit test Issue #31 asks for ("プロセスを起動せず
    // `WorkbookDiff`を渡して文字列を検証").
    #[test]
    fn format_workbook_diff_renders_a_changed_cell() {
        let diff = sample_diff();
        let md = format_workbook_diff(&diff, &MarkdownOptions::default());
        assert!(md.contains("**Sheet `Sheet1`** — 0 added, 1 modified, 0 deleted"));
        assert!(md.contains("```diff\n@@ A1 @@\n- 1\n+ 2\n```\n"));
    }

    #[test]
    fn format_workbook_diff_reports_no_differences() {
        let diff = WorkbookDiff { sheets: Vec::new() };
        let md = format_workbook_diff(&diff, &MarkdownOptions::default());
        assert_eq!(md, "_No differences detected._\n");
    }

    #[test]
    fn max_rows_per_sheet_caps_cell_hunks_and_notes_the_remainder() {
        let mut diff = sample_diff();
        diff.sheets[0].cells = (1..=5)
            .map(|i| CellDiff {
                row: i,
                col: 1,
                status: DiffStatus::Added,
                old_col: None,
                old_row: None,
                old_value: None,
                new_value: Some(JsonCellValue::Number(i as f64)),
                old_style: None,
                new_style: None,
            })
            .collect();
        let options = MarkdownOptions {
            max_rows_per_sheet: 2,
            ..MarkdownOptions::default()
        };
        let md = format_workbook_diff(&diff, &options);
        assert_eq!(md.matches("@@ ").count(), 2);
        assert!(md.contains("_...and 3 more change(s) in this sheet._"));
    }

    #[test]
    fn format_workbook_diff_renders_merge_hunks() {
        let diff = WorkbookDiff {
            sheets: vec![SheetDiff {
                name: "報告書".to_string(),
                status: DiffStatus::Modified,
                old_visibility: None,
                new_visibility: None,
                cells: Vec::new(),
                merges: vec![
                    MergeDiff {
                        status: DiffStatus::Added,
                        start: CellPos { row: 3, col: 7 },
                        old_end: None,
                        new_end: Some(CellPos { row: 3, col: 8 }),
                    },
                    MergeDiff {
                        status: DiffStatus::Deleted,
                        start: CellPos { row: 3, col: 1 },
                        old_end: Some(CellPos { row: 4, col: 1 }),
                        new_end: None,
                    },
                    MergeDiff {
                        status: DiffStatus::Modified,
                        start: CellPos { row: 1, col: 2 },
                        old_end: Some(CellPos { row: 1, col: 5 }),
                        new_end: Some(CellPos { row: 1, col: 6 }),
                    },
                ],
            }],
        };
        let md = format_workbook_diff(&diff, &MarkdownOptions::default());
        // 0 cell-level changes but 3 merge changes, all counted as "modified".
        assert!(md.contains("**Sheet `報告書`** — 0 added, 3 modified, 0 deleted"));
        assert!(md.contains("@@ G3:H3 (merge) @@\n+ merged\n"));
        assert!(md.contains("@@ A3:A4 (merge) @@\n- merged\n"));
        assert!(md.contains("@@ B1 (merge) @@\n- merged B1:E1\n+ merged B1:F1\n"));
    }

    #[test]
    fn format_workbook_diff_notes_added_and_removed_sheets() {
        let added_sheet = WorkbookDiff {
            sheets: vec![SheetDiff {
                name: "New".to_string(),
                status: DiffStatus::Added,
                old_visibility: None,
                new_visibility: None,
                merges: Vec::new(),
                cells: Vec::new(),
            }],
        };
        let md = format_workbook_diff(&added_sheet, &MarkdownOptions::default());
        assert!(md.contains("**Sheet `New` (sheet added)**"));

        let removed_sheet = WorkbookDiff {
            sheets: vec![SheetDiff {
                name: "Old".to_string(),
                status: DiffStatus::Deleted,
                old_visibility: None,
                new_visibility: None,
                merges: Vec::new(),
                cells: Vec::new(),
            }],
        };
        let md = format_workbook_diff(&removed_sheet, &MarkdownOptions::default());
        assert!(md.contains("**Sheet `Old` (sheet removed)**"));
    }

    #[test]
    fn format_cell_hunk_handles_deleted_cell_with_no_new_value() {
        let diff = WorkbookDiff {
            sheets: vec![SheetDiff {
                name: "Sheet1".to_string(),
                status: DiffStatus::Modified,
                old_visibility: None,
                new_visibility: None,
                merges: Vec::new(),
                cells: vec![CellDiff {
                    row: 1,
                    col: 1,
                    status: DiffStatus::Deleted,
                    old_col: None,
                    old_row: None,
                    old_value: Some(JsonCellValue::Number(9.0)),
                    new_value: None,
                    old_style: None,
                    new_style: None,
                }],
            }],
        };
        let md = format_workbook_diff(&diff, &MarkdownOptions::default());
        assert!(md.contains("@@ A1 @@\n- 9\n```"));
    }

    #[test]
    fn format_value_covers_every_json_cell_value_variant() {
        assert_eq!(format_value(&JsonCellValue::Boolean(true)), "true");
        assert_eq!(
            format_value(&JsonCellValue::DateTime("2024-01-01T00:00:00".to_string())),
            "2024-01-01T00:00:00"
        );
        assert_eq!(
            format_value(&JsonCellValue::Error("#DIV/0!".to_string())),
            "`#DIV/0!`"
        );
        assert_eq!(format_value(&JsonCellValue::Empty), "_(empty)_");
    }

    #[test]
    fn format_sheet_diff_reports_visibility_change() {
        let diff = WorkbookDiff {
            sheets: vec![SheetDiff {
                name: "Sheet1".to_string(),
                status: DiffStatus::Modified,
                old_visibility: Some("visible"),
                new_visibility: Some("hidden"),
                merges: Vec::new(),
                cells: Vec::new(),
            }],
        };
        let md = format_workbook_diff(&diff, &MarkdownOptions::default());
        assert!(md.contains("_Visibility: `visible` → `hidden`_"));
    }

    #[test]
    fn format_file_section_added_renders_summary() {
        let status = FileStatus::Added(Some(AddedSummary {
            sheet_count: 2,
            cell_count: 10,
        }));
        let md = format_file_section("book.xlsx", &status, &MarkdownOptions::default());
        assert!(md.starts_with("### 🆕 Added · `book.xlsx`\n\n**New file.**\n"));
        assert!(md.contains("2 sheet(s), 10 total cell(s)."));
    }

    #[test]
    fn format_file_section_added_with_no_summary() {
        let md = format_file_section(
            "book.xlsx",
            &FileStatus::Added(None),
            &MarkdownOptions::default(),
        );
        assert_eq!(md, "### 🆕 Added · `book.xlsx`\n\n**New file.**\n");
    }

    #[test]
    fn format_file_section_added_parse_error() {
        let md = format_file_section(
            "book.xlsx",
            &FileStatus::AddedParseError("bad zip"),
            &MarkdownOptions::default(),
        );
        assert!(md.contains("**New file.**\n"));
        assert!(md.contains("⚠️ Could not parse: bad zip\n"));
    }

    #[test]
    fn format_file_section_deleted() {
        let md = format_file_section(
            "book.xlsx",
            &FileStatus::Deleted,
            &MarkdownOptions::default(),
        );
        assert_eq!(md, "### 🗑️ Deleted · `book.xlsx`\n\n**File removed.**\n");
    }

    #[test]
    fn format_file_section_modified_embeds_workbook_diff() {
        let diff = sample_diff();
        let status = FileStatus::Modified(&diff);
        let md = format_file_section("book.xlsx", &status, &MarkdownOptions::default());
        assert!(md.starts_with("### ✏️ Modified · `book.xlsx`\n\n"));
        assert!(md.contains("```diff\n@@ A1 @@\n- 1\n+ 2\n```\n"));
    }

    #[test]
    fn format_file_section_modified_missing_content() {
        let md = format_file_section(
            "book.xlsx",
            &FileStatus::ModifiedMissingContent,
            &MarkdownOptions::default(),
        );
        assert!(md.contains("_Missing before/after content, skipped._\n"));
    }

    #[test]
    fn format_file_section_modified_parse_error_names_the_failing_side() {
        let base_md = format_file_section(
            "book.xlsx",
            &FileStatus::ModifiedParseError(RevisionSide::Base, "corrupt"),
            &MarkdownOptions::default(),
        );
        assert!(base_md.contains("⚠️ Could not parse the previous version: corrupt\n"));

        let head_md = format_file_section(
            "book.xlsx",
            &FileStatus::ModifiedParseError(RevisionSide::Head, "corrupt"),
            &MarkdownOptions::default(),
        );
        assert!(head_md.contains("⚠️ Could not parse the new version: corrupt\n"));
    }

    #[test]
    fn format_file_section_unrecognized_status() {
        let md = format_file_section(
            "book.xlsx",
            &FileStatus::Unrecognized("R100"),
            &MarkdownOptions::default(),
        );
        assert!(md.starts_with("### ❓ Unrecognized · `book.xlsx`\n\n"));
        assert!(md.contains("_Unrecognized git status `R100`, skipped._\n"));
    }

    #[test]
    fn escapes_newline_in_text_values() {
        let v = JsonCellValue::Text(std::sync::Arc::from("a|b\nc"));
        assert_eq!(format_value(&v), "\"a|b\\nc\"");
    }

    #[test]
    fn escapes_carriage_return_in_text_values() {
        // Some Markdown renderers treat a bare `\r` as a line break the
        // same way `\n` is, which would otherwise let a text value spill
        // across lines and desync the diff fence's one-line-per-value
        // shape — so `\r` needs the same escaping `\n` already gets.
        let v = JsonCellValue::Text(std::sync::Arc::from("a\rb\r\nc"));
        assert_eq!(format_value(&v), "\"a\\rb\\r\\nc\"");
    }

    #[test]
    fn does_not_escape_pipe_in_text_values() {
        // Unlike the Markdown-table format this replaced, a diff line
        // isn't table syntax, so `|` needs no escaping.
        let v = JsonCellValue::Text(std::sync::Arc::from("a|b"));
        assert_eq!(format_value(&v), "\"a|b\"");
    }

    #[test]
    fn code_span_wraps_plain_text_with_single_backticks() {
        assert_eq!(code_span("Sheet1"), "`Sheet1`");
    }

    #[test]
    fn code_span_widens_the_fence_past_an_embedded_backtick_run() {
        assert_eq!(code_span("a`b"), "``a`b``");
        assert_eq!(code_span("a``b"), "```a``b```");
    }

    #[test]
    fn code_span_pads_when_content_starts_or_ends_with_a_backtick() {
        assert_eq!(code_span("`leading"), "`` `leading ``");
        assert_eq!(code_span("trailing`"), "`` trailing` ``");
    }

    #[test]
    fn format_sheet_diff_widens_the_diff_fence_past_a_cell_error_values_backticks() {
        // An `Error` value renders as `` `{e}` ``; an error message that
        // itself ends in backticks pushes a 4-long run right up against
        // the value's own closing backtick, which a fixed 3-backtick
        // fence could not contain without closing early.
        let diff = WorkbookDiff {
            sheets: vec![SheetDiff {
                name: "Sheet1".to_string(),
                status: DiffStatus::Modified,
                old_visibility: None,
                new_visibility: None,
                merges: Vec::new(),
                cells: vec![CellDiff {
                    row: 1,
                    col: 1,
                    status: DiffStatus::Modified,
                    old_col: None,
                    old_row: None,
                    old_value: None,
                    new_value: Some(JsonCellValue::Error("err```".to_string())),
                    old_style: None,
                    new_style: None,
                }],
            }],
        };
        let md = format_workbook_diff(&diff, &MarkdownOptions::default());
        assert!(md.contains("\n`````diff\n@@ A1 @@\n+ `err````\n`````\n"));
    }

    #[test]
    fn format_file_section_escapes_backticks_in_the_display_path() {
        let out = format_file_section(
            "a`b.xlsx",
            &FileStatus::Deleted,
            &MarkdownOptions::default(),
        );
        assert!(out.starts_with("### 🗑️ Deleted · ``a`b.xlsx``\n"));
    }

    #[test]
    fn format_sheet_diff_escapes_backticks_in_the_sheet_name() {
        let sheet = SheetDiff {
            name: "a`b".to_string(),
            status: DiffStatus::Modified,
            cells: vec![],
            merges: vec![],
            old_visibility: None,
            new_visibility: None,
        };
        let out = format_sheet_diff(&sheet, &MarkdownOptions::default());
        assert!(out.starts_with("**Sheet ``a`b``** — 0 added, 0 modified, 0 deleted\n"));
    }

    // --- diff_file_section_from_paths (Issue #32) ---
    //
    // Unlike the tests above, `diff_file_section_from_paths` is a
    // path-based orchestrator (it calls `parse_workbook`), so it can't be
    // tested with in-memory struct literals alone. It still avoids any
    // dependency on `tests/fixtures/` (the convention `src/lib.rs`'s own
    // parser tests already follow): each test builds a minimal `.xlsx` as
    // bytes with `minimal_xlsx_zip`/`worksheet_xml` below, writes it to a
    // uniquely-named file under `std::env::temp_dir()`, and cleans up
    // after itself.

    use std::io::Write as _;

    const ROOT_RELS_XML: &[u8] = br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#;
    const RELS_XML: &[u8] = br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
  <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
</Relationships>"#;
    const WORKBOOK_XML: &[u8] = br#"<?xml version="1.0"?>
<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets>
    <sheet name="Sheet1" sheetId="1" r:id="rId1"/>
  </sheets>
</workbook>"#;
    const STYLES_XML: &[u8] = br#"<styleSheet><cellXfs><xf numFmtId="0"/></cellXfs></styleSheet>"#;

    fn xlsx_zip_from_worksheet(worksheet_xml: &str) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, data) in [
                ("_rels/.rels", ROOT_RELS_XML),
                ("xl/_rels/workbook.xml.rels", RELS_XML),
                ("xl/workbook.xml", WORKBOOK_XML),
                ("xl/styles.xml", STYLES_XML),
                ("xl/worksheets/sheet1.xml", worksheet_xml.as_bytes()),
            ] {
                writer.start_file(name, options).unwrap();
                writer.write_all(data).unwrap();
            }
            writer.finish().unwrap();
        }
        buf
    }

    fn minimal_xlsx_zip(cell_value: &str) -> Vec<u8> {
        let worksheet_xml = format!(
            r#"<worksheet><sheetData><row r="1"><c r="A1"><v>{cell_value}</v></c></row></sheetData></worksheet>"#
        );
        xlsx_zip_from_worksheet(&worksheet_xml)
    }

    /// Builds a worksheet with one row per entry of `values`, each holding
    /// a single numeric cell in column A — `None` renders as a row with no
    /// `<c>` element at all (a blank row), not a cell with an empty
    /// value. Used to build the "blank row inserted" pair the
    /// `diff_mode_*` tests below need: row alignment ([`DiffMode::Auto`])
    /// should explain the whole shift away, while coordinate-based diffing
    /// ([`DiffMode::Coordinate`]) sees every shifted cell as its own
    /// change (see `diff_workbooks_best_effort`'s own doc comment for why).
    fn xlsx_zip_column_a(values: &[Option<i64>]) -> Vec<u8> {
        let mut rows = String::new();
        for (i, value) in values.iter().enumerate() {
            let row = i + 1;
            match value {
                Some(v) => rows.push_str(&format!(
                    r#"<row r="{row}"><c r="A{row}"><v>{v}</v></c></row>"#
                )),
                None => rows.push_str(&format!(r#"<row r="{row}"/>"#)),
            }
        }
        let worksheet_xml = format!(r#"<worksheet><sheetData>{rows}</sheetData></worksheet>"#);
        xlsx_zip_from_worksheet(&worksheet_xml)
    }

    /// Writes `bytes` to a uniquely-named file under `std::env::temp_dir()`
    /// and returns a guard that deletes it on drop, so a test can't leak a
    /// temp file behind on an assertion failure/panic.
    struct TempFile(std::path::PathBuf);

    impl TempFile {
        fn new(test_name: &str, bytes: &[u8]) -> Self {
            let path = std::env::temp_dir().join(format!(
                "exceldiff-markdown-test-{}-{test_name}.xlsx",
                std::process::id()
            ));
            std::fs::write(&path, bytes).unwrap();
            Self(path)
        }

        fn path_str(&self) -> &str {
            self.0.to_str().unwrap()
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn diff_file_section_from_paths_renders_a_valid_added_file() {
        let head = TempFile::new("added_valid", &minimal_xlsx_zip("42"));
        let md = diff_file_section_from_paths(
            "path/a.xlsx",
            "A",
            None,
            Some(head.path_str()),
            &MarkdownOptions::default(),
        );
        assert!(md.starts_with("### 🆕 Added · `path/a.xlsx`\n"));
        assert!(md.contains("1 sheet(s), 1 total cell(s)."));
    }

    #[test]
    fn diff_file_section_from_paths_added_without_head_path_has_no_summary() {
        let md = diff_file_section_from_paths(
            "path/a.xlsx",
            "A",
            None,
            None,
            &MarkdownOptions::default(),
        );
        assert_eq!(
            md,
            format_file_section(
                "path/a.xlsx",
                &FileStatus::Added(None),
                &MarkdownOptions::default()
            )
        );
    }

    #[test]
    fn diff_file_section_from_paths_reports_an_added_parse_error() {
        let head = TempFile::new("added_parse_error", b"not a zip file");
        let md = diff_file_section_from_paths(
            "path/a.xlsx",
            "A",
            None,
            Some(head.path_str()),
            &MarkdownOptions::default(),
        );
        assert!(md.contains("⚠️ Could not parse:"));
    }

    #[test]
    fn diff_file_section_from_paths_renders_deleted() {
        let md = diff_file_section_from_paths(
            "path/d.xlsx",
            "D",
            None,
            None,
            &MarkdownOptions::default(),
        );
        assert_eq!(
            md,
            format_file_section(
                "path/d.xlsx",
                &FileStatus::Deleted,
                &MarkdownOptions::default()
            )
        );
    }

    #[test]
    fn diff_file_section_from_paths_renders_a_real_modified_diff() {
        let base = TempFile::new("modified_base", &minimal_xlsx_zip("42"));
        let head = TempFile::new("modified_head", &minimal_xlsx_zip("100"));
        let md = diff_file_section_from_paths(
            "path/m.xlsx",
            "M",
            Some(base.path_str()),
            Some(head.path_str()),
            &MarkdownOptions::default(),
        );
        assert!(md.contains("@@ A1 @@"));
        assert!(md.contains("- 42"));
        assert!(md.contains("+ 100"));
    }

    #[test]
    fn diff_file_section_from_paths_modified_missing_content() {
        let md = diff_file_section_from_paths(
            "path/m.xlsx",
            "M",
            None,
            None,
            &MarkdownOptions::default(),
        );
        assert_eq!(
            md,
            format_file_section(
                "path/m.xlsx",
                &FileStatus::ModifiedMissingContent,
                &MarkdownOptions::default()
            )
        );
    }

    #[test]
    fn diff_file_section_from_paths_reports_a_base_parse_error() {
        let base = TempFile::new("modified_base_error", b"not a zip file");
        let head = TempFile::new("modified_base_error_head", &minimal_xlsx_zip("42"));
        let md = diff_file_section_from_paths(
            "path/m.xlsx",
            "M",
            Some(base.path_str()),
            Some(head.path_str()),
            &MarkdownOptions::default(),
        );
        assert!(md.contains("⚠️ Could not parse the previous version:"));
    }

    #[test]
    fn diff_file_section_from_paths_reports_a_head_parse_error() {
        let base = TempFile::new("modified_head_error_base", &minimal_xlsx_zip("42"));
        let head = TempFile::new("modified_head_error", b"not a zip file");
        let md = diff_file_section_from_paths(
            "path/m.xlsx",
            "M",
            Some(base.path_str()),
            Some(head.path_str()),
            &MarkdownOptions::default(),
        );
        assert!(md.contains("⚠️ Could not parse the new version:"));
    }

    #[test]
    fn diff_file_section_from_paths_renders_unrecognized_status() {
        let md = diff_file_section_from_paths(
            "path/x.xlsx",
            "R",
            None,
            None,
            &MarkdownOptions::default(),
        );
        assert_eq!(
            md,
            format_file_section(
                "path/x.xlsx",
                &FileStatus::Unrecognized("R"),
                &MarkdownOptions::default()
            )
        );
    }

    // --- diff_mode (Issue #24) ---
    //
    // A blank row inserted at the top of a 2-value column is the same
    // scenario `diff_workbooks_best_effort`'s own
    // `blank_row_insertion_reaches_the_ok_none_floor` test uses (just a
    // 2-row version, not 20): a pure, monotonic shift with no new content,
    // which row alignment explains away entirely.

    #[test]
    fn diff_mode_auto_collapses_a_blank_row_insertion() {
        let base = TempFile::new(
            "diff_mode_auto_base",
            &xlsx_zip_column_a(&[Some(1), Some(2)]),
        );
        let head = TempFile::new(
            "diff_mode_auto_head",
            &xlsx_zip_column_a(&[None, Some(1), Some(2)]),
        );
        let md = diff_file_section_from_paths(
            "path/m.xlsx",
            "M",
            Some(base.path_str()),
            Some(head.path_str()),
            &MarkdownOptions::default(), // diff_mode: Auto
        );
        assert!(
            md.contains("_No differences detected._"),
            "unexpected stdout: {md}"
        );
    }

    #[test]
    fn diff_mode_coordinate_sees_the_row_shift_cascade() {
        let base = TempFile::new(
            "diff_mode_coordinate_base",
            &xlsx_zip_column_a(&[Some(1), Some(2)]),
        );
        let head = TempFile::new(
            "diff_mode_coordinate_head",
            &xlsx_zip_column_a(&[None, Some(1), Some(2)]),
        );
        let options = MarkdownOptions {
            diff_mode: DiffMode::Coordinate,
            ..MarkdownOptions::default()
        };
        let md = diff_file_section_from_paths(
            "path/m.xlsx",
            "M",
            Some(base.path_str()),
            Some(head.path_str()),
            &options,
        );
        // Every shifted cell shows up as its own change: A1 deleted, A2
        // modified (2 -> 1), A3 added (2) — the cascade Auto mode avoids.
        assert!(md.contains("@@ A1 @@\n- 1\n"), "unexpected stdout: {md}");
        assert!(
            md.contains("@@ A2 @@\n- 2\n+ 1\n"),
            "unexpected stdout: {md}"
        );
        assert!(md.contains("@@ A3 @@\n+ 2\n"), "unexpected stdout: {md}");
    }
}
