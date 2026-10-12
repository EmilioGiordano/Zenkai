use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, ReadBuf};

// rmcp buffers a whole line before parsing it, with no upper bound; a client sending an
// endless line would grow that buffer until Zenkai runs out of memory.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;

pub struct LineLimited<R> {
    inner: R,
    since_newline: usize,
}

impl<R> LineLimited<R> {
    pub fn new(inner: R) -> LineLimited<R> {
        LineLimited {
            inner,
            since_newline: 0,
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for LineLimited<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let polled = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = polled {
            for byte in &buf.filled()[before..] {
                if *byte == b'\n' {
                    self.since_newline = 0;
                } else {
                    self.since_newline += 1;
                }
            }
            if self.since_newline > MAX_LINE_BYTES {
                // A read that fails must not hand over bytes.
                buf.set_filled(before);
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "an MCP message is longer than Zenkai accepts",
                )));
            }
        }
        polled
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::AsyncReadExt;

    use super::*;

    fn read_all(bytes: Vec<u8>) -> io::Result<usize> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let mut out = Vec::new();
            LineLimited::new(bytes.as_slice())
                .read_to_end(&mut out)
                .await
        })
    }

    #[test]
    fn many_short_lines_pass() {
        let line = format!("{}\n", "x".repeat(1000));
        assert_eq!(
            read_all(line.repeat(3000).into_bytes()).unwrap(),
            3000 * 1001
        );
    }

    #[test]
    fn an_endless_line_is_refused() {
        let error = read_all(vec![b'x'; MAX_LINE_BYTES + 10]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
