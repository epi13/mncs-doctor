use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use mncs_model::ExecutionValue;
use serde_json::Value;

const HASH_CALL_BUDGET: u64 = 500_000;

struct HashKernel {
    session: mncs_embed::Session,
    module: String,
    backend: String,
    artifact_identity: String,
    artifact_digest: String,
    step_budget: u64,
    chunk_bytes: usize,
}

fn main() {
    if let Err(error) = run() {
        panic!("Doctor native source digest generation failed: {error}");
    }
}

fn run() -> Result<(), String> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest_path = root.join("mncs/doctor/source-hash-inputs.json");
    println!("cargo:rerun-if-changed={}", manifest_path.display());
    println!("cargo:rerun-if-changed=build.rs");

    let manifest_bytes = fs::read(&manifest_path)
        .map_err(|error| format!("read {}: {error}", manifest_path.display()))?;
    let manifest: Value = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| format!("parse {}: {error}", manifest_path.display()))?;
    let schema = string_field(&manifest, "schema")?;
    if schema != "mncs.doctor.source-hash-inputs.v1" {
        return Err(format!("unsupported source hash input schema {schema}"));
    }

    let profile = string_field(&manifest, "profile")?;
    let policy_backend = string_field(&manifest, "policy_backend")?;
    let language_revision = string_field(&manifest, "language_revision")?;
    let family_source_path = relative_path(&root, string_field(&manifest, "family_source")?)?;
    let family_module = string_field(&manifest, "family_module")?;
    let family_artifact_path = relative_path(&root, string_field(&manifest, "family_artifact")?)?;
    let expected_family_artifact_sha256 = string_field(&manifest, "family_artifact_sha256")?;
    let digest_module = string_field(&manifest, "digest_module")?;
    let digest_backend = string_field(&manifest, "digest_backend")?;
    let digest_artifact_path = relative_path(&root, string_field(&manifest, "digest_artifact")?)?;
    let expected_digest_file_sha256 = string_field(&manifest, "digest_artifact_file_sha256")?;
    let chunk_bytes = usize_field(&manifest, "chunk_bytes")?;
    let step_budget = usize_field(&manifest, "step_budget")? as u64;
    if chunk_bytes == 0 || chunk_bytes > 1024 {
        return Err(format!(
            "SHA-256 chunk size must be between 1 and 1024, got {chunk_bytes}"
        ));
    }
    if step_budget == 0 || step_budget > HASH_CALL_BUDGET {
        return Err(format!(
            "SHA-256 step budget must be between 1 and {HASH_CALL_BUDGET}"
        ));
    }

    let family_source = read_input(&family_source_path)?;
    let family_source_text = std::str::from_utf8(&family_source)
        .map_err(|error| format!("family source is not UTF-8: {error}"))?;
    if declared_module(family_source_text) != Some(family_module) {
        return Err(format!(
            "family source module does not match manifest module {family_module}"
        ));
    }
    let family_artifact = read_input(&family_artifact_path)?;
    let digest_artifact_bytes = read_input(&digest_artifact_path)?;
    println!("cargo:rerun-if-changed={}", family_source_path.display());
    println!("cargo:rerun-if-changed={}", family_artifact_path.display());
    println!("cargo:rerun-if-changed={}", digest_artifact_path.display());

    let actual_family_artifact_sha256 = mncs_model::sha256_hex(&family_artifact);
    if actual_family_artifact_sha256 != expected_family_artifact_sha256 {
        return Err(format!(
            "Doctor family artifact pin mismatch: expected {expected_family_artifact_sha256}, found {actual_family_artifact_sha256}"
        ));
    }
    let family_artifact_value: Value = serde_json::from_slice(&family_artifact)
        .map_err(|error| format!("parse Doctor family artifact module bindings: {error}"))?;
    let artifact_modules = family_artifact_value
        .get("callable_bindings")
        .and_then(Value::as_array)
        .ok_or_else(|| "Doctor family artifact has no callable_bindings array".to_owned())?
        .iter()
        .filter_map(|binding| binding.get("module").and_then(Value::as_str))
        .filter(|module| module.starts_with("doctor."))
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let actual_digest_file_sha256 = mncs_model::sha256_hex(&digest_artifact_bytes);
    if actual_digest_file_sha256 != expected_digest_file_sha256 {
        return Err(format!(
            "Stdlib SHA-256 artifact pin mismatch: expected {expected_digest_file_sha256}, found {actual_digest_file_sha256}"
        ));
    }

    let artifact = mncs_embed::Artifact::from_json(&digest_artifact_bytes)
        .map_err(|error| format!("refuse frozen Stdlib SHA-256 artifact: {error}"))?;
    if artifact.backend_name() != digest_backend {
        return Err(format!(
            "frozen Stdlib SHA-256 artifact backend is {}, expected {digest_backend}",
            artifact.backend_name()
        ));
    }
    let artifact_identity = artifact.artifact_identity().to_owned();
    let artifact_digest = artifact.digest().to_owned();
    let session = mncs_embed::Session::open(artifact)
        .map_err(|error| format!("open frozen Stdlib SHA-256 artifact: {error}"))?;
    let kernel = HashKernel {
        session,
        module: digest_module.to_owned(),
        backend: digest_backend.to_owned(),
        artifact_identity,
        artifact_digest,
        step_budget,
        chunk_bytes,
    };

    let family_source_sha256 = kernel.hash(&family_source)?;
    let modules = manifest
        .get("modules")
        .and_then(Value::as_array)
        .ok_or_else(|| "source hash input field modules must be an array".to_owned())?;
    let mut module_specs = Vec::with_capacity(modules.len());
    let mut module_keys = BTreeSet::new();
    let mut source_module_ids = BTreeSet::from([family_module.to_owned()]);
    for module in modules {
        let key = string_field(module, "key")?;
        if key.is_empty() || !module_keys.insert(key.to_owned()) {
            return Err(format!("duplicate or empty source hash module key {key:?}"));
        }
        let module_id = string_field(module, "module")?;
        if !source_module_ids.insert(module_id.to_owned()) {
            return Err(format!("duplicate Doctor module identity {module_id}"));
        }
        let source_path = relative_path(&root, string_field(module, "source")?)?;
        println!("cargo:rerun-if-changed={}", source_path.display());
        let source = read_input(&source_path)?;
        let source_text = std::str::from_utf8(&source).map_err(|error| {
            format!(
                "policy source {} is not UTF-8: {error}",
                source_path.display()
            )
        })?;
        if declared_module(source_text) != Some(module_id) {
            return Err(format!(
                "policy source {} does not declare manifest module {module_id}",
                source_path.display()
            ));
        }
        let source_sha256 = kernel.hash(&source)?;
        module_specs.push((key.to_owned(), source_path, source_sha256));
    }
    if module_specs.is_empty() {
        return Err("source hash input manifest has no policy modules".to_owned());
    }
    if source_module_ids != artifact_modules {
        return Err(format!(
            "source hash module manifest does not match Doctor modules in the frozen family artifact (manifest: {source_module_ids:?}, artifact: {artifact_modules:?})"
        ));
    }

    let mut identity_material = Vec::with_capacity(1024);
    for field in [
        profile,
        policy_backend,
        language_revision,
        expected_family_artifact_sha256,
        digest_backend,
        expected_digest_file_sha256,
        &family_source_sha256,
    ] {
        identity_material.extend_from_slice(field.as_bytes());
        identity_material.push(0);
    }
    for (key, _, source_sha256) in &module_specs {
        identity_material.extend_from_slice(key.as_bytes());
        identity_material.push(0);
        identity_material.extend_from_slice(source_sha256.as_bytes());
        identity_material.push(0);
    }
    let static_policy_identity = kernel.hash(&identity_material)?;

    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("Cargo did not set OUT_DIR")?);
    let generated_path = out_dir.join("doctor_source_hashes.rs");
    let mut generated = String::new();
    writeln!(generated, "pub const POLICY_PROFILE: &str = {:?};", profile).unwrap();
    writeln!(
        generated,
        "pub const POLICY_BACKEND: &str = {:?};",
        policy_backend
    )
    .unwrap();
    writeln!(
        generated,
        "pub const MNCS_LANGUAGE_REV: &str = {:?};",
        language_revision
    )
    .unwrap();
    writeln!(
        generated,
        "const FAMILY_ARTIFACT_SHA256: &str = {:?};",
        expected_family_artifact_sha256
    )
    .unwrap();
    writeln!(
        generated,
        "const FAMILY_SOURCE_SHA256: &str = {:?};",
        family_source_sha256
    )
    .unwrap();
    writeln!(
        generated,
        "const SOURCE_HASH_BACKEND: &str = {:?};",
        kernel.backend
    )
    .unwrap();
    writeln!(
        generated,
        "const SOURCE_HASH_ARTIFACT_IDENTITY: &str = {:?};",
        kernel.artifact_identity
    )
    .unwrap();
    writeln!(
        generated,
        "const SOURCE_HASH_ARTIFACT_SHA256: &str = {:?};",
        kernel.artifact_digest
    )
    .unwrap();
    writeln!(generated, "#[cfg(test)]").unwrap();
    writeln!(
        generated,
        "const SOURCE_HASH_ARTIFACT_FILE_SHA256: &str = {:?};",
        expected_digest_file_sha256
    )
    .unwrap();
    writeln!(
        generated,
        "const STATIC_POLICY_IDENTITY: &str = {:?};",
        static_policy_identity
    )
    .unwrap();
    writeln!(generated, "const MODULES: &[ModuleSpec] = &[").unwrap();
    for (key, source_path, source_sha256) in &module_specs {
        let relative = source_path.strip_prefix(&root).map_err(|_| {
            format!(
                "policy source {} escaped the project root",
                source_path.display()
            )
        })?;
        let relative = relative.to_string_lossy().replace('\\', "/");
        writeln!(
            generated,
            "    ModuleSpec {{ key: {:?}, #[cfg(test)] source: include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/{}\")), source_sha256: {:?} }},",
            key, relative, source_sha256
        )
        .unwrap();
    }
    writeln!(generated, "]; ").unwrap();
    fs::write(&generated_path, generated)
        .map_err(|error| format!("write {}: {error}", generated_path.display()))?;
    Ok(())
}

