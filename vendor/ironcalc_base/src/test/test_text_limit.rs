#![allow(clippy::unwrap_used)]

use crate::arithmetic::{BYTES_PER_ELEMENT, MAX_TEXT_ARRAY_BYTES};
use crate::test::util::new_empty_model;

// Excel holds at most 32,767 characters in a text, counted in UTF-16 code units, and a
// result that would be longer is #VALUE!.
const LIMIT: usize = 32_767;

fn x(count: usize) -> String {
    "x".repeat(count)
}

fn expected_join(left: &str, right: &str) -> String {
    if left.encode_utf16().count() + right.encode_utf16().count() > LIMIT {
        "#VALUE!".to_string()
    } else {
        format!("{left}{right}")
    }
}

// Every way of joining two texts, the array path of `&` included (a one-cell range is an
// array), gives the text or #VALUE! exactly as Excel does, around the limit and with
// characters of one, two, three and four bytes.
#[test]
fn every_join_agrees_with_excel_at_the_text_limit() {
    let texts = [
        "y".to_string(),
        "é".to_string(),
        "😀".to_string(),
        x(16_383),
        x(32_766),
        x(LIMIT),
        "é".repeat(32_766),
        "€".repeat(16_384),
        "😀".repeat(16_383),
        format!("{}😀", x(32_765)),
    ];
    let forms: [fn(&str, &str) -> String; 5] = [
        |l: &str, r: &str| format!("={l}&{r}"),
        |l: &str, r: &str| format!("={l}&{r}:{r}"),
        |l: &str, r: &str| format!("=CONCAT({l},{r})"),
        |l: &str, r: &str| format!("=CONCATENATE({l},{r})"),
        |l: &str, r: &str| format!("=TEXTJOIN(\"\",TRUE,{l},{r})"),
    ];
    let columns = ["B", "C", "D", "E", "F"];
    let mut model = new_empty_model();
    for (i, text) in texts.iter().enumerate() {
        model._set(&format!("A{}", i + 1), text);
    }
    let pairs: Vec<(usize, usize)> = (0..texts.len())
        .flat_map(|i| (0..texts.len()).map(move |j| (i, j)))
        .collect();
    for (row, (i, j)) in pairs.iter().enumerate() {
        let (left, right) = (format!("A{}", i + 1), format!("A{}", j + 1));
        for (form, column) in forms.iter().zip(columns) {
            model._set(&format!("{column}{}", row + 1), &form(&left, &right));
        }
    }
    model.evaluate();
    for (row, (i, j)) in pairs.iter().enumerate() {
        let expected = expected_join(&texts[*i], &texts[*j]);
        for column in columns {
            let cell = format!("{column}{}", row + 1);
            assert!(
                model._get_text(&cell) == expected,
                "{cell}: texts {i} and {j}"
            );
        }
    }
}

