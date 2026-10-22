use std::collections::{HashMap, VecDeque};
use std::io::{BufReader, BufWriter, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use async_channel::{Receiver, Sender};

use crate::codec::{Expect, decode_reply, encode_request, reply_id};
use crate::frame::{read_frame, write_frame};
use crate::wire::{BrokenFile, DocId, Job, OpenError, PageContent, Reply, Request};

const HOST_FILE: &str = "zenkai-pdf-host.exe";
const MAX_IN_FLIGHT: usize = 4;
const SLOT_POLL: Duration = Duration::from_millis(50);
const WATCH_POLL: Duration = Duration::from_millis(250);
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// How long the host may take over one request before it is taken for stuck.
#[derive(Clone, Copy, Debug)]
pub struct Deadlines {
    pub open: Duration,
    pub other: Duration,
}

impl Default for Deadlines {
    fn default() -> Deadlines {
        Deadlines {
            open: Duration::from_secs(30),
            other: Duration::from_secs(15),
        }
    }
}

// The way the app talks to the PDF host process. One host serves every PDF tab; a host that
// stopped, for whatever reason, is replaced by the next open.
#[derive(Clone)]
pub struct PdfClient {
    host: Option<PathBuf>,
    arguments: Vec<String>,
    environment: Vec<(String, String)>,
    deadlines: Deadlines,
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
    deadline: Instant,
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
    inbox: Receiver<Request>,
    child: Mutex<Option<Child>>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Link {
    fn new(inbox: Receiver<Request>, alive: Arc<AtomicBool>) -> Link {
        Link {
            book: Mutex::new(Book::default()),
            changed: Condvar::new(),
            alive,
            inbox,
            child: Mutex::new(None),
        }
    }
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
            arguments: Vec::new(),
            environment: Vec::new(),
            deadlines: Deadlines::default(),
            running: Arc::new(Mutex::new(None)),
        }
    }

    pub fn with_environment(mut self, name: &str, value: &str) -> PdfClient {
        self.environment.push((name.to_string(), value.to_string()));
        self
    }

    pub fn with_arguments(mut self, arguments: &[&str]) -> PdfClient {
        self.arguments = arguments.iter().map(ToString::to_string).collect();
        self
    }

    pub fn with_deadlines(mut self, deadlines: Deadlines) -> PdfClient {
        self.deadlines = deadlines;
        self
    }

    // Never waits for the host, not even to start it: the request is queued for a thread that
    // starts the process and writes to the pipe, and the answer arrives on the request's own
    // channel.
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
        let alive = Arc::new(AtomicBool::new(true));
        let (requests, inbox) = async_channel::unbounded();
        let link = Arc::new(Link::new(inbox, alive.clone()));
        let launch = Launch {
            host: self.host.clone(),
            arguments: self.arguments.clone(),
            environment: self.environment.clone(),
            deadlines: self.deadlines,
        };
        let thread_link = link.clone();
        std::thread::Builder::new()
            .name("pdf-host-writer".to_string())
            .spawn(move || run_writer(&thread_link, &launch))
            .map_err(|error| OpenError::HostFailed(error.to_string()))?;
        Ok(Running { requests, alive })
    }
}

struct Launch {
    host: Option<PathBuf>,
    arguments: Vec<String>,
    environment: Vec<(String, String)>,
    deadlines: Deadlines,
}

