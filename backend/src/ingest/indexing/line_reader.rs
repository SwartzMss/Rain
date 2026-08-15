use std::io;

use tokio::io::{AsyncBufRead, AsyncBufReadExt};

pub const TRUNCATED_LINE_MARKER: &str = " ... [line truncated]";

#[derive(Debug, PartialEq, Eq)]
pub enum LimitedLine {
    EndOfFile,
    Line {
        bytes_read: usize,
        original_length: usize,
        truncated: bool,
    },
    ScanLimit {
        bytes_read: usize,
    },
}

pub(crate) fn clean_log_line(line: &[u8], truncated: bool) -> String {
    // SQLite text values should not contain embedded null bytes in this app.
    decode_log_line(line, truncated).trim().replace('\0', "")
}

pub async fn read_line_bytes_limited<R>(
    reader: &mut R,
    output: &mut Vec<u8>,
    max_bytes: usize,
) -> Result<Option<(usize, usize, bool)>, io::Error>
where
    R: AsyncBufRead + Unpin,
{
    match read_line_bytes_limited_with_budget(reader, output, max_bytes, usize::MAX).await? {
        LimitedLine::EndOfFile => Ok(None),
        LimitedLine::Line {
            bytes_read,
            original_length,
            truncated,
        } => Ok(Some((bytes_read, original_length, truncated))),
        LimitedLine::ScanLimit { .. } => Err(io::Error::other("line scan budget exhausted")),
    }
}

pub async fn read_line_bytes_limited_with_budget<R>(
    reader: &mut R,
    output: &mut Vec<u8>,
    max_bytes: usize,
    scan_budget: usize,
) -> Result<LimitedLine, io::Error>
where
    R: AsyncBufRead + Unpin,
{
    read_line_bytes_limited_with_budget_and_callback(reader, output, max_bytes, scan_budget, |_| {})
        .await
}

pub async fn read_line_bytes_limited_with_budget_and_callback<R, F>(
    reader: &mut R,
    output: &mut Vec<u8>,
    max_bytes: usize,
    scan_budget: usize,
    mut on_content: F,
) -> Result<LimitedLine, io::Error>
where
    R: AsyncBufRead + Unpin,
    F: FnMut(&[u8]),
{
    output.clear();
    let mut total_read = 0usize;
    let mut previous_byte = None;

    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return if total_read == 0 {
                Ok(LimitedLine::EndOfFile)
            } else {
                Ok(LimitedLine::Line {
                    bytes_read: total_read,
                    original_length: total_read,
                    truncated: total_read > max_bytes,
                })
            };
        }

        let remaining_budget = scan_budget.saturating_sub(total_read);
        if remaining_budget == 0 {
            return Ok(LimitedLine::ScanLimit {
                bytes_read: total_read,
            });
        }
        let available = &available[..available.len().min(remaining_budget)];
        let newline_pos = available.iter().position(|byte| *byte == b'\n');
        let has_carriage_return = newline_pos.map(|position| {
            if position > 0 {
                available[position - 1] == b'\r'
            } else {
                previous_byte == Some(b'\r')
            }
        });
        let consume_len = newline_pos.map_or(available.len(), |pos| pos + 1);
        let content_end = newline_pos.map_or(consume_len, |position| {
            if position > 0 && available[position - 1] == b'\r' {
                position - 1
            } else {
                position
            }
        });
        let chunk = &available[..content_end];
        on_content(chunk);
        total_read = total_read.saturating_add(chunk.len());

        let remaining = max_bytes.saturating_sub(output.len());
        if remaining > 0 {
            let keep_len = remaining.min(chunk.len());
            output.extend_from_slice(&chunk[..keep_len]);
        }

        let consumed_last_byte = available.get(consume_len.saturating_sub(1)).copied();
        let skipped_bytes = consume_len.saturating_sub(content_end);
        total_read = total_read.saturating_add(skipped_bytes);

        reader.consume(consume_len);

        if newline_pos.is_none() && total_read == scan_budget {
            if reader.fill_buf().await?.is_empty() {
                return Ok(LimitedLine::Line {
                    bytes_read: total_read,
                    original_length: total_read,
                    truncated: total_read > max_bytes,
                });
            }
            return Ok(LimitedLine::ScanLimit {
                bytes_read: total_read,
            });
        }

        if let Some(has_carriage_return) = has_carriage_return {
            let line_ending_bytes = 1 + usize::from(has_carriage_return);
            let original_length = total_read.saturating_sub(line_ending_bytes);
            if has_carriage_return
                && newline_pos == Some(0)
                && original_length < max_bytes
                && output.last() == Some(&b'\r')
            {
                output.pop();
            }
            return Ok(LimitedLine::Line {
                bytes_read: total_read,
                original_length,
                truncated: original_length > max_bytes,
            });
        }
        previous_byte = consumed_last_byte;
    }
}

