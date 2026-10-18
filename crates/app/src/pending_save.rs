#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PendingSave {
    #[default]
    None,
    Requested,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobEnd {
    Saved { edited_meanwhile: bool },
    // Whatever its outcome, an autosave never wrote the file.
    Autosaved,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterJob {
    Nothing,
    SaveAgain,
}

impl PendingSave {
    pub fn request(&mut self) {
        *self = PendingSave::Requested;
    }

    pub fn settle(&mut self, end: JobEnd) -> AfterJob {
        let requested = std::mem::take(self) == PendingSave::Requested;
        match end {
            JobEnd::Saved {
                edited_meanwhile: true,
            }
            | JobEnd::Autosaved
                if requested =>
            {
                AfterJob::SaveAgain
            }
            _ => AfterJob::Nothing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EDITED: JobEnd = JobEnd::Saved {
        edited_meanwhile: true,
    };
    const UNCHANGED: JobEnd = JobEnd::Saved {
        edited_meanwhile: false,
    };

    #[test]
    fn nothing_is_owed_without_a_press() {
        let mut pending = PendingSave::None;
        assert_eq!(pending.settle(EDITED), AfterJob::Nothing);
        assert_eq!(pending.settle(JobEnd::Autosaved), AfterJob::Nothing);
    }

    #[test]
    fn a_press_saves_again_only_when_the_workbook_changed() {
        let mut pending = PendingSave::None;
        pending.request();
        assert_eq!(pending.settle(UNCHANGED), AfterJob::Nothing);
        pending.request();
        assert_eq!(pending.settle(EDITED), AfterJob::SaveAgain);
    }

    #[test]
    fn several_presses_collapse_into_one_save() {
        let mut pending = PendingSave::None;
        pending.request();
        pending.request();
        pending.request();
        assert_eq!(pending.settle(EDITED), AfterJob::SaveAgain);
        assert_eq!(pending, PendingSave::None);
        assert_eq!(pending.settle(EDITED), AfterJob::Nothing);
    }

    #[test]
    fn a_failed_save_drops_the_press() {
        let mut pending = PendingSave::None;
        pending.request();
        assert_eq!(pending.settle(JobEnd::Failed), AfterJob::Nothing);
        assert_eq!(pending, PendingSave::None);
        assert_eq!(pending.settle(EDITED), AfterJob::Nothing);
    }

    #[test]
    fn a_press_during_an_autosave_saves_once_it_ends() {
        let mut pending = PendingSave::None;
        pending.request();
        assert_eq!(pending.settle(JobEnd::Autosaved), AfterJob::SaveAgain);
        assert_eq!(pending.settle(JobEnd::Autosaved), AfterJob::Nothing);
    }
}
