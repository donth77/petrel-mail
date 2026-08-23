//! Runs the store's migrations and repairs against a database, and reports
//! what changed — shapes and counts only, never a subject or an address.
//!
//! Pointed at a *copy* of a real mailbox. Migrations and re-extraction are the
//! things a fresh in-memory test cannot honestly cover: a fresh store is built
//! by the schema, not by the upgrade path, so a broken repair passes every test
//! and fails on the first real database.

use petrel_engine::blob::BlobStore;
use petrel_engine::store::Store;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: mig_check <db> <blobs>");
    let blob_dir = args.next().expect("usage: mig_check <db> <blobs>");

    let mut store = Store::open(std::path::Path::new(&path)).expect("open (runs migrations)");
    let blobs = BlobStore::open(std::path::Path::new(&blob_dir)).expect("open blobs");

    println!("opened and migrated");
    let repaired = store.reindex_bodies(&blobs).expect("re-extract");
    println!("  re-extracted:  {repaired} message(s)");
}
