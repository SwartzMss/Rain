use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Serialize, PartialEq)]
pub struct Fixture {
    pub bytes: u64,
    pub lines: u64,
    pub sha256: String,
}

/// Fixed-width sequence and deterministic mixed-language lines; memory is bounded by one line.
pub fn generate(path: &Path, minimum_bytes: u64, stream: usize) -> Fixture {
    let mut output = BufWriter::new(File::create(path).unwrap());
    let mut hash = Sha256::new();
    let (mut bytes, mut lines) = (0, 0);
    while bytes < minimum_bytes {
        let marker = if lines % 10007 == 0 {
            "RARE_SENTINEL requestId=550e8400-e29b-41d4-a716-446655440000 中文连续文本"
        } else {
            "routine request completed"
        };
        let line =
            format!("2026-01-01 00:00:00.000 INFO stream={stream:04} seq={lines:012} {marker}\n");
        output.write_all(line.as_bytes()).unwrap();
        hash.update(line.as_bytes());
        bytes += line.len() as u64;
        lines += 1;
    }
    output.flush().unwrap();
    Fixture {
        bytes,
        lines,
        sha256: format!("{:x}", hash.finalize()),
    }
}

pub struct TestDir(pub PathBuf);
impl TestDir {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!("rain-large-log-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Package the existing disk file using bounded streaming copies; headers are deterministic.
pub fn package(source: &Path, variant: &str) -> PathBuf {
    if variant == "plain" {
        return source.to_owned();
    }
    let path = source.with_extension(variant);
    let output = File::create(&path).unwrap();
    match variant {
        "zip" => {
            let mut writer = zip::ZipWriter::new(output);
            writer
                .start_file(
                    "fixture.log",
                    zip::write::FileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated),
                )
                .unwrap();
            std::io::copy(&mut File::open(source).unwrap(), &mut writer).unwrap();
            writer.finish().unwrap();
        }
        "targz" => {
            let gzip = flate2::write::GzEncoder::new(output, flate2::Compression::default());
            let mut writer = tar::Builder::new(gzip);
            let mut header = tar::Header::new_gnu();
            header.set_size(fs::metadata(source).unwrap().len());
            header.set_mode(0o644);
            header.set_mtime(0);
            header.set_cksum();
            writer
                .append_data(&mut header, "fixture.log", File::open(source).unwrap())
                .unwrap();
            writer.into_inner().unwrap().finish().unwrap();
        }
        _ => panic!("RAIN_BENCH_VARIANT must be plain, zip, or targz"),
    }
    path
}

pub fn file_sha256(path: &Path) -> String {
    use std::io::Read;
    let mut reader = File::open(path).unwrap();
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    format!("{:x}", hash.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixture_is_deterministic_and_exactly_described() {
        let dir = TestDir::new();
        let first = generate(&dir.0.join("first"), 4096, 0);
        let second = generate(&dir.0.join("second"), 4096, 0);
        assert_eq!(first, second);
        let content = fs::read(dir.0.join("first")).unwrap();
        assert_eq!(first.bytes, content.len() as u64);
        assert_eq!(
            first.lines,
            content.iter().filter(|&&b| b == b'\n').count() as u64
        );
        assert_eq!(first.sha256, format!("{:x}", Sha256::digest(&content)));
        assert!((4096..4300).contains(&first.bytes));
        let text = std::str::from_utf8(&content).unwrap();
        for term in [
            "INFO",
            "RARE_SENTINEL",
            "550e8400-e29b-41d4-a716-446655440000",
            "中文连续文本",
        ] {
            assert!(text.contains(term));
        }
        assert_ne!(first.sha256, generate(&dir.0.join("third"), 4096, 1).sha256);
    }
}
