//! Freshness-aware consumer of the `mncs-language` capability index.

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const SCHEMA: &str = "mncs.language-capabilities/1";
const MIGRATION_SCHEMA: &str = "mncs.language-migrations/1";

/// Machine-readable source migration metadata published by `mncs-language`.
/// The language owns the semantic identity and safety claim; Doctor only
/// applies the explicitly described textual transformation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageMigrationManifest {
    pub schema_version: String,
    pub language: String,
    pub canonicalization_revision: String,
    pub rewrites: Vec<CanonicalModuleRewrite>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalModuleRewrite {
    pub id: String,
    pub kind: String,
    pub obsolete: String,
    pub canonical: String,
    pub mechanically_safe: bool,
    pub semantic_caveats: String,
    pub minimum_profile: String,
    pub source_transformation: String,
    pub verification: MigrationVerification,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationVerification {
    pub command: String,
    pub obligation: String,
}

/// Locate the language-owned migration manifest without making consumers
/// remember a repository-specific path.
pub fn discover_migrations(root: Option<&Path>) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("MNCS_LANGUAGE_MIGRATIONS") {
        candidates.push(PathBuf::from(path));
    }
    if let Some(path) = env::var_os("MNCS_LANGUAGE_ROOT") {
        candidates.push(PathBuf::from(path).join("docs/language-migrations.json"));
    }
    if let Some(root) = root {
        candidates.push(root.join("docs/language-migrations.json"));
        if let Some(parent) = root.parent() {
            candidates.push(parent.join("mncs-language/docs/language-migrations.json"));
        }
    }
    if let Ok(current) = env::current_dir() {
        candidates.push(current.join("docs/language-migrations.json"));
        if let Some(parent) = current.parent() {
            candidates.push(parent.join("mncs-language/docs/language-migrations.json"));
        }
    }
    let mut seen = BTreeSet::new();
    candidates
        .into_iter()
        .find(|path| seen.insert(path.clone()) && path.is_file())
}

/// Load and validate language-owned migration knowledge. A missing manifest
/// is ordinary for non-MNCS repositories; malformed present knowledge is a
/// hard error so Doctor never silently guesses a rewrite.
pub fn load_migrations(
    root: Option<&Path>,
) -> Result<Option<(PathBuf, LanguageMigrationManifest)>, String> {
    let Some(path) = discover_migrations(root) else {
        return Ok(None);
    };
    let text = fs::read_to_string(&path).map_err(|error| {
        format!(
            "cannot read language migration manifest {}: {error}",
            path.display()
        )
    })?;
    let manifest: LanguageMigrationManifest = serde_json::from_str(&text).map_err(|error| {
        format!(
            "cannot decode language migration manifest {}: {error}",
            path.display()
        )
    })?;
    if manifest.schema_version != MIGRATION_SCHEMA || manifest.language != "MNCS" {
        return Err(format!(
            "unsupported language migration manifest {} (schema {}, language {})",
            path.display(),
            manifest.schema_version,
            manifest.language
        ));
    }
    if manifest.canonicalization_revision.trim().is_empty() || manifest.rewrites.is_empty() {
        return Err(format!(
            "language migration manifest {} has no canonicalization revision or rewrites",
            path.display()
        ));
    }
    let mut ids = BTreeSet::new();
    let mut obsolete = BTreeSet::new();
    for rewrite in &manifest.rewrites {
        if rewrite.kind != "module_import"
            || rewrite.id.trim().is_empty()
            || rewrite.obsolete.trim().is_empty()
            || rewrite.canonical.trim().is_empty()
            || rewrite.obsolete == rewrite.canonical
            || rewrite.semantic_caveats.trim().is_empty()
            || rewrite.minimum_profile.trim().is_empty()
            || rewrite.source_transformation.trim().is_empty()
            || rewrite.verification.command.trim().is_empty()
            || rewrite.verification.obligation.trim().is_empty()
        {
            return Err(format!(
                "invalid language migration entry {:?} in {}",
                rewrite.id,
                path.display()
            ));
        }
        if !ids.insert(rewrite.id.clone()) {
            return Err(format!(
                "duplicate language migration id {:?} in {}",
                rewrite.id,
                path.display()
            ));
        }
        if !obsolete.insert(rewrite.obsolete.clone()) {
            return Err(format!(
                "duplicate obsolete module identity {:?} in {}",
                rewrite.obsolete,
                path.display()
            ));
        }
    }
    Ok(Some((path, manifest)))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageKnowledgeStatus {
    pub schema_version: String,
    pub state: String,
    pub freshness: String,
    pub source_path: Option<String>,
    pub content_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compiler_inventory_identity: Option<String>,
    pub current_profile: Option<String>,
    pub module_count: usize,
    pub intrinsic_count: usize,
    pub provenance_count: usize,
    #[serde(default)]
    pub language_delta_history_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migration_manifest_path: Option<String>,
    #[serde(default)]
    pub migration_count: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stale_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn probe(root: Option<&Path>) -> LanguageKnowledgeStatus {
    let Some(path) = discover(root) else {
        return missing("authoritative language capability index was not found");
    };
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => return invalid(&path, error.to_string()),
    };
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => return invalid(&path, error.to_string()),
    };
    if value.get("schema_version").and_then(|v| v.as_str()) != Some(SCHEMA)
        || value.get("language").and_then(|v| v.as_str()) != Some("MNCS")
    {
        return invalid(
            &path,
            "unsupported capability index schema or language".to_owned(),
        );
    }
    let content_identity = value
        .get("content_identity")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let compiler_inventory = value.get("compiler_inventory");
    let compiler_inventory_identity = value
        .get("compiler_inventory_identity")
        .and_then(|v| v.as_str())
        .or_else(|| {
            compiler_inventory
                .and_then(|v| v.get("inventory_identity"))
                .and_then(|v| v.as_str())
        })
        .map(str::to_owned);
    let current_profile = value
        .get("current_profile")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let module_count = value
        .get("library_modules")
        .and_then(|v| v.as_array())
        .map_or(0, Vec::len);
    let intrinsic_count = value
        .get("intrinsics")
        .and_then(|v| v.as_array())
        .map_or(0, Vec::len);
    let intrinsics_match_compiler = compiler_inventory
        .and_then(|inventory| inventory.get("intrinsics"))
        .is_some_and(|intrinsics| value.get("intrinsics") == Some(intrinsics));
    let provenance = value
        .get("provenance")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if content_identity.is_none()
        || compiler_inventory_identity.is_none()
        || current_profile.is_none()
        || provenance.is_empty()
        || !intrinsics_match_compiler
    {
        let reason = if !intrinsics_match_compiler {
            "capability index intrinsics do not match the compiler-owned inventory"
        } else if compiler_inventory_identity.is_none() {
            "capability index has no authoritative compiler inventory identity"
        } else {
            "capability index envelope is incomplete"
        };
        return invalid(&path, reason.to_owned());
    }
    let language_delta_history_available = path
        .with_file_name("language-capability-deltas.json")
        .is_file();
    let (migration_manifest_path, migration_count) = match load_migrations(root) {
        Ok(Some((migration_path, manifest))) => (
            Some(migration_path.to_string_lossy().into_owned()),
            manifest.rewrites.len(),
        ),
        Ok(None) => (None, 0),
        Err(error) => return invalid(&path, error),
    };
    let language_root = path.parent().and_then(Path::parent);
    let mut stale_paths = Vec::new();
    let mut unavailable = false;
    if let Some(language_root) = language_root {
        for item in provenance {
            let Some(relative) = item.get("path").and_then(|v| v.as_str()) else {
                unavailable = true;
                continue;
            };
            let Some(expected) = item.get("source_identity").and_then(|v| v.as_str()) else {
                unavailable = true;
                continue;
            };
            if let Some(compiler_identity) = relative.strip_prefix("compiler:") {
                let observed = item
                    .get("inventory_identity")
                    .and_then(|value| value.as_str())
                    .or_else(|| compiler_inventory_identity.as_deref());
                if compiler_identity == "language-inventory"
                    && observed == compiler_inventory_identity.as_deref()
                {
                    continue;
                }
                stale_paths.push(relative.to_owned());
                continue;
            }
            let source = language_root.join(relative);
            if !source.is_file() {
                unavailable = true;
                continue;
            }
            match sha256_file(&source) {
                Ok(actual) if actual == expected => {}
                Ok(_) => stale_paths.push(relative.to_owned()),
                Err(_) => unavailable = true,
            }
        }
    } else {
        unavailable = true;
    }
    let freshness = if !stale_paths.is_empty() {
        "stale"
    } else if unavailable {
        "unavailable"
    } else {
        "verified"
    };
    LanguageKnowledgeStatus {
        schema_version: SCHEMA.to_owned(),
        state: "loaded".to_owned(),
        freshness: freshness.to_owned(),
        source_path: Some(path.to_string_lossy().into_owned()),
        content_identity,
        compiler_inventory_identity,
        current_profile,
        module_count,
        intrinsic_count,
        provenance_count: value
            .get("provenance")
            .and_then(|v| v.as_array())
            .map_or(0, Vec::len),
        language_delta_history_available,
        migration_manifest_path,
        migration_count,
        stale_paths,
        error: None,
    }
}

