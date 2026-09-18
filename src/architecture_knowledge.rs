//! Bounded consumer of the Commons architecture projection.
//!
//! Doctor does not clone or parse every family repository.  When Commons and
//! an optional project-owned claims file are available, it compares the
//! project's declared canonical paths, shadow state, contracts, and language
//! identity with the family projection.  Missing context is reported as
//! unavailable rather than inferred as healthy or drifted.

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::language_knowledge::LanguageKnowledgeStatus;

const MODEL_SCHEMA: &str = "commons.mncs.architecture-model/1";
const CLAIMS_SCHEMA: &str = "mncs.doctor.architecture-claims/1";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ArchitectureKnowledgeStatus {
    pub schema_identity: Option<String>,
    pub state: String,
    pub source_path: Option<String>,
    pub content_identity: Option<String>,
    pub capability_count: usize,
    pub claims_state: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

pub fn probe(
    root: Option<&Path>,
    language: Option<&LanguageKnowledgeStatus>,
) -> ArchitectureKnowledgeStatus {
    let project_root = root
        .map(Path::to_path_buf)
        .or_else(|| env::current_dir().ok());
    let Some(model_path) = discover_model(project_root.as_deref()) else {
        return ArchitectureKnowledgeStatus {
            state: "missing".to_owned(),
            claims_state: "unavailable".to_owned(),
            warnings: vec!["Commons architecture model was not found".to_owned()],
            ..ArchitectureKnowledgeStatus::default()
        };
    };
    let text = match fs::read_to_string(&model_path) {
        Ok(text) => text,
        Err(error) => return invalid(&model_path, error.to_string()),
    };
    let model: serde_json::Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => return invalid(&model_path, error.to_string()),
    };
    let schema_identity = model
        .get("schema_identity")
        .or_else(|| model.get("schema_version"))
        .and_then(|value| value.as_str())
        .map(str::to_owned);
    let content_identity = model
        .get("content_identity")
        .and_then(|value| value.as_str())
        .map(str::to_owned);
    let capabilities = model
        .get("capabilities")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    if schema_identity.as_deref() != Some(MODEL_SCHEMA) {
        errors.push(format!(
            "unsupported Commons architecture schema: {}",
            schema_identity.as_deref().unwrap_or("missing")
        ));
    }
    if content_identity
        .as_deref()
        .is_none_or(|identity| !valid_content_identity(identity))
    {
        errors.push("Commons architecture content_identity is missing or malformed".to_owned());
    }

    let claims_path = project_root
        .as_ref()
        .map(|path| path.join(".mncs/architecture-claims.json"));
    let claims = claims_path.as_ref().and_then(|path| {
        if !path.is_file() {
            None
        } else {
            match fs::read_to_string(path)
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            {
                Some(value) => Some(value),
                None => {
                    errors.push(format!(
                        "architecture claims are not valid JSON: {}",
                        path.display()
                    ));
                    None
                }
            }
        }
    });
    let claims_state = if let Some(claims) = claims.as_ref() {
        validate_claims(
            project_root.as_deref(),
            &model,
            claims,
            content_identity.as_deref(),
            language,
            &mut errors,
        );
        if errors.is_empty() {
            "verified"
        } else {
            "drift"
        }
    } else {
        warnings.push(
            "project architecture claims are unavailable; Commons facts were not compared to local declarations"
                .to_owned(),
        );
        "unavailable"
    };
    let state = if errors.is_empty() {
        "loaded"
    } else {
        "invalid"
    };
    ArchitectureKnowledgeStatus {
        schema_identity,
        state: state.to_owned(),
        source_path: Some(model_path.to_string_lossy().into_owned()),
        content_identity,
        capability_count: capabilities.len(),
        claims_state: claims_state.to_owned(),
        errors,
        warnings,
    }
}

fn valid_content_identity(identity: &str) -> bool {
    identity.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn discover_model(root: Option<&Path>) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("MNCS_COMMONS_ROOT") {
        candidates.push(PathBuf::from(path).join("family/architecture-model-v1.json"));
    }
    if let Some(root) = root {
        if root.join("family/architecture-model-v1.json").is_file() {
            candidates.push(root.join("family/architecture-model-v1.json"));
        }
        if let Some(parent) = root.parent() {
            candidates.push(parent.join("MNCS-Commons/family/architecture-model-v1.json"));
        }
    }
    if let Ok(current) = env::current_dir() {
        candidates.push(current.join("family/architecture-model-v1.json"));
        if let Some(parent) = current.parent() {
            candidates.push(parent.join("MNCS-Commons/family/architecture-model-v1.json"));
        }
    }
    let mut seen = BTreeSet::new();
    candidates
        .into_iter()
        .find(|path| seen.insert(path.clone()) && path.is_file())
}

