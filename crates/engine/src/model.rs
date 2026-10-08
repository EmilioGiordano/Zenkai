use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::{Mutex, MutexGuard, PoisonError};

use ironcalc::base::UserModel;
use zenkai_types::{CellPos, ColIdx, RowIdx, SheetId};

// IronCalc computes a sheet's dimension by walking every cell, which on a large sheet
// is too slow to repeat on every scroll. Any mutable access to the model goes through
// `deref_mut`, which drops the cached ends, so they can never go stale.
pub struct CachedModel {
    model: UserModel<'static>,
    used_ends: Mutex<HashMap<SheetId, CellPos>>,
}

impl CachedModel {
    pub fn new(model: UserModel<'static>) -> CachedModel {
        CachedModel {
            model,
            used_ends: Mutex::new(HashMap::new()),
        }
    }

    // The cache only holds derived values, so a lock poisoned by a panicked reader
    // still holds valid entries.
    fn cache(&self) -> MutexGuard<'_, HashMap<SheetId, CellPos>> {
        self.used_ends
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub fn used_end(&self, sheet: SheetId) -> CellPos {
        if let Some(end) = self.cache().get(&sheet) {
            return *end;
        }
        let dimension = self
            .model
            .get_model()
            .workbook
            .worksheet(sheet.0)
            .map(|ws| ws.dimension());
        let end = match dimension {
            Ok(d) => CellPos::new(
                RowIdx::clamped(i64::from(d.max_row) - 1),
                ColIdx::clamped(i64::from(d.max_column) - 1),
            ),
            Err(_) => CellPos::default(),
        };
        self.cache().insert(sheet, end);
        end
    }
}

impl Deref for CachedModel {
    type Target = UserModel<'static>;

    fn deref(&self) -> &UserModel<'static> {
        &self.model
    }
}

impl DerefMut for CachedModel {
    fn deref_mut(&mut self) -> &mut UserModel<'static> {
        self.used_ends
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        &mut self.model
    }
}
