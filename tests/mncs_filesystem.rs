//! Doctor-specific Profile 0.16 filesystem excursion.
//!
//! This is intentionally an isolated scratch-root test. It exercises the
//! actual embedded research backend rather than only checking that the
//! source profile registry lists the filesystem intrinsics.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use mncs_embed::{Artifact, CallOptions, Grant, Session};
use serde_json::{json, Value};

const SOURCE: &str = r#"mncs 0.16;
module doctor.fs_probe;

fn stage(name: [byte; up_to 64], content: [byte; up_to 64]) -> (result: u64)
    capability fs_root
    effect fs_write authorized_by fs_root
{
    return fs_create_file(name, content);
}

fn grow(entry: u64, chunk: [byte; up_to 64]) -> (result: u64)
    capability fs_root
    effect fs_write authorized_by fs_root
{
    return fs_append_bytes_at(entry, chunk);
}

fn patch(entry: u64, offset: u64, chunk: [byte; up_to 64]) -> (result: u64)
    capability fs_root
    effect fs_write authorized_by fs_root
{
    return fs_write_bytes_at(entry, offset, chunk);
}

fn make_dir(name: [byte; up_to 64]) -> (result: u64)
    capability fs_root
    effect fs_write authorized_by fs_root
{
    return fs_mkdir(name);
}

fn publish(entry: u64, name: [byte; up_to 64]) -> (result: u64)
    capability fs_root
    effect fs_write authorized_by fs_root
{
    return fs_rename_at(entry, name);
}

fn barrier(entry: u64) -> (result: u64)
    capability fs_root
    effect fs_write authorized_by fs_root
{
    return fs_sync_at(entry);
}

fn list_count() -> (result: u64)
    capability fs_root
    effect fs_list authorized_by fs_root
{
    return fs_list_count();
}

fn read_back(entry: u64, offset: u64, length: u64) -> (result: [byte; up_to 64])
    capability fs_root
    effect fs_read authorized_by fs_root
{
    return fs_read_bytes_at(entry, offset, length);
}
"#;

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn scratch_root() -> std::path::PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "mncs-doctor-profile-016-{}-{id}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("scratch root");
    root
}

fn session() -> Session {
    let artifact = Artifact::from_source(SOURCE, "mncs-research-bytecode").expect("compile");
    Session::open(artifact).expect("open")
}

fn u64_arg(value: u64) -> Value {
    json!({
        "integer": {
            "value": value,
            "type": {"bits": 64, "signed": false}
        }
    })
}

fn view_arg(bytes: &[u8]) -> Value {
    json!({
        "sequence": {
            "values": bytes.iter().map(|byte| json!({"byte": {"value": *byte}})).collect::<Vec<_>>()
        }
    })
}

fn call(session: &Session, function: &str, args: Vec<Value>, root: Option<&Path>) -> Value {
    let mut options = CallOptions::budgeted(100_000);
    if let Some(root) = root {
        options
            .grants
            .push(Grant::fs_root("fs_root", &root.to_string_lossy()));
    }
    let output = session
        .call_json(
            "doctor.fs_probe",
            function,
            &serde_json::to_string(&args).expect("args JSON"),
            &options,
        )
        .expect("call transport");
    serde_json::to_value(output).expect("output JSON")
}

fn returned_u64(output: &Value) -> u64 {
    assert_eq!(output["status"], "returned", "{output:#}");
    output["returned"][0]["integer"]["value"]
        .as_u64()
        .expect("u64 return")
}

fn returned_bytes(output: &Value) -> Vec<u8> {
    assert_eq!(output["status"], "returned", "{output:#}");
    output["returned"][0]["sequence"]["values"]
        .as_array()
        .expect("byte view return")
        .iter()
        .map(|value| value["byte"]["value"].as_u64().expect("byte") as u8)
        .collect()
}

#[test]
fn profile_016_embedded_lifecycle_is_byte_exact_and_isolated() {
    let root = scratch_root();
    let session = session();

    // Create, append, positioned-write, mkdir, atomic same-directory rename,
    // barrier, listing, and two bounded reads all use the real grant.
    let staged = call(
        &session,
        "stage",
        vec![view_arg(b"candidate.tmp"), view_arg(b"0123")],
        Some(&root),
    );
    assert_eq!(staged["backend"], "mncs-research-bytecode");
    assert!(staged["artifact_identity"].as_str().is_some());
    assert!(staged["artifact_sha256"].as_str().is_some());
    assert_eq!(staged["reused_session"], true);
    assert_eq!(staged["effects"][0]["kind"], "fs_write");
    assert!(staged["effects"][0]["provenance"]
        .as_str()
        .unwrap()
        .contains("op:fs_create_file"));
    let entry = returned_u64(&staged);
    assert_eq!(entry, 0);
    let dir_entry = returned_u64(&call(
        &session,
        "make_dir",
        vec![view_arg(b"staging")],
        Some(&root),
    ));
    assert_eq!(dir_entry, 1);
    assert_eq!(
        returned_u64(&call(
            &session,
            "grow",
            vec![u64_arg(entry), view_arg(b"4567")],
            Some(&root),
        )),
        4
    );
    assert_eq!(
        returned_u64(&call(
            &session,
            "patch",
            vec![u64_arg(entry), u64_arg(1), view_arg(b"X")],
            Some(&root),
        )),
        1
    );
    assert_eq!(
        returned_u64(&call(
            &session,
            "barrier",
            vec![u64_arg(entry)],
            Some(&root),
        )),
        1
    );
    let published = returned_u64(&call(
        &session,
        "publish",
        vec![u64_arg(entry), view_arg(b"candidate.mncs")],
        Some(&root),
    ));
    assert_eq!(published, 0);
    assert_eq!(
        returned_u64(&call(&session, "list_count", vec![], Some(&root))),
        2
    );
    assert_eq!(
        returned_bytes(&call(
            &session,
            "read_back",
            vec![u64_arg(published), u64_arg(0), u64_arg(4)],
            Some(&root),
        )),
        b"0X23"
    );
    assert_eq!(
        returned_bytes(&call(
            &session,
            "read_back",
            vec![u64_arg(published), u64_arg(4), u64_arg(64)],
            Some(&root),
        )),
        b"4567"
    );
    assert_eq!(
        std::fs::read(root.join("candidate.mncs")).unwrap(),
        b"0X234567"
    );
    assert!(root.join("staging").is_dir());
    assert!(!root.join("candidate.tmp").exists());

    // The same mutation without a grant is Unsupported and leaves the root
    // untouched, proving effect authority is explicit at the embed boundary.
    let refused = call(
        &session,
        "stage",
        vec![view_arg(b"refused"), view_arg(b"nope")],
        None,
    );
    assert_eq!(refused["status"], "unsupported", "{refused:#}");
    assert!(!root.join("refused").exists());

    let _ = std::fs::remove_dir_all(&root);
}
