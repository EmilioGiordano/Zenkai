use std::io::Read;
use std::time::Duration;

use super::version::{NotAReleaseList, Version, newest_stable};

// Fixed at build time: no setting, environment variable or argument can point the check
// somewhere else.
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
        .user_agent(USER_AGENT)
        .build();
    let response = ureq::Agent::new_with_config(config)
        .get(RELEASES_API)
        .header("Accept", "application/vnd.github+json")
        .call()?;
    let body = read_limited(response.into_body().into_reader(), MAX_RESPONSE_BYTES)?;
    Ok(newest_stable(&body)?)
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
    use super::{FetchError, read_limited};

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