impl HashKernel {
    fn hash(&self, bytes: &[u8]) -> Result<String, String> {
        let mut state = self.call("init", Vec::new())?;
        for chunk in bytes.chunks(self.chunk_bytes) {
            let input = ExecutionValue::Sequence {
                values: Arc::new(
                    chunk
                        .iter()
                        .map(|byte| ExecutionValue::Byte {
                            value: i128::from(*byte),
                        })
                        .collect(),
                ),
            };
            state = self.call("update_blocks_standard", vec![state, input])?;
        }
        let digest = self.call("finalize_standard", vec![state])?;
        let values = match digest {
            ExecutionValue::Sequence { values } if values.len() == 32 => values,
            _ => return Err("MNCS SHA-256 returned a value other than 32 bytes".to_owned()),
        };
        let mut output = String::with_capacity(64);
        for value in values.iter() {
            let ExecutionValue::Byte { value } = value else {
                return Err("MNCS SHA-256 result contains a non-byte value".to_owned());
            };
            let byte = u8::try_from(*value)
                .map_err(|_| "MNCS SHA-256 returned a byte outside 0..=255".to_owned())?;
            write!(output, "{byte:02x}").unwrap();
        }
        Ok(output)
    }

    fn call(
        &self,
        function: &str,
        arguments: Vec<ExecutionValue>,
    ) -> Result<ExecutionValue, String> {
        let output = self.session.call(
            &self.module,
            function,
            arguments,
            &mncs_embed::CallOptions::budgeted(self.step_budget),
        );
        if output.artifact_identity != self.artifact_identity
            || output.artifact_sha256 != self.artifact_digest
            || output.backend != self.backend
        {
            return Err(format!(
                "MNCS SHA-256 {function} returned mismatched artifact provenance"
            ));
        }
        if output.status != "returned" {
            return Err(format!(
                "MNCS SHA-256 {function} failed: {}",
                output.failure_reason.unwrap_or_else(|| output.status)
            ));
        }
        let mut returned = output.returned;
        if returned.len() != 1 {
            return Err(format!(
                "MNCS SHA-256 {function} returned {} values, expected one",
                returned.len()
            ));
        }
        Ok(returned.remove(0))
    }
}

fn string_field<'a>(value: &'a Value, name: &str) -> Result<&'a str, String> {
    value
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("source hash input field {name} must be a string"))
}

fn usize_field(value: &Value, name: &str) -> Result<usize, String> {
    value
        .get(name)
        .and_then(Value::as_u64)
        .and_then(|number| usize::try_from(number).ok())
        .ok_or_else(|| format!("source hash input field {name} must be a positive integer"))
}

fn relative_path(root: &Path, path: &str) -> Result<PathBuf, String> {
    let relative = Path::new(path);
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "source hash input path must be a clean relative path: {path}"
        ));
    }
    Ok(root.join(relative))
}

fn read_input(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))
}

fn declared_module(source: &str) -> Option<&str> {
    source
        .lines()
        .find_map(|line| line.trim().strip_prefix("module "))
        .and_then(|declaration| declaration.strip_suffix(';'))
}
