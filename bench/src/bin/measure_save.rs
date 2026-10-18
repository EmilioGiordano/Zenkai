#![forbid(unsafe_code)]

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{PoisonError, RwLock, RwLockWriteGuard, mpsc};
use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use peak_alloc::PeakAlloc;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format::FmtSpan;
use zenkai_engine::{
    Engine, EngineError, SaveTarget, Workbook, open_xlsx, save_xlsx, write_xlsx_to,
};
use zenkai_types::{CellPos, RowIdx, SheetId};

#[global_allocator]
static PEAK: PeakAlloc = PeakAlloc;

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

// PeakPagedMemorySize64 is the Win32 PeakPagefileUsage: peak committed private bytes
// over the whole process, so a stage only shows here when it beats every earlier one.
fn peak_commit_mb() -> f64 {
    let script = format!(
        "(Get-Process -Id {}).PeakPagedMemorySize64",
        std::process::id()
    );
    Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|text| text.trim().parse::<f64>().ok())
        .map_or(-1.0, |bytes| bytes / 1024.0 / 1024.0)
}

fn begin(stage: &str) {
    println!("stage={stage} begin");
    PEAK.reset_peak_usage();
}

// heap_peak_mb is the allocator peak inside the stage alone, the figure that shows what
// one save costs on top of the open workbook.
fn done(stage: &str, start: Instant, extra: &str) {
    println!(
        "stage={stage} ms={:.0} heap_now_mb={:.0} heap_peak_mb={:.0} peak_commit_mb={:.0} {extra}",
        ms(start),
        PEAK.current_usage_as_mb(),
        PEAK.peak_usage_as_mb(),
        peak_commit_mb()
    );
}

fn flip_number(current: &str) -> String {
    match current.parse::<f64>() {
        Ok(number) => (number + 1.0).to_string(),
        Err(_) => "1".to_string(),
    }
}

fn write_lock(shared: &RwLock<Workbook>) -> RwLockWriteGuard<'_, Workbook> {
    shared.write().unwrap_or_else(PoisonError::into_inner)
}

fn flip(workbook: &mut Workbook, sheet: SheetId, cells: &[CellPos]) -> Result<()> {
    for &pos in cells {
        let before = workbook.input(sheet, pos);
        workbook.set_input(sheet, pos, &flip_number(&before))?;
    }
    Ok(())
}

// `count` cells down the column of `first`, spread evenly from its row to the sheet's
// last used row, so each edit lands in rows of its own.
fn scattered(workbook: &Workbook, sheet: SheetId, first: CellPos, count: u32) -> Vec<CellPos> {
    let start = i64::from(first.row.get());
    let end = i64::from(workbook.used_end(sheet).row.get()).max(start);
    let count = i64::from(count.max(1));
    let step = (end - start + 1) / count;
    (0..count)
        .map(|index| CellPos::new(RowIdx::clamped(start + index * step), first.col))
        .collect()
}

// Writes as the app does after an edit: the snapshot under the read lock, the write after
// it, and once more whole when the file could not be copied from.
fn save_as_app(
    shared: &RwLock<Workbook>,
    target: &Path,
    locked: mpsc::Sender<()>,
    start: Instant,
) -> Result<(f64, &'static str)> {
    let guard = shared.read().unwrap_or_else(PoisonError::into_inner);
    locked.send(())?;
    let snapshot = guard.save_snapshot(target, SaveTarget::Document)?;
    drop(guard);
    let locked_ms = ms(start);
    match save_xlsx(snapshot) {
        Err(EngineError::BaseUnusable { reason, .. }) => {
            println!("retry reason=\"{reason}\"");
            let snapshot = shared
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .save_snapshot(target, SaveTarget::Document)?;
            save_xlsx(snapshot)?;
            Ok((locked_ms, "whole_after_retry"))
        }
        written => {
            written?;
            Ok((locked_ms, "first_try"))
        }
    }
}

