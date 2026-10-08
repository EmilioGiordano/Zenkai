use anyhow::{Result, bail};

use crate::measure::Seen;

pub enum Want {
    Number(f64),
    Text(&'static str),
    Bool(bool),
    AnyNumber,
}

pub struct Case {
    pub function: &'static str,
    pub formula: &'static str,
    pub want: Want,
}

pub const SETUP: [(&str, &str); 6] = [
    ("A1", "1"),
    ("A2", "2"),
    ("A3", "3"),
    ("B1", "x"),
    ("B2", "y"),
    ("B3", "x"),
];

const fn case(function: &'static str, formula: &'static str, want: Want) -> Case {
    Case {
        function,
        formula,
        want,
    }
}

use Want::{AnyNumber, Bool, Number, Text};

pub const CASES: &[Case] = &[
    case("SUM", "=SUM(A1:A3)", Number(6.0)),
    case("SUMIF", "=SUMIF(B1:B3,\"x\",A1:A3)", Number(4.0)),
    case("SUMIFS", "=SUMIFS(A1:A3,B1:B3,\"x\")", Number(4.0)),
    case("PRODUCT", "=PRODUCT(A1:A3)", Number(6.0)),
    case("ROUND", "=ROUND(2.345,2)", Number(2.35)),
    case("ROUNDUP", "=ROUNDUP(2.341,2)", Number(2.35)),
    case("ROUNDDOWN", "=ROUNDDOWN(2.349,2)", Number(2.34)),
    case("ABS", "=ABS(-3)", Number(3.0)),
    case("MOD", "=MOD(7,3)", Number(1.0)),
    case("INT", "=INT(-2.5)", Number(-3.0)),
    case("AVERAGE", "=AVERAGE(A1:A3)", Number(2.0)),
    case("AVERAGEIF", "=AVERAGEIF(B1:B3,\"x\",A1:A3)", Number(2.0)),
    case("AVERAGEIFS", "=AVERAGEIFS(A1:A3,B1:B3,\"x\")", Number(2.0)),
    case("MIN", "=MIN(A1:A3)", Number(1.0)),
    case("MAX", "=MAX(A1:A3)", Number(3.0)),
    case("COUNT", "=COUNT(A1:B3)", Number(3.0)),
    case("COUNTA", "=COUNTA(A1:B3)", Number(6.0)),
    case("COUNTBLANK", "=COUNTBLANK(C1:C3)", Number(3.0)),
    case("COUNTIF", "=COUNTIF(B1:B3,\"x\")", Number(2.0)),
    case(
        "COUNTIFS",
        "=COUNTIFS(B1:B3,\"x\",A1:A3,\">1\")",
        Number(1.0),
    ),
    case("MEDIAN", "=MEDIAN(A1:A3)", Number(2.0)),
    case("IF", "=IF(1>0,\"y\",\"n\")", Text("y")),
    case("IFS", "=IFS(1>2,\"a\",TRUE,\"b\")", Text("b")),
    case("AND", "=AND(TRUE,FALSE)", Bool(false)),
    case("OR", "=OR(TRUE,FALSE)", Bool(true)),
    case("NOT", "=NOT(FALSE)", Bool(true)),
    case("IFERROR", "=IFERROR(1/0,\"e\")", Text("e")),
    case("IFNA", "=IFNA(NA(),\"n\")", Text("n")),
    case("VLOOKUP", "=VLOOKUP(2,A1:B3,2,FALSE)", Text("y")),
    case("HLOOKUP", "=HLOOKUP(1,A1:A3,2,FALSE)", Number(2.0)),
    case("XLOOKUP", "=XLOOKUP(\"y\",B1:B3,A1:A3)", Number(2.0)),
    case("INDEX", "=INDEX(A1:A3,2)", Number(2.0)),
    case("MATCH", "=MATCH(3,A1:A3,0)", Number(3.0)),
    case("CONCAT", "=CONCAT(\"a\",\"b\")", Text("ab")),
    case("CONCATENATE", "=CONCATENATE(\"a\",\"b\")", Text("ab")),
    case("TEXTJOIN", "=TEXTJOIN(\"-\",TRUE,\"a\",\"b\")", Text("a-b")),
    case("LEFT", "=LEFT(\"hello\",2)", Text("he")),
    case("RIGHT", "=RIGHT(\"hello\",2)", Text("lo")),
    case("MID", "=MID(\"hello\",2,3)", Text("ell")),
    case("LEN", "=LEN(\"hello\")", Number(5.0)),
    case("TRIM", "=TRIM(\"  a  b \")", Text("a b")),
    case("UPPER", "=UPPER(\"ab\")", Text("AB")),
    case("LOWER", "=LOWER(\"AB\")", Text("ab")),
    case("PROPER", "=PROPER(\"hello world\")", Text("Hello World")),
    case(
        "SUBSTITUTE",
        "=SUBSTITUTE(\"a-b\",\"-\",\"+\")",
        Text("a+b"),
    ),
    case("FIND", "=FIND(\"l\",\"hello\")", Number(3.0)),
    case("SEARCH", "=SEARCH(\"L\",\"hello\")", Number(3.0)),
    case("TEXT", "=TEXT(0.5,\"0%\")", Text("50%")),
    case("VALUE", "=VALUE(\"12\")", Number(12.0)),
    case("TODAY", "=TODAY()", AnyNumber),
    case("NOW", "=NOW()", AnyNumber),
    case("DATE", "=DATE(2024,1,31)", Number(45322.0)),
    case("YEAR", "=YEAR(45322)", Number(2024.0)),
    case("MONTH", "=MONTH(45322)", Number(1.0)),
    case("DAY", "=DAY(45322)", Number(31.0)),
    case("WEEKDAY", "=WEEKDAY(45322)", Number(4.0)),
    case("EDATE", "=EDATE(45322,1)", Number(45351.0)),
    case("EOMONTH", "=EOMONTH(45322,1)", Number(45351.0)),
    case(
        "DATEDIF",
        "=DATEDIF(DATE(2024,1,1),DATE(2024,3,1),\"m\")",
        Number(2.0),
    ),
    case(
        "NETWORKDAYS",
        "=NETWORKDAYS(DATE(2024,1,1),DATE(2024,1,31))",
        Number(23.0),
    ),
    case("ISBLANK", "=ISBLANK(C1)", Bool(true)),
    case("ISNUMBER", "=ISNUMBER(A1)", Bool(true)),
    case("ISTEXT", "=ISTEXT(B1)", Bool(true)),
    case("ISERROR", "=ISERROR(1/0)", Bool(true)),
    case("arithmetic", "=1+2*3-4/2", Number(5.0)),
    case("power", "=2^3", Number(8.0)),
    case("comparison", "=1<2", Bool(true)),
    case("concat &", "=\"a\"&\"b\"", Text("ab")),
    case("absolute ref", "=$A$1+A2", Number(3.0)),
    case("div by zero", "=1/0", Text("#DIV/0!")),
];

pub fn passes(want: &Want, seen: &Seen) -> bool {
    match (want, seen) {
        (Want::Number(a), Seen::Number(b)) => (a - b).abs() < 1e-9,
        (Want::Text(a), Seen::Text(b)) => a == b,
        (Want::Text(a), Seen::Error(b)) => a == b,
        (Want::Bool(a), Seen::Bool(b)) => a == b,
        (Want::AnyNumber, Seen::Number(_)) => true,
        _ => false,
    }
}

pub fn a1(cell: &str) -> Result<(u32, u16)> {
    let split = cell
        .find(|c: char| c.is_ascii_digit())
        .unwrap_or(cell.len());
    let (letters, digits) = cell.split_at(split);
    let mut col: u16 = 0;
    for c in letters.chars() {
        if !c.is_ascii_uppercase() {
            bail!("bad cell {cell}");
        }
        col = col * 26 + (c as u16 - u16::from(b'A') + 1);
    }
    let row: u32 = digits.parse()?;
    if col == 0 || row == 0 {
        bail!("bad cell {cell}");
    }
    Ok((row - 1, col - 1))
}
