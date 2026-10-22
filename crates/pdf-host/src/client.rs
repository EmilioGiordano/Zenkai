use std::collections::{HashMap, VecDeque};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use async_channel::{Receiver, Sender};

use crate::codec::{Expect, decode_reply, encode_request, reply_id};
use crate::frame::{read_frame, write_frame};
use crate::wire::{BrokenFile, DocId, Job, OpenError, PageContent, Reply, Request};

const HOST_FILE: &str = "zenkai-pdf-host.exe";
const MAX_IN_FLIGHT: usize = 4;
const SLOT_POLL: Duration = Duration::from_millis(50);
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// The way the app talks to the PDF host process. One host serves every PDF tab; a host that
// stopped, for whatever reason, is replaced by the next open.
#[derive(Clone)]
pub struct PdfClient {
    host: Option<PathBuf>,
    environment: Vec<(String, String)>,
    running: Arc<Mutex<Option<Running>>>,
}

struct Running {
    requests: Sender<Request>,
    alive: Arc<AtomicBool>,
}

struct Pending {
    reply: Sender<Reply>,
    expect: Expect,
    doc: DocId,
    closes: bool,
}

#[derive(Default)]
struct Book {
    pending: HashMap<u64, Pending>,
    documents: HashMap<DocId, Sender<Reply>>,
    in_flight: usize,
    stopped: bool,
}

struct Link {
    book: Mutex<Book>,
    changed: Condvar,
    alive: Arc<AtomicBool>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl PdfClient {
    // The host is looked for next to the running exe and nowhere else.
    pub fn next_to_exe() -> PdfClient {
        let host = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|folder| folder.join(HOST_FILE)));
        PdfClient::new(host)
    }

    pub fn new(host: Option<PathBuf>) -> PdfClient {
        PdfClient {
            host,
            environment: Vec::new(),
            running: Arc::new(Mutex::new(None)),
        }
    }

    pub fn with_environment(mut self, name: &str, value: &str) -> PdfClient {
        self.environment.push((name.to_string(), value.to_string()));
        self
    }

    // Never waits for the host: the request is queued for a thread that writes to the pipe,
    // and the answer arrives on the request's own channel.
    pub fn submit(&self, request: Request) {
        let mut running = locked(&self.running);
        if running
            .as_ref()
            .is_some_and(|running| !running.alive.load(Ordering::Acquire))
        {
            *running = None;
        }
        if running.is_none() {
            if !matches!(request.job, Job::Open { .. }) {
                tell(&request.reply, Reply::Stopped);
                return;
            }
            match self.start() {
                Ok(started) => *running = Some(started),
                Err(error) => {
                    tell(&request.reply, Reply::Opened(Err(error)));
                    return;
                }
            }
        }
        let Some(current) = running.as_ref() else {
            return;
        };
        if let Err(refused) = current.requests.try_send(request) {
            current.alive.store(false, Ordering::Release);
            tell(&refused.into_inner().reply, Reply::Stopped);
        }
    }

    fn start(&self) -> Result<Running, OpenError> {
        let host = self.host.as_deref().ok_or(OpenError::HostMissing)?;
        let (child, stdin, stdout) = spawn_host(host, &self.environment)?;
        let alive = Arc::new(AtomicBool::new(true));
        let link = Arc::new(Link {
            book: Mutex::new(Book::default()),
            changed: Condvar::new(),
            alive: alive.clone(),
        });
        let (requests, inbox) = async_channel::unbounded();
        let writer_link = link.clone();
        let reader_link = link.clone();
        let writer = std::thread::Builder::new()
            .name("pdf-host-writer".to_string())
            .spawn(move || write_requests(&writer_link, &inbox, stdin));
        let reader = std::thread::Builder::new()
            .name("pdf-host-reader".to_string())
            .spawn(move || read_replies(&reader_link, stdout, child));
        if let (Err(error), _) | (_, Err(error)) = (&writer, &reader) {
            // A thread that did start ends with the pipe it owns, once the link is stopped.
            stop(&link);
            return Err(OpenError::HostFailed(error.to_string()));
        }
        Ok(Running { requests, alive })
    }
}

fn spawn_host(
    path: &Path,
    environment: &[(String, String)],
) -> Result<(Child, std::process::ChildStdin, std::process::ChildStdout), OpenError> {
    let mut command = Command::new(path);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (name, value) in environment {
        command.env(name, value);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            OpenError::HostMissing
        } else {
            OpenError::HostFailed(error.to_string())
        }
    })?;
    match (child.stdin.take(), child.stdout.take()) {
        (Some(stdin), Some(stdout)) => Ok((child, stdin, stdout)),
        _ => {
            kill(&mut child);
            Err(OpenError::HostFailed("the pipes did not open".to_string()))
        }
    }
}

fn kill(child: &mut Child) {
    if let Err(error) = child.kill() {
        tracing::debug!(%error, "the PDF host was already gone");
    }
    if let Err(error) = child.wait() {
        tracing::debug!(%error, "could not wait for the PDF host");
    }
}

fn tell(reply: &Sender<Reply>, message: Reply) {
    if reply.try_send(message).is_err() {
        tracing::debug!("a PDF view closed before its answer arrived");
    }
}

