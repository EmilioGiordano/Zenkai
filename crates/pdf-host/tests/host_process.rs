// These tests start the real zenkai-pdf-host.exe. The ones that read a PDF need pdfium.dll
// next to the binaries (target/<profile>) and return early without it.
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_channel::{Receiver, Sender};
use zenkai_pdf_host::client::PdfClient;
use zenkai_pdf_host::host::ABORT_ENVIRONMENT;
use zenkai_pdf_host::wire::{DocId, Job, OpenError, PageIdx, Reply, Request};

const HOST: &str = env!("CARGO_BIN_EXE_zenkai-pdf-host");
const WAIT: Duration = Duration::from_secs(60);

fn host_folder() -> PathBuf {
    Path::new(HOST).parent().unwrap().to_path_buf()
}

fn library_present() -> bool {
    host_folder().join("pdfium.dll").is_file()
}

fn client() -> PdfClient {
    PdfClient::new(Some(PathBuf::from(HOST))).with_environment(ABORT_ENVIRONMENT, "1")
}

fn channel() -> (Sender<Reply>, Receiver<Reply>) {
    async_channel::unbounded()
}

fn ask(client: &PdfClient, doc: u64, reply: &Sender<Reply>, job: Job) {
    client.submit(Request {
        doc: DocId(doc),
        reply: reply.clone(),
        job,
        token: None,
    });
}

fn next(replies: &Receiver<Reply>) -> Reply {
    let replies = replies.clone();
    let (done, wait) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        if done.send(replies.recv_blocking()).is_err() {
            // The test already gave up waiting.
        }
    });
    wait.recv_timeout(WAIT)
        .expect("the host answered in time")
        .expect("the reply channel stayed open")
}

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

fn open_job(path: &Path) -> Job {
    Job::Open {
        path: path.to_path_buf(),
        password: None,
    }
}

#[test]
fn a_missing_host_exe_is_reported_on_the_open() {
    let client = PdfClient::new(Some(PathBuf::from(
        "C:\\no\\such\\folder\\zenkai-pdf-host.exe",
    )));
    let (reply, replies) = channel();
    ask(&client, 1, &reply, open_job(Path::new("a.pdf")));
    assert!(matches!(
        next(&replies),
        Reply::Opened(Err(OpenError::HostMissing))
    ));
    let (reply, replies) = channel();
    ask(&client, 1, &reply, Job::Sizes);
    assert!(matches!(next(&replies), Reply::Stopped));
}

#[test]
fn a_host_that_aborts_is_reported_and_the_next_open_starts_a_new_one() {
    let client = client();
    let work = tempfile::tempdir().unwrap();
    let missing = work.path().join("absent.pdf");

    let (reply, replies) = channel();
    ask(&client, 1, &reply, open_job(&missing));
    assert!(matches!(next(&replies), Reply::Opened(Err(_))));

    ask(&client, 1, &reply, Job::Abort);
    assert!(matches!(next(&replies), Reply::Stopped));

    // The document that was open learns the same, and anything sent now is told so.
    ask(&client, 1, &reply, Job::Sizes);
    assert!(matches!(next(&replies), Reply::Stopped));

    let (second_reply, second) = channel();
    ask(&client, 2, &second_reply, open_job(&missing));
    assert!(matches!(next(&second), Reply::Opened(Err(_))));
}

#[test]
fn every_open_document_hears_that_the_host_stopped() {
    if !library_present() {
        return;
    }
    let client = client();
    let work = tempfile::tempdir().unwrap();
    let path = work.path().join("tiny.pdf");
    std::fs::write(&path, tiny_pdf()).unwrap();

    let (first_reply, first) = channel();
    let (second_reply, second) = channel();
    ask(&client, 1, &first_reply, open_job(&path));
    ask(&client, 2, &second_reply, open_job(&path));
    for replies in [&first, &second] {
        assert!(matches!(next(replies), Reply::Opened(Ok(_))));
    }

    ask(&client, 1, &first_reply, Job::Abort);
    for replies in [&first, &second] {
        assert!(matches!(next(replies), Reply::Stopped));
    }

    ask(&client, 3, &first_reply, open_job(&path));
    assert!(matches!(next(&first), Reply::Opened(Ok(_))));
}

#[test]
fn a_real_pdf_opens_renders_and_is_searched_through_the_process() {
    if !library_present() {
        return;
    }
    let client = PdfClient::new(Some(PathBuf::from(HOST)));
    let work = tempfile::tempdir().unwrap();
    let path = work.path().join("tiny.pdf");
    std::fs::write(&path, tiny_pdf()).unwrap();
    let (reply, replies) = channel();

    ask(&client, 1, &reply, open_job(&path));
    let Reply::Opened(Ok(opened)) = next(&replies) else {
        panic!("the file should open");
    };
    assert_eq!(opened.page_count, 2);

    ask(
        &client,
        1,
        &reply,
        Job::Render {
            page: PageIdx(0),
            pixel_width: 400,
            pixel_height: 200,
            thumbnail: false,
            generation: 3,
        },
    );
    let Reply::Rendered {
        bitmap: Some(bitmap),
        generation: 3,
        ..
    } = next(&replies)
    else {
        panic!("a bitmap should come back");
    };
    assert_eq!((bitmap.width, bitmap.height), (400, 200));
    assert_eq!(bitmap.bgra.len(), 400 * 200 * 4);

    ask(
        &client,
        1,
        &reply,
        Job::Find {
            search: 1,
            query: "hello".to_string(),
            first: PageIdx(0),
            count: 10,
        },
    );
    let Reply::Found { matches, .. } = next(&replies) else {
        panic!("matches should come back");
    };
    assert_eq!(matches.len(), 2);
}

// whoami.exe prints a line of text and exits: not a frame, so the reply is a violation.
#[cfg(windows)]
#[test]
fn a_host_that_answers_with_garbage_is_dropped() {
    let system = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap();
    let impostor = system.join("System32").join("whoami.exe");
    let client = PdfClient::new(Some(impostor));
    let (reply, replies) = channel();
    ask(&client, 1, &reply, open_job(Path::new("a.pdf")));
    assert!(matches!(next(&replies), Reply::Stopped));
}
