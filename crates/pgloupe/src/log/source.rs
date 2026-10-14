//! Opening log files (plain or gzip) and recognizing their format.

use std::{
    fs::File,
    io::{self, BufRead, BufReader, Read},
    path::Path,
};

use flate2::read::MultiGzDecoder;

use super::{Format, time::looks_like_log_time};

/// Opens a log file; gzip is detected by its magic bytes, not by the file name.
///
/// # Errors
///
/// Fails if the file cannot be opened or read.
pub fn open(path: &Path) -> io::Result<Box<dyn BufRead>> {
    open_reader(File::open(path)?)
}

/// Wraps any reader the same way [`open`] wraps a file.
///
/// # Errors
///
/// Fails if the first bytes cannot be read.
pub fn open_reader(inner: impl Read + 'static) -> io::Result<Box<dyn BufRead>> {
    let mut buffered = BufReader::new(inner);
    if buffered.fill_buf()?.starts_with(&[0x1f, 0x8b]) {
        Ok(Box::new(BufReader::new(MultiGzDecoder::new(buffered))))
    } else {
        Ok(Box::new(buffered))
    }
}

/// Guesses the format from the first bytes of a log.
///
/// JSON starts with `{`; csvlog starts with a `log_time` field followed by a comma; anything
/// else is treated as stderr, which also needs the prefix to be decoded.
#[must_use]
pub fn sniff(peek: &[u8]) -> Format {
    let text = String::from_utf8_lossy(peek);
    let text = text.trim_start_matches('\u{feff}');
    let first = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    if first.trim_start().starts_with('{') {
        return Format::Json;
    }
    match first.split_once(',') {
        Some((head, _)) if looks_like_log_time(head) => Format::Csv,
        _ => Format::Stderr,
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use flate2::{Compression, write::GzEncoder};

    use super::*;

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn read_all(mut reader: Box<dyn BufRead>) -> io::Result<String> {
        let mut out = String::new();
        reader.read_to_string(&mut out)?;
        Ok(out)
    }

    #[test]
    fn plain_input_passes_through() {
        let reader = open_reader(Cursor::new(b"hello\nworld\n".to_vec())).unwrap();
        assert_eq!(read_all(reader).unwrap(), "hello\nworld\n");
    }

    #[test]
    fn empty_input_is_empty() {
        let reader = open_reader(Cursor::new(Vec::new())).unwrap();
        assert_eq!(read_all(reader).unwrap(), "");
    }

    #[test]
    fn gzip_is_detected_by_magic_bytes_not_by_name() {
        let reader = open_reader(Cursor::new(gzip(b"hello\n"))).unwrap();
        assert_eq!(read_all(reader).unwrap(), "hello\n");
    }

    #[test]
    fn multi_member_gzip_is_read_to_the_end() {
        let both = [gzip(b"hello\n"), gzip(b"world\n")].concat();
        let reader = open_reader(Cursor::new(both)).unwrap();
        assert_eq!(read_all(reader).unwrap(), "hello\nworld\n");
    }

    #[test]
    fn truncated_gzip_keeps_earlier_records_and_reports_the_error() {
        let text = "line\n".repeat(20_000);
        let mut compressed = gzip(text.as_bytes());
        compressed.truncate(compressed.len() / 2);
        let mut reader = open_reader(Cursor::new(compressed)).unwrap();
        let mut lines = 0;
        let mut line = String::new();
        let error = loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => panic!("expected an error before the end of input"),
                Ok(_) => lines += 1,
                Err(err) => break err,
            }
        };
        assert!(lines > 0, "lines before the break must be readable");
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn sniff_recognizes_the_three_formats() {
        assert_eq!(
            sniff(b"{\"timestamp\":\"2026-10-14 10:20:09.289 UTC\"}\n"),
            Format::Json
        );
        assert_eq!(sniff(b"  \n{\"a\":1}\n"), Format::Json);
        assert_eq!(
            sniff(b"2026-10-14 10:20:09.289 UTC,,,42,,6acf5759.2a,1,,2026-10-14 10:20:09 UTC,,0,LOG,00000,\"hi\"\n"),
            Format::Csv
        );
        assert_eq!(
            sniff(b"2026-10-14 10:20:09.289 UTC [42] LOG:  starting, with a comma\n"),
            Format::Stderr
        );
        assert_eq!(sniff(b"\xEF\xBB\xBF{\"a\":1}\n"), Format::Json);
        assert_eq!(sniff(b""), Format::Stderr);
        assert_eq!(sniff(b"\x00\x01\x02 garbage"), Format::Stderr);
    }
}
