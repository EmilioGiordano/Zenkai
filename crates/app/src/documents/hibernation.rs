use std::path::PathBuf;

use zenkai_agent::protected_view::FileOrigin;
use zenkai_engine::{Engine, Workbook};
use zenkai_types::{CellPos, SheetId, WorkbookId};

use super::test_support::*;
use super::{Documents, Dumped, Loaded, Provenance};
use crate::document::{self, FileJob};
use crate::entry::Entry;
use crate::pending_save::{AfterJob, JobEnd};

fn cell(workbook: &Workbook) -> String {
    workbook.cell(SheetId(0), CellPos::default()).text
}

// Two workbooks: the first, off screen, holds the edit "42" and an autosave copy that has it.
fn with_unsaved_work() -> (Documents, WorkbookId, tempfile::TempDir) {
    let mut documents = busy();
    let mut book = blank();
    book.set_input(SheetId(0), CellPos::default(), "42")
        .unwrap();
    let id = documents.open(book, Some(PathBuf::from("a.xlsx")), Vec::new());
    open_file(&mut documents, "b.xlsx");
    let document = documents.get_mut(id).unwrap();
    document.dirty = true;
    document.recovery_written(document.edit_count());
    (documents, id, tempfile::tempdir().unwrap())
}

fn dump_of(
    documents: &mut Documents,
    id: WorkbookId,
    dir: &tempfile::TempDir,
) -> Option<(Dumped, PathBuf)> {
    let claim = documents.begin_hibernation(id)?;
    let dump = dir.path().join("a.zkh");
    let kept = document::write_hibernation(&claim.workbook, &dump).unwrap();
    let dumped = Dumped {
        generation: claim.generation,
        edits: claim.edits,
        recovery: dir.path().join("autosave.xlsx"),
        dump: dump.clone(),
        kept,
    };
    Some((dumped, dump))
}

#[test]
fn unsaved_work_leaves_memory_as_a_link_and_comes_back_with_its_history() {
    let (mut documents, id, dir) = with_unsaved_work();
    let (dumped, dump) = dump_of(&mut documents, id, &dir).unwrap();
    assert!(documents.finish_hibernation(id, dumped));
    assert!(documents.get(id).is_none());
    let Some(Entry::Link(link)) = documents.entry(id) else {
        panic!("expected a link");
    };
    assert!(link.dirty);
    assert_eq!(link.recovery, Some(dir.path().join("autosave.xlsx")));
    let hibernation = link.rested.as_ref().unwrap().hibernation.as_ref().unwrap();
    assert!(dump.exists());

    let woken = Workbook::wake(&hibernation.dump, &hibernation.kept).unwrap();
    documents.start_loading(id);
    documents.install(Loaded {
        id,
        workbook: woken,
        unsupported: Vec::new(),
        read_only: false,
        origin: FileOrigin::Local,
        provenance: Provenance::Dump,
    });
    let document = documents.get_mut(id).unwrap();
    assert!(document.dirty);
    assert!(!document.restored);
    assert!(document.recovery_is_current());
    assert!(document.has_recovery_copy());
    let mut workbook = document.workbook_mut().unwrap();
    assert_eq!(cell(&workbook), "42");
    workbook.undo().unwrap();
    assert_eq!(cell(&workbook), "");
}

#[test]
fn a_workbook_that_leaves_memory_is_handed_back_to_be_freed_elsewhere() {
    let (mut documents, id, dir) = with_unsaved_work();
    assert!(documents.take_retired().is_empty());
    let (dumped, _) = dump_of(&mut documents, id, &dir).unwrap();
    assert!(documents.finish_hibernation(id, dumped));
    let retired = documents.take_retired();
    assert_eq!(retired.len(), 1);
    assert_eq!(std::sync::Arc::strong_count(&retired[0]), 1);
    assert!(documents.take_retired().is_empty());

    let clean = open_file(&mut documents, "c.xlsx");
    open_file(&mut documents, "d.xlsx");
    assert!(documents.unload(clean));
    assert_eq!(documents.take_retired().len(), 1);
}

#[test]
fn an_edit_while_the_dump_is_written_keeps_the_workbook_in_memory() {
    let (mut documents, id, dir) = with_unsaved_work();
    let (dumped, _) = dump_of(&mut documents, id, &dir).unwrap();
    documents.get_mut(id).unwrap().queue(Box::new(|_| Ok(())));
    assert!(!documents.finish_hibernation(id, dumped));
    assert!(documents.get(id).is_some());
    assert_eq!(documents.get(id).unwrap().file_job(), FileJob::Idle);
}

