use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

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
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
// A stalled transfer must end on its own: cancelling is only seen between two reads.
const BODY_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const READ_CHUNK: usize = 64 * 1024;
pub const PROGRESS_STEP: u64 = 1024 * 1024;

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

pub fn agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_recv_response(Some(RESPONSE_TIMEOUT))
        .timeout_recv_body(Some(BODY_TIMEOUT))
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

// Blocks on the network: call it from a background thread only.
pub fn download(
    agent: &ureq::Agent,
    url: Url,
    sink: &mut impl Write,
    limit: u64,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<u64, DownloadError> {
    let response = follow(url, |url| request(agent, url))?;
    let total = response.body().content_length();
    if total.is_some_and(|total| total > limit) {
        return Err(DownloadError::TooLarge(limit));
    }
    let mut body = response.into_body().into_reader();
    copy_limited(&mut body, sink, limit, cancel, |received| {
        progress(received, total)
    })
}

// The limit counts what arrives, whatever the headers announced.
pub fn copy_limited(
    source: &mut impl Read,
    sink: &mut impl Write,
    limit: u64,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> Result<u64, DownloadError> {
    let mut buffer = vec![0; READ_CHUNK];
    let mut received = 0u64;
    let mut reported = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(DownloadError::Cancelled);
        }
        let read = match source.read(&mut buffer) {
            Ok(0) => return Ok(received),
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(DownloadError::Transfer(error)),
        };
        received += read as u64;
        if received > limit {
            return Err(DownloadError::TooLarge(limit));
        }
        sink.write_all(&buffer[..read])
            .map_err(DownloadError::Write)?;
        if received - reported >= PROGRESS_STEP {
            reported = received;
            progress(received);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::atomic::AtomicBool;

    use url::Url;

    use super::super::version::Version;
    use super::{
        Answer, DownloadError, MAX_REDIRECTS, PROGRESS_STEP, allowed, asset_url, copy_limited,
        follow,
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

    #[test]
    fn a_body_within_the_limit_is_copied_whole() {
        let mut sink = Vec::new();
        let received = copy_limited(
            &mut &b"12345"[..],
            &mut sink,
            5,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        assert_eq!(received, 5);
        assert_eq!(sink, b"12345");
    }

    #[test]
    fn a_body_past_the_limit_stops_there() {
        let mut sink = Vec::new();
        let result = copy_limited(
            &mut std::io::repeat(b'x'),
            &mut sink,
            1000,
            &AtomicBool::new(false),
            |_| {},
        );
        assert!(matches!(result, Err(DownloadError::TooLarge(1000))));
        assert!(sink.len() <= 1000);
    }

    #[test]
    fn a_cancelled_download_stops_before_the_next_read() {
        let cancel = AtomicBool::new(true);
        let mut sink = Vec::new();
        let result = copy_limited(
            &mut std::io::repeat(b'x'),
            &mut sink,
            u64::MAX,
            &cancel,
            |_| {},
        );
        assert!(matches!(result, Err(DownloadError::Cancelled)));
        assert!(sink.is_empty());
    }

    #[test]
    fn progress_is_reported_once_per_step() {
        let size = PROGRESS_STEP * 3 + 10;
        let mut reported = Vec::new();
        let mut source = std::io::Read::take(std::io::repeat(0), size);
        copy_limited(
            &mut source,
            &mut std::io::sink(),
            size,
            &AtomicBool::new(false),
            |received| reported.push(received),
        )
        .unwrap();
        assert_eq!(reported.len(), 3);
        assert!(reported.windows(2).all(|pair| pair[0] < pair[1]));
    }
}
