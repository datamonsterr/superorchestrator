//! Newline-delimited reading with a length cap, shared by server and client.

use tokio::io::{AsyncBufRead, AsyncBufReadExt};

pub(crate) enum Line {
    Eof,
    TooLong,
    Data(Vec<u8>),
}

/// Reads one `\n`-terminated line. A line longer than `cap` is consumed and discarded
/// (not buffered) and reported as `TooLong`, so the stream stays in sync.
pub(crate) async fn read_line<R: AsyncBufRead + Unpin>(
    r: &mut R,
    cap: usize,
) -> std::io::Result<Line> {
    let mut buf = Vec::new();
    let mut too_long = false;
    loop {
        let chunk = r.fill_buf().await?;
        if chunk.is_empty() {
            return Ok(if too_long {
                Line::TooLong
            } else if buf.is_empty() {
                Line::Eof
            } else {
                Line::Data(buf)
            });
        }
        let pos = chunk.iter().position(|b| *b == b'\n');
        let take = pos.unwrap_or(chunk.len());
        if !too_long {
            buf.extend_from_slice(&chunk[..take]);
            if buf.len() > cap {
                too_long = true;
                buf = Vec::new();
            }
        }
        r.consume(take + usize::from(pos.is_some()));
        if pos.is_some() {
            return Ok(if too_long {
                Line::TooLong
            } else {
                Line::Data(buf)
            });
        }
    }
}
