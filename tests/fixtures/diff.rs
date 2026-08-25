//! Fixtures for `diff::engine`/`diff::storage` integration tests (Issue
//! #3). Unlike every other category here, each function returns a `(base,
//! target)` pair of in-memory `.xlsx` packages — the two revisions a diff
//! test compares — rather than a single package to parse in isolation.
//! `tests/diff.rs` parses both sides through the real pipeline
//! (`parse_workbook_reader`) before diffing them, exercising the same path
//! a real caller would, unlike `src/diff/engine.rs`'s own unit tests (which
//! build `Sheet`s directly via the public model API and never touch
//! ZIP/XML at all).

use super::builder::*;

fn single_sheet(name: &str, state: Option<&str>, rows_xml: &str) -> Vec<u8> {
    build_zip(&[
        (
            "xl/_rels/workbook.xml.rels",
            rels_xml(&[("rId1", "worksheet", "worksheets/sheet1.xml")]).as_bytes(),
        ),
        (
            "xl/workbook.xml",
            workbook_xml(&[(name, "rId1", state)]).as_bytes(),
        ),
        (
            "xl/worksheets/sheet1.xml",
            worksheet_xml(rows_xml, "").as_bytes(),
        ),
    ])
}

/// B2's value changes 100 -> 120; every other cell is byte-for-byte
/// identical. Verifies a pure value modification round-trips as exactly
/// one `Modified` cell diff at (row 2, col 2).
pub fn cell_modified() -> (Vec<u8>, Vec<u8>) {
    let base_rows = r#"<row r="1"><c r="A1" t="str"><v>Item</v></c><c r="B1" t="str"><v>Price</v></c></row>
<row r="2"><c r="A2" t="str"><v>Apple</v></c><c r="B2"><v>100</v></c></row>"#;
    let target_rows = r#"<row r="1"><c r="A1" t="str"><v>Item</v></c><c r="B1" t="str"><v>Price</v></c></row>
<row r="2"><c r="A2" t="str"><v>Apple</v></c><c r="B2"><v>120</v></c></row>"#;
    (
        single_sheet("Inventory", None, base_rows),
        single_sheet("Inventory", None, target_rows),
    )
}

/// The target sheet gains a populated cell (C2) the base sheet never had
/// at that coordinate at all (not merely blank).
pub fn cell_added() -> (Vec<u8>, Vec<u8>) {
    let base_rows = r#"<row r="2"><c r="A2" t="str"><v>Apple</v></c></row>"#;
    let target_rows =
        r#"<row r="2"><c r="A2" t="str"><v>Apple</v></c><c r="C2" t="str"><v>Fruit</v></c></row>"#;
    (
        single_sheet("Sheet1", None, base_rows),
        single_sheet("Sheet1", None, target_rows),
    )
}

/// The base sheet has a populated cell (C2) that's gone entirely from the
/// target — the mirror image of [`cell_added`].
pub fn cell_deleted() -> (Vec<u8>, Vec<u8>) {
    let (target, base) = cell_added();
    (base, target)
}

