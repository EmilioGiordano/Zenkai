use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::document::{describe, process, render};
use super::library;
use super::page_text::{read_content, search_text};
use super::*;
use crate::codec::{Expect, decode_reply, encode_request};
use crate::frame::{read_frame, write_frame};
use crate::textmatch::find_in_page;
use crate::wire::{BrokenFile, DocId, PageIdx, PageSize};

// Two pages: the first is 200 by 100 points with the text "Hello PDF Hello", the second is
// blank and 300 by 400. The offsets of the cross-reference table are computed here.
fn tiny_pdf() -> Vec<u8> {
    let stream = "BT /F1 24 Tf 20 40 Td (Hello PDF Hello) Tj ET";
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R \
                /Resources << /Font << /F1 6 0 R >> >> >>";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".to_string(),
        page.to_string(),
        format!(
            "<< /Length {} >>\nstream\n{stream}\nendstream",
            stream.len()
        ),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    let mut bytes = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend(format!("{} 0 obj\n{body}\nendobj\n", index + 1).as_bytes());
    }
    let xref_at = bytes.len();
    bytes.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    bytes
}

#[test]
fn the_tiny_pdf_has_a_cross_reference_table_that_points_at_its_objects() {
    let bytes = tiny_pdf();
    let text = String::from_utf8(bytes).unwrap();
    let start: usize = text
        .rsplit("startxref\n")
        .next()
        .and_then(|tail| tail.lines().next())
        .and_then(|line| line.parse().ok())
        .unwrap();
    assert!(text[start..].starts_with("xref"));
    let first_entry = text[start..].lines().nth(3).unwrap();
    let offset: usize = first_entry[..10].parse().unwrap();
    assert!(text[offset..].starts_with("1 0 obj"));
}

// The DLL sits next to zenkai.exe, one folder above the test binaries. Without it the tests
// that need pdfium say so and pass, so a machine that never fetched it stays green.
fn library_folder() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let folder = exe.parent()?.parent()?.to_path_buf();
    if library::library_path(&folder).is_file() {
        Some(folder)
    } else {
        None
    }
}

fn write_pdf(folder: &Path, bytes: &[u8]) -> PathBuf {
    let path = folder.join("tiny.pdf");
    std::fs::write(&path, bytes).unwrap();
    path
}

fn a_pdf_renders_gives_its_text_and_is_found(pdfium: &Pdfium) {
    let work = tempfile::tempdir().unwrap();
    let path = write_pdf(work.path(), &tiny_pdf());
    let document = pdfium.load_pdf_from_file(&path, None).unwrap();
    let opened = describe(&document).unwrap();
    assert_eq!(opened.page_count, 2);
    assert_eq!(opened.first_page.width, 200.0);
    assert_eq!(opened.first_page.height, 100.0);
    assert!(opened.outline.is_empty());

    let bitmap = render(&document, PageIdx(0), 400, 200).unwrap();
    assert_eq!((bitmap.width, bitmap.height), (400, 200));
    assert_eq!(bitmap.bgra.len(), 400 * 200 * 4);
    let dark = bitmap
        .bgra
        .chunks(4)
        .filter(|pixel| pixel[0] < 128 && pixel[1] < 128 && pixel[2] < 128)
        .count();
    assert!(
        dark > 100,
        "the text should leave dark pixels, found {dark}"
    );
    assert_eq!(&bitmap.bgra[..4], [255, 255, 255, 255]);

    let page = document.pages().get(0).unwrap();
    let content = read_content(&page);
    let text: String = content.chars.iter().map(|found| found.ch).collect();
    assert!(text.contains("Hello PDF Hello"), "{text:?}");
    let first = content.chars.iter().find(|found| found.ch == 'H').unwrap();
    assert!(first.rect.left >= 19.0 && first.rect.right <= 200.0);
    assert!(first.rect.top >= 0.0 && first.rect.bottom <= 100.0);

    let chars = search_text(&page).unwrap();
    assert_eq!(find_in_page(&chars, "hello").len(), 2);
    assert!(search_text(&document.pages().get(1).unwrap()).is_none());
}

