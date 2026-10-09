use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use zenkai_engine::{
    ReaderComparison, XlsxReader, compare_readers, open_xlsx_with, read_xlsx_bytes,
};

use crate::PEAK;

fn parse_reader(name: &str) -> Result<XlsxReader> {
    match name {
        "ironcalc" => Ok(XlsxReader::IronCalc),
        "fast" => Ok(XlsxReader::Fast),
        _ => bail!("unknown reader {name}, expected ironcalc or fast"),
    }
}

// One measurement in this process: the read step alone (preflight included), then the
// whole open (model built and evaluated). Run in a fresh process so the peak is its own.
pub fn probe(reader: &str, file: &Path) -> Result<()> {
    let reader = parse_reader(reader)?;
    let bytes = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    PEAK.reset_peak_usage();
    let start = Instant::now();
    let read = read_xlsx_bytes(&bytes, "probe", reader)?;
    let read_ms = start.elapsed().as_secs_f64() * 1000.0;
    let read_peak_mb = PEAK.peak_usage_as_mb();
    let fallback = read.fallback().map(str::to_string);
    drop(read);
    drop(bytes);
    PEAK.reset_peak_usage();
    let start = Instant::now();
    let opened = open_xlsx_with(file, reader)?;
    let open_ms = start.elapsed().as_secs_f64() * 1000.0;
    let open_peak_mb = PEAK.peak_usage_as_mb();
    let model_mb = PEAK.current_usage_as_mb();
    drop(opened);
    println!(
        "read_ms={read_ms:.0} read_peak_mb={read_peak_mb:.0} open_ms={open_ms:.0} open_peak_mb={open_peak_mb:.0} model_mb={model_mb:.0} fallback={}",
        fallback.as_deref().unwrap_or("none")
    );
    Ok(())
}

pub fn bench(file: &Path, runs: usize) -> Result<()> {
    let exe = std::env::current_exe()?;
    println!("| Reader | Read ms | Read peak MB | Open ms | Open peak MB | Model MB | Fallback |");
    println!("| --- | --- | --- | --- | --- | --- | --- |");
    for reader in ["ironcalc", "fast"] {
        let mut samples: Vec<Vec<(String, String)>> = Vec::new();
        for _ in 0..runs {
            let output = Command::new(&exe)
                .args(["reader-probe", reader])
                .arg(file)
                .output()?;
            if !output.status.success() {
                bail!("{reader}: {}", String::from_utf8_lossy(&output.stderr));
            }
            let line = String::from_utf8_lossy(&output.stdout).trim().to_string();
            eprintln!("{reader}: {line}");
            samples.push(
                line.split(' ')
                    .filter_map(|pair| pair.split_once('='))
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            );
        }
        let median = |key: &str| {
            let mut values: Vec<f64> = samples
                .iter()
                .filter_map(|s| s.iter().find(|(k, _)| k == key))
                .filter_map(|(_, v)| v.parse().ok())
                .collect();
            values.sort_by(f64::total_cmp);
            values.get(values.len() / 2).copied().unwrap_or(f64::NAN)
        };
        let fallback = samples
            .first()
            .and_then(|s| s.iter().find(|(k, _)| k == "fallback"))
            .map_or("?", |(_, v)| v.as_str());
        println!(
            "| {reader} | {:.0} | {:.0} | {:.0} | {:.0} | {:.0} | {fallback} |",
            median("read_ms"),
            median("read_peak_mb"),
            median("open_ms"),
            median("open_peak_mb"),
            median("model_mb"),
        );
    }
    Ok(())
}

pub fn compare(paths: &[&str]) -> Result<()> {
    let mut files: Vec<PathBuf> = Vec::new();
    for path in paths.iter().map(PathBuf::from) {
        if path.is_dir() {
            let mut found: Vec<PathBuf> = std::fs::read_dir(&path)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|ext| ext == "xlsx"))
                .collect();
            found.sort();
            files.extend(found);
        } else {
            files.push(path);
        }
    }
    let mut bad = 0;
    for file in files {
        let bytes = std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
        let verdict = match compare_readers(&bytes) {
            ReaderComparison::Same => "0 differences".to_string(),
            ReaderComparison::BothRefused => "both refuse it".to_string(),
            ReaderComparison::FastDeclined(reason) => format!("left to IronCalc: {reason}"),
            other => {
                bad += 1;
                format!("{other:#?}")
            }
        };
        println!("{}: {verdict}", file.display());
    }
    if bad > 0 {
        bail!("{bad} file(s) read differently");
    }
    Ok(())
}
