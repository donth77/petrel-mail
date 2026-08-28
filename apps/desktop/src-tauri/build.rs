fn main() {
    // The running version's release notes are compiled in from the
    // environment (see commands/updates.rs). Without this, changing the
    // variable would not rebuild and the pane would keep showing the notes
    // from whenever the crate last happened to compile.
    println!("cargo:rerun-if-env-changed=PETREL_RELEASE_NOTES");
    tauri_build::build()
}
