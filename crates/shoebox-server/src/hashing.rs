//! BLAKE3 hashing of files. Streamed so we don't pull a 50 MB RAW into
//! memory.

use anyhow::{Context, Result};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

const BUF_SIZE: usize = 256 * 1024;

/// Hash a file with BLAKE3, returning the 32-byte digest.
///
/// # Errors
///
/// Returns an error if the file cannot be opened or if reading from it
/// fails partway through.
pub fn blake3_file(path_to_hash: &Path) -> Result<[u8; 32]> {
    let file =
        File::open(path_to_hash).with_context(|| format!("opening {}", path_to_hash.display()))?;
    let mut buffered_reader = BufReader::with_capacity(BUF_SIZE, file);
    let mut hasher = blake3::Hasher::new();
    let mut read_buffer = vec![0u8; BUF_SIZE];
    loop {
        let bytes_read = buffered_reader.read(&mut read_buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&read_buffer[..bytes_read]);
    }
    Ok(*hasher.finalize().as_bytes())
}

/// Lowercase-hex BLAKE3 of a file.
///
/// # Errors
///
/// Returns an error if the file cannot be opened or if reading from it
/// fails partway through.
pub fn blake3_hex(path_to_hash: &Path) -> Result<String> {
    Ok(hex::encode(blake3_file(path_to_hash)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn known_vector_empty_file() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("empty");
        File::create(&p).unwrap();
        // BLAKE3 of empty input
        assert_eq!(
            blake3_hex(&p).unwrap(),
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
    }

    #[test]
    fn known_vector_abc() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("abc");
        let mut f = File::create(&p).unwrap();
        f.write_all(b"abc").unwrap();
        assert_eq!(
            blake3_hex(&p).unwrap(),
            "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"
        );
    }
}
