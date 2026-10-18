use crate::cf_types::{
    CfRuleInput, Cfvo, ColorScaleThreshold, Icon, IconThreshold, PeriodType, TextOperator,
    ValueOperator,
};
use crate::types::{Color, Dxf, Fill};
use crate::{Recalculation, UserModel};

use super::{color_scale_rule, data_bar_rule};

const SHEETS: u32 = 2;
const ROWS: i32 = 22;
const COLUMNS: i32 = 12;

fn dxf(index: u32) -> Dxf {
    Dxf {
        fill: Some(Fill {
            color: Color::Rgb(format!("#{:06X}", 0x0001_0000 * (index + 1))),
        }),
        font: None,
        border: None,
        num_fmt: None,
        alignment: None,
    }
}

fn cell_is(operator: ValueOperator, formula: &str, formula2: Option<&str>) -> CfRuleInput {
    CfRuleInput::CellIs {
        operator,
        formula: formula.to_string(),
        formula2: formula2.map(str::to_string),
        format: dxf(0),
        stop_if_true: false,
    }
}

fn formula(formula: &str, index: u32) -> CfRuleInput {
    CfRuleInput::Formula {
        formula: formula.to_string(),
        format: dxf(index),
        stop_if_true: false,
    }
}

fn threshold(cfvo: Cfvo) -> IconThreshold {
    IconThreshold {
        icon: Icon::Circle,
        cfvo,
        color: Color::Rgb("#000000".to_string()),
        is_strict: false,
    }
}

// One rule of each kind, some overlapping and one that stops the ones below it.
fn rules() -> Vec<(u32, &'static str, CfRuleInput)> {
    vec![
        (0, "A1:A20", color_scale_rule()),
        (0, "B1:B20", data_bar_rule()),
        (
            0,
            "C1:C20",
            CfRuleInput::IconSet {
                thresholds: vec![
                    threshold(Cfvo::Number(8.0)),
                    threshold(Cfvo::Number(4.0)),
                    threshold(Cfvo::Number(0.0)),
                ],
                show_value: true,
            },
        ),
        (
            0,
            "D1:D20",
            CfRuleInput::IconRating {
                icon: Icon::Star,
                color: Color::Rgb("#FFD700".to_string()),
                thresholds: vec![(Cfvo::Number(3.0), true), (Cfvo::Number(6.0), true)],
                show_value: true,
            },
        ),
        (
            0,
            "L1:L20",
            CfRuleInput::ColorScale {
                thresholds: vec![
                    ColorScaleThreshold {
                        cfvo: Cfvo::Formula("=$K$3".to_string()),
                        color: Color::Rgb("#FF0000".to_string()),
                    },
                    ColorScaleThreshold {
                        cfvo: Cfvo::Max,
                        color: Color::Rgb("#00FF00".to_string()),
                    },
                ],
            },
        ),
        (
            0,
            "A1:C20",
            CfRuleInput::CellIs {
                operator: ValueOperator::GreaterThan,
                formula: "10".to_string(),
                formula2: None,
                format: dxf(1),
                stop_if_true: true,
            },
        ),
        (
            0,
            "E1:E20",
            cell_is(ValueOperator::Between, "=$K$1", Some("=$K$2+1")),
        ),
        (0, "A1:B10 D3:E12", formula("=A1+B1>8", 2)),
        (0, "F1:F20", formula("=$A1>$B1", 3)),
        (0, "F1:F20", formula("=ROW()=3", 4)),
        (0, "G1:G20", formula("=INDIRECT(\"A1\")>5", 5)),
        (
            0,
            "G1:H20",
            CfRuleInput::Text {
                operator: TextOperator::Contains,
                value: "a".to_string(),
                format: dxf(6),
                stop_if_true: false,
            },
        ),
        (
            0,
            "G1:H20",
            CfRuleInput::Blanks {
                format: dxf(7),
                stop_if_true: false,
            },
        ),
        (
            0,
            "H1:H20",
            CfRuleInput::NotBlanks {
                format: dxf(8),
                stop_if_true: false,
            },
        ),
        (
            0,
            "I1:I20",
            CfRuleInput::Errors {
                format: dxf(9),
                stop_if_true: false,
            },
        ),
        (
            0,
            "I1:I20",
            CfRuleInput::NoErrors {
                format: dxf(10),
                stop_if_true: false,
            },
        ),
        (
            0,
            "H1:H20",
            CfRuleInput::DuplicateValues {
                format: dxf(11),
                stop_if_true: false,
            },
        ),
        (
            0,
            "G1:G20",
            CfRuleInput::UniqueValues {
                format: dxf(12),
                stop_if_true: false,
            },
        ),
        (
            0,
            "B1:B20",
            CfRuleInput::AboveAverage {
                format: dxf(13),
                stop_if_true: false,
            },
        ),
        (
            0,
            "C1:C20",
            CfRuleInput::BelowAverage {
                format: dxf(14),
                stop_if_true: false,
            },
        ),
        (
            0,
            "C1:C20",
            CfRuleInput::Top10 {
                rank: 3,
                percent: false,
                format: dxf(15),
                stop_if_true: false,
            },
        ),
        (
            0,
            "D1:D20",
            CfRuleInput::Bottom10 {
                rank: 20,
                percent: true,
                format: dxf(16),
                stop_if_true: false,
            },
        ),
        (
            0,
            "J1:J20",
            CfRuleInput::TimePeriod {
                time_period: PeriodType::Between,
                date1: Some("2020-01-01".to_string()),
                date2: Some("2020-12-31".to_string()),
                format: dxf(17),
                stop_if_true: false,
            },
        ),
        (1, "A1:A10", formula("=Sheet1!A1>3", 18)),
        (
            1,
            "B1:B10",
            cell_is(ValueOperator::LessThan, "=Sheet1!K1", None),
        ),
    ]
}

