//! Freshness-aware consumer of the `mncs-language` capability index.

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const SCHEMA: &str = "mncs.language-capabilities/1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageKnowledgeStatus {
    pub schema_version: String,
    pub state: String,
    pub freshness: String,
    pub source_path: Option<String>,
    pub content_identity: Option<String>,
    pub current_profile: Option<String>,
    pub module_count: usize,
    pub intrinsic_count: usize,
    pub provenance_count: usize,
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
    let provenance = value
        .get("provenance")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if content_identity.is_none() || current_profile.is_none() || provenance.is_empty() {
        return invalid(&path, "capability index envelope is incomplete".to_owned());
    }
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
        current_profile,
        module_count,
        intrinsic_count,
        provenance_count: value
            .get("provenance")
            .and_then(|v| v.as_array())
            .map_or(0, Vec::len),
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
        current_profile: None,
        module_count: 0,
        intrinsic_count: 0,
        provenance_count: 0,
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
