use thiserror::Error;

mod reply;
mod request;

pub use reply::{Expect, decode_reply, encode_reply, reply_id};
pub use request::{check_request, decode_request, encode_request};

#[derive(Debug, Error)]
pub enum CodecError {
    #[error("the frame could not be read: {0}")]
    Format(#[from] serde_json::Error),
    #[error("the frame breaks a rule: {0}")]
    Rule(&'static str),
}
