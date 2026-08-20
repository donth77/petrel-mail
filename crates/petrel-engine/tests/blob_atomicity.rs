//! Spike A3 — blob-store crash atomicity.
//!
//! The durability promise ("your mail is yours") is worth nothing if a crash
//! can leave half a message on disk that later reads back as whole. This kills
//! a real child process with SIGKILL while it is writing blobs — no unwinding,
//! no destructors, no cooperation — and then asserts what a cold start finds.
//!
//! The invariant under test: **a blob is either absent or complete.** Writes go
//! to a temp file, are fsynced, and only then renamed into place, so an
//! interrupted write can leave litter in `tmp/` but never a torn blob at its
//! final address.

use std::process::{Command, Stdio};
use std::time::Duration;

use petrel_engine::blob::BlobStore;

/// Mail-shaped, quick to build (one chunk, repeated), and compressible.
fn payload(seed: u8, size: usize) -> Vec<u8> {
    let chunk = format!(
        "Subject: message {seed}\r\nFrom: sender{seed}@example.com\r\n\r\n{}\r\n",
        "quarterly report body line, moderately compressible. ".repeat(40)
    );
    let chunk = chunk.as_bytes();
    let mut v = Vec::with_capacity(size + chunk.len());
    while v.len() < size {
        v.extend_from_slice(chunk);
    }
    v.truncate(size);
    v
}

/// Child mode: write one blob, announce that real work has begun, then keep
/// writing large blobs until killed. The marker lets the parent kill during a
/// write rather than during startup — otherwise the test passes vacuously.
fn child_writer(root: &str) -> ! {
    let path = std::path::Path::new(root);
    let store = BlobStore::open(path).expect("open store");

    let (hash, _) = store.write(&payload(0, 256 * 1024)).expect("first write");
    assert!(store.is_intact(&hash));
    std::fs::write(path.join("STARTED"), hash.as_bytes()).expect("marker");

    for seed in 1u8..=255 {
        let data = payload(seed, 48 * 1024 * 1024);
        let (hash, _) = store.write(&data).expect("write blob");
        assert!(store.is_intact(&hash), "survived write must verify");
    }
    std::process::exit(0);
}

#[test]
fn blob_is_never_torn_by_a_kill() {
    // Re-entry point: the test binary re-invokes itself as the writer.
    if let Ok(root) = std::env::var("PETREL_BLOB_CHILD_ROOT") {
        child_writer(&root);
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();

    let exe = std::env::current_exe().expect("test exe path");
    let mut child = Command::new(exe)
        .arg("blob_is_never_torn_by_a_kill")
        .arg("--exact")
        .arg("--nocapture")
        .env("PETREL_BLOB_CHILD_ROOT", &root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn writer child");

    // Wait until the child is demonstrably past startup and into a large write,
    // so the kill lands mid-flight. Without this the test can "pass" having
    // interrupted nothing at all.
    let marker = root.join("STARTED");
    let mut started = false;
    for _ in 0..200 {
        if marker.exists() {
            started = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        started,
        "child never began writing — test would prove nothing"
    );
    std::thread::sleep(Duration::from_millis(150));
    child.kill().expect("SIGKILL the writer");
    let status = child.wait().expect("reap child");
    assert!(
        !status.success(),
        "child must have been killed, not finished"
    );

    // Cold start over whatever the crash left behind.
    let store = BlobStore::open(&root).expect("reopen store");

    let mut finals = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read dir") {
            let entry = entry.expect("entry");
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "tmp") {
                    continue; // litter lives here by design
                }
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "zst") {
                finals.push(path);
            }
        }
    }

    let pending_before = store.pending_temp_files().expect("count temp");
    println!(
        "after kill: {} finalized blob(s), {} temp file(s) pending",
        finals.len(),
        pending_before
    );

    // Guard against a vacuous pass: the crash must have interrupted real work.
    assert!(
        !finals.is_empty(),
        "expected at least the completed blob written before the marker"
    );

    // THE invariant: everything that reached its final address is complete and
    // verifies against its own hash. A torn write would fail here.
    for path in &finals {
        let hash = path
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("hash from filename")
            .to_string();
        let bytes = store
            .read(&hash)
            .unwrap_or_else(|e| panic!("torn blob at {}: {e}", path.display()));
        assert!(!bytes.is_empty());
    }

    // Interrupted writes may leave temp litter — which is harmless, unreferenced,
    // and sweepable. That is the price of atomicity, and it must be reclaimable.
    let swept = store.sweep_tmp().expect("sweep");
    assert_eq!(swept, pending_before, "sweep must reclaim every orphan");
    assert_eq!(store.pending_temp_files().expect("recount"), 0);

    // And the store keeps working after a crash: rewriting the interrupted
    // payload succeeds and round-trips.
    let data = payload(7, 512 * 1024);
    let (hash, _) = store.write(&data).expect("write after crash");
    assert_eq!(store.read(&hash).expect("read back"), data);
}

/// The kill above proves no torn blob survives, but whether it lands *during*
/// the file write is a race. This covers the other half of the contract
/// deterministically: temp litter is never visible as a blob, and is fully
/// reclaimable.
#[test]
fn interrupted_write_litter_is_invisible_and_reclaimable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = BlobStore::open(dir.path()).expect("open");

    let data = payload(5, 128 * 1024);
    let hash = blake3::hash(&data).to_hex().to_string();

    // Exactly what a crash mid-write leaves: a partial temp file, no final blob.
    let partial = zstd::encode_all(&data[..data.len() / 3], 3).expect("compress");
    std::fs::write(dir.path().join("tmp").join(format!("{hash}.part")), partial).expect("litter");

    // The store must not see it: an interrupted write never becomes a message.
    assert!(
        !store.is_intact(&hash),
        "a partial write must not be readable as a blob"
    );
    assert_eq!(store.pending_temp_files().expect("count"), 1);

    assert_eq!(store.sweep_tmp().expect("sweep"), 1);
    assert_eq!(store.pending_temp_files().expect("recount"), 0);

    // Re-writing after the interruption produces a complete, verifiable blob.
    let (written, _) = store.write(&data).expect("rewrite");
    assert_eq!(written, hash);
    assert_eq!(store.read(&hash).expect("read"), data);
}

#[test]
fn corruption_is_detected_not_served() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = BlobStore::open(dir.path()).expect("open");
    let data = payload(3, 64 * 1024);
    let (hash, _) = store.write(&data).expect("write");
    assert_eq!(store.read(&hash).expect("clean read"), data);

    // Simulate bit rot / a bad restore by rewriting the file with another
    // message's (validly compressed) bytes filed under the original name.
    let other = zstd::encode_all(payload(9, 64 * 1024).as_slice(), 3).expect("compress");
    let path = dir
        .path()
        .join(&hash[0..2])
        .join(&hash[2..4])
        .join(format!("{hash}.zst"));
    std::fs::write(&path, other).expect("corrupt the blob");

    // It decompresses fine — only the content hash catches it. Without
    // verification the engine would serve the wrong message as if it were real.
    let err = store.read(&hash).expect_err("must not serve wrong content");
    println!("corruption detected: {err}");
    assert!(!store.is_intact(&hash));
}
