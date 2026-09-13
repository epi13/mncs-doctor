//! Shared integration-test helpers: fixture staging and CLI invocation.
//!
//! Every test copies fixtures into a unique temp dir so the committed
//! fixtures are never mutated.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Stage `fixtures/<path>` into a fresh temp dir; returns the staging root.
/// The fixture tree is copied verbatim (the top-level copied dir *is* the
/// workspace root when fixtures contain a marker, e.g. `repos/healthy`).
pub fn stage(fixture_rel: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dest = std::env::temp_dir().join(format!("mncs-doctor-it-{}-{}", std::process::id(), id));
    let _ = fs::remove_dir_all(&dest);
    fs::create_dir_all(&dest).unwrap();
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(fixture_rel);
    copy_dir(&src, &dest);
    dest
}

fn copy_dir(src: &Path, dest: &Path) {
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dest.join(entry.file_name());
        let ft = entry.file_type().unwrap();
        if ft.is_dir() {
            fs::create_dir_all(&to).unwrap();
            copy_dir(&from, &to);
        } else {
            fs::copy(&from, &to).unwrap();
        }
    }
}

/// Run the built `mncs-doctor` binary with `args` in `cwd`.
pub fn run(cwd: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mncs-doctor"))
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run mncs-doctor binary")
}

/// Parse stdout as JSON.
pub fn stdout_json(output: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).expect("stdout must be JSON with --json")
}

/// Read a staged file as a string.
#[allow(dead_code)]
pub fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join(rel)).unwrap()
}

/// Read a staged file as raw bytes.
#[allow(dead_code)]
pub fn read_bytes(root: &Path, rel: &str) -> Vec<u8> {
    fs::read(root.join(rel)).unwrap()
}
