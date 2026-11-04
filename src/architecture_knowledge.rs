//! Bounded consumer of the Commons architecture projection.
//!
//! Doctor does not clone or parse every family repository.  When Commons and
//! an optional project-owned claims file are available, it compares the
//! project's declared canonical paths, shadow state, contracts, and language
//! identity with the family projection.  Missing context is reported as
//! unavailable rather than inferred as healthy or drifted.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::language_knowledge::LanguageKnowledgeStatus;
use crate::mncs_runtime::{ArchitectureCapability, DoctorMncsRuntime};

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
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let runtime = match DoctorMncsRuntime::production() {
        Ok(runtime) => runtime,
        Err(error) => return invalid(&model_path, error.to_string()),
    };
    let model_decision =
        match runtime.architecture_model(schema_identity.as_deref(), content_identity.as_deref()) {
            Ok(decision) => decision,
            Err(error) => return invalid(&model_path, error.to_string()),
        };
    if model_decision.value_too_long {
        errors.push("Commons architecture values exceed native policy text bounds".to_owned());
    }
    if !model_decision.schema_supported {
        errors.push(format!(
            "unsupported Commons architecture schema: {}",
            schema_identity.as_deref().unwrap_or("missing")
        ));
    }
    if !model_decision.content_identity_valid {
        errors.push("Commons architecture content_identity is missing or malformed".to_owned());
    }

    let model_capabilities: Vec<ArchitectureCapability> =
        capabilities.iter().take(64).map(model_capability).collect();
    let model_contracts = collect_model_contracts(capabilities, 65);
    let model_contracts_exceeded = model_contracts.len() > 64;

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
        if let Err(error) = validate_claims(
            project_root.as_deref(),
            claims,
            &model_capabilities,
            capabilities.len() > 64,
            &model_contracts,
            model_contracts_exceeded,
            content_identity.as_deref(),
            language,
            runtime,
            &mut errors,
        ) {
            errors.push(error.to_string());
        }
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
    let mut seen = std::collections::BTreeSet::new();
    candidates
        .into_iter()
        .find(|path| seen.insert(path.clone()) && path.is_file())
}