fn spawn_host(launch: &Launch) -> Result<(Child, ChildStdin, ChildStdout), OpenError> {
    let path: &Path = launch.host.as_deref().ok_or(OpenError::HostMissing)?;
    let mut command = Command::new(path);
    command
        .args(&launch.arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (name, value) in &launch.environment {
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

// Ends the link: nothing more is sent, the host is killed, the queue is closed so the writer
// ends, and every document that was open learns the host is gone. Safe to call more than once.
fn stop(link: &Link) {
    link.alive.store(false, Ordering::Release);
    link.inbox.close();
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
    if let Some(mut child) = locked(&link.child).take() {
        kill(&mut child);
    }
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

// The host could not be started: whatever is queued, and arrives, is answered with why.
fn fail_start(link: &Link, error: &OpenError) {
    link.alive.store(false, Ordering::Release);
    link.inbox.close();
    locked(&link.book).stopped = true;
    while let Ok(request) = link.inbox.try_recv() {
        let reply = if matches!(request.job, Job::Open { .. }) {
            Reply::Opened(Err(error.clone()))
        } else {
            Reply::Stopped
        };
        tell(&request.reply, reply);
    }
}

fn run_writer(link: &Arc<Link>, launch: &Launch) {
    let (child, stdin, stdout) = match spawn_host(launch) {
        Ok(started) => started,
        Err(error) => {
            fail_start(link, &error);
            return;
        }
    };
    *locked(&link.child) = Some(child);
    let reader_link = link.clone();
    let watch_link = link.clone();
    let deadlines = launch.deadlines;
    let readers = std::thread::Builder::new()
        .name("pdf-host-reader".to_string())
        .spawn(move || {
            read_replies(&reader_link, BufReader::new(stdout));
            stop(&reader_link);
        })
        .and_then(|_| {
            std::thread::Builder::new()
                .name("pdf-host-watchdog".to_string())
                .spawn(move || watch(&watch_link))
        });
    if readers.is_err() {
        stop(link);
        return;
    }
    write_requests(link, stdin, deadlines);
    stop(link);
}

fn write_requests(link: &Link, stdin: ChildStdin, deadlines: Deadlines) {
    let mut output = BufWriter::new(stdin);
    let mut queue: VecDeque<Request> = VecDeque::new();
    let mut next_id = 1u64;
    loop {
        if queue.is_empty() {
            match link.inbox.recv_blocking() {
                Ok(request) => queue.push_back(request),
                Err(_) => break,
            }
        }
        if !wait_for_slot(link, &mut queue) {
            break;
        }
        let Some(request) = queue.pop_front() else {
            continue;
        };
        let id = next_id;
        next_id += 1;
        let Ok(frame) = encode_request(id, request.doc, &request.job) else {
            tell(&request.reply, refusal(&request));
            continue;
        };
        {
            let mut book = locked(&link.book);
            if book.stopped {
                break;
            }
            book.in_flight += 1;
            let is_open = matches!(request.job, Job::Open { .. });
            if is_open {
                book.documents.insert(request.doc, request.reply.clone());
            }
            let allowed = if is_open {
                deadlines.open
            } else {
                deadlines.other
            };
            book.pending.insert(
                id,
                Pending {
                    reply: request.reply.clone(),
                    expect: Expect::of(&request.job),
                    doc: request.doc,
                    closes: matches!(request.job, Job::Close),
                    deadline: Instant::now() + allowed,
                },
            );
        }
        if write_frame(&mut output, &frame).is_err() {
            break;
        }
    }
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
fn wait_for_slot(link: &Link, queue: &mut VecDeque<Request>) -> bool {
    let mut book = locked(&link.book);
    loop {
        while let Ok(request) = link.inbox.try_recv() {
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

// Reads replies until the stream ends or breaks a rule. A reply that fails its checks costs
// only the document it was for: the frame itself was well formed, so the host stays.
fn read_replies(link: &Link, mut input: impl Read) {
    loop {
        let Ok(Some(frame)) = read_frame(&mut input) else {
            return;
        };
        let Ok(id) = reply_id(&frame) else {
            return;
        };
        let Some(pending) = ({
            let mut book = locked(&link.book);
            let taken = book.pending.remove(&id);
            if taken.is_some() {
                book.in_flight = book.in_flight.saturating_sub(1);
            }
            taken
        }) else {
            return;
        };
        link.changed.notify_all();
        let Ok(decoded) = decode_reply(frame, &pending.expect) else {
            tracing::warn!("a PDF host reply broke the rules; the document is dropped");
            locked(&link.book).documents.remove(&pending.doc);
            tell(&pending.reply, Reply::Stopped);
            continue;
        };
        if pending.closes {
            locked(&link.book).documents.remove(&pending.doc);
        }
        if let Some(reply) = decoded.or_else(|| nothing_to_give(&pending.expect)) {
            tell(&pending.reply, reply);
        }
    }
}

// Stops the link when a request has been with the host longer than it may be.
fn watch(link: &Link) {
    let mut book = locked(&link.book);
    loop {
        if book.stopped {
            return;
        }
        let now = Instant::now();
        let earliest = book.pending.values().map(|pending| pending.deadline).min();
        if earliest.is_some_and(|deadline| deadline <= now) {
            drop(book);
            tracing::warn!("the PDF host took too long; it is stopped");
            stop(link);
            return;
        }
        let wait = earliest.map_or(WATCH_POLL, |deadline| {
            deadline.saturating_duration_since(now).min(WATCH_POLL)
        });
        book = link
            .changed
            .wait_timeout(book, wait)
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .0;
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::encode_reply;
    use crate::wire::{PageIdx, PageSize};

    fn link() -> Link {
        let (_requests, inbox) = async_channel::unbounded();
        Link::new(inbox, Arc::new(AtomicBool::new(true)))
    }

    fn expect_pending(link: &Link, id: u64, expect: Expect, doc: u64) -> Receiver<Reply> {
        let (reply, replies) = async_channel::unbounded();
        let mut book = locked(&link.book);
        book.in_flight += 1;
        book.documents.insert(DocId(doc), reply.clone());
        book.pending.insert(
            id,
            Pending {
                reply,
                expect,
                doc: DocId(doc),
                closes: false,
                deadline: Instant::now() + Duration::from_secs(60),
            },
        );
        replies
    }

    fn stream(frames: &[crate::frame::Frame]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for frame in frames {
            write_frame(&mut bytes, frame).unwrap();
        }
        bytes
    }

    #[test]
    fn a_reply_that_breaks_a_rule_costs_only_its_document() {
        let link = link();
        let broken = expect_pending(&link, 1, Expect::Sizes, 10);
        let fine = expect_pending(&link, 2, Expect::Sizes, 20);
        let bad = encode_reply(
            1,
            Some(Reply::Sizes(vec![PageSize {
                width: 0.0,
                height: 5.0,
            }])),
        )
        .unwrap();
        let good = encode_reply(2, Some(Reply::Sizes(vec![PageSize::LETTER]))).unwrap();
        read_replies(&link, stream(&[bad, good]).as_slice());
        assert!(matches!(broken.try_recv(), Ok(Reply::Stopped)));
        assert!(matches!(fine.try_recv(), Ok(Reply::Sizes(_))));
        assert!(!locked(&link.book).stopped);
        assert!(!locked(&link.book).documents.contains_key(&DocId(10)));
        assert!(locked(&link.book).documents.contains_key(&DocId(20)));
    }

    #[test]
    fn a_broken_stream_or_a_stranger_reply_ends_the_reading() {
        let link = link();
        let waiting = expect_pending(&link, 1, Expect::Sizes, 10);
        let stranger = encode_reply(99, Some(Reply::Sizes(vec![PageSize::LETTER]))).unwrap();
        let after = encode_reply(1, Some(Reply::Sizes(vec![PageSize::LETTER]))).unwrap();
        read_replies(&link, stream(&[stranger, after]).as_slice());
        assert!(waiting.try_recv().is_err());
        assert_eq!(locked(&link.book).pending.len(), 1);
        read_replies(&link, b"garbage that is no frame at all".as_slice());
        stop(&link);
        assert!(matches!(waiting.try_recv(), Ok(Reply::Stopped)));
        assert!(waiting.try_recv().is_err());
    }

    #[test]
    fn nothing_from_the_host_still_answers_a_render() {
        let link = link();
        let render = Expect::Render {
            page: PageIdx(1),
            pixel_width: 5,
            thumbnail: false,
            generation: 2,
        };
        let replies = expect_pending(&link, 4, render, 1);
        read_replies(&link, stream(&[encode_reply(4, None).unwrap()]).as_slice());
        assert!(matches!(
            replies.try_recv(),
            Ok(Reply::Rendered { bitmap: None, .. })
        ));
    }
}