// Saves as the app does: the snapshot under the read lock, the write after it. An edit
// asked for once the save holds the lock reports how long it waited, which is what a user
// typing during a save feels.
fn edit_and_save(
    shared: &RwLock<Workbook>,
    sheet: SheetId,
    cells: &[CellPos],
    target: &Path,
) -> Result<()> {
    begin("edit");
    let start = Instant::now();
    flip(&mut write_lock(shared), sheet, cells)?;
    done("edit", start, &format!("edits={}", cells.len()));

    begin("save");
    let (locked, lock_taken) = mpsc::channel();
    let start = Instant::now();
    let (written, edit_wait_ms) = std::thread::scope(|scope| -> Result<_> {
        let saver = scope.spawn(|| save_as_app(shared, target, locked, start));
        lock_taken.recv()?;
        let asked = Instant::now();
        flip(&mut write_lock(shared), sheet, &cells[..1])?;
        let edit_wait_ms = ms(asked);
        let written = saver
            .join()
            .map_err(|_| anyhow!("the save thread panicked"))??;
        Ok((written, edit_wait_ms))
    })?;
    let (locked_ms, how) = written;
    let zip_mb = std::fs::metadata(target)?.len() as f64 / 1024.0 / 1024.0;
    done(
        "save",
        start,
        &format!(
            "locked_ms={locked_ms:.0} queued_edit_wait_ms={edit_wait_ms:.0} zip_mb={zip_mb:.1} written={how}"
        ),
    );
    Ok(())
}

// Takes the bytes a save writes and keeps only their count; the zip writer seeks back to
// finish each entry's header, so the position follows the seeks.
#[derive(Default)]
struct Discard {
    position: u64,
    len: u64,
}

impl Write for Discard {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.position += buf.len() as u64;
        self.len = self.len.max(self.position);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Seek for Discard {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let moved = |base: u64, offset: i64| {
            base.checked_add_signed(offset)
                .ok_or_else(|| io::Error::other("seek before the start"))
        };
        self.position = match to {
            SeekFrom::Start(offset) => offset,
            SeekFrom::End(offset) => moved(self.len, offset)?,
            SeekFrom::Current(offset) => moved(self.position, offset)?,
        };
        Ok(self.position)
    }
}

// The save as the app takes it, written into nothing: the source file is only read, and
// the verification, the sync and the rename are left out.
fn edit_and_write_in_memory(
    workbook: &mut Workbook,
    sheet: SheetId,
    cells: &[CellPos],
    source: &Path,
) -> Result<()> {
    begin("edit");
    let start = Instant::now();
    flip(workbook, sheet, cells)?;
    done("edit", start, &format!("edits={}", cells.len()));

    begin("save");
    let start = Instant::now();
    let snapshot = workbook.save_snapshot(source, SaveTarget::Document)?;
    let locked_ms = ms(start);
    let written = write_xlsx_to(snapshot, Discard::default())?;
    done(
        "save",
        start,
        &format!(
            "locked_ms={locked_ms:.0} zip_mb={:.1} written=in_memory",
            written.len as f64 / 1024.0 / 1024.0
        ),
    );
    Ok(())
}

// The deflate levels the floor compares; None is the level a save writes with.
const FLOOR_LEVELS: [Option<i32>; 3] = [Some(1), Some(3), None];

