//! Structured health model: check registry, statuses, findings.
//!
//! Health checks are structured data first and print statements second: every
//! check yields a [`CheckResult`] with findings that carry explanations and
//! suggested actions, and the whole report serializes to JSON. Human
//! rendering lives in [`crate::report`].

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::diagnostics::{Diagnostic, Severity};
use crate::discovery::{Inventory, ManifestKind};
use crate::toolchain::ToolchainStatus;
use crate::version::current_version;

/// Check outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pass,
    Warning,
    Fail,
    Skipped,
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Status::Pass => write!(f, "pass"),
            Status::Warning => write!(f, "warning"),
            Status::Fail => write!(f, "fail"),
            Status::Skipped => write!(f, "skipped"),
        }
    }
}

/// One actionable finding within a check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub severity: Severity,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub explanation: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub suggested_action: String,
}

/// Result of one registered health check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    pub id: String,
    pub title: String,
    pub status: Status,
    #[serde(default)]
    pub findings: Vec<Finding>,
}

/// All inputs a health check may inspect.
pub struct HealthContext<'a> {
    pub inventory: &'a Inventory,
    /// Per-file diagnostics keyed by relative path.
    pub diagnostics: &'a BTreeMap<String, Vec<Diagnostic>>,
    pub toolchain: &'a ToolchainStatus,
}

/// Run the full registered check suite.
pub fn run_all_checks(ctx: &HealthContext<'_>) -> Vec<CheckResult> {
    vec![
        check_source_parse_health(ctx),
        check_header_health(ctx),
        check_version_drift(ctx),
        check_newline_encoding(ctx),
        check_manifest_health(ctx),
        check_toolchain_health(ctx),
        check_language_knowledge(ctx),
        check_migration_availability(ctx),
    ]
}

/// Worst of two check statuses: fail dominates, then warning; skipped is
/// neutral and yields to any decided status.
pub fn worst_of(a: Status, b: Status) -> Status {
    match (a, b) {
        (_, Status::Fail) | (Status::Fail, _) => Status::Fail,
        (_, Status::Warning) | (Status::Warning, _) => Status::Warning,
        (Status::Skipped, s) => s,
        (s, Status::Skipped) => s,
        _ => Status::Pass,
    }
}

/// Worst status across all checks.
pub fn overall_status(results: &[CheckResult]) -> Status {
    let mut overall = Status::Pass;
    for result in results {
        overall = worst_of(overall, result.status);
    }
    overall
}

fn finding(diag: &Diagnostic, path: &str, message: Option<String>) -> Finding {
    Finding {
        severity: diag.severity,
        message: message.unwrap_or_else(|| format!("{}: {}", diag.code, diag.message)),
        path: Some(path.to_owned()),
        explanation: diag.explanation.clone(),
        suggested_action: diag.suggested_action.clone(),
    }
}

fn by_code<'a>(
    diagnostics: &'a BTreeMap<String, Vec<Diagnostic>>,
    codes: &[&str],
) -> Vec<(&'a String, &'a Diagnostic)> {
    let mut out = Vec::new();
    for (path, diags) in diagnostics {
        for diag in diags {
            if codes.contains(&diag.code.as_str()) {
                out.push((path, diag));
            }
        }
    }
    out
}

fn check_source_parse_health(ctx: &HealthContext<'_>) -> CheckResult {
    let hits = by_code(ctx.diagnostics, &["DOC106", "DOC108"]);
    let mut findings: Vec<Finding> = hits
        .iter()
        .map(|(path, diag)| finding(diag, path, None))
        .collect();
    findings.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then_with(|| b.severity.cmp(&a.severity))
    });
    let status = if findings.iter().any(|f| f.severity == Severity::Error) {
        Status::Fail
    } else if findings.is_empty() {
        Status::Pass
    } else {
        Status::Warning
    };
    CheckResult {
        id: "source-parse-health".to_owned(),
        title: "Source parse health".to_owned(),
        status,
        findings,
    }
}

fn check_header_health(ctx: &HealthContext<'_>) -> CheckResult {
    let hits = by_code(ctx.diagnostics, &["DOC101", "DOC102", "DOC105"]);
    let mut findings: Vec<Finding> = hits
        .iter()
        .map(|(path, diag)| finding(diag, path, None))
        .collect();
    findings.sort_by(|a, b| a.path.cmp(&b.path));
    let status = if findings.iter().any(|f| f.severity == Severity::Error) {
        Status::Fail
    } else if findings.is_empty() {
        Status::Pass
    } else {
        Status::Warning
    };
    CheckResult {
        id: "header-health".to_owned(),
        title: "Source header and module health".to_owned(),
        status,
        findings,
    }
}

