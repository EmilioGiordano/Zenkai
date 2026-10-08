#![forbid(unsafe_code)]

mod baseline;
mod compat;
mod coverage;
mod fixtures;
mod ironcalc_run;
mod logisheets_run;
mod measure;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use peak_alloc::PeakAlloc;

use fixtures::Fixture;

#[global_allocator]
static PEAK: PeakAlloc = PeakAlloc;

const ENGINES: [&str; 2] = ["ironcalc", "logisheets"];
const BASELINE_ENGINE: &str = "ironcalc";
const RUN_TIMEOUT: Duration = Duration::from_secs(600);

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["generate", dir] => generate(Path::new(dir)),
        ["measure", engine, file, dir] => measure_one(engine, file, Path::new(dir)),
        ["coverage"] => print_coverage(),
        ["compat-generate", dir] => compat::generate(Path::new(dir)),
        ["compat", dir, report] => compat::report(Path::new(dir), Path::new(report)),
        ["run", dir] => run_all(Path::new(dir), 5),
        ["run", dir, runs] => run_all(Path::new(dir), runs.parse()?),
        ["check", dir, baseline, current] => {
            check(Path::new(dir), Path::new(baseline), Path::new(current), 5)
        }
        ["check", dir, baseline, current, runs] => check(
            Path::new(dir),
            Path::new(baseline),
            Path::new(current),
            runs.parse()?,
        ),
        _ => bail!(
            "usage: zenkai-bench generate <dir> | measure <engine> <fixture file> <dir> | coverage | run <dir> [runs] | check <dir> <baseline.csv> <current.csv> [runs] | compat-generate <dir> | compat <dir> <report.md>"
        ),
    }
}

fn ensure_fixtures(dir: &Path) -> Result<()> {
    if Fixture::ALL
        .iter()
        .any(|f| !dir.join(f.file_name()).exists())
    {
        generate(dir)?;
    }
    Ok(())
}

fn check(dir: &Path, baseline_path: &Path, current_path: &Path, runs: usize) -> Result<()> {
    let text = std::fs::read_to_string(baseline_path)
        .with_context(|| format!("reading {}", baseline_path.display()))?;
    let baseline =
        baseline::parse(&text).with_context(|| format!("parsing {}", baseline_path.display()))?;
    ensure_fixtures(dir)?;
    let exe = std::env::current_exe()?;
    let mut current = Vec::new();
    for fixture in baseline::fixtures(&baseline) {
        let samples: Result<Vec<_>> = (0..runs)
            .map(|_| measure_subprocess(&exe, BASELINE_ENGINE, fixture, dir))
            .collect();
        match samples {
            Ok(samples) => current.extend(baseline::Metric::ALL.into_iter().filter_map(|metric| {
                median(&samples, metric.key()).map(|value| baseline::Measurement {
                    fixture,
                    metric,
                    value,
                })
            })),
            Err(e) => eprintln!("{} FAILED: {e:#}", fixture.file_name()),
        }
    }
    std::fs::write(current_path, baseline::to_csv(&current))
        .with_context(|| format!("writing {}", current_path.display()))?;
    print!("{}", baseline::to_csv(&current));
    let problems = baseline::compare(&baseline, &current);
    if !problems.is_empty() {
        let list: Vec<String> = problems.iter().map(ToString::to_string).collect();
        bail!(
            "{} metric(s) regressed more than 15% or are missing:\n{}",
            problems.len(),
            list.join("\n")
        );
    }
    println!("no metric regressed more than 15% against the baseline");
    Ok(())
}

fn generate(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    for fixture in Fixture::ALL {
        let start = Instant::now();
        fixtures::generate(fixture, dir)?;
        eprintln!("generated {} in {:?}", fixture.file_name(), start.elapsed());
    }
    Ok(())
}

fn measure_one(engine: &str, file: &str, dir: &Path) -> Result<()> {
    let fixture = Fixture::parse(file).ok_or_else(|| anyhow!("unknown fixture {file}"))?;
    let path = dir.join(file);
    let metrics = match engine {
        "ironcalc" => ironcalc_run::run(fixture, &path)?,
        "logisheets" => logisheets_run::run(fixture, &path)?,
        _ => bail!("unknown engine {engine}"),
    };
    println!("{metrics}");
    Ok(())
}