fn validate_claims(
    project_root: Option<&Path>,
    claims: &serde_json::Value,
    model_capabilities: &[ArchitectureCapability],
    model_capabilities_exceeded: bool,
    model_contracts: &[String],
    model_contracts_exceeded: bool,
    model_identity: Option<&str>,
    language: Option<&LanguageKnowledgeStatus>,
    runtime: &DoctorMncsRuntime,
    errors: &mut Vec<String>,
) -> Result<(), crate::mncs_runtime::RuntimeError> {
    let header = runtime.architecture_claims_header(
        string_field(claims, "schema_version"),
        string_field(claims, "architecture_content_identity"),
        model_identity,
        string_field(claims, "language_content_identity"),
        language.and_then(|status| status.content_identity.as_deref()),
        string_field(claims, "language_profile"),
        language.and_then(|status| status.current_profile.as_deref()),
    )?;
    if header.value_too_long {
        errors.push("architecture claims values exceed native policy text bounds".to_owned());
    }
    if !header.schema_supported {
        errors.push(format!(
            "architecture claims schema must be {CLAIMS_SCHEMA}"
        ));
    }
    if header.architecture_identity_drift {
        errors.push(format!(
            "architecture content identity differs: {} != {}",
            string_field(claims, "architecture_content_identity").unwrap_or("missing"),
            model_identity.unwrap_or("missing")
        ));
    }
    if header.language_identity_drift {
        errors.push(format!(
            "language capability identity differs: {} != {}",
            string_field(claims, "language_content_identity").unwrap_or("missing"),
            language
                .and_then(|status| status.content_identity.as_deref())
                .unwrap_or("missing")
        ));
    }
    if header.language_profile_drift {
        errors.push(format!(
            "language profile differs: {} != {}",
            string_field(claims, "language_profile").unwrap_or("missing"),
            language
                .and_then(|status| status.current_profile.as_deref())
                .unwrap_or("missing")
        ));
    }

    let claims_capabilities = claims
        .get("capabilities")
        .and_then(|value| value.as_array())
        .map(Vec::as_slice)
        .unwrap_or_default();
    if model_capabilities_exceeded {
        errors.push(
            "Commons architecture capability list exceeds native policy bound (64)".to_owned(),
        );
    } else if claims_capabilities.len() > 64 {
        errors.push("architecture capability claims exceed native policy bound (64)".to_owned());
    } else {
        for claim in claims_capabilities {
            let id = string_field(claim, "id");
            let claim_path = string_field(claim, "canonical_path");
            let claim_shadow = string_field(claim, "active_shadow_path");
            let claim_shadow_state = string_field(claim, "shadow_state");
            let canonical_path_exists = project_root
                .zip(claim_path)
                .is_some_and(|(root, path)| root.join(path).exists());
            let decision = runtime.architecture_capability(
                id,
                claim_path,
                claim_shadow,
                claim_shadow_state,
                model_capabilities,
                project_root.is_some(),
                canonical_path_exists,
            )?;
            if decision.missing_claim_id {
                errors.push("architecture capability claims require id".to_owned());
                continue;
            }
            if decision.value_too_long {
                errors.push(format!(
                    "architecture capability values exceed native policy text bounds for {}",
                    id.unwrap_or("missing")
                ));
            }
            let Some(id) = id else {
                errors.push("architecture capability claims require id".to_owned());
                continue;
            };
            if !decision.found {
                if !decision.value_too_long {
                    errors.push(format!("architecture capability is not in Commons: {id}"));
                }
                continue;
            }
            let model_capability = usize::try_from(decision.model_index)
                .ok()
                .and_then(|index| model_capabilities.get(index));
            let model_path = model_capability.and_then(|item| item.canonical_path.as_deref());
            let model_shadow = model_capability.and_then(|item| item.shadow_path.as_deref());
            let model_shadow_state = model_capability
                .and_then(|item| item.retirement_state.as_deref())
                .unwrap_or("none");
            if decision.canonical_path_drift {
                errors.push(format!(
                    "canonical path drift for {id}: {} != {}",
                    claim_path.unwrap_or("missing"),
                    model_path.unwrap_or("missing")
                ));
            }
            if decision.stale_canonical_path {
                errors.push(format!(
                    "stale canonical path for {id}: {}",
                    claim_path.unwrap_or("missing")
                ));
            }
            if decision.shadow_path_drift {
                errors.push(format!(
                    "shadow path drift for {id}: {} != {}",
                    claim_shadow.unwrap_or("none"),
                    model_shadow.unwrap_or("none")
                ));
            }
            if decision.shadow_state_drift {
                errors.push(format!(
                    "shadow state drift for {id}: {} != {model_shadow_state}",
                    claim_shadow_state.unwrap_or("none")
                ));
            }
        }
    }

    let declared_contracts = claims
        .get("contracts")
        .and_then(|value| value.as_array())
        .map(Vec::as_slice)
        .unwrap_or_default();
    if model_contracts_exceeded {
        errors
            .push("Commons architecture contract list exceeds native policy bound (64)".to_owned());
    } else if declared_contracts
        .iter()
        .filter(|value| value.is_string())
        .count()
        > 64
    {
        errors.push("architecture contract claims exceed native policy bound (64)".to_owned());
    } else {
        for contract in declared_contracts
            .iter()
            .filter_map(serde_json::Value::as_str)
        {
            let decision = runtime.architecture_contract(contract, model_contracts)?;
            if decision.value_too_long {
                errors.push(format!(
                    "architecture contract exceeds native policy text bound (64): {contract}"
                ));
            }
            if !decision.resolved {
                errors.push(format!(
                    "declared architecture contract cannot be resolved: {contract}"
                ));
            }
        }
    }
    Ok(())
}

fn string_field<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(serde_json::Value::as_str)
}

fn model_capability(value: &serde_json::Value) -> ArchitectureCapability {
    ArchitectureCapability {
        id: string_field(value, "id").map(str::to_owned),
        canonical_path: value
            .get("canonical")
            .and_then(|canonical| string_field(canonical, "path"))
            .map(str::to_owned),
        shadow_path: value
            .get("shadow")
            .and_then(|shadow| string_field(shadow, "path"))
            .map(str::to_owned),
        retirement_state: value
            .get("shadow")
            .and_then(|shadow| string_field(shadow, "retirement_state"))
            .map(str::to_owned),
    }
}