fn is_stale(request: &Request) -> bool {
    match (&request.job, &request.token) {
        (Job::Render { generation, .. }, Some(token)) => {
            token.load(Ordering::Relaxed) != *generation
        }
        _ => false,
    }
}

// Ends the link: nothing more is sent, and every document that was open learns the host is
// gone. Safe to call more than once.
fn stop(link: &Link) {
    link.alive.store(false, Ordering::Release);
    let (pending, documents) = {
        let mut book = locked(&link.book);
        if book.stopped {
            return;
        }
        book.stopped = true;
        book.in_flight = 0;
        (
            std::mem::take(&mut book.pending),
            std::mem::take(&mut book.documents),
        )
    };
    link.changed.notify_all();
    // A view that has a request in flight and an open document is told once.
    let mut told: Vec<Sender<Reply>> = Vec::new();
    for reply in pending
        .into_values()
        .map(|pending| pending.reply)
        .chain(documents.into_values())
    {
        if !told.iter().any(|known| known.same_channel(&reply)) {
            tell(&reply, Reply::Stopped);
            told.push(reply);
        }
    }
}

fn write_requests(link: &Link, inbox: &Receiver<Request>, stdin: std::process::ChildStdin) {
    let mut output = BufWriter::new(stdin);
    let mut queue: VecDeque<Request> = VecDeque::new();
    let mut next_id = 1u64;
    loop {
        if queue.is_empty() {
            match inbox.recv_blocking() {
                Ok(request) => queue.push_back(request),
                Err(_) => break,
            }
        }
        if !wait_for_slot(link, inbox, &mut queue) {
            break;
        }
        let Some(request) = queue.pop_front() else {
            continue;
        };
        let id = next_id;
        next_id += 1;
        let frame = match encode_request(id, request.doc, &request.job) {
            Ok(frame) => frame,
            Err(_) => {
                tell(&request.reply, refusal(&request));
                continue;
            }
        };
        {
            let mut book = locked(&link.book);
            if book.stopped {
                break;
            }
            book.in_flight += 1;
            if matches!(request.job, Job::Open { .. }) {
                book.documents.insert(request.doc, request.reply.clone());
            }
            book.pending.insert(
                id,
                Pending {
                    reply: request.reply.clone(),
                    expect: Expect::of(&request.job),
                    doc: request.doc,
                    closes: matches!(request.job, Job::Close),
                },
            );
        }
        if write_frame(&mut output, &frame).is_err() {
            break;
        }
    }
    stop(link);
}

fn refusal(request: &Request) -> Reply {
    match &request.job {
        Job::Open { .. } => Reply::Opened(Err(OpenError::Broken(BrokenFile::Unreadable(
            "the path cannot be sent".to_string(),
        )))),
        _ => Reply::Stopped,
    }
}

// Waits until fewer than the allowed requests are with the host, dropping renders that went
// stale meanwhile, so a fast scroll does not leave the host busy with pages nobody sees.
// False when the link was stopped.
fn wait_for_slot(link: &Link, inbox: &Receiver<Request>, queue: &mut VecDeque<Request>) -> bool {
    let mut book = locked(&link.book);
    loop {
        while let Ok(request) = inbox.try_recv() {
            queue.push_back(request);
        }
        queue.retain(|request| !is_stale(request));
        if book.stopped {
            return false;
        }
        if queue.is_empty() || book.in_flight < MAX_IN_FLIGHT {
            return true;
        }
        book = link
            .changed
            .wait_timeout(book, SLOT_POLL)
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .0;
    }
}

fn read_replies(link: &Link, stdout: std::process::ChildStdout, mut child: Child) {
    let mut input = BufReader::new(stdout);
    loop {
        let Ok(Some(frame)) = read_frame(&mut input) else {
            break;
        };
        let Ok(id) = reply_id(&frame) else {
            break;
        };
        let Some(pending) = ({
            let mut book = locked(&link.book);
            let taken = book.pending.remove(&id);
            if taken.is_some() {
                book.in_flight = book.in_flight.saturating_sub(1);
            }
            taken
        }) else {
            break;
        };
        link.changed.notify_all();
        let Ok(decoded) = decode_reply(&frame, &pending.expect) else {
            tell(&pending.reply, Reply::Stopped);
            break;
        };
        if pending.closes {
            locked(&link.book).documents.remove(&pending.doc);
        }
        if let Some(reply) = decoded.or_else(|| nothing_to_give(&pending.expect)) {
            tell(&pending.reply, reply);
        }
    }
    stop(link);
    kill(&mut child);
}

// What the view should see when the host had nothing for a request, so it does not wait.
fn nothing_to_give(expect: &Expect) -> Option<Reply> {
    match expect {
        Expect::Render {
            page,
            pixel_width,
            thumbnail,
            generation,
        } => Some(Reply::Rendered {
            page: *page,
            pixel_width: *pixel_width,
            thumbnail: *thumbnail,
            generation: *generation,
            bitmap: None,
        }),
        Expect::Content { page } => Some(Reply::Content {
            page: *page,
            content: PageContent::default(),
        }),
        Expect::Find { search, .. } => Some(Reply::Found {
            search: *search,
            matches: Vec::new(),
            next: None,
        }),
        Expect::Open | Expect::Sizes | Expect::Nothing => None,
    }
}
