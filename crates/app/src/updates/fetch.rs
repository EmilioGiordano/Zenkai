use std::io::Read;
use std::time::Duration;

use super::version::{NotAReleaseList, Version, newest_stable};

// Fixed at build time: no setting or argument can point the check somewhere else. The proxy
// variables ureq reads (ALL_PROXY, HTTPS_PROXY) only route the same TLS connection to GitHub.
const RELEASES_API: &str =
    "https://api.github.com/repos/EmilioGiordano/Zenkai/releases?per_page=30";
const USER_AGENT: &str = "Zenkai";
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("the request failed: {0}")]
    Request(#[from] ureq::Error),
    #[error("the server answered with status {0}")]
    Status(u16),
    #[error("could not read the response: {0}")]
    Read(#[from] std::io::Error),
    #[error("the response is larger than {} bytes", MAX_RESPONSE_BYTES)]
    TooLarge,
    #[error(transparent)]
    NotAReleaseList(#[from] NotAReleaseList),
}

// Blocks on the network: call it from a background task only.
pub fn newest_release() -> Result<Option<Version>, FetchError> {
    let config = ureq::Agent::config_builder()
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_global(Some(TOTAL_TIMEOUT))
        .https_only(true)
        .max_redirects(0)
        .user_agent(USER_AGENT)
        .build();
    let response = ureq::Agent::new_with_config(config)
        .get(RELEASES_API)
        .header("Accept", "application/vnd.github+json")
        .call()?;
    require_ok(response.status().as_u16())?;
    let body = read_limited(response.into_body().into_reader(), MAX_RESPONSE_BYTES)?;
    Ok(newest_stable(&body)?)
}

// With redirects off, ureq hands a 3xx back as a response, not as an error.
fn require_ok(status: u16) -> Result<(), FetchError> {
    if status == 200 {
        Ok(())
    } else {
        Err(FetchError::Status(status))
    }
}

// A response past the limit is a failed check, never a partial read: a big release in the
// list must not make the app look at half of it.
fn read_limited(reader: impl Read, limit: usize) -> Result<Vec<u8>, FetchError> {
    let mut body = Vec::new();
    reader.take(limit as u64 + 1).read_to_end(&mut body)?;
    if body.len() > limit {
        return Err(FetchError::TooLarge);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::{FetchError, read_limited, require_ok};

    #[test]
    fn only_a_200_answer_is_read() {
        assert!(require_ok(200).is_ok());
        for status in [201, 204, 301, 302, 304, 307, 308, 403, 404, 500] {
            assert!(
                matches!(require_ok(status), Err(FetchError::Status(found)) if found == status),
                "{status}"
            );
        }
    }

    #[test]
    fn a_response_within_the_limit_is_read_whole() {
        assert_eq!(read_limited(&b"[1,2]"[..], 5).unwrap(), b"[1,2]");
        assert_eq!(read_limited(&b""[..], 5).unwrap(), b"");
    }

    #[test]
    fn a_response_one_byte_over_the_limit_fails() {
        assert!(matches!(
            read_limited(&b"[1,2,"[..], 4),
            Err(FetchError::TooLarge)
        ));
    }

    #[test]
    fn an_endless_response_stops_at_the_limit() {
        assert!(matches!(
            read_limited(std::io::repeat(b'x'), 1024),
            Err(FetchError::TooLarge)
        ));
    }
}
