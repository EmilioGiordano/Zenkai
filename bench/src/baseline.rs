use std::fmt;

use anyhow::{Result, anyhow, bail};

use crate::fixtures::Fixture;

const HEADER: &str = "fixture,metric,value";
const ALLOWED_REGRESSION: f64 = 0.15;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    OpenMs,
    RecalcMs,
    EditMs,
    SaveMs,
    PeakMb,
    IdleMb,
}

impl Metric {
    pub const ALL: [Metric; 6] = [
        Metric::OpenMs,
        Metric::RecalcMs,
        Metric::EditMs,
        Metric::SaveMs,
        Metric::PeakMb,
        Metric::IdleMb,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Metric::OpenMs => "open_ms",
            Metric::RecalcMs => "recalc_ms",
            Metric::EditMs => "edit_ms",
            Metric::SaveMs => "save_ms",
            Metric::PeakMb => "peak_mb",
            Metric::IdleMb => "idle_mb",
        }
    }

    fn parse(key: &str) -> Option<Metric> {
        Metric::ALL.into_iter().find(|m| m.key() == key)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measurement {
    pub fixture: Fixture,
    pub metric: Metric,
    pub value: f64,
}

#[derive(Debug, PartialEq)]
pub enum Problem {
    Regressed {
        fixture: Fixture,
        metric: Metric,
        baseline: f64,
        current: f64,
    },
    Missing {
        fixture: Fixture,
        metric: Metric,
    },
    Wrong {
        fixture: Fixture,
        check: String,
        seen: String,
    },
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::Regressed {
                fixture,
                metric,
                baseline,
                current,
            } => write!(
                f,
                "{} {}: {current:.1} vs baseline {baseline:.1} ({:+.0}%)",
                fixture.file_name(),
                metric.key(),
                (current / baseline - 1.0) * 100.0
            ),
            Problem::Missing { fixture, metric } => write!(
                f,
                "{} {}: not measured in this run",
                fixture.file_name(),
                metric.key()
            ),
            Problem::Wrong {
                fixture,
                check,
                seen,
            } => write!(f, "{} {check}={seen}: wrong results", fixture.file_name()),
        }
    }
}

pub fn parse(text: &str) -> Result<Vec<Measurement>> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    if lines.next() != Some(HEADER) {
        bail!("baseline must start with the header `{HEADER}`");
    }
    lines
        .map(|line| {
            let fields: Vec<&str> = line.split(',').collect();
            let [fixture, metric, value] = fields.as_slice() else {
                bail!("expected three fields in `{line}`");
            };
            Ok(Measurement {
                fixture: Fixture::parse(fixture)
                    .ok_or_else(|| anyhow!("unknown fixture `{fixture}`"))?,
                metric: Metric::parse(metric)
                    .ok_or_else(|| anyhow!("unknown metric `{metric}`"))?,
                value: value
                    .parse()
                    .map_err(|e| anyhow!("bad value `{value}` in `{line}`: {e}"))?,
            })
        })
        .collect()
}

pub fn to_csv(measurements: &[Measurement]) -> String {
    let mut csv = format!("{HEADER}\n");
    for m in measurements {
        csv.push_str(&format!(
            "{},{},{:.1}\n",
            m.fixture.file_name(),
            m.metric.key(),
            m.value
        ));
    }
    csv
}

pub fn fixtures(baseline: &[Measurement]) -> Vec<Fixture> {
    Fixture::ALL
        .into_iter()
        .filter(|f| baseline.iter().any(|m| m.fixture == *f))
        .collect()
}

pub fn compare(baseline: &[Measurement], current: &[Measurement]) -> Vec<Problem> {
    baseline
        .iter()
        .filter_map(|base| {
            let found = current
                .iter()
                .find(|m| m.fixture == base.fixture && m.metric == base.metric);
            match found {
                None => Some(Problem::Missing {
                    fixture: base.fixture,
                    metric: base.metric,
                }),
                Some(now) if now.value - base.value > base.value * ALLOWED_REGRESSION => {
                    Some(Problem::Regressed {
                        fixture: base.fixture,
                        metric: base.metric,
                        baseline: base.value,
                        current: now.value,
                    })
                }
                Some(_) => None,
            }
        })
        .collect()
}

