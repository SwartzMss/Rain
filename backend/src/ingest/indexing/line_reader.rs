use std::io;

use tokio::io::{AsyncBufRead, AsyncBufReadExt};

const TRUNCATED_LINE_MARKER: &str = " ... [line truncated]";

pub(crate) fn clean_log_line(line: &[u8], truncated: bool) -> String {
    // SQLite text values should not contain embedded null bytes in this app.
    decode_log_line(line, truncated).trim().replace('\0', "")
}

pub async fn read_line_bytes_limited<R>(
    reader: &mut R,
    output: &mut Vec<u8>,
    max_bytes: usize,
) -> Result<Option<(usize, bool)>, io::Error>
where
    R: AsyncBufRead + Unpin,
{
    output.clear();
    let mut total_read = 0usize;
    let mut truncated = false;

    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return if total_read == 0 {
                Ok(None)
            } else {
                Ok(Some((total_read, truncated)))
            };
        }

        let newline_pos = available.iter().position(|byte| *byte == b'\n');
        let consume_len = newline_pos.map_or(available.len(), |pos| pos + 1);
        let chunk = &available[..consume_len];
        total_read = total_read.saturating_add(chunk.len());

        let remaining = max_bytes.saturating_sub(output.len());
        if remaining > 0 {
            let keep_len = remaining.min(chunk.len());
            output.extend_from_slice(&chunk[..keep_len]);
            if keep_len < chunk.len() {
                truncated = true;
            }
        } else {
            truncated = true;
        }

        reader.consume(consume_len);

        if newline_pos.is_some() {
            return Ok(Some((total_read, truncated)));
        }
    }
}

pub fn decode_log_line(line: &[u8], truncated: bool) -> String {
    let mut decoded = String::from_utf8_lossy(line)
        .trim_end_matches(['\r', '\n'])
        .to_string();
    if truncated {
        decoded.push_str(TRUNCATED_LINE_MARKER);
    }
    decoded
}
