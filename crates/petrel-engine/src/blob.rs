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
    /// The stored bytes do not match the hash they are filed under. The blob is
    /// unusable; the engine's response is to refetch from the server.
    #[error("corrupt blob: {0}")]
    Corrupt(String),
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

    /// Where a blob lives, sharded two levels deep so no directory holds a
    /// hundred thousand files.
    ///
    /// Returns None for anything that is not a hash. Slicing a short string
    /// here panicked, which turned "this message has no stored body" — an
    /// ordinary thing to find in a database — into a crash inside whatever was
    /// reading it.
    fn path_for(&self, hash: &str) -> Option<PathBuf> {
        if hash.len() < 4 || !hash.is_char_boundary(2) || !hash.is_char_boundary(4) {
            return None;
        }
        Some(
            self.root
                .join(&hash[0..2])
                .join(&hash[2..4])
                .join(format!("{hash}.zst")),
        )
    }

    /// Writes bytes, returns (hex hash, compressed size). Idempotent: an
    /// existing blob with the same hash is left untouched (dedupe).
    pub fn write(&self, bytes: &[u8]) -> Result<(String, u64)> {
        let hash = blake3::hash(bytes).to_hex().to_string();
        let final_path = self
            .path_for(&hash)
            .expect("blake3 hex is always long enough to shard");
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

    /// Reads and **verifies** the blob against its own name. Blobs are
    /// content-addressed, so the filename is a checksum we get for free —
    /// spending microseconds to check it turns silent corruption (bit rot, a
    /// truncated restore, an antivirus rewrite) into a clean error the engine
    /// can heal from by refetching, instead of a wrong message on screen.
    pub fn read(&self, hash: &str) -> Result<Vec<u8>> {
        let path = self
            .path_for(hash)
            .ok_or_else(|| BlobError::Corrupt(format!("{hash:?} is not a blob hash")))?;
        let compressed = fs::read(path)?;
        let bytes = zstd::decode_all(compressed.as_slice())
            .map_err(|e| BlobError::Corrupt(format!("{hash}: decompression failed: {e}")))?;
        let actual = blake3::hash(&bytes).to_hex().to_string();
        if actual != hash {
            return Err(BlobError::Corrupt(format!(
                "{hash}: content hash mismatch (found {actual})"
            )));
        }
        Ok(bytes)
    }

    /// True when the blob exists and passes verification.
    pub fn is_intact(&self, hash: &str) -> bool {
        self.read(hash).is_ok()
    }

    /// Deletes one blob's file. Called only by GC, and only for a hash the
    /// store has already proven unreachable.
    pub fn remove(&self, hash: &str) -> Result<()> {
        // Nothing to remove for a hash that could never have been stored.
        let Some(path) = self.path_for(hash) else {
            return Ok(());
        };
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            // Already gone is success: GC is idempotent by design.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(BlobError::Io(e)),
        }
    }

    /// Removes orphaned temp files left by interrupted writes. Safe to run at
    /// any time: a `.part` file is never referenced by the store.
    pub fn sweep_tmp(&self) -> Result<usize> {
        let mut removed = 0;
        for entry in fs::read_dir(self.root.join("tmp"))? {
            let entry = entry?;
            fs::remove_file(entry.path())?;
            removed += 1;
        }
        Ok(removed)
    }

    /// Number of `.part` files awaiting a sweep (diagnostics/tests).
    /// Bytes on disk across every stored blob.
    ///
    /// Walked rather than tracked in a counter: a counter drifts the first time
    /// a crash lands between writing a blob and recording it, and the number
    /// people check disk usage for is the one the filesystem actually reports.
    pub fn pending_temp_files(&self) -> Result<usize> {
        Ok(fs::read_dir(self.root.join("tmp"))?.count())
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
