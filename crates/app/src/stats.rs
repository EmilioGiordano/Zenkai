use zenkai_engine::{Engine, Workbook};
use zenkai_types::{Range, SheetId, ValueKind};

const MAX_SCANNED: u64 = 500_000;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SelectionStats {
    pub count: u64,
    pub numbers: u64,
    pub sum: f64,
}

impl SelectionStats {
    pub fn average(&self) -> Option<f64> {
        (self.numbers > 0).then(|| self.sum / self.numbers as f64)
    }
}

pub fn compute(workbook: &Workbook, sheet: SheetId, selection: Range) -> Option<SelectionStats> {
    let end = workbook.used_end(sheet);
    let Some(clipped) = selection.clip_to(end) else {
        return Some(SelectionStats::default());
    };
    if clipped.cell_count() > MAX_SCANNED {
        return None;
    }
    let mut stats = SelectionStats::default();
    for pos in clipped.positions() {
        let cell = workbook.cell(sheet, pos);
        if cell.kind != ValueKind::Empty {
            stats.count += 1;
        }
        if let Some(n) = cell.number {
            stats.numbers += 1;
            stats.sum += n;
        }
    }
    Some(stats)
}
