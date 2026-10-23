use std::io::{self, Read, Write};

use thiserror::Error;

use crate::wire::MAX_RENDER_PIXELS;

pub const MAX_HEADER_BYTES: usize = 64 * 1024;
// A full-size bitmap in four bytes a pixel, plus room for the JSON bodies, which are smaller.
pub const MAX_PAYLOAD_BYTES: usize = MAX_RENDER_PIXELS as usize * 4 + 1024 * 1024;

const MAGIC: [u8; 4] = *b"ZPH1";
const PREFIX_BYTES: usize = 12;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub head: Vec<u8>,
    pub payload: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("the pipe failed: {0}")]
    Io(#[from] io::Error),
    #[error("the frame does not start like one")]
    BadMagic,
    #[error("a frame header of {0} bytes is over the limit")]
    HeaderTooLarge(usize),
    #[error("a frame payload of {0} bytes is over the limit")]
    PayloadTooLarge(usize),
    #[error("the frame ends before its announced length")]
    Truncated,
}

pub fn write_frame(writer: &mut impl Write, frame: &Frame) -> Result<(), FrameError> {
    if frame.head.len() > MAX_HEADER_BYTES {
        return Err(FrameError::HeaderTooLarge(frame.head.len()));
    }
    if frame.payload.len() > MAX_PAYLOAD_BYTES {
        return Err(FrameError::PayloadTooLarge(frame.payload.len()));
    }
    let mut prefix = [0u8; PREFIX_BYTES];
    prefix[..4].copy_from_slice(&MAGIC);
    prefix[4..8].copy_from_slice(&(frame.head.len() as u32).to_le_bytes());
    prefix[8..].copy_from_slice(&(frame.payload.len() as u32).to_le_bytes());
    writer.write_all(&prefix)?;
    writer.write_all(&frame.head)?;
    writer.write_all(&frame.payload)?;
    writer.flush()?;
    Ok(())
}

// None when the other end closed the pipe between two frames.
pub fn read_frame(reader: &mut impl Read) -> Result<Option<Frame>, FrameError> {
    let mut prefix = [0u8; PREFIX_BYTES];
    let mut filled = 0;
    while filled < PREFIX_BYTES {
        match reader.read(&mut prefix[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => return Err(FrameError::Truncated),
            Ok(count) => filled += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
    }
    if prefix[..4] != MAGIC {
        return Err(FrameError::BadMagic);
    }
    let head_len = u32::from_le_bytes([prefix[4], prefix[5], prefix[6], prefix[7]]) as usize;
    let payload_len = u32::from_le_bytes([prefix[8], prefix[9], prefix[10], prefix[11]]) as usize;
    if head_len > MAX_HEADER_BYTES {
        return Err(FrameError::HeaderTooLarge(head_len));
    }
    if payload_len > MAX_PAYLOAD_BYTES {
        return Err(FrameError::PayloadTooLarge(payload_len));
    }
    Ok(Some(Frame {
        head: read_exact_growing(reader, head_len)?,
        payload: read_exact_growing(reader, payload_len)?,
    }))
}

// Grows with what arrives instead of reserving the announced size up front.
fn read_exact_growing(reader: &mut impl Read, length: usize) -> Result<Vec<u8>, FrameError> {
    let mut bytes = Vec::new();
    reader.take(length as u64).read_to_end(&mut bytes)?;
    if bytes.len() == length {
        Ok(bytes)
    } else {
        Err(FrameError::Truncated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(head: &[u8], payload: &[u8]) -> Frame {
        Frame {
            head: head.to_vec(),
            payload: payload.to_vec(),
        }
    }

    fn encoded(frame: &Frame) -> Vec<u8> {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, frame).unwrap();
        bytes
    }

    #[test]
    fn a_frame_round_trips_and_several_follow_each_other() {
        let first = frame(b"{\"a\":1}", &[1, 2, 3, 4]);
        let second = frame(b"", b"");
        let mut bytes = encoded(&first);
        bytes.extend(encoded(&second));
        let mut reader = bytes.as_slice();
        assert_eq!(read_frame(&mut reader).unwrap(), Some(first));
        assert_eq!(read_frame(&mut reader).unwrap(), Some(second));
        assert_eq!(read_frame(&mut reader).unwrap(), None);
    }

    #[test]
    fn an_oversize_header_is_refused_on_both_ends() {
        let big = frame(&vec![b' '; MAX_HEADER_BYTES + 1], b"");
        assert!(matches!(
            write_frame(&mut Vec::new(), &big),
            Err(FrameError::HeaderTooLarge(_))
        ));
        let mut bytes = encoded(&frame(b"x", b""));
        bytes[4..8].copy_from_slice(&((MAX_HEADER_BYTES + 1) as u32).to_le_bytes());
        assert!(matches!(
            read_frame(&mut bytes.as_slice()),
            Err(FrameError::HeaderTooLarge(_))
        ));
    }

    #[test]
    fn an_oversize_payload_is_refused_before_it_is_read() {
        let big = frame(b"x", &vec![0; MAX_PAYLOAD_BYTES + 1]);
        assert!(matches!(
            write_frame(&mut Vec::new(), &big),
            Err(FrameError::PayloadTooLarge(_))
        ));
        let mut bytes = encoded(&frame(b"x", b""));
        bytes[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            read_frame(&mut bytes.as_slice()),
            Err(FrameError::PayloadTooLarge(_))
        ));
    }

    #[test]
    fn a_frame_cut_short_is_truncated_not_closed() {
        let bytes = encoded(&frame(b"head", b"payload"));
        for cut in [1, 11, 13, bytes.len() - 1] {
            assert!(
                matches!(read_frame(&mut &bytes[..cut]), Err(FrameError::Truncated)),
                "cut at {cut}"
            );
        }
    }

    #[test]
    fn garbage_is_not_a_frame() {
        let garbage = b"this is certainly not a frame at all".to_vec();
        assert!(matches!(
            read_frame(&mut garbage.as_slice()),
            Err(FrameError::BadMagic)
        ));
    }

    #[test]
    fn an_empty_pipe_is_a_clean_end() {
        assert_eq!(read_frame(&mut [].as_slice()).unwrap(), None);
    }
}