fn discover(root: Option<&Path>) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("MNCS_LANGUAGE_CAPABILITY_INDEX") {
        candidates.push(PathBuf::from(path));
    }
    if let Some(path) = env::var_os("MNCS_LANGUAGE_ROOT") {
        candidates.push(PathBuf::from(path).join("docs/language-capabilities.json"));
    }
    if let Some(root) = root {
        candidates.push(root.join("docs/language-capabilities.json"));
        if let Some(parent) = root.parent() {
            candidates.push(parent.join("mncs-language/docs/language-capabilities.json"));
        }
    }
    if let Ok(current) = env::current_dir() {
        candidates.push(current.join("docs/language-capabilities.json"));
        if let Some(parent) = current.parent() {
            candidates.push(parent.join("mncs-language/docs/language-capabilities.json"));
        }
    }
    let mut seen = BTreeSet::new();
    candidates
        .into_iter()
        .find(|path| seen.insert(path.clone()) && path.is_file())
}

fn sha256_file(path: &Path) -> Result<String, std::io::Error> {
    let mut hasher = Sha256::new();
    hasher.update(fs::read(path)?);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn missing(message: &str) -> LanguageKnowledgeStatus {
    LanguageKnowledgeStatus {
        schema_version: SCHEMA.to_owned(),
        state: "missing".to_owned(),
        freshness: "unavailable".to_owned(),
        source_path: None,
        content_identity: None,
        compiler_inventory_identity: None,
        current_profile: None,
        module_count: 0,
        intrinsic_count: 0,
        provenance_count: 0,
        language_delta_history_available: false,
        migration_manifest_path: None,
        migration_count: 0,
        stale_paths: Vec::new(),
        error: Some(message.to_owned()),
    }
}

fn invalid(path: &Path, message: String) -> LanguageKnowledgeStatus {
    let mut status = missing(&message);
    status.state = "invalid".to_owned();
    status.source_path = Some(path.to_string_lossy().into_owned());
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_index_is_explicitly_unavailable() {
        let status = missing("not present");
        assert_eq!(status.state, "missing");
        assert_eq!(status.freshness, "unavailable");
    }
}