fn validate_claims(
    project_root: Option<&Path>,
    model: &serde_json::Value,
    claims: &serde_json::Value,
    model_identity: Option<&str>,
    language: Option<&LanguageKnowledgeStatus>,
    errors: &mut Vec<String>,
) {
    if claims
        .get("schema_version")
        .and_then(|value| value.as_str())
        != Some(CLAIMS_SCHEMA)
    {
        errors.push(format!(
            "architecture claims schema must be {CLAIMS_SCHEMA}"
        ));
    }
    if let (Some(expected), Some(actual)) = (
        claims
            .get("architecture_content_identity")
            .and_then(|value| value.as_str()),
        model_identity,
    ) {
        if expected != actual {
            errors.push(format!(
                "architecture content identity differs: {expected} != {actual}"
            ));
        }
    }
    if let (Some(expected), Some(actual)) = (
        claims
            .get("language_content_identity")
            .and_then(|value| value.as_str()),
        language.and_then(|status| status.content_identity.as_deref()),
    ) {
        if expected != actual {
            errors.push(format!(
                "language capability identity differs: {expected} != {actual}"
            ));
        }
    }
    if let (Some(expected), Some(actual)) = (
        claims
            .get("language_profile")
            .and_then(|value| value.as_str()),
        language.and_then(|status| status.current_profile.as_deref()),
    ) {
        if expected != actual {
            errors.push(format!("language profile differs: {expected} != {actual}"));
        }
    }
    let model_capabilities = model
        .get("capabilities")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    let claims_capabilities = claims
        .get("capabilities")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    for claim in claims_capabilities {
        let Some(id) = claim.get("id").and_then(|value| value.as_str()) else {
            errors.push("architecture capability claims require id".to_owned());
            continue;
        };
        let Some(model_capability) = model_capabilities
            .iter()
            .find(|item| item.get("id").and_then(|value| value.as_str()) == Some(id))
        else {
            errors.push(format!("architecture capability is not in Commons: {id}"));
            continue;
        };
        let model_path = model_capability
            .get("canonical")
            .and_then(|value| value.get("path"))
            .and_then(|value| value.as_str());
        let claim_path = claim.get("canonical_path").and_then(|value| value.as_str());
        if model_path != claim_path {
            errors.push(format!(
                "canonical path drift for {id}: {} != {}",
                claim_path.unwrap_or("missing"),
                model_path.unwrap_or("missing")
            ));
        }
        if let (Some(root), Some(path)) = (project_root, claim_path) {
            if !root.join(path).exists() {
                errors.push(format!("stale canonical path for {id}: {path}"));
            }
        }
        let model_shadow = model_capability
            .get("shadow")
            .and_then(|value| value.get("path"))
            .and_then(|value| value.as_str());
        let claim_shadow = claim
            .get("active_shadow_path")
            .and_then(|value| value.as_str());
        if claim_shadow != model_shadow && (claim_shadow.is_some() || model_shadow.is_some()) {
            errors.push(format!(
                "shadow path drift for {id}: {} != {}",
                claim_shadow.unwrap_or("none"),
                model_shadow.unwrap_or("none")
            ));
        }
        if let Some(claim_state) = claim.get("shadow_state").and_then(|value| value.as_str()) {
            let model_state = model_capability
                .get("shadow")
                .and_then(|value| value.get("retirement_state"))
                .and_then(|value| value.as_str())
                .unwrap_or("none");
            if claim_state != model_state {
                errors.push(format!(
                    "shadow state drift for {id}: {claim_state} != {model_state}"
                ));
            }
        }
    }
    let declared_contracts = claims
        .get("contracts")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str());
    let model_contracts: BTreeSet<String> = model_capabilities
        .iter()
        .flat_map(|item| {
            item.get("versioning")
                .and_then(|value| value.get("compatibility"))
                .and_then(|value| value.get("wire_contracts"))
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
                .filter_map(|value| value.as_str().map(str::to_owned))
        })
        .collect();
    for contract in declared_contracts {
        if !model_contracts.contains(contract) {
            errors.push(format!(
                "declared architecture contract cannot be resolved: {contract}"
            ));
        }
    }
}

fn invalid(path: &Path, message: String) -> ArchitectureKnowledgeStatus {
    ArchitectureKnowledgeStatus {
        state: "invalid".to_owned(),
        source_path: Some(path.to_string_lossy().into_owned()),
        claims_state: "unavailable".to_owned(),
        errors: vec![message],
        ..ArchitectureKnowledgeStatus::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root() -> PathBuf {
        let root = env::temp_dir().join(format!(
            "mncs-doctor-architecture-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("family")).unwrap();
        root
    }

    #[test]
    fn missing_claims_are_explicitly_unavailable() {
        let root = temp_root();
        let commons = root.join("family/architecture-model-v1.json");
        fs::write(
            &commons,
            serde_json::json!({
                "schema_version": MODEL_SCHEMA,
                "schema_identity": MODEL_SCHEMA,
                "content_identity": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
                "capabilities": []
            })
            .to_string(),
        )
        .unwrap();
        let status = probe(Some(&root), None);
        assert_eq!(status.state, "loaded");
        assert_eq!(status.claims_state, "unavailable");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_claims_are_reported_as_architecture_drift() {
        let root = temp_root();
        let commons = root.join("family/architecture-model-v1.json");
        fs::write(
            &commons,
            serde_json::json!({
                "schema_version": MODEL_SCHEMA,
                "schema_identity": MODEL_SCHEMA,
                "content_identity": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
                "capabilities": [{
                    "id": "fixture.capability",
                    "canonical": {"path": "canonical.mncs"},
                    "shadow": null,
                    "versioning": {"compatibility": {"wire_contracts": []}}
                }]
            })
            .to_string(),
        )
        .unwrap();
        fs::create_dir_all(root.join(".mncs")).unwrap();
        fs::write(
            root.join(".mncs/architecture-claims.json"),
            serde_json::json!({
                "schema_version": CLAIMS_SCHEMA,
                "architecture_content_identity": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
                "capabilities": [{
                    "id": "fixture.capability",
                    "canonical_path": "stale.mncs"
                }]
            })
            .to_string(),
        )
        .unwrap();
        let status = probe(Some(&root), None);
        assert_eq!(status.claims_state, "drift");
        assert!(status
            .errors
            .iter()
            .any(|error| error.contains("stale canonical path")));
        fs::remove_dir_all(root).unwrap();
    }
}