#[test]
fn ampersand_chain_past_the_limit_is_value() {
    let mut model = new_empty_model();
    model._set("A1", &x(30_000));
    model._set("B1", &format!("={}A1", "A1&".repeat(20)));
    model._set("B2", r#"=REPT("x",32766)&"y""#);
    model._set("B3", r#"=REPT("x",32767)&"y""#);
    model.evaluate();
    assert_eq!(model._get_text("B1"), "#VALUE!");
    assert_eq!(model._get_text("B2"), format!("{}y", x(32_766)));
    assert_eq!(model._get_text("B3"), "#VALUE!");
}

// Only the elements whose text would be too long are #VALUE!, whichever side the array is on.
#[test]
fn ampersand_over_arrays_is_value_only_where_the_text_is_too_long() {
    let mut model = new_empty_model();
    model._set("A1", r#"=REPT("x",32766)&SEQUENCE(10)"#);
    model._set("B1", r#"=SEQUENCE(1,10)&REPT("x",32766)"#);
    model._set("C3", r#"=(REPT("x",32766)&SEQUENCE(2))&{"","y"}"#);
    model.evaluate();
    assert_eq!(model._get_text("A1"), format!("{}1", x(32_766)));
    assert_eq!(model._get_text("A9"), format!("{}9", x(32_766)));
    assert_eq!(model._get_text("A10"), "#VALUE!");
    assert_eq!(model._get_text("B1"), format!("1{}", x(32_766)));
    assert_eq!(model._get_text("J1"), format!("9{}", x(32_766)));
    assert_eq!(model._get_text("K1"), "#VALUE!");
    assert_eq!(model._get_text("C3"), format!("{}1", x(32_766)));
    assert_eq!(model._get_text("D3"), "#VALUE!");
    assert_eq!(model._get_text("C4"), format!("{}2", x(32_766)));
    assert_eq!(model._get_text("D4"), "#VALUE!");
}

// 16,384 elements of 16,384 bytes, node included, fill the 256 MiB an `&` array may hold;
// one more is refused before any element is built.
#[test]
fn ampersand_array_stops_at_its_byte_cap() {
    assert_eq!(16_384 * 16_384, MAX_TEXT_ARRAY_BYTES);
    let repeated = 16_384 - BYTES_PER_ELEMENT - "TRUE".len();
    let mut model = new_empty_model();
    model._set(
        "A1",
        &format!(r#"=COUNTA(REPT("x",{repeated})&(SEQUENCE(16384)>0))"#),
    );
    model._set(
        "B1",
        &format!(r#"=REPT("x",{repeated})&(SEQUENCE(16385)>0)"#),
    );
    model.evaluate();
    assert_eq!(model._get_text("A1"), "16384");
    assert_eq!(model._get_text("B1"), "#VALUE!");
    assert_eq!(model._get_text("B2"), "");
}

// A million texts of 244 bytes fit the cap on their own, but not with the node each needs.
#[test]
fn ampersand_array_counts_each_element_toward_its_byte_cap() {
    assert!(1_000_000 * 244 < MAX_TEXT_ARRAY_BYTES);
    let mut model = new_empty_model();
    model._set("A1", r#"=REPT("x",240)&(SEQUENCE(1000000)>0)"#);
    model.evaluate();
    assert_eq!(model._get_text("A1"), "#VALUE!");
}

// A column broadcast against a row holds more elements than either; past the million SEQUENCE
// allows it is refused before a single one is visited, even for empty ranges. An operand as
// large as a whole column is not.
#[test]
fn ampersand_broadcast_stops_at_the_array_size_limit() {
    let mut model = new_empty_model();
    model._set("B2", "=A1:A1048576&A1:XFD1");
    model._set("B3", "=SEQUENCE(2000)&SEQUENCE(1,1000)");
    model._set("B4", "=COUNTA(SEQUENCE(1000)&SEQUENCE(1,1000))");
    model._set("C1", r#"=COUNTA(A1:A1048576&"")"#);
    model.evaluate();
    assert_eq!(model._get_text("B2"), "#VALUE!");
    assert_eq!(model._get_text("B3"), "#VALUE!");
    assert_eq!(model._get_text("B4"), "1000000");
    assert_eq!(model._get_text("C1"), "1048576");
}

#[test]
fn concat_over_a_range_stops_at_the_text_limit() {
    let mut model = new_empty_model();
    model._set("A1", r#"=REPT("x",16383)"#);
    model._set("A2", r#"=REPT("x",16383)"#);
    model._set("A3", "y");
    model._set("B1", "=CONCAT(A1:A3)");
    model._set("B2", r#"=CONCAT(A1:A3,"z")"#);
    model.evaluate();
    assert_eq!(model._get_text("B1"), format!("{}y", x(32_766)));
    assert_eq!(model._get_text("B2"), "#VALUE!");
}

#[test]
fn textjoin_counts_its_delimiters_toward_the_text_limit() {
    let mut model = new_empty_model();
    model._set(
        "A1",
        r#"=TEXTJOIN(",",TRUE,REPT("x",16383),REPT("x",16383))"#,
    );
    model._set(
        "A2",
        r#"=TEXTJOIN(",,",TRUE,REPT("x",16383),REPT("x",16383))"#,
    );
    model._set("B1", r#"=TEXTJOIN(",",FALSE,C1:C32768)"#);
    model._set("B2", r#"=TEXTJOIN(",",FALSE,C1:C32769)"#);
    model.evaluate();
    assert_eq!(
        model._get_text("A1"),
        format!("{},{}", x(16_383), x(16_383))
    );
    assert_eq!(model._get_text("A2"), "#VALUE!");
    assert_eq!(model._get_text("B1"), ",".repeat(LIMIT));
    assert_eq!(model._get_text("B2"), "#VALUE!");
}

#[test]
fn substitute_stops_at_the_text_limit() {
    let mut model = new_empty_model();
    model._set("A1", r#"=SUBSTITUTE(REPT("x",7),"x",REPT("y",4681))"#);
    model._set("A2", r#"=SUBSTITUTE(REPT("x",7)&"z","x",REPT("y",4681))"#);
    model._set("A3", r#"=SUBSTITUTE(REPT("x",7),"x",REPT("é",4681))"#);
    // Built whole, this result would hold 32,767 squared characters, about 1 GB.
    model._set("A4", r#"=SUBSTITUTE(REPT("x",32767),"x",REPT("y",32767))"#);
    model._set("A5", r#"=SUBSTITUTE(REPT("x",32767),"x","y",1)"#);
    model._set("A6", r#"=SUBSTITUTE(REPT("x",32767),"x","yy",1)"#);
    model.evaluate();
    assert_eq!(model._get_text("A1"), "y".repeat(LIMIT));
    assert_eq!(model._get_text("A2"), "#VALUE!");
    assert_eq!(model._get_text("A3"), "é".repeat(LIMIT));
    assert_eq!(model._get_text("A4"), "#VALUE!");
    assert_eq!(model._get_text("A5"), format!("y{}", x(32_766)));
    assert_eq!(model._get_text("A6"), "#VALUE!");
}

// An argument holding an error gives that error even after the text grew too long, as the
// text limit is only #VALUE! once every argument is read.
#[test]
fn an_error_argument_after_the_text_limit_gives_its_error() {
    let mut model = new_empty_model();
    model._set("A1", r#"=CONCAT(REPT("x",32767),"y",NA())"#);
    model._set("A2", r#"=CONCATENATE(REPT("x",32767),"y",NA())"#);
    model._set("A3", r#"=TEXTJOIN(",",TRUE,REPT("x",32767),"y",NA())"#);
    model._set("A4", r#"=CONCAT(REPT("x",32767),"y",B1:B2)"#);
    model._set("B2", "=1/0");
    model.evaluate();
    assert_eq!(model._get_text("A1"), "#N/A");
    assert_eq!(model._get_text("A2"), "#N/A");
    assert_eq!(model._get_text("A3"), "#N/A");
    assert_eq!(model._get_text("A4"), "#DIV/0!");
}