pub fn wrong_results(fixture: Fixture, sample: &[(String, String)]) -> Vec<Problem> {
    sample
        .iter()
        .filter(|(check, seen)| is_wrong(check, seen))
        .map(|(check, seen)| Problem::Wrong {
            fixture,
            check: check.clone(),
            seen: seen.clone(),
        })
        .collect()
}

fn is_wrong(check: &str, seen: &str) -> bool {
    match check {
        "edit_ok" => seen == "false",
        "correct" | "roundtrip" | "styles" | "merges" => {
            seen != "-" && seen.split_once('/').is_none_or(|(ok, total)| ok != total)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(metric: Metric, value: f64) -> Measurement {
        Measurement {
            fixture: Fixture::Chain,
            metric,
            value,
        }
    }

    #[test]
    fn csv_round_trips() {
        let measurements = vec![chain(Metric::OpenMs, 48.0), chain(Metric::PeakMb, 12.5)];
        assert_eq!(parse(&to_csv(&measurements)).unwrap(), measurements);
    }

    #[test]
    fn parse_rejects_missing_header_and_unknown_names() {
        assert!(parse("4-chain.xlsx,open_ms,48\n").is_err());
        assert!(parse("fixture,metric,value\nnope.xlsx,open_ms,48\n").is_err());
        assert!(parse("fixture,metric,value\n4-chain.xlsx,cpu,48\n").is_err());
        assert!(parse("fixture,metric,value\n4-chain.xlsx,open_ms,fast\n").is_err());
    }

    #[test]
    fn within_fifteen_percent_passes() {
        let baseline = [chain(Metric::OpenMs, 100.0)];
        assert!(compare(&baseline, &[chain(Metric::OpenMs, 115.0)]).is_empty());
        assert!(compare(&baseline, &[chain(Metric::OpenMs, 40.0)]).is_empty());
    }

    #[test]
    fn above_fifteen_percent_regresses() {
        let baseline = [chain(Metric::IdleMb, 100.0)];
        assert_eq!(
            compare(&baseline, &[chain(Metric::IdleMb, 116.0)]),
            vec![Problem::Regressed {
                fixture: Fixture::Chain,
                metric: Metric::IdleMb,
                baseline: 100.0,
                current: 116.0,
            }]
        );
    }

    #[test]
    fn metric_missing_from_the_run_is_a_problem() {
        let baseline = [chain(Metric::EditMs, 11.0)];
        assert_eq!(
            compare(&baseline, &[chain(Metric::OpenMs, 48.0)]),
            vec![Problem::Missing {
                fixture: Fixture::Chain,
                metric: Metric::EditMs,
            }]
        );
    }

    fn sample(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn correct_sample_has_no_wrong_results() {
        let clean = sample(&[
            ("open_ms", "48.0"),
            ("edit_ok", "true"),
            ("correct", "10000/10000"),
            ("roundtrip", "10000/10000"),
            ("styles", "-"),
            ("merges", "-"),
        ]);
        assert!(wrong_results(Fixture::Chain, &clean).is_empty());
    }

    #[test]
    fn wrong_edit_values_or_round_trip_are_reported() {
        let broken = sample(&[
            ("edit_ok", "false"),
            ("correct", "9999/10000"),
            ("roundtrip", "10000/10000"),
            ("merges", "garbled"),
        ]);
        let checks: Vec<String> = wrong_results(Fixture::Chain, &broken)
            .into_iter()
            .map(|p| match p {
                Problem::Wrong { check, .. } => check,
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(checks, ["edit_ok", "correct", "merges"]);
    }

    #[test]
    fn fixtures_follow_bench_order_without_duplicates() {
        let baseline = [
            chain(Metric::OpenMs, 1.0),
            Measurement {
                fixture: Fixture::Values,
                metric: Metric::OpenMs,
                value: 1.0,
            },
            chain(Metric::SaveMs, 1.0),
        ];
        assert_eq!(fixtures(&baseline), vec![Fixture::Values, Fixture::Chain]);
    }
}
