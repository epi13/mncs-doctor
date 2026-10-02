//! Standard-library composition knowledge (Stage F).
//!
//! Doctor validates the selected language/compiler/stdlib composition from
//! machine-readable facts owned by `mncs-stdlib`: the compatibility
//! manifest (`stdlib-manifest.json`) and the content-addressed bundle pin.
//! This module probes the stdlib checkout, verifies manifest/bundle/tree
//! consistency, and records the facts the `stdlib-health` check reports on.
//! It is inspect-only: regeneration belongs to the stdlib repository.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MANIFEST_SCHEMA: &str = "mncs.stdlib-manifest/1";
const BUNDLE_SCHEMA: &str = "mncs.stdlib-bundle/1";

/// Verified view of the selected standard library.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StdlibStatus {
    pub schema_version: String,
    /// `ready` (verified), `missing` (no checkout found), or `invalid`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_profile_min: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_profile_max: Option<String>,
    #[serde(default)]
    pub module_count: usize,
    /// Manifest modules (name + declared profile) for import resolution.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<StdlibModule>,
    /// Modules whose tree bytes disagree with the manifest (stale pin).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stale_paths: Vec<String>,
    /// Manifest imports naming modules absent from the manifest.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved_imports: Vec<String>,
    /// Import cycles within the manifest (each a `a -> b -> a` chain).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cycles: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One manifest module for import resolution and reporting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StdlibModule {
    pub name: String,
    pub profile: String,
}

fn missing(reason: &str) -> StdlibStatus {
    StdlibStatus {
        schema_version: "mncs.doctor.stdlib-status/1".to_owned(),
        state: "missing".to_owned(),
        root: None,
        manifest_path: None,
        bundle_identity: None,
        requires_profile_min: None,
        requires_profile_max: None,
        module_count: 0,
        modules: Vec::new(),
        stale_paths: Vec::new(),
        unresolved_imports: Vec::new(),
        cycles: Vec::new(),
        error: Some(reason.to_owned()),
    }
}

fn invalid(root: &Path, reason: String) -> StdlibStatus {
    StdlibStatus {
        schema_version: "mncs.doctor.stdlib-status/1".to_owned(),
        state: "invalid".to_owned(),
        root: Some(root.to_string_lossy().into_owned()),
        manifest_path: None,
        bundle_identity: None,
        requires_profile_min: None,
        requires_profile_max: None,
        module_count: 0,
        modules: Vec::new(),
        stale_paths: Vec::new(),
        unresolved_imports: Vec::new(),
        cycles: Vec::new(),
        error: Some(reason),
    }
}

/// Candidate stdlib checkouts, in precedence order: explicit
/// `MNCS_STDLIB_ROOT`, the `mncs-stdlib` sibling of the selected
/// language checkout, then family-layout siblings of the project root
/// and working directory.
pub fn discover(root: Option<&Path>, language_source: Option<&str>) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("MNCS_STDLIB_ROOT") {
        let path = PathBuf::from(path);
        if !path.as_os_str().is_empty() {
            candidates.push(path);
        }
    }
    if let Some(source) = language_source {
        // `<language>/docs/language-capabilities.json` -> sibling checkout.
        let path = Path::new(source);
        if let Some(docs) = path.parent() {
            if let Some(language) = docs.parent() {
                if let Some(family) = language.parent() {
                    candidates.push(family.join("mncs-stdlib"));
                }
            }
        }
    }
    for anchor in [root, env::current_dir().ok().as_deref()]
        .into_iter()
        .flatten()
    {
        candidates.push(anchor.join("mncs-stdlib"));
        if let Some(parent) = anchor.parent() {
            candidates.push(parent.join("mncs-stdlib"));
        }
    }
    let mut seen = BTreeSet::new();
    candidates.into_iter().find(|path| {
        seen.insert(path.clone())
            && path.join("stdlib-manifest.json").is_file()
            && path.join("library").is_dir()
    })
}

fn bundle_identity_for(modules: &[(&str, &str)]) -> String {
    let mut ordered: Vec<(&str, &str)> = modules.to_vec();
    ordered.sort();
    let mut digest = Sha256::new();
    for (name, content) in ordered {
        digest.update(name.as_bytes());
        digest.update(b"\x00");
        digest.update(content.as_bytes());
        digest.update(b"\n");
    }
    format!("mncs:stdlib-bundle:{:x}", digest.finalize())
}

