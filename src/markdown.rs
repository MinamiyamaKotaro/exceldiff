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

use crate::diff::{CellDiff, CellPos, DiffStatus, MergeDiff, SheetDiff, WorkbookDiff};
use crate::json::JsonCellValue;
use crate::model::CellRef;

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
}

impl Default for MarkdownOptions {
    fn default() -> Self {
        Self {
            max_rows_per_sheet: 30,
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
    let mut out = format!("### {badge} · `{display_path}`\n\n");
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
        "**Sheet `{}`{sheet_note}** — {added} added, {modified} modified, {deleted} deleted\n",
        sheet.name
    ));
    if let (Some(old_v), Some(new_v)) = (sheet.old_visibility, sheet.new_visibility) {
        out.push_str(&format!("_Visibility: `{old_v}` → `{new_v}`_\n"));
    }
    out.push('\n');

    if sheet.cells.is_empty() && sheet.merges.is_empty() {
        return out;
    }

    out.push_str("```diff\n");
    for cell in sheet.cells.iter().take(options.max_rows_per_sheet) {
        out.push_str(&format_cell_hunk(cell));
    }
    for merge in &sheet.merges {
        out.push_str(&format_merge_hunk(merge));
    }
    out.push_str("```\n");
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

/// Renders one cell value for a `-`/`+` diff line — escaping a newline
/// embedded in a text value, which would otherwise spill it across
/// multiple lines and desync the diff fence's one-line-per-value shape.
/// `|` needs no escaping here (unlike a Markdown table cell), since a
/// diff line isn't Markdown table syntax.
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
    s.replace('\n', "\\n")
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
    fn does_not_escape_pipe_in_text_values() {
        // Unlike the Markdown-table format this replaced, a diff line
        // isn't table syntax, so `|` needs no escaping.
        let v = JsonCellValue::Text(std::sync::Arc::from("a|b"));
        assert_eq!(format_value(&v), "\"a|b\"");
    }
}