struct Random(u64);

impl Random {
    fn below(&mut self, bound: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) % bound
    }

    fn cell(&mut self) -> String {
        let column = char::from(b'A' + u8::try_from(self.below(12)).unwrap());
        let prefix = if self.below(4) == 0 { "Sheet2!" } else { "" };
        format!("{prefix}{column}{}", 1 + self.below(20))
    }

    fn input(&mut self) -> String {
        match self.below(14) {
            0..=4 => self.below(13).to_string(),
            5 => (43_800 + self.below(400)).to_string(),
            6 => ["a", "ab", "b"][usize::try_from(self.below(3)).unwrap()].to_string(),
            7 | 8 => String::new(),
            9 => format!("={}", self.cell()),
            10 => "=1/0".to_string(),
            11 => format!("={}+1", self.cell()),
            12 => format!("=SUM({}:{})", self.cell(), self.cell()),
            _ => format!("={}*2", self.cell()),
        }
    }
}

fn model_with_rules(random: &mut Random) -> UserModel<'static> {
    let mut model = UserModel::new_empty("model", "en", "UTC", "en").unwrap();
    model.new_sheet().unwrap();
    for sheet in 0..SHEETS {
        for _ in 0..60 {
            let row = 1 + i32::try_from(random.below(20)).unwrap();
            let column = 1 + i32::try_from(random.below(12)).unwrap();
            let input = random.input();
            model.set_user_input(sheet, row, column, &input).unwrap();
        }
    }
    for (sheet, range, rule) in rules() {
        model
            .add_conditional_formatting(sheet, range, rule)
            .unwrap();
    }
    model
}

fn results(model: &UserModel) -> Vec<String> {
    let mut results = Vec::new();
    for sheet in 0..SHEETS {
        for row in 1..=ROWS {
            for column in 1..=COLUMNS {
                let cell = model.get_model().cf_results(sheet, row, column);
                results.push(format!("{cell:?}"));
            }
        }
    }
    results
}

fn styles(model: &UserModel) -> Vec<String> {
    let mut styles = Vec::new();
    for sheet in 0..SHEETS {
        for row in 1..=ROWS {
            for column in 1..=COLUMNS {
                let style = model
                    .get_model()
                    .get_extended_style_for_cell(sheet, row, column)
                    .unwrap();
                styles.push(serde_json::to_string(&style).unwrap());
            }
        }
    }
    styles
}

fn evaluated_from_scratch(model: &UserModel) -> UserModel<'static> {
    let mut fresh = UserModel::from_bytes(&model.to_bytes(), "en").unwrap();
    fresh.evaluate();
    assert_eq!(fresh.get_model().last_recalculation(), Recalculation::Full);
    fresh
}

#[test]
fn incremental_conditional_formatting_matches_a_full_evaluation() {
    let mut incremental_steps = 0;
    let mut steps = 0;
    for seed in 0..8u64 {
        let mut random = Random(seed);
        let mut model = model_with_rules(&mut random);
        assert_eq!(results(&model), results(&evaluated_from_scratch(&model)));
        for _ in 0..25 {
            let sheet = u32::try_from(random.below(2)).unwrap();
            let row = 1 + i32::try_from(random.below(20)).unwrap();
            let column = 1 + i32::try_from(random.below(12)).unwrap();
            let input = random.input();
            // A write over part of an array is refused; the model is left as it was.
            let _refused = model.set_user_input(sheet, row, column, &input);
            if random.below(6) == 0 {
                let _nothing_to_undo = model.undo();
            }
            steps += 1;
            if model.get_model().last_recalculation() == Recalculation::Incremental {
                incremental_steps += 1;
            }
            let fresh = evaluated_from_scratch(&model);
            assert_eq!(results(&model), results(&fresh), "seed {seed}");
        }
        let fresh = evaluated_from_scratch(&model);
        assert_eq!(styles(&model), styles(&fresh), "seed {seed}");
    }
    assert!(
        incremental_steps * 4 > steps,
        "only {incremental_steps} of {steps} steps were incremental"
    );
}

#[test]
fn the_rules_give_results_to_the_cells() {
    let mut random = Random(3);
    let model = model_with_rules(&mut random);
    let given = results(&model)
        .iter()
        .filter(|cell| cell.as_str() != "[]")
        .count();
    assert!(given > 20, "only {given} cells have a result");
}
