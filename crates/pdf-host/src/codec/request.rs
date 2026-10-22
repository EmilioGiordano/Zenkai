use serde::{Deserialize, Serialize};

use super::CodecError;
use crate::frame::Frame;
use crate::wire::{DocId, Job, MAX_MESSAGE_CHARS, MAX_QUERY_CHARS, MAX_RENDER_PIXELS};

const MAX_PATH_CHARS: usize = 32_768;
const MAX_PAGES_PER_FIND: usize = 1000;

#[derive(Serialize, Deserialize)]
struct RequestHead {
    id: u64,
    doc: u64,
    job: Job,
}

pub fn encode_request(id: u64, doc: DocId, job: &Job) -> Result<Frame, CodecError> {
    let head = serde_json::to_vec(&RequestHead {
        id,
        doc: doc.0,
        job: job.clone(),
    })?;
    Ok(Frame {
        head,
        payload: Vec::new(),
    })
}

// Reads a request. A request that is well formed but asks for too much still comes back, so the
// host can refuse that one request (see `check_request`) and carry on.
pub fn decode_request(frame: &Frame) -> Result<(u64, DocId, Job), CodecError> {
    let head: RequestHead = serde_json::from_slice(&frame.head)?;
    if !frame.payload.is_empty() {
        return Err(CodecError::Rule("a request has no payload"));
    }
    Ok((head.id, DocId(head.doc), head.job))
}

// The host treats what the app sends as input to check, not to trust.
pub fn check_request(job: &Job) -> Result<(), &'static str> {
    match job {
        Job::Open { path, password } => {
            let path_ok = path
                .to_str()
                .is_some_and(|text| text.chars().count() <= MAX_PATH_CHARS);
            let password_ok = password
                .as_ref()
                .is_none_or(|text| text.chars().count() <= MAX_MESSAGE_CHARS);
            if path_ok && password_ok {
                Ok(())
            } else {
                Err("an open request is too long")
            }
        }
        Job::Render {
            pixel_width,
            pixel_height,
            ..
        } => {
            let pixels = u64::from(*pixel_width) * u64::from(*pixel_height);
            if pixels == 0 || pixels > MAX_RENDER_PIXELS {
                Err("a render asks for a bad size")
            } else {
                Ok(())
            }
        }
        Job::Find { query, count, .. } => {
            if query.chars().count() > MAX_QUERY_CHARS || *count > MAX_PAGES_PER_FIND {
                Err("a find request is too large")
            } else {
                Ok(())
            }
        }
        Job::Sizes | Job::Content { .. } | Job::Close | Job::Abort => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::wire::PageIdx;

    #[test]
    fn every_job_survives_the_request_frame() {
        let jobs = [
            Job::Open {
                path: PathBuf::from("C:\\docs\\a.pdf"),
                password: Some("secret".to_string()),
            },
            Job::Sizes,
            Job::Render {
                page: PageIdx(3),
                pixel_width: 100,
                pixel_height: 200,
                thumbnail: true,
                generation: 8,
            },
            Job::Content { page: PageIdx(1) },
            Job::Find {
                search: 4,
                query: "hello".to_string(),
                first: PageIdx(0),
                count: 24,
            },
            Job::Close,
            Job::Abort,
        ];
        for job in jobs {
            let frame = encode_request(11, DocId(3), &job).unwrap();
            assert_eq!(decode_request(&frame).unwrap(), (11, DocId(3), job));
        }
    }

    #[test]
    fn the_host_refuses_requests_that_break_the_limits() {
        let big_render = Job::Render {
            page: PageIdx(0),
            pixel_width: 100_000,
            pixel_height: 100_000,
            thumbnail: false,
            generation: 0,
        };
        let empty_render = Job::Render {
            page: PageIdx(0),
            pixel_width: 0,
            pixel_height: 5,
            thumbnail: false,
            generation: 0,
        };
        let long_query = Job::Find {
            search: 0,
            query: "x".repeat(MAX_QUERY_CHARS + 1),
            first: PageIdx(0),
            count: 1,
        };
        let long_password = Job::Open {
            path: PathBuf::from("a.pdf"),
            password: Some("p".repeat(MAX_MESSAGE_CHARS + 1)),
        };
        let long_path = Job::Open {
            path: PathBuf::from("a".repeat(MAX_PATH_CHARS + 1)),
            password: None,
        };
        for job in [
            big_render,
            empty_render,
            long_query,
            long_password,
            long_path,
        ] {
            assert!(check_request(&job).is_err(), "{job:?}");
        }
        let garbage = Frame {
            head: b"not json".to_vec(),
            payload: Vec::new(),
        };
        assert!(decode_request(&garbage).is_err());
    }
}
