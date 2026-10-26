use std::collections::HashMap;
use std::path::PathBuf;

use gpui_kit::*;
use zenkai_i18n::t;
use zenkai_types::WorkbookId;

use super::{Severity, Workspace};
use crate::files;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum SaveAsStage {
    Dialog,
    Checking,
}

#[derive(Default)]
pub(super) struct SaveAsFlows {
    stages: HashMap<WorkbookId, SaveAsStage>,
}

impl SaveAsFlows {
    pub(super) fn is_pending(&self, id: WorkbookId) -> bool {
        self.stages.contains_key(&id)
    }

    pub(super) fn begin(&mut self, id: WorkbookId) -> bool {
        if self.is_pending(id) {
            return false;
        }
        self.stages.insert(id, SaveAsStage::Dialog);
        true
    }

    pub(super) fn start_checking(&mut self, id: WorkbookId) {
        self.stages.insert(id, SaveAsStage::Checking);
    }

    pub(super) fn finish(&mut self, id: WorkbookId) {
        self.stages.remove(&id);
    }
}

pub(super) struct CheckedTarget {
    pub(super) path: PathBuf,
    pub(super) source: Option<PathBuf>,
    pub(super) probe: files::TargetProbe,
}

pub(super) fn source_unchanged(probed: &Option<PathBuf>, current: &Option<PathBuf>) -> bool {
    probed == current
}

impl Workspace {
    pub(super) fn refuse_during_save_as(
        &mut self,
        id: WorkbookId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.save_as_flows.is_pending(id) {
            return false;
        }
        self.notify(Severity::Warning, t!("notice.still_saving"), cx);
        self.save_ended(id, false, window, cx);
        true
    }

    pub(super) fn begin_save_as(
        &mut self,
        id: WorkbookId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.refuse_during_save_as(id, window, cx) {
            return false;
        }
        self.save_as_flows.begin(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_flow_on_the_same_document_is_refused() {
        let mut flows = SaveAsFlows::default();
        let id = WorkbookId(1);
        assert!(flows.begin(id));
        assert!(!flows.begin(id));
        flows.start_checking(id);
        assert!(flows.is_pending(id));
        assert!(!flows.begin(id));
    }

    #[test]
    fn a_finished_flow_lets_the_next_one_start() {
        let mut flows = SaveAsFlows::default();
        let id = WorkbookId(1);
        assert!(flows.begin(id));
        flows.finish(id);
        assert!(!flows.is_pending(id));
        assert!(flows.begin(id));
    }

    #[test]
    fn a_changed_source_is_detected() {
        let a = Some(PathBuf::from("a.xlsx"));
        let b = Some(PathBuf::from("b.xlsx"));
        assert!(source_unchanged(&a, &a));
        assert!(source_unchanged(&None, &None));
        assert!(!source_unchanged(&a, &b));
        assert!(!source_unchanged(&None, &a));
    }
}
