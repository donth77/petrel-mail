//! Content-addressed blob store: zstd-compressed files named by BLAKE3 hash,
//! written atomically (temp file → fsync → rename). The hash is identity for
//! `raw` blobs and a cache key for `generated` ones; that distinction lives in
//! the metadata store, not here.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum BlobError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, BlobError>;

pub struct BlobStore {
    root: PathBuf,
}

impl BlobStore {
    pub fn open(root: &Path) -> Result<Self> {
        fs::create_dir_all(root.join("tmp"))?;
        Ok(BlobStore {
            root: root.to_path_buf(),
        })
    }

    fn path_for(&self, hash: &str) -> PathBuf {
        self.root
            .join(&hash[0..2])
            .join(&hash[2..4])
            .join(format!("{hash}.zst"))
    }

    /// Writes bytes, returns (hex hash, compressed size). Idempotent: an
    /// existing blob with the same hash is left untouched (dedupe).
    pub fn write(&self, bytes: &[u8]) -> Result<(String, u64)> {
        let hash = blake3::hash(bytes).to_hex().to_string();
        let final_path = self.path_for(&hash);
        if let Ok(meta) = fs::metadata(&final_path) {
            return Ok((hash, meta.len()));
        }
        fs::create_dir_all(final_path.parent().expect("blob path has parent"))?;
        let tmp = self.root.join("tmp").join(format!("{hash}.part"));
        {
            let mut f = fs::File::create(&tmp)?;
            let compressed = zstd::encode_all(bytes, 3)?;
            f.write_all(&compressed)?;
            f.sync_all()?;
        }
        fs::rename(&tmp, &final_path)?;
        let size = fs::metadata(&final_path)?.len();
        Ok((hash, size))
    }

    pub fn read(&self, hash: &str) -> Result<Vec<u8>> {
        let compressed = fs::read(self.path_for(hash))?;
        Ok(zstd::decode_all(compressed.as_slice())?)
    }

    /// Removes orphaned temp files from interrupted writes.
    pub fn sweep_tmp(&self) -> Result<usize> {
        let mut removed = 0;
        for entry in fs::read_dir(self.root.join("tmp"))? {
            let entry = entry?;
            fs::remove_file(entry.path())?;
            removed += 1;
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::BlobStore;

    #[test]
    fn write_read_roundtrip_and_dedupe() {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::open(dir.path()).unwrap();
        let data = b"From: a@example.com\r\nSubject: hi\r\n\r\nbody body body".repeat(50);
        let (h1, s1) = store.write(&data).unwrap();
        let (h2, s2) = store.write(&data).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(s1, s2);
        assert_eq!(store.read(&h1).unwrap(), data);
        assert!(
            s1 < data.len() as u64,
            "zstd should compress repetitive mail"
        );
    }
}