fn check_version_drift(ctx: &HealthContext<'_>) -> CheckResult {
    let outdated = by_code(ctx.diagnostics, &["DOC103"]);
    let unsupported = by_code(ctx.diagnostics, &["DOC102", "DOC104"]);
    let mut findings: Vec<Finding> = unsupported
        .iter()
        .map(|(path, diag)| finding(diag, path, None))
        .collect();
    for (path, diag) in &outdated {
        findings.push(finding(
            diag,
            path,
            Some(format!(
                "{} declares sealed profile {}; current is {}",
                path,
                diag.language_version
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "?".to_owned()),
                current_version()
            )),
        ));
    }
    findings.sort_by(|a, b| a.path.cmp(&b.path));
    let status = if findings.iter().any(|f| f.severity == Severity::Error) {
        Status::Fail
    } else if findings.is_empty() {
        Status::Pass
    } else {
        Status::Warning
    };
    CheckResult {
        id: "version-drift".to_owned(),
        title: "Language version drift".to_owned(),
        status,
        findings,
    }
}

fn check_newline_encoding(ctx: &HealthContext<'_>) -> CheckResult {
    let mut findings = Vec::new();
    for source in &ctx.inventory.sources {
        if source.text.is_none() {
            findings.push(Finding {
                severity: Severity::Error,
                message: "file is not valid UTF-8".to_owned(),
                path: Some(source.relative.clone()),
                explanation: "Cannot be analyzed or safely rewritten.".to_owned(),
                suggested_action: "Re-encode as UTF-8.".to_owned(),
            });
        }
        if source.has_bom {
            findings.push(Finding {
                severity: Severity::Warning,
                message: "UTF-8 BOM present".to_owned(),
                path: Some(source.relative.clone()),
                explanation: "Risks confusing header recognition.".to_owned(),
                suggested_action: "Remove the BOM (`fix` handles this as safe).".to_owned(),
            });
        }
        if source.newline == crate::discovery::NewlineStyle::Mixed {
            findings.push(Finding {
                severity: Severity::Warning,
                message: "mixed line endings".to_owned(),
                path: Some(source.relative.clone()),
                explanation: "Mixed LF/CRLF endings complicate diffs and spans.".to_owned(),
                suggested_action: "Normalize to LF.".to_owned(),
            });
        }
    }
    findings.sort_by(|a, b| a.path.cmp(&b.path));
    let status = if findings.iter().any(|f| f.severity == Severity::Error) {
        Status::Fail
    } else if findings.is_empty() {
        Status::Pass
    } else {
        Status::Warning
    };
    CheckResult {
        id: "newline-encoding".to_owned(),
        title: "Newline and encoding health".to_owned(),
        status,
        findings,
    }
}

fn check_manifest_health(ctx: &HealthContext<'_>) -> CheckResult {
    let mut findings = Vec::new();
    for manifest in &ctx.inventory.manifests {
        match manifest.kind {
            ManifestKind::ManifestJson => {
                let bytes = std::fs::read(&manifest.path).unwrap_or_default();
                if serde_json::from_slice::<serde_json::Value>(&bytes).is_err() {
                    findings.push(Finding {
                        severity: Severity::Error,
                        message: "manifest JSON does not parse".to_owned(),
                        path: Some(manifest.relative.clone()),
                        explanation: "Sidecar manifest is not valid JSON.".to_owned(),
                        suggested_action: "Fix the JSON syntax.".to_owned(),
                    });
                }
            }
            ManifestKind::Forge | ManifestKind::Workspace => {
                let text = std::fs::read_to_string(&manifest.path).unwrap_or_default();
                if !text.contains("version") {
                    findings.push(Finding {
                        severity: Severity::Warning,
                        message: "workspace manifest has no version field".to_owned(),
                        path: Some(manifest.relative.clone()),
                        explanation: "Versionless workspace files cannot be checked \
                            for metadata upgrades."
                            .to_owned(),
                        suggested_action: "Add the workspace `version` field.".to_owned(),
                    });
                }
            }
            ManifestKind::Cargo => {}
        }
    }
    findings.extend(check_family_repository_manifest(ctx).findings);
    findings.sort_by(|a, b| a.path.cmp(&b.path));
    let status = if findings.iter().any(|f| f.severity == Severity::Error) {
        Status::Fail
    } else if findings.is_empty() {
        Status::Pass
    } else {
        Status::Warning
    };
    CheckResult {
        id: "manifest-health".to_owned(),
        title: "Project metadata health".to_owned(),
        status,
        findings,
    }
}