// What any save of a changed sheet costs whatever it copies: the sheet is one deflated zip
// entry, so it is inflated from the old file and deflated into the new one whole.
fn floor(target: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(File::open(target)?)?;
    let mut largest: Option<(String, u64)> = None;
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index)?;
        let bigger = largest
            .as_ref()
            .is_none_or(|(_, size)| entry.size() > *size);
        if entry.name().starts_with("xl/worksheets/") && bigger {
            largest = Some((entry.name().to_string(), entry.size()));
        }
    }
    let Some((name, _)) = largest else {
        bail!("{} has no worksheet", target.display());
    };
    begin("floor");
    let start = Instant::now();
    let mut xml = Vec::new();
    archive.by_name(&name)?.read_to_end(&mut xml)?;
    let inflate_ms = ms(start);
    let mut levels = Vec::new();
    for level in FLOOR_LEVELS {
        let deflated = Instant::now();
        let mut zip = zip::ZipWriter::new(Discard::default());
        let options = zip::write::FileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(level);
        zip.start_file(name.as_str(), options)?;
        zip.write_all(&xml)?;
        let compressed = zip.finish()?.len;
        let label = level.map_or("default".to_string(), |level| level.to_string());
        levels.push(format!(
            "level_{label}_ms={:.0} level_{label}_mb={:.1}",
            ms(deflated),
            compressed as f64 / 1024.0 / 1024.0
        ));
    }
    done(
        "floor",
        start,
        &format!(
            "part={name} xml_mb={:.1} inflate_ms={inflate_ms:.0} {}",
            xml.len() as f64 / 1024.0 / 1024.0,
            levels.join(" ")
        ),
    );
    Ok(())
}

// Each span of the save is printed with its time when it closes, on stderr.
fn trace_spans() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("zenkai_engine=debug,zenkai_xlsx_reader=debug"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_span_events(FmtSpan::CLOSE)
        .with_ansi(false)
        .with_writer(io::stderr)
        .init();
}

// The second save copies from the file the first one wrote, as saving a document again does.
// The first snapshot, of the workbook as opened, costs what the fingerprint taken at open
// costs: every sheet hashed and nothing copied. With `--in-memory` in place of the saved
// file, one save copies from the source file into nothing and nothing is written to disk.
fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: measure_save <file.xlsx> <saved.xlsx | --in-memory> [edited cell, A1] [sheet index, 0] \
                 [edits down the cell's column, 1]";
    let (source, target, cell, sheet, edits) = match args.as_slice() {
        [source, target] => (source, target, "A1", "0", "1"),
        [source, target, cell] => (source, target, cell.as_str(), "0", "1"),
        [source, target, cell, sheet] => (source, target, cell.as_str(), sheet.as_str(), "1"),
        [source, target, cell, sheet, edits] => (
            source,
            target,
            cell.as_str(),
            sheet.as_str(),
            edits.as_str(),
        ),
        _ => bail!(usage),
    };
    let edits: u32 = edits
        .parse()
        .with_context(|| format!("{edits} is not a number of edits"))?;
    let source = PathBuf::from(source);
    let in_memory = target == "--in-memory";
    let target = PathBuf::from(target);
    if !in_memory && target.exists() {
        bail!("{} exists; pick a new path", target.display());
    }
    let pos = CellPos::parse_a1(cell).ok_or_else(|| anyhow!("{cell} is not a cell"))?;
    let sheet = SheetId(
        sheet
            .parse()
            .with_context(|| format!("{sheet} is not a sheet index"))?,
    );

    trace_spans();
    begin("open");
    let start = Instant::now();
    let mut workbook: Workbook = open_xlsx(&source)
        .with_context(|| format!("opening {}", source.display()))?
        .workbook;
    done("open", start, "");

    begin("unedited_snapshot");
    let start = Instant::now();
    let unedited_target = if in_memory { &source } else { &target };
    drop(workbook.save_snapshot(unedited_target, SaveTarget::Document)?);
    done("unedited_snapshot", start, "");

    let cells = scattered(&workbook, sheet, pos, edits);
    if in_memory {
        println!(
            "save=in_memory sheet={} cell={cell} edits={}",
            sheet.0,
            cells.len()
        );
        edit_and_write_in_memory(&mut workbook, sheet, &cells, &source)?;
        return floor(&source);
    }
    let shared = RwLock::new(workbook);
    println!(
        "save=first sheet={} cell={cell} edits={}",
        sheet.0,
        cells.len()
    );
    edit_and_save(&shared, sheet, &cells, &target)?;
    println!(
        "save=again sheet={} cell={cell} edits={}",
        sheet.0,
        cells.len()
    );
    edit_and_save(&shared, sheet, &cells, &target)?;
    floor(&target)
}