fn collect_model_contracts(capabilities: &[serde_json::Value], limit: usize) -> Vec<String> {
    capabilities
        .iter()
        .flat_map(|item| {
            item.get("versioning")
                .and_then(|value| value.get("compatibility"))
                .and_then(|value| value.get("wire_contracts"))
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
        })
        .take(limit)
        .map(str::to_owned)
        .collect()
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

    fn valid_model(identity: &str) -> serde_json::Value {
        serde_json::json!({
            "schema_version": "commons.mncs.architecture-model/1",
            "schema_identity": "commons.mncs.architecture-model/1",
            "content_identity": identity,
            "capabilities": [{
                "id": "fixture.capability",
                "canonical": {"path": "canonical.mncs"},
                "shadow": {
                    "path": "legacy.mncs",
                    "retirement_state": "retired"
                },
                "versioning": {"compatibility": {
                    "wire_contracts": ["fixture.wire/1"]
                }}
            }]
        })
    }

    fn valid_claims(identity: &str) -> serde_json::Value {
        serde_json::json!({
            "schema_version": CLAIMS_SCHEMA,
            "architecture_content_identity": identity,
            "language_content_identity": "language-identity-a",
            "language_profile": "0.16",
            "capabilities": [{
                "id": "fixture.capability",
                "canonical_path": "canonical.mncs",
                "active_shadow_path": "legacy.mncs",
                "shadow_state": "retired"
            }],
            "contracts": ["fixture.wire/1"]
        })
    }

    fn write_model(root: &Path, model: &serde_json::Value) {
        fs::write(
            root.join("family/architecture-model-v1.json"),
            model.to_string(),
        )
        .unwrap();
    }

    fn write_claims(root: &Path, claims: &serde_json::Value) {
        fs::create_dir_all(root.join(".mncs")).unwrap();
        fs::write(
            root.join(".mncs/architecture-claims.json"),
            claims.to_string(),
        )
        .unwrap();
    }

    fn language_status() -> LanguageKnowledgeStatus {
        LanguageKnowledgeStatus {
            schema_version: "fixture-language-schema".to_owned(),
            state: "loaded".to_owned(),
            freshness: "current".to_owned(),
            source_path: None,
            content_identity: Some("language-identity-a".to_owned()),
            compiler_inventory_identity: None,
            current_profile: Some("0.16".to_owned()),
            standard_library_namespaces: Vec::new(),
            module_count: 0,
            intrinsic_count: 0,
            provenance_count: 0,
            language_delta_history_available: false,
            migration_manifest_path: None,
            migration_count: 0,
            stale_paths: Vec::new(),
            error: None,
        }
    }

    #[test]
    fn missing_claims_are_explicitly_unavailable() {
        let root = temp_root();
        write_model(
            &root,
            &serde_json::json!({
                "schema_version": "commons.mncs.architecture-model/1",
                "schema_identity": "commons.mncs.architecture-model/1",
                "content_identity": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
                "capabilities": []
            }),
        );
        let status = probe(Some(&root), None);
        assert_eq!(status.state, "loaded");
        assert_eq!(status.claims_state, "unavailable");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_claims_are_reported_as_architecture_drift() {
        let root = temp_root();
        let identity = "sha256:1111111111111111111111111111111111111111111111111111111111111111";
        write_model(&root, &valid_model(identity));
        let mut claims = valid_claims(identity);
        claims["capabilities"][0]["canonical_path"] = serde_json::json!("stale.mncs");
        write_claims(&root, &claims);
        let status = probe(Some(&root), None);
        assert_eq!(status.claims_state, "drift");
        assert!(status
            .errors
            .iter()
            .any(|error| error.contains("stale canonical path")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_policy_accepts_valid_claims_and_uppercase_sha256_hex() {
        let root = temp_root();
        let identity = format!("sha256:{}", "A".repeat(64));
        let model = valid_model(&identity);
        let claims = valid_claims(&identity);
        write_model(&root, &model);
        write_claims(&root, &claims);
        fs::write(root.join("canonical.mncs"), "fixture").unwrap();

        let language = language_status();
        let status = probe(Some(&root), Some(&language));
        assert_eq!(status.state, "loaded");
        assert_eq!(status.claims_state, "verified");
        assert_eq!(status.capability_count, 1);
        assert!(status.errors.is_empty(), "{:?}", status.errors);

        let provenance = DoctorMncsRuntime::production()
            .unwrap()
            .provenance()
            .unwrap();
        for entrypoint in [
            "doctor.architecture.v1::check_model",
            "doctor.architecture.v1::check_claims_header",
            "doctor.architecture.v1::check_capability",
            "doctor.architecture.v1::check_contract",
        ] {
            assert!(provenance.entrypoints.iter().any(|seen| seen == entrypoint));
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_model_policy_rejects_wrong_schema_and_malformed_identity() {
        let root = temp_root();
        write_model(
            &root,
            &serde_json::json!({
                "schema_version": "commons.mncs.architecture-model/1",
                "schema_identity": "commons.mncs.architecture-model/2",
                "content_identity": "sha256:xyz",
                "capabilities": []
            }),
        );
        let status = probe(Some(&root), None);
        assert_eq!(status.state, "invalid");
        assert!(status
            .errors
            .iter()
            .any(|error| { error.contains("unsupported Commons architecture schema") }));
        assert!(status
            .errors
            .iter()
            .any(|error| { error.contains("content_identity is missing or malformed") }));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_claims_header_reports_independent_identity_and_profile_drift() {
        let root = temp_root();
        let identity = "sha256:2222222222222222222222222222222222222222222222222222222222222222";
        write_model(&root, &valid_model(identity));
        let mut claims = valid_claims(identity);
        claims["architecture_content_identity"] = serde_json::json!("architecture-old");
        claims["language_content_identity"] = serde_json::json!("language-old");
        claims["language_profile"] = serde_json::json!("0.15");
        claims["capabilities"] = serde_json::json!([]);
        claims["contracts"] = serde_json::json!([]);
        write_claims(&root, &claims);

        let language = language_status();
        let status = probe(Some(&root), Some(&language));
        assert_eq!(status.claims_state, "drift");
        assert!(status
            .errors
            .iter()
            .any(|error| { error.contains("architecture content identity differs") }));
        assert!(status
            .errors
            .iter()
            .any(|error| { error.contains("language capability identity differs") }));
        assert!(status
            .errors
            .iter()
            .any(|error| error.contains("language profile differs")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_claims_require_capability_ids_and_known_capabilities() {
        let root = temp_root();
        let identity = "sha256:2222222222222222222222222222222222222222222222222222222222222222";
        write_model(&root, &valid_model(identity));
        let mut claims = valid_claims(identity);
        claims["capabilities"] = serde_json::json!([
            {},
            {"id": "unknown.capability"}
        ]);
        claims["contracts"] = serde_json::json!([]);
        write_claims(&root, &claims);

        let status = probe(Some(&root), None);
        assert_eq!(status.claims_state, "drift");
        assert!(status
            .errors
            .iter()
            .any(|error| error.contains("capability claims require id")));
        assert!(status
            .errors
            .iter()
            .any(|error| error.contains("capability is not in Commons: unknown.capability")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_capability_and_contract_policy_reports_all_drift_kinds() {
        let root = temp_root();
        let identity = "sha256:3333333333333333333333333333333333333333333333333333333333333333";
        write_model(&root, &valid_model(identity));
        let mut claims = valid_claims(identity);
        claims["capabilities"][0]["canonical_path"] = serde_json::json!("stale.mncs");
        claims["capabilities"][0]["active_shadow_path"] = serde_json::json!("new-legacy.mncs");
        claims["capabilities"][0]["shadow_state"] = serde_json::json!("active");
        claims["contracts"] = serde_json::json!(["missing.wire/9"]);
        write_claims(&root, &claims);

        let status = probe(Some(&root), None);
        assert_eq!(status.claims_state, "drift");
        for fragment in [
            "canonical path drift",
            "stale canonical path",
            "shadow path drift",
            "shadow state drift",
            "declared architecture contract cannot be resolved",
        ] {
            assert!(
                status.errors.iter().any(|error| error.contains(fragment)),
                "missing {fragment}: {:?}",
                status.errors
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_policy_fails_closed_when_text_exceeds_its_transport_width() {
        let root = temp_root();
        let identity = "sha256:4444444444444444444444444444444444444444444444444444444444444444";
        write_model(&root, &valid_model(identity));
        let mut claims = valid_claims(identity);
        claims["capabilities"][0]["id"] = serde_json::json!("x".repeat(65));
        write_claims(&root, &claims);

        let status = probe(Some(&root), None);
        assert_eq!(status.claims_state, "drift");
        assert!(status.errors.iter().any(|error| {
            error.contains("architecture capability values exceed native policy text bounds")
        }));
        assert!(!status
            .errors
            .iter()
            .any(|error| { error.contains("architecture capability is not in Commons") }));
        fs::remove_dir_all(root).unwrap();
    }
}