/// Validate the repository-owned MNCS family manifest without reimplementing
/// the Standard's full JSON Schema.  The Standard remains the schema
/// authority; Doctor checks the binding facts that affect local freshness and
/// safe context entry.
fn check_family_repository_manifest(ctx: &HealthContext<'_>) -> CheckResult {
    let relative = ".mncs/project.json";
    let path = ctx.inventory.root.join(relative);
    if !path.is_file() {
        return CheckResult {
            id: "family-repository-manifest".to_owned(),
            title: "Family repository manifest".to_owned(),
            status: Status::Skipped,
            findings: Vec::new(),
        };
    }

    let mut findings = Vec::new();
    let value = match std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
    {
        Some(value) => value,
        None => {
            findings.push(Finding {
                severity: Severity::Error,
                message: "family repository manifest is not valid JSON".to_owned(),
                path: Some(relative.to_owned()),
                explanation:
                    "The repository-owned manifest cannot be used as bounded family context."
                        .to_owned(),
                suggested_action:
                    "Repair .mncs/project.json and validate it against the MNCS Standard schema."
                        .to_owned(),
            });
            return CheckResult {
                id: "family-repository-manifest".to_owned(),
                title: "Family repository manifest".to_owned(),
                status: Status::Fail,
                findings,
            };
        }
    };

    let object = match value.as_object() {
        Some(object) => object,
        None => {
            findings.push(Finding {
                severity: Severity::Error,
                message: "family repository manifest must be a JSON object".to_owned(),
                path: Some(relative.to_owned()),
                explanation: "The family-manifest contract is object-shaped.".to_owned(),
                suggested_action: "Use the MNCS Standard repository-manifest schema.".to_owned(),
            });
            return CheckResult {
                id: "family-repository-manifest".to_owned(),
                title: "Family repository manifest".to_owned(),
                status: Status::Fail,
                findings,
            };
        }
    };

    if object.get("schema_version").and_then(|v| v.as_str())
        != Some("mncs-family.repository-manifest/v0alpha1")
    {
        findings.push(manifest_finding(
            Severity::Error,
            "family repository manifest schema is unsupported",
            "Doctor only accepts the existing Standard-owned family-manifest contract.",
            "Use mncs-family.repository-manifest/v0alpha1; do not introduce a parallel manifest.",
        ));
    }
    if object
        .get("repository")
        .and_then(|v| v.as_str())
        .is_none_or(str::is_empty)
    {
        findings.push(manifest_finding(
            Severity::Error,
            "family repository manifest has no repository identity",
            "A bounded context packet needs a stable repository identifier.",
            "Set the repository field to the repository's canonical family identity.",
        ));
    }
    if object
        .get("revision")
        .and_then(|v| v.as_u64())
        .is_none_or(|revision| revision == 0)
    {
        findings.push(manifest_finding(
            Severity::Error,
            "family repository manifest revision is missing or invalid",
            "Manifest revisions make repository-owned declarations auditable.",
            "Set revision to a positive integer and increment it for declaration changes.",
        ));
    }

    match object.get("contracts").and_then(|v| v.as_object()) {
        Some(contracts) => {
            for key in ["provides", "consumes", "tests"] {
                if !contracts.get(key).is_some_and(serde_json::Value::is_array) {
                    findings.push(manifest_finding(
                        Severity::Error,
                        &format!("family repository manifest contracts.{key} is not an array"),
                        "The existing manifest contract keeps declared contracts and evidence structurally queryable.",
                        "Use arrays for provides, consumes, and tests.",
                    ));
                }
            }
            validate_fingerprint_sources(&mut findings, &path, contracts);
        }
        None => findings.push(manifest_finding(
            Severity::Error,
            "family repository manifest has no contracts object",
            "Context consumers cannot determine declared providers or evidence obligations.",
            "Add the contracts object required by the MNCS Standard manifest schema.",
        )),
    }

    if let Some(organization) = object.get("organization") {
        validate_organization(&mut findings, organization);
        validate_generated_region_files(
            &mut findings,
            &ctx.inventory.root,
            organization,
            ctx.toolchain,
        );
    }

    findings.sort_by(|a, b| a.message.cmp(&b.message));
    let status = if findings.iter().any(|f| f.severity == Severity::Error) {
        Status::Fail
    } else if findings.iter().any(|f| f.severity == Severity::Warning) {
        Status::Warning
    } else {
        Status::Pass
    };
    CheckResult {
        id: "family-repository-manifest".to_owned(),
        title: "Family repository manifest".to_owned(),
        status,
        findings,
    }
}