/// Probe and verify the selected stdlib checkout.
pub fn probe(root: Option<&Path>, language_source: Option<&str>) -> StdlibStatus {
    let Some(checkout) = discover(root, language_source) else {
        return missing("no mncs-stdlib checkout found (MNCS_STDLIB_ROOT or a family sibling)");
    };
    let manifest_path = checkout.join("stdlib-manifest.json");
    let text = match fs::read_to_string(&manifest_path) {
        Ok(text) => text,
        Err(error) => return invalid(&checkout, format!("cannot read manifest: {error}")),
    };
    let manifest: serde_json::Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => return invalid(&checkout, format!("manifest is not JSON: {error}")),
    };
    if manifest.get("schema_version").and_then(|v| v.as_str()) != Some(MANIFEST_SCHEMA) {
        return invalid(
            &checkout,
            format!("unsupported manifest schema (want {MANIFEST_SCHEMA})"),
        );
    }
    let modules = manifest
        .get("modules")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let bundle_identity = manifest
        .get("bundle_identity")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let requires = manifest.get("requires_profile");
    let requires_min = requires
        .and_then(|v| v.get("min"))
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let requires_max = requires
        .and_then(|v| v.get("max"))
        .and_then(|v| v.as_str())
        .map(str::to_owned);

    let mut stale_paths = Vec::new();
    let mut unresolved_imports = Vec::new();
    let mut cycles = Vec::new();

    // Manifest entries must match tree bytes exactly.
    let mut names = BTreeSet::new();
    let mut digests: BTreeMap<String, String> = BTreeMap::new();
    let mut summaries = Vec::new();
    for module in &modules {
        let name = module.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        let rel = module.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let want = module
            .get("content_sha256")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let profile = module
            .get("profile")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_owned();
        names.insert(name.to_owned());
        digests.insert(name.to_owned(), want.to_owned());
        summaries.push(StdlibModule {
            name: name.to_owned(),
            profile,
        });
        let path = checkout.join("library").join(rel);
        match fs::read(&path) {
            Ok(bytes) => {
                let mut digest = Sha256::new();
                digest.update(&bytes);
                if format!("{:x}", digest.finalize()) != want {
                    stale_paths.push(format!("library/{rel}"));
                }
            }
            Err(_) => stale_paths.push(format!("library/{rel}")),
        }
    }

    // Imports must resolve within the manifest; the graph must be acyclic.
    let mut graph: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for module in &modules {
        let name = module
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_owned();
        let imports = module
            .get("imports")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut edges = Vec::new();
        for import in imports {
            if let Some(target) = import.as_str() {
                if !names.contains(target) {
                    unresolved_imports.push(format!("{name} -> {target}"));
                } else {
                    edges.push(target.to_owned());
                }
            }
        }
        graph.insert(name, edges);
    }
    let mut visiting: Vec<String> = Vec::new();
    let mut done: BTreeSet<String> = BTreeSet::new();
    fn visit(
        node: &str,
        graph: &BTreeMap<String, Vec<String>>,
        visiting: &mut Vec<String>,
        done: &mut BTreeSet<String>,
        cycles: &mut Vec<String>,
    ) {
        if done.contains(node) {
            return;
        }
        if let Some(pos) = visiting.iter().position(|n| n == node) {
            let mut chain = visiting[pos..].to_vec();
            chain.push(node.to_owned());
            cycles.push(chain.join(" -> "));
            return;
        }
        visiting.push(node.to_owned());
        if let Some(edges) = graph.get(node) {
            for edge in edges {
                visit(edge, graph, visiting, done, cycles);
            }
        }
        visiting.pop();
        done.insert(node.to_owned());
    }
    for name in names.iter() {
        visit(name, &graph, &mut visiting, &mut done, &mut cycles);
    }

    // The bundle pin must verify and agree with the manifest.
    let bundle_path = checkout.join(
        manifest
            .get("bundle_path")
            .and_then(|v| v.as_str())
            .unwrap_or("dist/stdlib-bundle.json"),
    );
    let mut error: Option<String> = None;
    match fs::read_to_string(&bundle_path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
    {
        Some(bundle)
            if bundle.get("schema_version").and_then(|v| v.as_str()) == Some(BUNDLE_SCHEMA) =>
        {
            let entries = bundle
                .get("modules")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let pairs: Vec<(&str, &str)> = entries
                .iter()
                .filter_map(|m| {
                    Some((m.get("name")?.as_str()?, m.get("content_sha256")?.as_str()?))
                })
                .collect();
            let recomputed = bundle_identity_for(&pairs);
            let pinned = bundle
                .get("bundle_identity")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if recomputed != pinned {
                error = Some(
                    "bundle pin identity does not recompute; the pin is corrupt or hand-edited"
                        .to_owned(),
                );
            } else if Some(pinned) != bundle_identity.as_deref() {
                error = Some("manifest bundle_identity disagrees with the bundle pin; regenerate the manifest".to_owned());
            } else {
                for (name, digest) in pairs {
                    if digests.get(name) != Some(&digest.to_owned()) {
                        stale_paths.push(format!("bundle:{name}"));
                    }
                }
            }
        }
        _ => {
            error = Some(format!(
                "bundle pin {} is missing or unparseable",
                bundle_path.to_string_lossy()
            ));
        }
    }

    let mut status = StdlibStatus {
        schema_version: "mncs.doctor.stdlib-status/1".to_owned(),
        state: "ready".to_owned(),
        root: Some(checkout.to_string_lossy().into_owned()),
        manifest_path: Some(manifest_path.to_string_lossy().into_owned()),
        bundle_identity,
        requires_profile_min: requires_min,
        requires_profile_max: requires_max,
        module_count: modules.len(),
        modules: summaries,
        stale_paths,
        unresolved_imports,
        cycles,
        error,
    };
    if status.error.is_some()
        || !status.stale_paths.is_empty()
        || !status.unresolved_imports.is_empty()
        || !status.cycles.is_empty()
    {
        status.state = "invalid".to_owned();
    }
    status
}