/// A new column is inserted before both existing columns; the two
/// existing columns' data shifts right unchanged. Each column carries 10
/// distinct numeric values, well over
/// `diff::col_alignment::MIN_DISTINCT_FOR_CONTENT_MATCH`, so
/// `diff_workbooks_aligned_columns` can align them by content alone (no
/// header row needed) — exercises the plain content-matching path end to
/// end through the real parse pipeline (Issue #5).
pub fn column_inserted() -> (Vec<u8>, Vec<u8>) {
    fn rows(cols: &[(&str, i64)]) -> String {
        let mut out = String::new();
        for row in 1..=10i64 {
            out.push_str(&format!(r#"<row r="{row}">"#));
            for &(col_letter, base_value) in cols {
                out.push_str(&format!(
                    r#"<c r="{col_letter}{row}"><v>{}</v></c>"#,
                    base_value + row
                ));
            }
            out.push_str("</row>\n");
        }
        out
    }

    let base_rows = rows(&[("A", 0), ("B", 100)]);
    let target_rows = rows(&[("A", 200), ("B", 0), ("C", 100)]);

    (
        single_sheet("Sheet1", None, &base_rows),
        single_sheet("Sheet1", None, &target_rows),
    )
}

/// A new row is inserted before all existing rows; the existing rows'
/// data shifts down unchanged. Each row carries 2 distinct numeric values
/// (row-number-derived, so every row is content-unique), so
/// `diff_workbooks_aligned_rows` can align them by content alone —
/// exercises the plain content-matching path end to end through the real
/// parse pipeline (Issue #4).
pub fn row_inserted() -> (Vec<u8>, Vec<u8>) {
    fn row_xml(row: i64, a: i64, b: i64) -> String {
        format!(r#"<row r="{row}"><c r="A{row}"><v>{a}</v></c><c r="B{row}"><v>{b}</v></c></row>"#)
    }

    let mut base_rows = String::new();
    for r in 1..=10i64 {
        base_rows.push_str(&row_xml(r, r, r * 100));
    }

    let mut target_rows = String::new();
    target_rows.push_str(&row_xml(1, 999, 9990));
    for r in 1..=10i64 {
        target_rows.push_str(&row_xml(r + 1, r, r * 100));
    }

    (
        single_sheet("Sheet1", None, &base_rows),
        single_sheet("Sheet1", None, &target_rows),
    )
}

/// Base and target are two independently-built packages that happen to
/// resolve to the same cell values — diffing them must report zero
/// changes, not merely "few" changes.
pub fn identical_workbooks() -> (Vec<u8>, Vec<u8>) {
    let rows = r#"<row r="1"><c r="A1"><v>42</v></c></row>"#;
    (
        single_sheet("Sheet1", None, rows),
        single_sheet("Sheet1", None, rows),
    )
}

/// The target workbook has a second sheet ("New") the base never had at
/// all. Returns `(base: 1 sheet, target: 2 sheets)`; [`sheet_deleted`]
/// reuses this swapped, so the two fixtures can never drift apart.
pub fn sheet_added() -> (Vec<u8>, Vec<u8>) {
    let sheet1_rows = r#"<row r="1"><c r="A1"><v>1</v></c></row>"#;
    let new_sheet_rows = r#"<row r="1"><c r="A1"><v>99</v></c></row>"#;

    let base = build_zip(&[
        (
            "xl/_rels/workbook.xml.rels",
            rels_xml(&[("rId1", "worksheet", "worksheets/sheet1.xml")]).as_bytes(),
        ),
        (
            "xl/workbook.xml",
            workbook_xml(&[("Sheet1", "rId1", None)]).as_bytes(),
        ),
        (
            "xl/worksheets/sheet1.xml",
            worksheet_xml(sheet1_rows, "").as_bytes(),
        ),
    ]);
    let target = build_zip(&[
        (
            "xl/_rels/workbook.xml.rels",
            rels_xml(&[
                ("rId1", "worksheet", "worksheets/sheet1.xml"),
                ("rId2", "worksheet", "worksheets/sheet2.xml"),
            ])
            .as_bytes(),
        ),
        (
            "xl/workbook.xml",
            workbook_xml(&[("Sheet1", "rId1", None), ("New", "rId2", None)]).as_bytes(),
        ),
        (
            "xl/worksheets/sheet1.xml",
            worksheet_xml(sheet1_rows, "").as_bytes(),
        ),
        (
            "xl/worksheets/sheet2.xml",
            worksheet_xml(new_sheet_rows, "").as_bytes(),
        ),
    ]);
    (base, target)
}

/// The target workbook is missing a sheet ("New") the base had — the
/// mirror image of [`sheet_added`].
pub fn sheet_deleted() -> (Vec<u8>, Vec<u8>) {
    let (target, base) = sheet_added();
    (base, target)
}

/// The same single sheet on both sides, differing only in its `<sheet
/// state="...">` attribute (visible -> hidden). No cell content changes at
/// all — verifies visibility changes surface even with zero cell diffs.
pub fn sheet_visibility_changed() -> (Vec<u8>, Vec<u8>) {
    let rows = r#"<row r="1"><c r="A1"><v>1</v></c></row>"#;
    (
        single_sheet("Sheet1", None, rows),
        single_sheet("Sheet1", Some("hidden"), rows),
    )
}

/// A1's value changes 1 -> 2 on a sheet that is `state="hidden"` on *both*
/// sides — unlike [`sheet_visibility_changed`], visibility never changes
/// here at all; only the cell content does. Verifies (Issue #16 open
/// question, `docs/design/diff/engine.md`) that a hidden sheet's own
/// content changes are diffed exactly like a visible sheet's, through the
/// real parse pipeline (`hidden_and_very_hidden_sheets_are_all_included` in
/// `src/pipeline.rs` already confirms hidden sheets survive parsing at
/// all; this confirms diffing doesn't separately skip them).
pub fn hidden_sheet_cell_modified() -> (Vec<u8>, Vec<u8>) {
    let base_rows = r#"<row r="1"><c r="A1"><v>1</v></c></row>"#;
    let target_rows = r#"<row r="1"><c r="A1"><v>2</v></c></row>"#;
    (
        single_sheet("Hidden", Some("hidden"), base_rows),
        single_sheet("Hidden", Some("hidden"), target_rows),
    )
}

/// A1's value is unchanged (`1` on both sides) but its style id changes
/// from 0 (default, not bold) to 1 (bold) — verifies a style-only change
/// (no value change) still surfaces as `Modified`.
pub fn style_only_change() -> (Vec<u8>, Vec<u8>) {
    let build = |rows: &str| {
        build_zip(&[
            (
                "xl/_rels/workbook.xml.rels",
                rels_xml(&[
                    ("rId1", "worksheet", "worksheets/sheet1.xml"),
                    ("rId2", "styles", "styles.xml"),
                ])
                .as_bytes(),
            ),
            (
                "xl/workbook.xml",
                workbook_xml(&[("Sheet1", "rId1", None)]).as_bytes(),
            ),
            ("xl/styles.xml", FONT_STYLES_XML),
            (
                "xl/worksheets/sheet1.xml",
                worksheet_xml(rows, "").as_bytes(),
            ),
        ])
    };

    (
        build(r#"<row r="1"><c r="A1" s="0"><v>1</v></c></row>"#),
        build(r#"<row r="1"><c r="A1" s="1"><v>1</v></c></row>"#),
    )
}

/// A1:B1 is merged only in the target — no cell value changes at all
/// (Issue #8). Verifies a merge-only change surfaces as a `SheetDiff` with
/// an empty `cells` list and one `Added` entry in `merges`.
pub fn merge_added() -> (Vec<u8>, Vec<u8>) {
    let rows = r#"<row r="1"><c r="A1"><v>1</v></c></row>"#;
    (
        single_sheet("Sheet1", None, rows),
        build_zip(&[
            (
                "xl/_rels/workbook.xml.rels",
                rels_xml(&[("rId1", "worksheet", "worksheets/sheet1.xml")]).as_bytes(),
            ),
            (
                "xl/workbook.xml",
                workbook_xml(&[("Sheet1", "rId1", None)]).as_bytes(),
            ),
            (
                "xl/worksheets/sheet1.xml",
                worksheet_xml(
                    rows,
                    r#"<mergeCells count="1"><mergeCell ref="A1:B1"/></mergeCells>"#,
                )
                .as_bytes(),
            ),
        ]),
    )
}

/// A1's style id changes 0 -> 1 (default -> bold, same as
/// [`style_only_change`]) on the very same sheet where C1:D1 also becomes
/// newly merged (same as [`merge_added`]) — unlike those two fixtures,
/// which each isolate a single kind of appearance-only change, this one
/// puts a `CellDiff` style change and a `MergeDiff` side by side in one
/// `SheetDiff`, the shape `diff::storage::DiffStore::save_diff` actually
/// walks in production (Issue #9: its `for sheet in &diff.sheets { for
/// cell ...; for merge ... }` loop processes both from the same `sheet`
/// value, a path neither single-purpose fixture alone exercises).
pub fn style_and_merge_changed() -> (Vec<u8>, Vec<u8>) {
    let build = |rows: &str, merge_cells_xml: &str| {
        build_zip(&[
            (
                "xl/_rels/workbook.xml.rels",
                rels_xml(&[
                    ("rId1", "worksheet", "worksheets/sheet1.xml"),
                    ("rId2", "styles", "styles.xml"),
                ])
                .as_bytes(),
            ),
            (
                "xl/workbook.xml",
                workbook_xml(&[("Sheet1", "rId1", None)]).as_bytes(),
            ),
            ("xl/styles.xml", FONT_STYLES_XML),
            (
                "xl/worksheets/sheet1.xml",
                worksheet_xml(rows, merge_cells_xml).as_bytes(),
            ),
        ])
    };

    let base_rows = r#"<row r="1"><c r="A1" s="0"><v>1</v></c><c r="C1"><v>2</v></c></row>"#;
    let target_rows = r#"<row r="1"><c r="A1" s="1"><v>1</v></c><c r="C1"><v>2</v></c></row>"#;
    (
        build(base_rows, ""),
        build(
            target_rows,
            r#"<mergeCells count="1"><mergeCell ref="C1:D1"/></mergeCells>"#,
        ),
    )
}