fn manifest_finding(
    severity: Severity,
    message: &str,
    explanation: &str,
    suggested_action: &str,
) -> Finding {
    Finding {
        severity,
        message: message.to_owned(),
        path: Some(".mncs/project.json".to_owned()),
        explanation: explanation.to_owned(),
        suggested_action: suggested_action.to_owned(),
    }
}

fn validate_fingerprint_sources(
    findings: &mut Vec<Finding>,
    manifest_path: &Path,
    contracts: &serde_json::Map<String, serde_json::Value>,
) {
    for (section, value) in contracts {
        let Some(entries) = value.as_array() else {
            continue;
        };
        for entry in entries {
            let Some(sources) = entry.get("fingerprint_sources").and_then(|v| v.as_array()) else {
                continue;
            };
            let Some(root) = manifest_path.parent().and_then(Path::parent) else {
                continue;
            };
            for source in sources.iter().filter_map(|v| v.as_str()) {
                if source.trim().is_empty() {
                    continue;
                }
                if !root.join(source).exists() {
                    findings.push(Finding {
                        severity: Severity::Error,
                        message: format!("manifest {section} fingerprint source is missing: {source}"),
                        path: Some(".mncs/project.json".to_owned()),
                        explanation: "A declared source cannot contribute a reproducible repository identity.".to_owned(),
                        suggested_action: "Correct the path or remove the stale contract declaration.".to_owned(),
                    });
                }
            }
        }
    }
}

fn validate_organization(findings: &mut Vec<Finding>, organization: &serde_json::Value) {
    let Some(organization) = organization.as_object() else {
        findings.push(manifest_finding(
            Severity::Error,
            "manifest organization must be an object",
            "Repository layout metadata is an optional object extension of the Standard manifest.",
            "Use the organization object shape from the Standard repository-organization contract.",
        ));
        return;
    };
    if organization.get("layout").and_then(|v| v.as_str()) != Some("mncs.repository-layout/1") {
        findings.push(manifest_finding(
            Severity::Error,
            "manifest organization layout is unsupported",
            "A stable layout identity lets tools interpret optional surface classifications.",
            "Set organization.layout to mncs.repository-layout/1.",
        ));
    }
    const CLASSES: &[&str] = &[
        "canonical",
        "host-boundary",
        "compatibility",
        "differential-oracle",
        "reference",
        "migration-shadow",
        "generated",
        "historical",
    ];
    if let Some(surfaces) = organization.get("surfaces").and_then(|v| v.as_array()) {
        for surface in surfaces {
            let Some(surface) = surface.as_object() else {
                findings.push(manifest_finding(
                    Severity::Error,
                    "manifest organization surface is not an object",
                    "Surface classifications must be individually queryable.",
                    "Use path and class fields for each organization surface.",
                ));
                continue;
            };
            let class = surface.get("class").and_then(|v| v.as_str());
            let path = surface.get("path").and_then(|v| v.as_str());
            if path.is_none_or(str::is_empty)
                || !class.is_some_and(|class| CLASSES.contains(&class))
            {
                findings.push(manifest_finding(
                    Severity::Error,
                    "manifest organization surface has an invalid path or class",
                    "Only the Standard-defined artifact classes may shape bounded repository context.",
                    "Set a non-empty path and one of the eight repository-layout artifact classes.",
                ));
            }
        }
    }
    if let Some(regions) = organization
        .get("generated_regions")
        .and_then(|v| v.as_array())
    {
        for region in regions {
            let valid = region.as_object().is_some_and(|region| {
                ["path", "begin", "end", "source"].iter().all(|key| {
                    region
                        .get(*key)
                        .and_then(|v| v.as_str())
                        .is_some_and(|s| !s.is_empty())
                })
            });
            if !valid {
                findings.push(manifest_finding(
                    Severity::Error,
                    "manifest generated region is incomplete",
                    "Generated projections need explicit bounded markers and an authoritative source.",
                    "Provide non-empty path, begin, end, and source fields.",
                ));
            }
        }
    }
}