#[test]
fn a_save_pressed_while_the_dump_is_written_keeps_the_workbook_in_memory() {
    let (mut documents, id, dir) = with_unsaved_work();
    let (dumped, _) = dump_of(&mut documents, id, &dir).unwrap();
    documents.get_mut(id).unwrap().request_save();
    let generation = dumped.generation;
    assert!(!documents.finish_hibernation(id, dumped));
    assert!(documents.get(id).unwrap().has_pending_save());
    let document = documents.get_mut(id).unwrap();
    assert_eq!(
        document.settle_save(generation, JobEnd::Autosaved),
        AfterJob::SaveAgain
    );
    assert!(!document.has_pending_save());
}

#[test]
fn a_save_pressed_while_a_failed_dump_is_written_is_still_owed() {
    let (mut documents, id, _dir) = with_unsaved_work();
    let claim = documents.begin_hibernation(id).unwrap();
    documents.get_mut(id).unwrap().request_save();
    documents.abandon_hibernation(id, claim.generation, true);
    let document = documents.get_mut(id).unwrap();
    assert!(document.has_pending_save());
    assert_eq!(
        document.settle_save(claim.generation, JobEnd::Autosaved),
        AfterJob::SaveAgain
    );
    assert!(!document.has_pending_save());
}

#[test]
fn a_workbook_with_a_save_waiting_is_not_offered() {
    let (mut documents, id, _dir) = with_unsaved_work();
    documents.get_mut(id).unwrap().request_save();
    assert!(documents.begin_hibernation(id).is_none());
}

#[test]
fn a_workbook_that_came_on_screen_while_the_dump_was_written_stays() {
    let (mut documents, id, dir) = with_unsaved_work();
    let (dumped, _) = dump_of(&mut documents, id, &dir).unwrap();
    assert!(documents.activate(id));
    assert!(!documents.finish_hibernation(id, dumped));
    assert_eq!(documents.active_id(), Some(id));
}

#[test]
fn only_off_screen_unsaved_work_with_a_current_recovery_copy_is_offered() {
    let (mut documents, id, _dir) = with_unsaved_work();
    let active = documents.active_id().unwrap();
    assert!(documents.begin_hibernation(active).is_none());

    documents.get_mut(id).unwrap().queue(Box::new(|_| Ok(())));
    let (shared, edits) = documents.get_mut(id).unwrap().take_batch().unwrap();
    assert!(document::run_batch_serving(&shared, edits, None).is_empty());
    let generation = documents.get(id).unwrap().generation();
    assert!(documents.get_mut(id).unwrap().finish_batch(generation));
    assert!(
        documents.begin_hibernation(id).is_none(),
        "the recovery copy misses the last edit"
    );

    let document = documents.get_mut(id).unwrap();
    document.recovery_written(document.edit_count());
    assert!(documents.begin_hibernation(id).is_some());

    let clean = open_file(&mut documents, "c.xlsx");
    open_file(&mut documents, "d.xlsx");
    assert!(documents.begin_hibernation(clean).is_none());
}

#[test]
fn a_failed_dump_is_not_tried_again_until_the_work_changes() {
    let (mut documents, id, _dir) = with_unsaved_work();
    let claim = documents.begin_hibernation(id).unwrap();
    assert!(
        documents.begin_hibernation(id).is_none(),
        "one job at a time"
    );
    documents.abandon_hibernation(id, claim.generation, true);
    assert!(documents.begin_hibernation(id).is_none());
    documents.get_mut(id).unwrap().queue(Box::new(|_| Ok(())));
    let (shared, edits) = documents.get_mut(id).unwrap().take_batch().unwrap();
    assert!(document::run_batch_serving(&shared, edits, None).is_empty());
    let generation = documents.get(id).unwrap().generation();
    assert!(documents.get_mut(id).unwrap().finish_batch(generation));
    let document = documents.get_mut(id).unwrap();
    document.recovery_written(document.edit_count());
    assert!(documents.begin_hibernation(id).is_some());
}

#[test]
fn a_workbook_being_waited_for_is_never_hibernated() {
    let (mut documents, id, _dir) = with_unsaved_work();
    documents.want(id, super::Intent::Switch);
    assert!(documents.begin_hibernation(id).is_none());
}
