use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};

use crate::scale_cases::{self, CORPUS_ENV};
use crate::scale_stage;

const HEADER: &str = "file,metric,value";
const ALLOWED_REGRESSION: f64 = 0.15;
const RUN_TIMEOUT: Duration = Duration::from_secs(600);

// Baselined per file; info keys the workers also print (open_ms inside steady,
// fill_rows, edit_ms and save_ms inside edit-save) are not compared.
pub const METRICS: [&str; 9] = [
    "open_ms",
    "open_peak_mb",
    "steady_mb",
    "fill_ms",
    "fill_peak_mb",
    "edit_save_ms",
    "edit_save_peak_mb",
    "save_ms",
    "save_peak_mb",
];

#[derive(Clone, Debug, PartialEq)]
pub struct Measurement {
    pub file: String,
    pub metric: String,
    pub value: f64,
}

#[derive(Debug, PartialEq)]
pub enum Problem {
    Regressed {
        file: String,
        metric: String,
        baseline: f64,
        current: f64,
    },
    Missing {
        file: String,
        metric: String,
    },
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::Regressed {
                file,
                metric,
                baseline: 0.0,
                current,
            } => write!(f, "{file} {metric}: {current:.1} vs baseline 0.0"),
            Problem::Regressed {
                file,
                metric,
                baseline,
                current,
            } => write!(
                f,
                "{file} {metric}: {current:.1} vs baseline {baseline:.1} ({:+.0}%)",
                (current / baseline - 1.0) * 100.0
            ),
            Problem::Missing { file, metric } => {
                write!(f, "{file} {metric}: not measured in this run")
            }
        }
    }
}