fn validate_generated_region_files(
    findings: &mut Vec<Finding>,
    root: &Path,
    organization: &serde_json::Value,
    toolchain: &ToolchainStatus,
) {
    let Some(regions) = organization
        .get("generated_regions")
        .and_then(|value| value.as_array())
    else {
        return;
    };
    for region in regions {
        let Some(region) = region.as_object() else {
            continue;
        };
        let Some(relative) = region.get("path").and_then(|value| value.as_str()) else {
            continue;
        };
        let path = Path::new(relative);
        if path.is_absolute()
            || path
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            findings.push(manifest_finding(
                Severity::Error,
                "manifest generated region escapes the repository root",
                "Generated projections must be bounded to repository-owned paths.",
                "Use a relative path without parent-directory components.",
            ));
            continue;
        }
        let target = root.join(path);
        let text = match std::fs::read_to_string(&target) {
            Ok(text) => text,
            Err(error) => {
                findings.push(Finding {
                    severity: Severity::Error,
                    message: format!("generated projection is missing or unreadable: {relative}"),
                    path: Some(relative.to_owned()),
                    explanation: format!(
                        "Doctor could not inspect the declared generated region: {error}"
                    ),
                    suggested_action: "Regenerate the projection or correct its declared path."
                        .to_owned(),
                });
                continue;
            }
        };
        let begin = region.get("begin").and_then(|value| value.as_str());
        let end = region.get("end").and_then(|value| value.as_str());
        let (Some(begin), Some(end)) = (begin, end) else {
            continue;
        };
        let Some(begin_offset) = text.find(begin) else {
            findings.push(generated_region_finding(
                Severity::Error,
                relative,
                "generated region begin marker is absent",
                "Run the owning deterministic projection generator.",
            ));
            continue;
        };
        let Some(end_offset) = text.find(end) else {
            findings.push(generated_region_finding(
                Severity::Error,
                relative,
                "generated region end marker is absent",
                "Run the owning deterministic projection generator.",
            ));
            continue;
        };
        if end_offset < begin_offset {
            findings.push(generated_region_finding(
                Severity::Error,
                relative,
                "generated region markers are out of order",
                "Restore the bounded generated region before checking freshness.",
            ));
            continue;
        }
        let generated = &text[begin_offset..end_offset];
        if generated.contains("Language profile:") {
            match toolchain
                .language_knowledge
                .as_ref()
                .and_then(|knowledge| knowledge.content_identity.as_deref())
            {
                Some(identity) if !generated.contains(identity) => {
                    findings.push(generated_region_finding(
                        Severity::Error,
                        relative,
                        "generated language identity is stale",
                        "Regenerate the projection from the current language capability index.",
                    ))
                }
                None => findings.push(generated_region_finding(
                    Severity::Info,
                    relative,
                    "generated language identity could not be checked",
                    "Provide current mncs-language knowledge before claiming freshness.",
                )),
                _ => {}
            }
        }
        if generated.contains("Commons architecture identity:") {
            match toolchain
                .architecture_knowledge
                .as_ref()
                .and_then(|knowledge| knowledge.content_identity.as_deref())
            {
                Some(identity) if !generated.contains(identity) => {
                    findings.push(generated_region_finding(
                        Severity::Error,
                        relative,
                        "generated Commons architecture identity is stale",
                        "Regenerate the projection from the current Commons architecture model.",
                    ))
                }
                None => findings.push(generated_region_finding(
                    Severity::Info,
                    relative,
                    "generated Commons architecture identity could not be checked",
                    "Provide current Commons architecture knowledge before claiming freshness.",
                )),
                _ => {}
            }
        }
    }
}

fn generated_region_finding(
    severity: Severity,
    path: &str,
    message: &str,
    suggested_action: &str,
) -> Finding {
    Finding {
        severity,
        message: message.to_owned(),
        path: Some(path.to_owned()),
        explanation: "Generated Markdown is a projection, not an independent source of truth."
            .to_owned(),
        suggested_action: suggested_action.to_owned(),
    }
}