fn the_host_loop_answers_open_sizes_render_and_find(pdfium: Pdfium) {
    let work = tempfile::tempdir().unwrap();
    let path = write_pdf(work.path(), &tiny_pdf());
    let jobs = [
        Job::Open {
            path,
            password: None,
        },
        Job::Sizes,
        Job::Render {
            page: PageIdx(1),
            pixel_width: 30,
            pixel_height: 40,
            thumbnail: true,
            generation: 9,
        },
        Job::Find {
            search: 7,
            query: "PDF".to_string(),
            first: PageIdx(0),
            count: 10,
        },
        Job::Close,
    ];
    let mut input = Vec::new();
    for (index, job) in jobs.iter().enumerate() {
        let frame = encode_request(index as u64, DocId(1), job).unwrap();
        write_frame(&mut input, &frame).unwrap();
    }
    let mut output = Vec::new();
    serve(&Ok(pdfium), input.as_slice(), &mut output).unwrap();
    let mut replies = output.as_slice();
    let mut next = |job: &Job| {
        let frame = read_frame(&mut replies).unwrap().unwrap();
        decode_reply(&frame, &Expect::of(job)).unwrap()
    };
    let opened = next(&jobs[0]);
    assert!(matches!(opened, Some(Reply::Opened(Ok(ref opened))) if opened.page_count == 2));
    let Some(Reply::Sizes(sizes)) = next(&jobs[1]) else {
        panic!("sizes expected");
    };
    assert_eq!(
        sizes[1],
        PageSize {
            width: 300.0,
            height: 400.0
        }
    );
    assert!(matches!(
        next(&jobs[2]),
        Some(Reply::Rendered {
            thumbnail: true,
            generation: 9,
            bitmap: Some(_),
            ..
        })
    ));
    let Some(Reply::Found {
        matches,
        next: after,
        ..
    }) = next(&jobs[3])
    else {
        panic!("found expected");
    };
    assert_eq!(matches.len(), 1);
    assert_eq!(after, None);
    assert!(next(&jobs[4]).is_none());
    assert!(read_frame(&mut replies).unwrap().is_none());
}

fn a_file_that_is_not_a_pdf_or_is_missing_says_so(pdfium: &Pdfium) {
    let work = tempfile::tempdir().unwrap();
    let path = write_pdf(work.path(), b"this is not a pdf at all");
    let mut documents = HashMap::new();
    let open = Job::Open {
        path,
        password: None,
    };
    let reply = process(pdfium, &mut documents, DocId(1), &open);
    assert!(matches!(
        reply,
        Some(Reply::Opened(Err(OpenError::Broken(BrokenFile::NotAPdf))))
    ));
    let missing = Job::Open {
        path: work.path().join("absent.pdf"),
        password: None,
    };
    let reply = process(pdfium, &mut documents, DocId(1), &missing);
    assert!(matches!(
        reply,
        Some(Reply::Opened(Err(OpenError::Broken(
            BrokenFile::Unreadable(_)
        ))))
    ));
}

// Pdfium binds once per process, so the steps that need the real library share one binding
// and run in turn inside a single test.
#[test]
fn the_real_library_renders_reads_finds_and_reports_bad_files() {
    let Some(folder) = library_folder() else {
        return;
    };
    let pdfium = library::bind_in(&folder).unwrap();
    a_pdf_renders_gives_its_text_and_is_found(&pdfium);
    a_file_that_is_not_a_pdf_or_is_missing_says_so(&pdfium);
    the_host_loop_answers_open_sizes_render_and_find(pdfium);
}

#[test]
fn without_the_library_the_host_reports_it_on_every_open_and_nothing_else() {
    let jobs = [
        Job::Open {
            path: PathBuf::from("a.pdf"),
            password: None,
        },
        Job::Sizes,
    ];
    let mut input = Vec::new();
    for (index, job) in jobs.iter().enumerate() {
        write_frame(
            &mut input,
            &encode_request(index as u64, DocId(1), job).unwrap(),
        )
        .unwrap();
    }
    let mut output = Vec::new();
    serve(
        &Err(OpenError::LibraryMissing),
        input.as_slice(),
        &mut output,
    )
    .unwrap();
    let mut replies = output.as_slice();
    let first = read_frame(&mut replies).unwrap().unwrap();
    assert!(matches!(
        decode_reply(&first, &Expect::Open).unwrap(),
        Some(Reply::Opened(Err(OpenError::LibraryMissing)))
    ));
    let second = read_frame(&mut replies).unwrap().unwrap();
    assert!(decode_reply(&second, &Expect::Sizes).unwrap().is_none());
}

#[test]
fn the_host_stops_on_a_request_it_cannot_read() {
    let garbage = b"definitely not a request";
    let result = serve(
        &Err(OpenError::LibraryMissing),
        garbage.as_slice(),
        Vec::new(),
    );
    assert!(result.is_err());
}
