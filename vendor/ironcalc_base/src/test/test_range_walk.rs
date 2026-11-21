use crate::test::util::new_empty_model;

// The cells of a range, a row at a time, as a list of single cell references.
fn cell_list(rows: i32, columns: &[&str]) -> String {
    (1..=rows)
        .flat_map(|row| columns.iter().map(move |column| format!("{column}{row}")))
        .collect::<Vec<_>>()
        .join(",")
}

fn evaluated(cells: &[(String, &str)], formula: &str) -> String {
    let mut model = new_empty_model();
    for (reference, value) in cells {
        model._set(reference, value);
    }
    model._set("H1", formula);
    model.evaluate();
    model._get_text("H1")
}

// CONCAT and TEXTJOIN over a range skip the positions without a cell when an empty cell adds
// nothing to their text. On random sheets with values, errors and spills, each gives what the
// same function gives for every cell of the range listed one by one, which reads them all.
#[test]
fn range_walk_agrees_with_every_cell_listed() {
    let values = [
        "a",
        "bc",
        "7",
        "TRUE",
        "=1/0",
        "=NA()",
        "=\"\"",
        "=SEQUENCE(2)",
        "=SEQUENCE(1,2)",
        "=SEQUENCE(2,2)",
    ];
    let columns = ["B", "C", "D", "E", "F"];
    let listed = cell_list(10, &columns);
    // The ranges reach far past the cells, which stop at row 8 and a spill below it.
    let pairs = [
        ("=CONCAT(B1:F1000)", format!("=CONCAT({listed})")),
        (
            "=TEXTJOIN(\",\",TRUE,B1:F1000)",
            format!("=TEXTJOIN(\",\",TRUE,{listed})"),
        ),
        (
            "=TEXTJOIN(\"\",FALSE,B1:F1000)",
            format!("=TEXTJOIN(\"\",FALSE,{listed})"),
        ),
        (
            "=TEXTJOIN(\"-\",FALSE,B1:F10)",
            format!("=TEXTJOIN(\"-\",FALSE,{listed})"),
        ),
    ];
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut random = move |bound: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % bound as u64) as usize
    };
    for sheet in 0..150 {
        let mut cells = Vec::new();
        for row in 1..=8 {
            for column in columns {
                if random(10) < 3 {
                    cells.push((format!("{column}{row}"), values[random(values.len())]));
                }
            }
        }
        for (walked, listed) in &pairs {
            assert_eq!(
                evaluated(&cells, walked),
                evaluated(&cells, listed),
                "sheet {sheet}: {walked} over {cells:?}"
            );
        }
    }
}

// A spill that evaluating a cell of the range writes, below or right of the cells the sheet
// held when the walk began, is read as a loop over every cell reads it.
#[test]
fn range_walk_reads_cells_a_spill_writes_during_the_walk() {
    let mut model = new_empty_model();
    model._set("A1", "=CONCAT(B1:B1048576)");
    model._set("B1", "=SEQUENCE(3)");
    model._set("A2", "=CONCAT(C5:XFD5)");
    model._set("C5", "=SEQUENCE(1,3)*2");
    model._set("A3", "=TEXTJOIN(\",\",TRUE,D10:E1048576)");
    model._set("D10", "=SEQUENCE(2,2)");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "123");
    assert_eq!(model._get_text("A2"), "246");
    assert_eq!(model._get_text("A3"), "1,2,3,4");
}

// Every position counts in TEXTJOIN with ignore_empty FALSE, a whole column too: its empty
// cells add their delimiters, past Excel's 32,767 characters for a whole column.
#[test]
fn textjoin_without_ignore_empty_counts_every_empty_cell() {
    let mut model = new_empty_model();
    model._set("C1", "a");
    model._set("C3", "b");
    model._set("D10", "z");
    model._set("A1", "=TEXTJOIN(\",\",FALSE,C:C)");
    model._set("A2", "=TEXTJOIN(\",\",FALSE,C1:C5)");
    model._set("A3", "=TEXTJOIN(\",\",TRUE,C:C)");
    model._set("A4", "=TEXTJOIN(\"\",FALSE,C:C)");
    model._set("A5", "=TEXTJOIN(\",\",FALSE,10:10)");
    model._set("A6", "=TEXTJOIN(\",\",FALSE,C:C,NA())");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "#VALUE!");
    assert_eq!(model._get_text("A2"), "a,,b,,");
    assert_eq!(model._get_text("A3"), "a,b");
    assert_eq!(model._get_text("A4"), "ab");
    assert_eq!(model._get_text("A5"), format!(",,,z{}", ",".repeat(16_380)));
    assert_eq!(model._get_text("A6"), "#N/A");
}

// A range over the whole sheet but its first row used to evaluate all 1.7e10 of its cells.
#[test]
fn concat_and_textjoin_over_the_whole_sheet_read_only_its_cells() {
    let mut model = new_empty_model();
    model._set("C3", "x");
    model._set("XFD1048576", "y");
    model._set("A1", "=CONCAT(A2:XFD1048576)");
    model._set("B1", "=TEXTJOIN(\",\",TRUE,A2:XFD1048576)");
    model._set("C1", "=TEXTJOIN(\"\",FALSE,A2:XFD1048576)");
    model._set("D1", "=TEXTJOIN(\",\",FALSE,A2:XFD1048576)");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "xy");
    assert_eq!(model._get_text("B1"), "x,y");
    assert_eq!(model._get_text("C1"), "xy");
    assert_eq!(model._get_text("D1"), "#VALUE!");
}