fn check_toolchain_health(ctx: &HealthContext<'_>) -> CheckResult {
    let mut findings = Vec::new();
    if ctx.toolchain.rust_cli.is_none() && ctx.toolchain.family_cli.is_none() {
        findings.push(Finding {
            severity: Severity::Warning,
            message: "no MNCS toolchain CLI found".to_owned(),
            path: None,
            explanation: "Neither the Rust language CLI (MNCS_CLI) nor the family \
                validator (`mncs`) was found. Doctor runs hygiene checks only; \
                semantic verification is unavailable."
                .to_owned(),
            suggested_action: "Install the toolchain or set MNCS_CLI.".to_owned(),
        });
    }
    if ctx.toolchain.cargo.is_none() {
        findings.push(Finding {
            severity: Severity::Info,
            message: "cargo not found; Rust-based verification unavailable".to_owned(),
            path: None,
            explanation: String::new(),
            suggested_action: String::new(),
        });
    }
    if ctx.toolchain.test_provider.is_none() {
        findings.push(Finding {
            severity: Severity::Info,
            message: "mncs-test provider not found".to_owned(),
            path: None,
            explanation: "The canonical first-class test provider is optional for Doctor's local hygiene checks; canonical test verification is unavailable until it is installed or MNCS_TEST_BIN is set.".to_owned(),
            suggested_action: "Install mncs-test or set MNCS_TEST_BIN when requesting test verification.".to_owned(),
        });
    }
    if ctx.toolchain.debug_provider.is_none() {
        findings.push(Finding {
            severity: Severity::Info,
            message: "mncs-debug provider not found".to_owned(),
            path: None,
            explanation: "Failure witnesses and structured traces are optional diagnostics; their absence must not change a test verdict.".to_owned(),
            suggested_action: "Install mncs-debug or set MNCS_DEBUG_BIN for failure diagnostics.".to_owned(),
        });
    }
    if let Some(protocol) = &ctx.toolchain.debug_protocol {
        if !protocol.compatible {
            findings.push(Finding {
                severity: Severity::Warning,
                message: "mncs-debug protocol compatibility is unavailable".to_owned(),
                path: Some(protocol.expected.clone()),
                explanation: format!(
                    "Expected {}, observed {}. Doctor will not infer debugger semantics from an incompatible provider.",
                    protocol.expected,
                    protocol.observed.as_deref().unwrap_or("no structured capabilities response")
                ),
                suggested_action: "Use the registered mncs-debug provider revision and verify its capabilities response.".to_owned(),
            });
        }
    }
    findings.extend(check_architecture_knowledge(ctx).findings);
    let status = if findings.iter().any(|f| f.severity == Severity::Error) {
        Status::Fail
    } else if findings.iter().any(|f| f.severity == Severity::Warning) {
        Status::Warning
    } else {
        Status::Pass
    };
    CheckResult {
        id: "toolchain-health".to_owned(),
        title: "Toolchain and architecture compatibility".to_owned(),
        status,
        findings,
    }
}

fn check_language_knowledge(ctx: &HealthContext<'_>) -> CheckResult {
    let mut findings = Vec::new();
    let status = match ctx.toolchain.language_knowledge.as_ref() {
        None => {
            findings.push(Finding {
                severity: Severity::Info,
                message: "authoritative language capability index was not probed".to_owned(),
                path: None,
                explanation: "Doctor cannot validate profile/compiler agreement without the shared mncs-language projection.".to_owned(),
                suggested_action: "Build or locate mncs-language/docs/language-capabilities.json.".to_owned(),
            });
            Status::Pass
        }
        Some(knowledge) if knowledge.state == "invalid" || knowledge.freshness == "stale" => {
            findings.push(Finding {
                severity: Severity::Error,
                message: "authoritative language capability index is stale or invalid".to_owned(),
                path: knowledge.source_path.clone(),
                explanation: knowledge
                    .error
                    .clone()
                    .unwrap_or_else(|| format!("stale source paths: {:?}", knowledge.stale_paths)),
                suggested_action: "Regenerate the index from mncs-language and rerun Doctor."
                    .to_owned(),
            });
            Status::Fail
        }
        Some(knowledge) if knowledge.freshness == "verified" => Status::Pass,
        Some(knowledge) => {
            findings.push(Finding {
                severity: Severity::Info,
                message: "authoritative language capability index could not verify its sources".to_owned(),
                path: knowledge.source_path.clone(),
                explanation: knowledge.error.clone().unwrap_or_else(|| "The index is usable but its provenance sources are not available in this checkout.".to_owned()),
                suggested_action: "Make the matching mncs-language checkout available for provenance verification.".to_owned(),
            });
            Status::Pass
        }
    };
    CheckResult {
        id: "language-knowledge".to_owned(),
        title: "Authoritative language knowledge".to_owned(),
        status,
        findings,
    }
}

fn check_architecture_knowledge(ctx: &HealthContext<'_>) -> CheckResult {
    let mut findings = Vec::new();
    let status = match ctx.toolchain.architecture_knowledge.as_ref() {
        None => {
            findings.push(Finding {
                severity: Severity::Info,
                message: "Commons architecture projection was not probed".to_owned(),
                path: None,
                explanation:
                    "Doctor cannot compare family ownership or shadow state without Commons facts."
                        .to_owned(),
                suggested_action: "Make MNCS-Commons available or set MNCS_COMMONS_ROOT."
                    .to_owned(),
            });
            Status::Skipped
        }
        Some(architecture)
            if architecture.state == "invalid" || !architecture.errors.is_empty() =>
        {
            for error in &architecture.errors {
                findings.push(Finding {
                    severity: Severity::Error,
                    message: "architecture drift or invalid Commons projection".to_owned(),
                    path: architecture.source_path.clone(),
                    explanation: error.clone(),
                    suggested_action:
                        "Refresh Commons architecture facts and the project's architecture claims."
                            .to_owned(),
                });
            }
            Status::Fail
        }
        Some(architecture) if architecture.claims_state == "verified" => Status::Pass,
        Some(architecture) => {
            findings.push(Finding {
                severity: Severity::Info,
                message: "Commons architecture facts loaded without local project claims".to_owned(),
                path: architecture.source_path.clone(),
                explanation: architecture
                    .warnings
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "No .mncs/architecture-claims.json was available.".to_owned()),
                suggested_action: "Add project architecture claims when a repository declares family-owned capabilities.".to_owned(),
            });
            Status::Skipped
        }
    };
    CheckResult {
        id: "architecture-knowledge".to_owned(),
        title: "Commons architecture drift".to_owned(),
        status,
        findings,
    }
}