/// Parse a `major.minor` profile for ordering. Unknown shapes sort last
/// so Doctor never silently treats an unrecognized profile as satisfied.
pub fn profile_key(profile: &str) -> Vec<u64> {
    profile
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(u64::MAX))
        .collect()
}

/// `true` when `required <= supported` under profile ordering.
pub fn profile_satisfied(required: &str, supported: &str) -> bool {
    profile_key(required) <= profile_key(supported)
}

/// Collect `use mncs.*` targets from project source text (bounded,
/// read-only scan; the compiler remains the resolution authority).
pub fn mncs_use_targets(text: &str) -> Vec<String> {
    let mut targets = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("use ") else {
            continue;
        };
        let end = rest.find(';').unwrap_or(rest.len());
        let mut target = rest[..end].trim();
        if let Some(pos) = target.find(" as ") {
            target = target[..pos].trim();
        }
        if target.starts_with("mncs.") && !targets.contains(&target.to_owned()) {
            targets.push(target.to_owned());
        }
    }
    targets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_ordering_compares_numerically() {
        assert!(profile_satisfied("0.5", "0.18"));
        assert!(profile_satisfied("0.18", "0.18"));
        assert!(!profile_satisfied("0.18", "0.5"));
        assert!(profile_satisfied("0.9", "0.18"));
        assert!(!profile_satisfied("0.10", "0.9"));
        assert!(profile_satisfied("0.9", "0.10"));
    }

    #[test]
    fn use_targets_scan_is_bounded_and_deduped() {
        let text = "mncs 0.6;\n\nmodule tmp.consumer;\n\nuse mncs.core.status.v1;\nuse mncs.std.chunk.v1 as chunk;\nuse mncs.core.status.v1;\nuse other.lib;\n";
        assert_eq!(
            mncs_use_targets(text),
            vec![
                "mncs.core.status.v1".to_owned(),
                "mncs.std.chunk.v1".to_owned()
            ]
        );
    }

    #[test]
    fn bundle_identity_matches_reference_rule() {
        // Single-module identity: sha256("name\x00digest\n") with prefix.
        let digest = Sha256::digest(b"hello");
        let hex = format!("{digest:x}");
        let identity = bundle_identity_for(&[("mncs.core.status.v1", &hex)]);
        let mut expected = Sha256::new();
        expected.update(b"mncs.core.status.v1");
        expected.update(b"\x00");
        expected.update(hex.as_bytes());
        expected.update(b"\n");
        assert_eq!(
            identity,
            format!("mncs:stdlib-bundle:{:x}", expected.finalize())
        );
    }
}
