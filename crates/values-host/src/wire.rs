use serde::{Deserialize, Serialize};
use thiserror::Error;
use zenkai_formats::ValuesError;
use zenkai_pdf_host::frame::Frame;

// Rows are sent in frames of about this many bytes, so no frame nears the frame size limit.
pub const TARGET_ROWS_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailureKind {
    Unreadable,
    TooLarge,
    Panicked,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub kind: FailureKind,
    pub detail: String,
}

impl From<&ValuesError> for Failure {
    fn from(error: &ValuesError) -> Failure {
        let kind = match error {
            ValuesError::Unreadable(_) => FailureKind::Unreadable,
            ValuesError::TooLarge => FailureKind::TooLarge,
            ValuesError::Panicked => FailureKind::Panicked,
        };
        Failure {
            kind,
            detail: error.to_string(),
        }
    }
}

// What the host sends, in this order: a Sheet, then its Blocks, for every sheet, then Done or
// Failed. The rows of a Block travel in the payload as a JSON array of arrays of strings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Message {
    Sheet { name: String },
    Block { first_row: u32, first_col: u32 },
    Done,
    Failed(Failure),
}

#[derive(Debug, Error)]
pub enum WireError {
    #[error("the frame could not be read: {0}")]
    Format(#[from] serde_json::Error),
    #[error("the frame breaks a rule: {0}")]
    Rule(&'static str),
}

pub fn encode(message: &Message, rows: &[Vec<String>]) -> Result<Frame, WireError> {
    let payload = match message {
        Message::Block { .. } => serde_json::to_vec(rows)?,
        _ if rows.is_empty() => Vec::new(),
        _ => return Err(WireError::Rule("only a block carries rows")),
    };
    Ok(Frame {
        head: serde_json::to_vec(message)?,
        payload,
    })
}

pub fn decode(frame: &Frame) -> Result<(Message, Vec<Vec<String>>), WireError> {
    let message: Message = serde_json::from_slice(&frame.head)?;
    match message {
        Message::Block { .. } => {
            let rows = serde_json::from_slice(&frame.payload)?;
            Ok((message, rows))
        }
        _ if frame.payload.is_empty() => Ok((message, Vec::new())),
        _ => Err(WireError::Rule("only a block carries rows")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_message_round_trips() {
        let rows = vec![
            vec!["a".to_string(), String::new()],
            vec!["é\"".to_string()],
        ];
        for (message, rows) in [
            (Message::Sheet { name: "S".into() }, Vec::new()),
            (
                Message::Block {
                    first_row: 3,
                    first_col: 4,
                },
                rows,
            ),
            (Message::Done, Vec::new()),
            (
                Message::Failed(Failure {
                    kind: FailureKind::TooLarge,
                    detail: "x".into(),
                }),
                Vec::new(),
            ),
        ] {
            let frame = encode(&message, &rows).unwrap();
            assert_eq!(decode(&frame).unwrap(), (message, rows));
        }
    }

    #[test]
    fn rows_on_a_message_that_is_not_a_block_are_refused() {
        let rows = vec![vec!["a".to_string()]];
        assert!(encode(&Message::Done, &rows).is_err());
        let mut frame = encode(&Message::Done, &[]).unwrap();
        frame.payload = b"[]".to_vec();
        assert!(decode(&frame).is_err());
    }

    #[test]
    fn a_block_whose_payload_is_not_rows_is_refused() {
        let mut frame = encode(
            &Message::Block {
                first_row: 0,
                first_col: 0,
            },
            &[],
        )
        .unwrap();
        frame.payload = b"{\"a\":1}".to_vec();
        assert!(decode(&frame).is_err());
    }
}