fn check_migration_availability(ctx: &HealthContext<'_>) -> CheckResult {
    let outdated = by_code(ctx.diagnostics, &["DOC103"]).len();
    let unmigratable = by_code(ctx.diagnostics, &["DOC102", "DOC104"]).len();
    let mut findings = Vec::new();
    if outdated > 0 {
        findings.push(Finding {
            severity: Severity::Warning,
            message: format!(
                "{outdated} file(s) on sealed profiles; transition knowledge is \
                unrecorded upstream, so migration is plan-only until rules land"
            ),
            path: None,
            explanation: "Run `migrate --plan` to inspect per-file paths.".to_owned(),
            suggested_action: "Review `migrate --plan` output.".to_owned(),
        });
    }
    if unmigratable > 0 {
        findings.push(Finding {
            severity: Severity::Error,
            message: format!(
                "{unmigratable} file(s) declare unknown/unsupported profiles and \
                cannot be placed on any migration path"
            ),
            path: None,
            explanation: "Fix headers to known profiles first.".to_owned(),
            suggested_action: "Correct the headers.".to_owned(),
        });
    }
    let status = if unmigratable > 0 {
        Status::Fail
    } else if outdated > 0 {
        Status::Warning
    } else {
        Status::Pass
    };
    CheckResult {
        id: "migration-availability".to_owned(),
        title: "Migration availability".to_owned(),
        status,
        findings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::{DiagnosticSource, Span};
    use crate::discovery::{DiscoveryOptions, Inventory};
    use crate::fix::Applicability;
    use std::path::PathBuf;

    fn empty_ctx() -> (
        Inventory,
        BTreeMap<String, Vec<Diagnostic>>,
        ToolchainStatus,
    ) {
        (
            Inventory {
                root: PathBuf::from("/x"),
                sources: Vec::new(),
                manifests: Vec::new(),
                skipped: Vec::new(),
                extension_counts: Default::default(),
                topology: Default::default(),
                metrics: Default::default(),
            },
            BTreeMap::new(),
            ToolchainStatus::default(),
        )
    }

    #[test]
    fn empty_workspace_passes_with_toolchain_note() {
        let (inv, diags, tc) = empty_ctx();
        let ctx = HealthContext {
            inventory: &inv,
            diagnostics: &diags,
            toolchain: &tc,
        };
        let results = run_all_checks(&ctx);
        assert_eq!(results.len(), 8);
        // No toolchain installed in test env guarantee: toolchain check is
        // warning at worst here, never fail.
        assert!(results.iter().all(|r| r.status != Status::Fail));
        let _ = DiscoveryOptions::default();
    }

    #[test]
    fn missing_optional_family_providers_is_informational() {
        let (inv, diags, tc) = empty_ctx();
        let ctx = HealthContext {
            inventory: &inv,
            diagnostics: &diags,
            toolchain: &tc,
        };
        let check = run_all_checks(&ctx)
            .into_iter()
            .find(|result| result.id == "toolchain-health")
            .expect("toolchain check");
        assert_eq!(check.status, Status::Warning);
        assert!(check
            .findings
            .iter()
            .any(|finding| finding.message.contains("mncs-debug")));
    }

    #[test]
    fn error_diagnostic_fails_its_check() {
        let (inv, mut diags, tc) = empty_ctx();
        diags.insert(
            "a.mncs".to_owned(),
            vec![Diagnostic {
                code: "DOC106".to_owned(),
                severity: Severity::Error,
                source: DiagnosticSource::Hygiene,
                span: Span::whole_file(10, 2),
                message: "unbalanced".to_owned(),
                explanation: String::new(),
                applicability: Applicability::Manual,
                suggested_action: String::new(),
                language_version: None,
                migration_transition: None,
                backend: "scanner".to_owned(),
            }],
        );
        let ctx = HealthContext {
            inventory: &inv,
            diagnostics: &diags,
            toolchain: &tc,
        };
        let results = run_all_checks(&ctx);
        let parse = results
            .iter()
            .find(|r| r.id == "source-parse-health")
            .unwrap();
        assert_eq!(parse.status, Status::Fail);
        assert_eq!(overall_status(&results), Status::Fail);
    }

    #[test]
    fn family_manifest_checks_local_identity_and_fingerprint_sources() {
        let root = std::env::temp_dir().join(format!(
            "mncs-doctor-family-manifest-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".mncs")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join(".mncs/project.json"),
            r#"{
              "schema_version": "mncs-family.repository-manifest/v0alpha1",
              "repository": "fixture",
              "revision": 1,
              "manifest_location": "in-repository",
              "organization": {"layout": "mncs.repository-layout/1", "surfaces": [{"path": "src/", "class": "canonical"}]},
              "contracts": {
                "provides": [{"contract": "fixture", "version": "1", "kind": "test", "stability": "experimental", "fingerprint_sources": ["src/"]}],
                "consumes": [],
                "tests": []
              }
            }"#,
        )
        .unwrap();
        let inventory = Inventory {
            root: root.clone(),
            sources: Vec::new(),
            manifests: Vec::new(),
            skipped: Vec::new(),
            extension_counts: Default::default(),
            topology: Default::default(),
            metrics: Default::default(),
        };
        let diagnostics = BTreeMap::new();
        let toolchain = ToolchainStatus::default();
        let check = check_family_repository_manifest(&HealthContext {
            inventory: &inventory,
            diagnostics: &diagnostics,
            toolchain: &toolchain,
        });
        assert_eq!(check.status, Status::Pass);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn family_manifest_rejects_missing_fingerprint_source() {
        let root = std::env::temp_dir().join(format!(
            "mncs-doctor-family-manifest-missing-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".mncs")).unwrap();
        std::fs::write(
            root.join(".mncs/project.json"),
            r#"{
              "schema_version": "mncs-family.repository-manifest/v0alpha1",
              "repository": "fixture",
              "revision": 1,
              "manifest_location": "in-repository",
              "contracts": {
                "provides": [{"contract": "fixture", "version": "1", "kind": "test", "stability": "experimental", "fingerprint_sources": ["missing/"]}],
                "consumes": [],
                "tests": []
              }
            }"#,
        )
        .unwrap();
        let inventory = Inventory {
            root: root.clone(),
            sources: Vec::new(),
            manifests: Vec::new(),
            skipped: Vec::new(),
            extension_counts: Default::default(),
            topology: Default::default(),
            metrics: Default::default(),
        };
        let diagnostics = BTreeMap::new();
        let toolchain = ToolchainStatus::default();
        let check = check_family_repository_manifest(&HealthContext {
            inventory: &inventory,
            diagnostics: &diagnostics,
            toolchain: &toolchain,
        });
        assert_eq!(check.status, Status::Fail);
        assert!(check
            .findings
            .iter()
            .any(|finding| finding.message.contains("fingerprint source is missing")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn family_manifest_detects_missing_generated_region_markers() {
        let root = std::env::temp_dir().join(format!(
            "mncs-doctor-generated-region-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".mncs")).unwrap();
        std::fs::write(root.join("README.md"), "# Human prose\n").unwrap();
        std::fs::write(
            root.join(".mncs/project.json"),
            r#"{
              "schema_version": "mncs-family.repository-manifest/v0alpha1",
              "repository": "fixture",
              "revision": 1,
              "manifest_location": "in-repository",
              "organization": {
                "layout": "mncs.repository-layout/1",
                "generated_regions": [{"path": "README.md", "begin": "<!-- begin -->", "end": "<!-- end -->", "source": "family context"}]
              },
              "contracts": {"provides": [], "consumes": [], "tests": []}
            }"#,
        )
        .unwrap();
        let inventory = Inventory {
            root: root.clone(),
            sources: Vec::new(),
            manifests: Vec::new(),
            skipped: Vec::new(),
            extension_counts: Default::default(),
            topology: Default::default(),
            metrics: Default::default(),
        };
        let diagnostics = BTreeMap::new();
        let toolchain = ToolchainStatus::default();
        let check = check_family_repository_manifest(&HealthContext {
            inventory: &inventory,
            diagnostics: &diagnostics,
            toolchain: &toolchain,
        });
        assert_eq!(check.status, Status::Fail);
        assert!(check
            .findings
            .iter()
            .any(|finding| finding.message.contains("begin marker is absent")));
        let _ = std::fs::remove_dir_all(root);
    }
}
