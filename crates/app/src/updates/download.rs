use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

use ureq::http::Response;
use url::{Host, Url};

use super::version::Version;

// Fixed at build time, like the release list: the asset is named from the version and the
// platform, never taken from the JSON, and every hop must stay on GitHub.
const RELEASE_DOWNLOADS: &str = "https://github.com/EmilioGiordano/Zenkai/releases/download";
const DOWNLOAD_HOSTS: [&str; 4] = [
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
    "github-releases.githubusercontent.com",
];
pub const MAX_REDIRECTS: usize = 5;
const USER_AGENT: &str = "Zenkai";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const READ_CHUNK: usize = 64 * 1024;
const QUEUED_CHUNKS: usize = 16;
pub const PROGRESS_STEP: u64 = 1024 * 1024;

// `stall` is how long the server may stay quiet, headers included; `poll` is how often a cancel
// is looked at while it is quiet.
#[derive(Clone, Copy, Debug)]
pub struct Patience {
    pub stall: Duration,
    pub poll: Duration,
}

pub const PATIENCE: Patience = Patience {
    stall: Duration::from_secs(60),
    poll: Duration::from_millis(200),
};

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("the request failed: {0}")]
    Request(#[from] ureq::Error),
    #[error("the server answered with status {0}")]
    Status(u16),
    #[error("a redirect has no usable location")]
    NoLocation,
    #[error("a redirect leads outside GitHub: {0}")]
    Forbidden(String),
    #[error("not a valid address: {0}")]
    Address(#[from] url::ParseError),
    #[error("more than {} redirects", MAX_REDIRECTS)]
    TooManyRedirects,
    #[error("the download is larger than {0} bytes")]
    TooLarge(u64),
    #[error("the transfer broke off: {0}")]
    Transfer(#[source] io::Error),
    #[error("the server stopped sending")]
    Stalled,
    #[error("could not write the download: {0}")]
    Write(#[source] io::Error),
    #[error("the download was cancelled")]
    Cancelled,
}

pub fn asset_url(version: Version, file: &str) -> Result<Url, DownloadError> {
    Ok(Url::parse(&format!(
        "{RELEASE_DOWNLOADS}/v{version}/{file}"
    ))?)
}

pub fn allowed(url: &Url) -> Result<(), DownloadError> {
    let host_allowed =
        matches!(url.host(), Some(Host::Domain(host)) if DOWNLOAD_HOSTS.contains(&host));
    let plain = url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none();
    if host_allowed && plain {
        Ok(())
    } else {
        Err(DownloadError::Forbidden(url.to_string()))
    }
}

pub enum Answer<B> {
    Body(B),
    Redirect(String),
}

// Each address is checked before it is requested, so no redirect ever reaches a host outside
// the list; `get` answers one request without following anything.
pub fn follow<B>(
    start: Url,
    mut get: impl FnMut(&Url) -> Result<Answer<B>, DownloadError>,
) -> Result<B, DownloadError> {
    let mut url = start;
    for _ in 0..=MAX_REDIRECTS {
        allowed(&url)?;
        match get(&url)? {
            Answer::Body(body) => return Ok(body),
            Answer::Redirect(location) => url = url.join(&location)?,
        }
    }
    Err(DownloadError::TooManyRedirects)
}

// No response timeout: ureq turns it into a fixed deadline for the whole body. A quiet server is
// caught by `receive`; the per-read body timeout only ends a transfer thread nobody waits for.
pub fn agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_recv_body(Some(PATIENCE.stall))
        .https_only(true)
        .max_redirects(0)
        .http_status_as_error(false)
        .user_agent(USER_AGENT)
        .build();
    ureq::Agent::new_with_config(config)
}

fn request(agent: &ureq::Agent, url: &Url) -> Result<Answer<Response<ureq::Body>>, DownloadError> {
    let response = agent.get(url.as_str()).call()?;
    match response.status().as_u16() {
        200 => Ok(Answer::Body(response)),
        301 | 302 | 303 | 307 | 308 => {
            let location = response
                .headers()
                .get("location")
                .and_then(|value| value.to_str().ok())
                .ok_or(DownloadError::NoLocation)?;
            Ok(Answer::Redirect(location.to_owned()))
        }
        status => Err(DownloadError::Status(status)),
    }
}

pub enum Piece {
    Head(Option<u64>),
    Bytes(Vec<u8>),
    End,
    Failed(DownloadError),
}

// Blocks: call it from a background thread only. The network is read on a thread of its own, so
// a server that goes quiet never keeps the caller from seeing a cancel or giving up.
pub fn download(
    agent: &ureq::Agent,
    url: Url,
    sink: &mut impl Write,
    limit: u64,
    cancel: &AtomicBool,
    progress: impl FnMut(u64, Option<u64>),
) -> Result<u64, DownloadError> {
    let (sender, pieces) = mpsc::sync_channel(QUEUED_CHUNKS);
    let agent = agent.clone();
    thread::Builder::new()
        .name("update transfer".to_owned())
        .spawn(move || transfer(&agent, url, &sender))
        .map_err(DownloadError::Transfer)?;
    receive(&pieces, sink, limit, cancel, PATIENCE, progress)
}

// A failed send means `receive` gave up; this thread then just ends.
fn transfer(agent: &ureq::Agent, url: Url, pieces: &SyncSender<Piece>) {
    let response = match follow(url, |url| request(agent, url)) {
        Ok(response) => response,
        Err(error) => {
            offer(pieces, Piece::Failed(error));
            return;
        }
    };
    if !offer(pieces, Piece::Head(response.body().content_length())) {
        return;
    }
    let mut body = response.into_body().into_reader();
    let mut buffer = vec![0; READ_CHUNK];
    loop {
        let piece = match body.read(&mut buffer) {
            Ok(0) => Piece::End,
            Ok(read) => Piece::Bytes(buffer[..read].to_vec()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => Piece::Failed(DownloadError::Transfer(error)),
        };
        let more = matches!(piece, Piece::Bytes(_));
        if !offer(pieces, piece) || !more {
            return;
        }
    }
}

fn offer(pieces: &SyncSender<Piece>, piece: Piece) -> bool {
    pieces.send(piece).is_ok()
}

// The limit counts what arrives, whatever the headers announced. Only silence counts against the
// server: a slow transfer that keeps sending may take as long as it needs.
pub fn receive(
    pieces: &Receiver<Piece>,
    sink: &mut impl Write,
    limit: u64,
    cancel: &AtomicBool,
    patience: Patience,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<u64, DownloadError> {
    let mut total = None;
    let mut received = 0u64;
    let mut reported = 0u64;
    let mut heard = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(DownloadError::Cancelled);
        }
        let piece = match pieces.recv_timeout(patience.poll) {
            Ok(piece) => piece,
            Err(RecvTimeoutError::Timeout) if heard.elapsed() >= patience.stall => {
                return Err(DownloadError::Stalled);
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                return Err(DownloadError::Transfer(io::Error::other(
                    "the transfer ended without finishing",
                )));
            }
        };
        heard = Instant::now();
        match piece {
            Piece::Head(length) => {
                if length.is_some_and(|length| length > limit) {
                    return Err(DownloadError::TooLarge(limit));
                }
                total = length;
            }
            Piece::Bytes(bytes) => {
                received += bytes.len() as u64;
                if received > limit {
                    return Err(DownloadError::TooLarge(limit));
                }
                sink.write_all(&bytes).map_err(DownloadError::Write)?;
                if received - reported >= PROGRESS_STEP {
                    reported = received;
                    progress(received, total);
                }
            }
            Piece::End => return Ok(received),
            Piece::Failed(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, Receiver};
    use std::thread;
    use std::time::{Duration, Instant};

    use url::Url;

    use super::super::version::Version;
    use super::{
        Answer, DownloadError, MAX_REDIRECTS, PROGRESS_STEP, Patience, Piece, allowed, asset_url,
        follow, receive,
    };

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    fn start() -> Url {
        asset_url(
            Version::parse_bare("0.2.0").unwrap(),
            "zenkai-v0.2.0-windows-x64.zip",
        )
        .unwrap()
    }

    #[test]
    fn the_asset_address_is_built_from_the_version_and_the_name() {
        assert_eq!(
            start().as_str(),
            "https://github.com/EmilioGiordano/Zenkai/releases/download/v0.2.0/zenkai-v0.2.0-windows-x64.zip"
        );
    }

    #[test]
    fn only_https_on_github_hosts_is_allowed() {
        for good in [
            "https://github.com/EmilioGiordano/Zenkai/releases/download/v0.2.0/a.zip",
            "https://objects.githubusercontent.com/x?sig=1",
            "https://release-assets.githubusercontent.com/x",
            "https://GITHUB.com:443/x",
        ] {
            assert!(allowed(&url(good)).is_ok(), "{good}");
        }
        for bad in [
            "http://github.com/x",
            "https://github.com.evil.example/x",
            "https://evil.example/github.com",
            "https://github.com./x",
            "https://user@github.com/x",
            "https://user:pass@github.com/x",
            "https://github.com:8443/x",
            "https://140.82.112.3/x",
            "https://[::1]/x",
            "https://raw.githubusercontent.com/x",
            "https://api.github.com/x",
            "file:///C:/Windows/notepad.exe",
            "ftp://github.com/x",
        ] {
            assert!(
                matches!(allowed(&url(bad)), Err(DownloadError::Forbidden(_))),
                "{bad}"
            );
        }
    }

    fn redirects(count: usize, to: &str) -> Result<Vec<String>, DownloadError> {
        let asked = std::cell::RefCell::new(Vec::new());
        let result = follow(start(), |url| {
            asked.borrow_mut().push(url.to_string());
            if asked.borrow().len() <= count {
                Ok(Answer::Redirect(to.to_owned()))
            } else {
                Ok(Answer::Body(()))
            }
        });
        result.map(|()| asked.into_inner())
    }

    #[test]
    fn five_redirects_are_followed() {
        let asked = redirects(MAX_REDIRECTS, "https://objects.githubusercontent.com/zip").unwrap();
        assert_eq!(asked.len(), MAX_REDIRECTS + 1);
    }

    #[test]
    fn a_sixth_redirect_is_refused() {
        assert!(matches!(
            redirects(
                MAX_REDIRECTS + 1,
                "https://objects.githubusercontent.com/zip"
            ),
            Err(DownloadError::TooManyRedirects)
        ));
    }

    #[test]
    fn a_redirect_loop_ends_at_the_limit() {
        let asked = Cell::new(0);
        let result: Result<(), DownloadError> = follow(start(), |_| {
            asked.set(asked.get() + 1);
            Ok(Answer::Redirect("/again".to_owned()))
        });
        assert!(matches!(result, Err(DownloadError::TooManyRedirects)));
        assert_eq!(asked.get(), MAX_REDIRECTS + 1);
    }

    #[test]
    fn a_redirect_outside_github_is_never_requested() {
        for location in [
            "http://objects.githubusercontent.com/zip",
            "https://evil.example/zip",
            "//evil.example/zip",
            "https://user@github.com/zip",
            "https://github.com:444/zip",
        ] {
            let asked = Cell::new(0);
            let result: Result<(), DownloadError> = follow(start(), |_| {
                asked.set(asked.get() + 1);
                if asked.get() == 1 {
                    Ok(Answer::Redirect(location.to_owned()))
                } else {
                    Ok(Answer::Body(()))
                }
            });
            assert!(
                matches!(result, Err(DownloadError::Forbidden(_))),
                "{location}"
            );
            assert_eq!(asked.get(), 1, "{location}");
        }
    }

    #[test]
    fn a_relative_redirect_stays_on_the_same_host() {
        let asked = redirects(1, "/EmilioGiordano/Zenkai/other.zip").unwrap();
        assert_eq!(
            asked[1],
            "https://github.com/EmilioGiordano/Zenkai/other.zip"
        );
    }

    enum Step {
        Send(Piece),
        Wait(Duration),
    }

    // Plays a server on its own thread; when the steps run out the channel closes.
    fn feed(steps: Vec<Step>) -> Receiver<Piece> {
        let (sender, pieces) = mpsc::sync_channel(4);
        thread::spawn(move || {
            for step in steps {
                match step {
                    Step::Send(piece) => {
                        if sender.send(piece).is_err() {
                            return;
                        }
                    }
                    Step::Wait(time) => thread::sleep(time),
                }
            }
        });
        pieces
    }

    fn bytes(count: usize) -> Step {
        Step::Send(Piece::Bytes(vec![7; count]))
    }

    const QUICK: Patience = Patience {
        stall: Duration::from_millis(300),
        poll: Duration::from_millis(10),
    };

    fn take(steps: Vec<Step>, limit: u64, cancel: &AtomicBool) -> Result<Vec<u8>, DownloadError> {
        let mut sink = Vec::new();
        receive(&feed(steps), &mut sink, limit, cancel, QUICK, |_, _| {})?;
        Ok(sink)
    }

    fn calm() -> AtomicBool {
        AtomicBool::new(false)
    }

    #[test]
    fn a_body_within_the_limit_is_copied_whole() {
        let steps = vec![
            Step::Send(Piece::Head(Some(5))),
            Step::Send(Piece::Bytes(b"12345".to_vec())),
            Step::Send(Piece::End),
        ];
        assert_eq!(take(steps, 5, &calm()).unwrap(), b"12345");
    }

    #[test]
    fn an_announced_size_past_the_limit_stops_before_any_byte() {
        let steps = vec![Step::Send(Piece::Head(Some(1001))), bytes(10)];
        assert!(matches!(
            take(steps, 1000, &calm()),
            Err(DownloadError::TooLarge(1000))
        ));
    }

    #[test]
    fn a_body_past_the_limit_stops_there_whatever_was_announced() {
        let steps = vec![
            Step::Send(Piece::Head(Some(10))),
            bytes(600),
            bytes(600),
            Step::Send(Piece::End),
        ];
        assert!(matches!(
            take(steps, 1000, &calm()),
            Err(DownloadError::TooLarge(1000))
        ));
    }

    #[test]
    fn a_cancelled_download_stops_before_the_next_piece() {
        let steps = vec![Step::Send(Piece::Head(None)), bytes(10)];
        assert!(matches!(
            take(steps, u64::MAX, &AtomicBool::new(true)),
            Err(DownloadError::Cancelled)
        ));
    }

    #[test]
    fn a_cancel_is_seen_while_the_server_is_quiet() {
        let patient = Patience {
            stall: Duration::from_secs(30),
            poll: Duration::from_millis(10),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let raised = Arc::clone(&cancel);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            raised.store(true, Ordering::Relaxed);
        });
        let pieces = feed(vec![
            Step::Send(Piece::Head(None)),
            Step::Wait(Duration::from_secs(5)),
        ]);
        let started = Instant::now();
        let result = receive(
            &pieces,
            &mut Vec::new(),
            u64::MAX,
            &cancel,
            patient,
            |_, _| {},
        );
        assert!(matches!(result, Err(DownloadError::Cancelled)));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_server_that_goes_quiet_mid_body_is_given_up() {
        let steps = vec![
            Step::Send(Piece::Head(Some(100))),
            bytes(10),
            Step::Wait(Duration::from_secs(2)),
            bytes(90),
            Step::Send(Piece::End),
        ];
        assert!(matches!(
            take(steps, 100, &calm()),
            Err(DownloadError::Stalled)
        ));
    }

    #[test]
    fn a_server_that_never_answers_is_given_up() {
        let steps = vec![Step::Wait(Duration::from_secs(2))];
        assert!(matches!(
            take(steps, 100, &calm()),
            Err(DownloadError::Stalled)
        ));
    }

    // Many times the stall limit in total, but never quiet for that long: it must finish.
    #[test]
    fn a_slow_steady_transfer_is_not_cut_short() {
        let mut steps = vec![Step::Send(Piece::Head(Some(30)))];
        for _ in 0..30 {
            steps.push(Step::Wait(Duration::from_millis(50)));
            steps.push(bytes(1));
        }
        steps.push(Step::Send(Piece::End));
        let started = Instant::now();
        assert_eq!(take(steps, 30, &calm()).unwrap().len(), 30);
        assert!(started.elapsed() > QUICK.stall * 3);
    }

    #[test]
    fn a_transfer_that_ends_without_finishing_is_an_error() {
        let steps = vec![Step::Send(Piece::Head(None)), bytes(10)];
        assert!(matches!(
            take(steps, 100, &calm()),
            Err(DownloadError::Transfer(_))
        ));
        let failed = vec![Step::Send(Piece::Failed(DownloadError::Status(500)))];
        assert!(matches!(
            take(failed, 100, &calm()),
            Err(DownloadError::Status(500))
        ));
    }

    #[test]
    fn progress_is_reported_once_per_step() {
        let chunk = 64 * 1024;
        let mut steps = vec![Step::Send(Piece::Head(None))];
        for _ in 0..(3 * PROGRESS_STEP as usize / chunk) {
            steps.push(bytes(chunk));
        }
        steps.push(bytes(10));
        steps.push(Step::Send(Piece::End));
        let mut reported = Vec::new();
        receive(
            &feed(steps),
            &mut std::io::sink(),
            u64::MAX,
            &calm(),
            QUICK,
            |received, total| reported.push((received, total)),
        )
        .unwrap();
        assert_eq!(reported.len(), 3);
        assert!(reported.windows(2).all(|pair| pair[0].0 < pair[1].0));
    }
}