fn print_coverage() -> Result<()> {
    let setup = coverage::SETUP;
    let formulas: Vec<&str> = coverage::CASES.iter().map(|c| c.formula).collect();
    let iron = ironcalc_run::evaluate_formulas(&setup, &formulas)?;
    let logi = logisheets_run::evaluate_formulas(&setup, &formulas)?;
    println!("| Function | IronCalc | logisheets |");
    println!("| --- | --- | --- |");
    let mut totals = [0usize; 2];
    for ((case, a), b) in coverage::CASES.iter().zip(&iron).zip(&logi) {
        let marks = [
            coverage::passes(&case.want, a),
            coverage::passes(&case.want, b),
        ];
        for (total, ok) in totals.iter_mut().zip(marks) {
            *total += usize::from(ok);
        }
        let show = |ok: bool, seen: &measure::Seen| {
            if ok {
                "ok".to_string()
            } else {
                format!("FAIL `{seen:?}`")
            }
        };
        println!(
            "| {} | {} | {} |",
            case.function,
            show(marks[0], a),
            show(marks[1], b)
        );
    }
    println!(
        "| **Total** | **{}/{n}** | **{}/{n}** |",
        totals[0],
        totals[1],
        n = coverage::CASES.len()
    );
    Ok(())
}

fn run_all(dir: &Path, runs: usize) -> Result<()> {
    ensure_fixtures(dir)?;
    let exe = std::env::current_exe()?;
    println!(
        "| Fixture | Engine | Open ms | Recalc ms | Edit ms | Save ms | Peak MB | Idle MB | Correct | Round trip | Bold | Merges |"
    );
    println!("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |");
    for fixture in Fixture::ALL {
        for engine in ENGINES {
            let mut samples = Vec::new();
            let mut failure = None;
            for _ in 0..runs {
                match measure_subprocess(&exe, engine, fixture, dir) {
                    Ok(line) => samples.push(line),
                    Err(e) => {
                        failure = Some(e);
                        break;
                    }
                }
            }
            match failure {
                Some(e) => println!(
                    "| {} | {engine} | FAILED: {} |",
                    fixture.label(),
                    first_line(&e)
                ),
                None => println!(
                    "| {} | {engine} | {} |",
                    fixture.label(),
                    median_row(&samples)
                ),
            }
        }
    }
    Ok(())
}

fn first_line(error: &anyhow::Error) -> String {
    format!("{error:#}")
        .lines()
        .next()
        .unwrap_or_default()
        .replace('|', "/")
}

fn measure_subprocess(
    exe: &PathBuf,
    engine: &str,
    fixture: Fixture,
    dir: &Path,
) -> Result<Vec<(String, String)>> {
    let mut child = Command::new(exe)
        .args(["measure", engine, fixture.file_name()])
        .arg(dir)
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
            bail!("timed out after {}s", RUN_TIMEOUT.as_secs());
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
        bail!("{reason}");
    }
    let stdout = String::from_utf8(output.stdout).context("non UTF-8 output")?;
    Ok(stdout
        .split_whitespace()
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect())
}

fn median(samples: &[Vec<(String, String)>], key: &str) -> Option<f64> {
    let mut values: Vec<f64> = samples
        .iter()
        .filter_map(|s| s.iter().find(|(k, _)| k == key))
        .filter_map(|(_, v)| v.parse().ok())
        .collect();
    values.sort_by(f64::total_cmp);
    values.get(values.len() / 2).copied()
}

fn median_row(samples: &[Vec<(String, String)>]) -> String {
    let field = |key: &str| -> Vec<String> {
        samples
            .iter()
            .filter_map(|s| s.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()))
            .collect()
    };
    let shown = |key: &str| median(samples, key).map_or("-".to_string(), |v| format!("{v:.1}"));
    let last = |key: &str| {
        field(key)
            .last()
            .cloned()
            .unwrap_or_else(|| "-".to_string())
    };
    let edit = match last("edit_ok").as_str() {
        "false" => format!("{} (WRONG)", shown("edit_ms")),
        _ => shown("edit_ms"),
    };
    [
        shown("open_ms"),
        shown("recalc_ms"),
        edit,
        shown("save_ms"),
        shown("peak_mb"),
        shown("idle_mb"),
        last("correct"),
        last("roundtrip"),
        last("styles"),
        last("merges"),
    ]
    .join(" | ")
}