pub fn decode_log_line(line: &[u8], truncated: bool) -> String {
    let line = if truncated {
        match std::str::from_utf8(line) {
            Err(error) if error.error_len().is_none() => &line[..error.valid_up_to()],
            _ => line,
        }
    } else {
        line
    };
    let mut decoded = String::from_utf8_lossy(line)
        .trim_end_matches(['\r', '\n'])
        .to_string();
    if truncated {
        decoded.push_str(TRUNCATED_LINE_MARKER);
    }
    decoded
}

#[cfg(test)]
mod tests {
    use tokio::io::BufReader;

    use super::{
        LimitedLine, decode_log_line, read_line_bytes_limited, read_line_bytes_limited_with_budget,
    };

    async fn read_with_capacity(
        content: &[u8],
        reader_capacity: usize,
        max_bytes: usize,
    ) -> ((usize, usize, bool), Vec<u8>) {
        let mut reader = BufReader::with_capacity(reader_capacity, content);
        let mut output = Vec::new();
        let result = read_line_bytes_limited(&mut reader, &mut output, max_bytes)
            .await
            .unwrap()
            .unwrap();
        (result, output)
    }

    #[tokio::test]
    async fn line_limit_excludes_lf_and_crlf_bytes() {
        let mut lf = vec![b'a'; 4096];
        lf.push(b'\n');
        let (lf_result, lf_output) = read_with_capacity(&lf, 8192, 4096).await;
        assert_eq!(lf_result, (4097, 4096, false));
        assert_eq!(lf_output.len(), 4096);
        assert_eq!(decode_log_line(&lf_output, lf_result.2), "a".repeat(4096));

        let mut crlf = vec![b'b'; 4095];
        crlf.extend_from_slice(b"\r\n");
        let (crlf_result, crlf_output) = read_with_capacity(&crlf, 8192, 4096).await;
        assert_eq!(crlf_result, (4097, 4095, false));
        assert_eq!(crlf_output.len(), 4095);
        assert_eq!(
            decode_log_line(&crlf_output, crlf_result.2),
            "b".repeat(4095)
        );

        let mut truncated = vec![b'c'; 4097];
        truncated.push(b'\n');
        let (truncated_result, truncated_output) = read_with_capacity(&truncated, 8192, 4096).await;
        assert_eq!(truncated_result, (4098, 4097, true));
        assert_eq!(truncated_output.len(), 4096);
    }

    #[tokio::test]
    async fn crlf_split_across_reader_buffers_has_exact_content_length() {
        let (result, output) = read_with_capacity(b"abc\r\n", 4, 16).await;
        assert_eq!(result, (5, 3, false));
        assert_eq!(output, b"abc");
    }

    #[tokio::test]
    async fn scan_budget_stops_a_line_without_growing_the_output() {
        let content = b"a".repeat(1024);
        let mut reader = BufReader::with_capacity(4, content.as_slice());
        let mut output = Vec::new();
        let result = read_line_bytes_limited_with_budget(&mut reader, &mut output, 16, 32)
            .await
            .unwrap();

        assert_eq!(result, LimitedLine::ScanLimit { bytes_read: 32 });
        assert_eq!(output.len(), 16);
        assert!(output.capacity() <= 16);
    }

    #[tokio::test]
    async fn exact_scan_budget_at_eof_returns_the_line() {
        let content = b"abc\n";
        let mut reader = BufReader::with_capacity(2, content.as_slice());
        let mut output = Vec::new();

        let result = read_line_bytes_limited_with_budget(&mut reader, &mut output, 16, 4)
            .await
            .unwrap();

        assert_eq!(
            result,
            LimitedLine::Line {
                bytes_read: 4,
                original_length: 3,
                truncated: false,
            }
        );
        assert_eq!(output, b"abc");
    }

    #[tokio::test]
    async fn scan_budget_plus_one_byte_returns_scan_limit() {
        let content = b"abcd\n";
        let mut reader = BufReader::with_capacity(2, content.as_slice());
        let mut output = Vec::new();

        let result = read_line_bytes_limited_with_budget(&mut reader, &mut output, 16, 4)
            .await
            .unwrap();

        assert_eq!(result, LimitedLine::ScanLimit { bytes_read: 4 });
    }
}
