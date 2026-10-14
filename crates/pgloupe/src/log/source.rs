//! Opening log files (plain, gzip, zstd, xz, lz4 or bzip2) and recognizing their format.

use std::{
    fs::File,
    io::{self, BufRead, BufReader, Read},
    path::Path,
};

use flate2::read::MultiGzDecoder;

use super::{Format, time::looks_like_log_time};

/// Opens a log file; the compression is detected by its magic bytes, not by the file name.
///
/// # Errors
///
/// Fails if the file cannot be opened or read.
pub fn open(path: &Path) -> io::Result<Box<dyn BufRead + Send>> {
    open_reader(File::open(path)?)
}

/// Wraps any reader the same way [`open`] wraps a file.
///
/// # Errors
///
/// Fails if the first bytes cannot be read.
pub fn open_reader(inner: impl Read + Send + 'static) -> io::Result<Box<dyn BufRead + Send>> {
    let mut buffered = BufReader::new(inner);
    let head = buffered.fill_buf()?;
    if head.starts_with(&[0x1f, 0x8b]) {
        Ok(Box::new(BufReader::new(MultiGzDecoder::new(buffered))))
    } else if head.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        Ok(Box::new(BufReader::new(zstd::Decoder::new(buffered)?)))
    } else if head.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0x00]) {
        Ok(Box::new(BufReader::new(
            xz2::read::XzDecoder::new_multi_decoder(buffered),
        )))
    } else if head.starts_with(&[0x04, 0x22, 0x4d, 0x18]) {
        Ok(Box::new(BufReader::new(Lz4Frames::new(buffered))))
    } else if head.starts_with(b"BZh") {
        Ok(Box::new(BufReader::new(bzip2::read::MultiBzDecoder::new(
            buffered,
        ))))
    } else {
        Ok(Box::new(buffered))
    }
}

/// The magic number every lz4 frame starts with.
const LZ4_MAGIC: [u8; 4] = [0x04, 0x22, 0x4d, 0x18];

/// Reads concatenated lz4 frames to the end; the library's decoder stops after the first one.
struct Lz4Frames<R: BufRead> {
    state: Lz4State<R>,
}

enum Lz4State<R: BufRead> {
    /// Inside a frame.
    Decoding(lz4_flex::frame::FrameDecoder<R>),
    /// Between frames, at a frame boundary.
    Between(R),
    Done,
}

impl<R: BufRead> Lz4Frames<R> {
    fn new(inner: R) -> Self {
        Lz4Frames {
            state: Lz4State::Between(inner),
        }
    }
}

impl<R: BufRead> Read for Lz4Frames<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            match std::mem::replace(&mut self.state, Lz4State::Done) {
                Lz4State::Decoding(mut decoder) => {
                    let n = decoder.read(buf)?;
                    if n > 0 {
                        self.state = Lz4State::Decoding(decoder);
                        return Ok(n);
                    }
                    self.state = Lz4State::Between(decoder.into_inner());
                }
                Lz4State::Between(mut inner) => {
                    let next = inner.fill_buf()?;
                    if next.is_empty() {
                        return Ok(0);
                    }
                    if !next.starts_with(&LZ4_MAGIC) {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "data after the end of an lz4 frame is not another frame",
                        ));
                    }
                    self.state = Lz4State::Decoding(lz4_flex::frame::FrameDecoder::new(inner));
                }
                Lz4State::Done => return Ok(0),
            }
        }
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

    const TEXT: &str = "hello\nworld\n";

    fn zstd(bytes: &[u8]) -> Vec<u8> {
        zstd::encode_all(bytes, 3).unwrap()
    }

    fn xz(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = xz2::write::XzEncoder::new(Vec::new(), 6);
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn lz4(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = lz4_flex::frame::FrameEncoder::new(Vec::new());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn bzip2(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    type Compress = fn(&[u8]) -> Vec<u8>;
    const FORMATS: [(&str, Compress); 5] = [
        ("gzip", gzip),
        ("zstd", zstd),
        ("xz", xz),
        ("lz4", lz4),
        ("bzip2", bzip2),
    ];

    /// Reads to the end; `Err` if the stream is damaged.
    fn read_text(bytes: Vec<u8>) -> io::Result<String> {
        let mut reader = open_reader(Cursor::new(bytes))?;
        let mut out = String::new();
        reader.read_to_string(&mut out)?;
        Ok(out)
    }

    #[test]
    fn every_compression_is_read_back_and_concatenations_continue() {
        for (name, compress) in FORMATS {
            assert_eq!(
                read_text(compress(TEXT.as_bytes())).unwrap(),
                TEXT,
                "{name}"
            );
            let both = [compress(b"hello\n"), compress(b"world\n")].concat();
            assert_eq!(read_text(both).unwrap(), TEXT, "{name} concatenated");
            assert_eq!(read_text(compress(b"")).unwrap(), "", "{name} empty");
        }
    }

    #[test]
    fn a_truncated_stream_is_an_error_not_a_short_log() {
        let text = "a line of log text\n".repeat(20_000);
        for (name, compress) in FORMATS {
            let mut bytes = compress(text.as_bytes());
            bytes.truncate(bytes.len() / 2);
            assert!(read_text(bytes).is_err(), "{name}");
        }
    }

    #[test]
    fn a_damaged_stream_is_an_error() {
        let text = "a line of log text\n".repeat(20_000);
        for (name, compress) in FORMATS {
            let mut bytes = compress(text.as_bytes());
            let middle = bytes.len() / 2;
            for byte in &mut bytes[middle..middle + 16] {
                *byte ^= 0xff;
            }
            assert!(
                read_text(bytes).is_err_and(|_| true),
                "{name} accepted damaged bytes"
            );
        }
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
