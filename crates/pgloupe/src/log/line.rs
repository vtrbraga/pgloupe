//! Reading lines of untrusted input without letting one line use unbounded memory.

use std::io::{self, BufRead};

/// The longest line the text decoders keep. A longer one is skipped whole.
pub const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

/// How a read ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Line {
    /// Nothing left to read.
    End,
    /// A line is in the buffer, without its `\n`.
    Complete,
    /// The line was longer than the limit; the buffer holds its start and the rest was dropped.
    TooLong,
}

/// Reads one line into `buf` (cleared first), keeping at most `max` bytes of it.
///
/// # Errors
///
/// Fails when the underlying reader does.
pub fn read_line_capped(
    reader: &mut impl BufRead,
    buf: &mut Vec<u8>,
    max: usize,
) -> io::Result<Line> {
    buf.clear();
    let mut too_long = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(if too_long {
                Line::TooLong
            } else if buf.is_empty() {
                Line::End
            } else {
                Line::Complete
            });
        }
        let end = available.iter().position(|b| *b == b'\n');
        let take = end.map_or(available.len(), |at| at + 1);
        let room = max.saturating_sub(buf.len());
        if take - usize::from(end.is_some()) > room {
            too_long = true;
        }
        let keep = (take - usize::from(end.is_some())).min(room);
        buf.extend_from_slice(&available[..keep]);
        reader.consume(take);
        if end.is_some() {
            return Ok(if too_long {
                Line::TooLong
            } else {
                Line::Complete
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_all(text: &[u8], max: usize) -> Vec<(Line, Vec<u8>)> {
        let mut reader = text;
        let mut out = Vec::new();
        let mut buf = Vec::new();
        loop {
            let kind = read_line_capped(&mut reader, &mut buf, max).unwrap();
            if kind == Line::End {
                return out;
            }
            out.push((kind, buf.clone()));
        }
    }

    #[test]
    fn lines_come_back_without_their_newline_and_the_last_one_may_lack_it() {
        let lines = read_all(b"one\ntwo\r\nthree", 100);
        assert_eq!(
            lines,
            [
                (Line::Complete, b"one".to_vec()),
                (Line::Complete, b"two\r".to_vec()),
                (Line::Complete, b"three".to_vec()),
            ]
        );
    }

    #[test]
    fn a_line_over_the_limit_is_cut_and_the_next_one_still_reads() {
        let lines = read_all(b"abcdefghij\nok\n", 4);
        assert_eq!(lines[0], (Line::TooLong, b"abcd".to_vec()));
        assert_eq!(lines[1], (Line::Complete, b"ok".to_vec()));
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn a_line_exactly_at_the_limit_is_kept() {
        let lines = read_all(b"abcd\nabcde", 4);
        assert_eq!(lines[0], (Line::Complete, b"abcd".to_vec()));
        assert_eq!(lines[1].0, Line::TooLong);
    }

    #[test]
    fn a_long_line_is_cut_across_buffer_refills() {
        // A reader that hands out two bytes at a time.
        let mut reader = io::BufReader::with_capacity(2, &b"0123456789\nx\n"[..]);
        let mut buf = Vec::new();
        assert_eq!(
            read_line_capped(&mut reader, &mut buf, 5).unwrap(),
            Line::TooLong
        );
        assert_eq!(buf, b"01234");
        assert_eq!(
            read_line_capped(&mut reader, &mut buf, 5).unwrap(),
            Line::Complete
        );
        assert_eq!(buf, b"x");
    }
}