pub fn parse(text: &str) -> Result<Vec<Measurement>> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    if lines.next() != Some(HEADER) {
        bail!("scale baseline must start with the header `{HEADER}`");
    }
    lines
        .map(|line| {
            let fields: Vec<&str> = line.split(',').collect();
            let [file, metric, value] = fields.as_slice() else {
                bail!("expected three fields in `{line}`");
            };
            if !METRICS.contains(metric) {
                bail!("unknown metric `{metric}`");
            }
            Ok(Measurement {
                file: file.to_string(),
                metric: metric.to_string(),
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
        csv.push_str(&format!("{},{},{:.1}\n", m.file, m.metric, m.value));
    }
    csv
}

pub fn compare(baseline: &[Measurement], current: &[Measurement]) -> Vec<Problem> {
    baseline
        .iter()
        .filter_map(|base| {
            let found = current
                .iter()
                .find(|m| m.file == base.file && m.metric == base.metric);
            match found {
                None => Some(Problem::Missing {
                    file: base.file.clone(),
                    metric: base.metric.clone(),
                }),
                // A 0.0 baseline means staging has not recorded this metric yet:
                // keep the number, flag nothing. A metric missing from the run
                // still fails above.
                Some(_) if base.value == 0.0 => None,
                Some(now) if now.value - base.value > base.value * ALLOWED_REGRESSION => {
                    Some(Problem::Regressed {
                        file: base.file.clone(),
                        metric: base.metric.clone(),
                        baseline: base.value,
                        current: now.value,
                    })
                }
                Some(_) => None,
            }
        })
        .collect()
}

pub fn run() -> Result<()> {
    let (workdir, measurements) = collect()?;
    print_table(&measurements);
    cleanup(&workdir);
    Ok(())
}

pub fn check(baseline_path: &Path, current_path: &Path) -> Result<()> {
    let text = std::fs::read_to_string(baseline_path)
        .with_context(|| format!("reading {}", baseline_path.display()))?;
    let baseline = parse(&text).with_context(|| format!("parsing {}", baseline_path.display()))?;
    let (workdir, measurements) = collect()?;
    std::fs::write(current_path, to_csv(&measurements))
        .with_context(|| format!("writing {}", current_path.display()))?;
    print!("{}", to_csv(&measurements));
    cleanup(&workdir);
    let problems = compare(&baseline, &measurements);
    if !problems.is_empty() {
        let list: Vec<String> = problems.iter().map(ToString::to_string).collect();
        bail!(
            "{} problem(s), over 15% worse or missing:\n{}",
            problems.len(),
            list.join("\n")
        );
    }
    println!("scale run matches its baseline");
    Ok(())
}

fn workdir() -> Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("zenkai-scale-{}", std::process::id()));
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

fn cleanup(dir: &Path) {
    if let Err(error) = std::fs::remove_dir_all(dir) {
        eprintln!("scale: could not remove {}: {error:#}", dir.display());
    }
}

// Every stage runs in its own OS process, so the peak committed bytes at the end
// of a stage process are that stage's peak.
fn collect() -> Result<(PathBuf, Vec<Measurement>)> {
    let workdir = workdir()?;
    let (cases, full) = scale_cases::discover(&workdir)?;
    if full {
        eprintln!("scale: full sizes and the private corpus from ${CORPUS_ENV}");
    } else {
        eprintln!("scale: ${CORPUS_ENV} is not set; skipping full sizes and the private corpus");
    }
    let exe = std::env::current_exe()?;
    let mut measurements = Vec::new();
    for case in &cases {
        measurements.extend(run_case(&exe, &workdir, case)?);
    }
    Ok((workdir, measurements))
}

// Each metric comes from its own stage only: every stage process prints its peak,
// so taking a metric from the wrong stage would mislabel it.
fn run_case(exe: &PathBuf, workdir: &Path, case: &scale_cases::Case) -> Result<Vec<Measurement>> {
    let file = case.path.to_string_lossy().into_owned();
    let mut measurements = Vec::new();
    let open = stage_subprocess(exe, &["open".to_string(), file.clone()])?;
    measurements.extend(pick(
        &case.name,
        &open,
        &[("open_ms", "open_ms"), ("peak_commit_mb", "open_peak_mb")],
    ));
    let steady = stage_subprocess(exe, &["steady".to_string(), file.clone()])?;
    measurements.extend(pick(
        &case.name,
        &steady,
        &[("steady_commit_mb", "steady_mb")],
    ));
    if case.fill_rows > 0 {
        let fill = stage_subprocess(
            exe,
            &[
                "fill".to_string(),
                "--rows".to_string(),
                case.fill_rows.to_string(),
            ],
        )?;
        measurements.extend(pick(
            &case.name,
            &fill,
            &[("fill_ms", "fill_ms"), ("peak_commit_mb", "fill_peak_mb")],
        ));
    }
    for (stage, keys) in [
        (
            "edit-save",
            &[
                ("edit_save_ms", "edit_save_ms"),
                ("peak_commit_mb", "edit_save_peak_mb"),
            ][..],
        ),
        (
            "save",
            &[("save_ms", "save_ms"), ("peak_commit_mb", "save_peak_mb")][..],
        ),
    ] {
        let out = scale_stage::out_path(workdir, &case.name, stage)
            .to_string_lossy()
            .into_owned();
        let values = stage_subprocess(
            exe,
            &[stage.to_string(), file.clone(), "--out".to_string(), out],
        )?;
        measurements.extend(pick(&case.name, &values, keys));
    }
    Ok(measurements)
}

fn pick(file: &str, values: &[(String, String)], keys: &[(&str, &str)]) -> Vec<Measurement> {
    keys.iter()
        .filter_map(|(from, to)| {
            values
                .iter()
                .find(|(key, _)| key == from)
                .and_then(|(_, v)| v.parse::<f64>().ok())
                .map(|value| Measurement {
                    file: file.to_string(),
                    metric: to.to_string(),
                    value,
                })
        })
        .collect()
}

fn stage_subprocess(exe: &PathBuf, args: &[String]) -> Result<Vec<(String, String)>> {
    let mut child = Command::new(exe)
        .arg("scale-stage")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let start = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if start.elapsed() > RUN_TIMEOUT {
            child.kill()?;
            child.wait()?;
            bail!("scale-stage {} timed out", args.join(" "));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr
            .lines()
            .find(|l| l.contains("panicked") || l.starts_with("Error"))
            .unwrap_or("non-zero exit");
        bail!("scale-stage {} FAILED: {reason}", args.join(" "));
    }
    let stdout = String::from_utf8(output.stdout).context("non UTF-8 output")?;
    Ok(stdout
        .split_whitespace()
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect())
}

fn print_table(measurements: &[Measurement]) {
    let value = |file: &str, metric: &str| -> String {
        measurements
            .iter()
            .find(|m| m.file == file && m.metric == metric)
            .map_or("-".to_string(), |m| format!("{:.1}", m.value))
    };
    let files: Vec<&str> = measurements
        .iter()
        .map(|m| m.file.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    println!(
        "| File | Open ms | Open peak MB | Steady MB | Fill ms | Fill peak MB | Edit+save ms | Edit+save peak MB | Save ms | Save peak MB |"
    );
    println!("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |");
    for file in files {
        println!(
            "| {} | {} |",
            file,
            METRICS
                .iter()
                .map(|metric| value(file, metric))
                .collect::<Vec<_>>()
                .join(" | ")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(file: &str, metric: &str, value: f64) -> Measurement {
        Measurement {
            file: file.to_string(),
            metric: metric.to_string(),
            value,
        }
    }

    #[test]
    fn csv_round_trips() {
        let measurements = vec![
            row("scale-small.xlsx", "open_ms", 48.0),
            row("scale-small.xlsx", "steady_mb", 12.5),
        ];
        assert_eq!(parse(&to_csv(&measurements)).unwrap(), measurements);
    }

    #[test]
    fn parse_rejects_a_bad_header_or_metric() {
        assert!(parse("scale-small.xlsx,open_ms,48\n").is_err());
        assert!(parse("file,metric,value\nscale-small.xlsx,cpu,48\n").is_err());
        assert!(parse("file,metric,value\nscale-small.xlsx,open_ms\n").is_err());
    }

    #[test]
    fn within_fifteen_percent_passes() {
        let baseline = [row("a.xlsx", "save_ms", 100.0)];
        assert!(compare(&baseline, &[row("a.xlsx", "save_ms", 115.0)]).is_empty());
        assert!(compare(&baseline, &[row("a.xlsx", "save_ms", 40.0)]).is_empty());
    }

    #[test]
    fn above_fifteen_percent_or_missing_regresses() {
        let baseline = [row("a.xlsx", "steady_mb", 100.0)];
        assert_eq!(
            compare(&baseline, &[row("a.xlsx", "steady_mb", 116.0)]),
            vec![Problem::Regressed {
                file: "a.xlsx".to_string(),
                metric: "steady_mb".to_string(),
                baseline: 100.0,
                current: 116.0,
            }]
        );
        assert_eq!(
            compare(&baseline, &[row("a.xlsx", "open_ms", 10.0)]),
            vec![Problem::Missing {
                file: "a.xlsx".to_string(),
                metric: "steady_mb".to_string(),
            }]
        );
    }

    #[test]
    fn zero_baseline_is_record_only_but_missing_still_fails() {
        let baseline = [row("a.xlsx", "steady_mb", 0.0)];
        assert!(compare(&baseline, &[row("a.xlsx", "steady_mb", 500.0)]).is_empty());
        assert_eq!(
            compare(&baseline, &[row("a.xlsx", "open_ms", 1.0)]),
            vec![Problem::Missing {
                file: "a.xlsx".to_string(),
                metric: "steady_mb".to_string(),
            }]
        );
    }

    #[test]
    fn current_entries_outside_the_baseline_are_ignored() {
        let baseline = [row("a.xlsx", "open_ms", 100.0)];
        let current = vec![
            row("a.xlsx", "open_ms", 90.0),
            row("corpus.xlsx", "open_ms", 10_000.0),
        ];
        assert!(compare(&baseline, &current).is_empty());
    }
}
