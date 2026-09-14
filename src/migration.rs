//! Version-aware migration: registry, path planning, provenance.
//!
//! Philosophy: migrations compose bounded adjacent transitions
//! (`0.16 -> 0.17`) rather than monolithic ancient-to-latest rewrites. Each
//! edge carries a [`TransitionKind`] and provenance; unknown edges are
//! reported, never silently rewritten.
//!
//! Coverage honesty: upstream `mncs-language` publishes no source-level
//! migration rules today (see `pressure/DOC-P-003.md`). The production
//! [`default_registry`] therefore records every adjacent edge as
//! [`TransitionKind::Unknown`] and the apply path refuses to cross unknown
//! edges. Doctor additionally provides one mechanical, explicitly
//! doctor-owned operation — the header-version bump ([`bump_header`],
//! classified REVIEW) — so `migrate --plan`/`--dry-run` are functional while
//! production `migrate --apply` stays fail-closed until real transition
//! knowledge lands upstream. Test/fixture registries with synthetic
//! `Source` transitions validate the engine (see tests).

use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::diagnostics::scan_header;
use crate::discovery::fingerprint;
use crate::fix::Applicability;
use crate::version::{current_version, LanguageVersion, TransitionId};

/// What an edge is known to require.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionKind {
    /// Proven no-op: crossing requires no source changes beyond advancing
    /// the declared version stamp.
    Noop,
    /// Only metadata changes (e.g. header version stamp).
    Metadata,
    /// Source transformations are required and recorded.
    Source,
    /// No recorded knowledge for this edge. Planning reports it; apply
    /// refuses it.
    Unknown,
}

impl std::fmt::Display for TransitionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransitionKind::Noop => write!(f, "no-op"),
            TransitionKind::Metadata => write!(f, "metadata"),
            TransitionKind::Source => write!(f, "source"),
            TransitionKind::Unknown => write!(f, "unknown"),
        }
    }
}

/// One mechanical source operation within a transition. This is the shape
/// future upstream rule data will fill; today only fixture registries and
/// the doctor-owned header bump use it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformStep {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub review_required: bool,
    #[serde(default)]
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<TransformOp>,
}

/// Executable transform operations (v1: substring replacement only).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransformOp {
    /// Replace all non-overlapping occurrences; records the count.
    ReplaceAll { needle: String, replacement: String },
}

/// A single adjacent transition with provenance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationTransition {
    pub id: TransitionId,
    pub kind: TransitionKind,
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// Safety of applying this transition automatically.
    pub applicability: Applicability,
    /// Where this knowledge came from (e.g. `mncs-language/rfc-XXXX`,
    /// `doctor-mechanical`, `fixture/...`). Never empty.
    pub provenance: String,
    #[serde(default)]
    pub transforms: Vec<TransformStep>,
}

/// Registry of known adjacent transitions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MigrationRegistry {
    #[serde(default)]
    transitions: BTreeMap<(LanguageVersion, LanguageVersion), MigrationTransition>,
}

impl MigrationRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, transition: MigrationTransition) {
        self.transitions
            .insert((transition.id.from, transition.id.to), transition);
    }

    pub fn get(&self, from: LanguageVersion, to: LanguageVersion) -> Option<&MigrationTransition> {
        self.transitions.get(&(from, to))
    }

    /// All edges out of `from`.
    pub fn outgoing(&self, from: LanguageVersion) -> Vec<&MigrationTransition> {
        let mut out: Vec<&MigrationTransition> = self
            .transitions
            .iter()
            .filter(|((f, _), _)| *f == from)
            .map(|(_, t)| t)
            .collect();
        out.sort_by_key(|t| t.id.to);
        out
    }

    pub fn len(&self) -> usize {
        self.transitions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.transitions.is_empty()
    }
}

/// Production registry: every adjacent sealed/current edge `0.1 -> ... ->
/// 0.17` recorded as [`TransitionKind::Unknown`] with explicit provenance.
/// See module docs for why this is the honest seed.
pub fn default_registry() -> MigrationRegistry {
    let mut registry = MigrationRegistry::new();
    for minor in 1u32..17 {
        let from = LanguageVersion::new(0, minor);
        let to = LanguageVersion::new(0, minor + 1);
        registry.insert(MigrationTransition {
            id: TransitionId::new(from, to),
            kind: TransitionKind::Unknown,
            title: format!("{from} -> {to}: transition knowledge unrecorded"),
            description: "Upstream mncs-language publishes no source-level rules for \
                this edge. Doctor can show the path and bump the header mechanically \
                (review), but cannot vouch for semantic deltas."
                .to_owned(),
            applicability: Applicability::Manual,
            provenance: "doctor-seed: unrecorded upstream (DOC-P-003)".to_owned(),
            transforms: Vec::new(),
        });
    }
    registry
}

/// Fixture registry used by tests and `fixtures/`: synthetic but
/// architecture-valid `Source` transitions over a reserved `9.x` version
/// line that can never collide with real profiles.
pub fn fixture_registry() -> MigrationRegistry {
    let mut registry = MigrationRegistry::new();
    registry.insert(MigrationTransition {
        id: TransitionId::new(LanguageVersion::new(9, 0), LanguageVersion::new(9, 1)),
        kind: TransitionKind::Source,
        title: "9.0 -> 9.1: rename oldfn to newfn (fixture)".to_owned(),
        description: "Synthetic fixture transition for engine validation.".to_owned(),
        applicability: Applicability::Safe,
        provenance: "fixture/synthetic-rename".to_owned(),
        transforms: vec![TransformStep {
            id: "rename-oldfn".to_owned(),
            title: "Rename oldfn to newfn".to_owned(),
            review_required: false,
            detail: "Fixture-only construct rename.".to_owned(),
            op: Some(TransformOp::ReplaceAll {
                needle: "oldfn".to_owned(),
                replacement: "newfn".to_owned(),
            }),
        }],
    });
    registry.insert(MigrationTransition {
        id: TransitionId::new(LanguageVersion::new(9, 1), LanguageVersion::new(9, 2)),
        kind: TransitionKind::Noop,
        title: "9.1 -> 9.2: no source changes (fixture)".to_owned(),
        description: "Synthetic no-op edge.".to_owned(),
        applicability: Applicability::Safe,
        provenance: "fixture/synthetic-noop".to_owned(),
        transforms: Vec::new(),
    });
    registry
}

/// File format for future upstream-supplied transition data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryFile {
    #[serde(default)]
    pub provenance: String,
    pub transitions: Vec<MigrationTransition>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MigrationError {
    #[error("no migration path from {from} to {to}: {reason}")]
    NoPath {
        from: LanguageVersion,
        to: LanguageVersion,
        reason: String,
    },
    #[error("refusing to apply across unrecorded edge {0}")]
    UnknownEdge(TransitionId),
    #[error("refusing to apply {0}: requires review")]
    ReviewRequired(TransitionId),
    #[error("header bump failed: {0}")]
    HeaderBump(String),
    #[error("transform {0} failed: {1}")]
    TransformFailed(String, String),
    #[error("registry file error: {0}")]
    RegistryFile(String),
}

/// Load additional transitions from a JSON [`RegistryFile`].
pub fn load_registry_file(json: &str) -> Result<Vec<MigrationTransition>, MigrationError> {
    serde_json::from_str::<RegistryFile>(json)
        .map(|f| f.transitions)
        .map_err(|e| MigrationError::RegistryFile(e.to_string()))
}

/// One step of a planned migration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedStep {
    pub transition: TransitionId,
    pub kind: TransitionKind,
    pub applicability: Applicability,
    pub title: String,
    pub provenance: String,
    pub transform_count: usize,
}

/// An ordered migration plan from one version to another.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationPlan {
    pub from: LanguageVersion,
    pub to: LanguageVersion,
    pub steps: Vec<PlannedStep>,
    /// True when every edge is known (no `Unknown`).
    pub fully_known: bool,
    /// True when no step changes anything (all `Noop`, or empty).
    pub is_noop: bool,
}

/// Build the shortest forward path `from -> to` over adjacent edges.
/// Downgrades are rejected (no down-migration knowledge exists).
/// Missing edges (outside the registry graph) yield [`MigrationError::NoPath`].
pub fn plan(
    from: LanguageVersion,
    to: LanguageVersion,
    registry: &MigrationRegistry,
) -> Result<MigrationPlan, MigrationError> {
    if from == to {
        return Ok(MigrationPlan {
            from,
            to,
            steps: Vec::new(),
            fully_known: true,
            is_noop: true,
        });
    }
    if to < from {
        return Err(MigrationError::NoPath {
            from,
            to,
            reason: "down-migrations are not supported".to_owned(),
        });
    }
    // BFS over the directed edge graph for the shortest path.
    let mut prev: BTreeMap<LanguageVersion, (LanguageVersion, TransitionId)> = BTreeMap::new();
    let mut queue: VecDeque<LanguageVersion> = VecDeque::from([from]);
    prev.insert(from, (from, TransitionId::new(from, from)));
    while let Some(current) = queue.pop_front() {
        if current == to {
            break;
        }
        for edge in registry.outgoing(current) {
            if let std::collections::btree_map::Entry::Vacant(slot) = prev.entry(edge.id.to) {
                slot.insert((current, edge.id));
                queue.push_back(edge.id.to);
            }
        }
    }
    if !prev.contains_key(&to) {
        return Err(MigrationError::NoPath {
            from,
            to,
            reason: "no connected path in the transition registry".to_owned(),
        });
    }
    let mut ids: Vec<TransitionId> = Vec::new();
    let mut cursor = to;
    while cursor != from {
        let (parent, id) = prev[&cursor];
        ids.push(id);
        cursor = parent;
    }
    ids.reverse();
    let mut steps = Vec::new();
    for id in ids {
        match registry.get(id.from, id.to) {
            Some(t) => steps.push(PlannedStep {
                transition: id,
                kind: t.kind,
                applicability: t.applicability,
                title: t.title.clone(),
                provenance: t.provenance.clone(),
                transform_count: t.transforms.len(),
            }),
            None => {
                return Err(MigrationError::NoPath {
                    from,
                    to,
                    reason: format!("registry lacks edge {id}"),
                })
            }
        }
    }
    let fully_known = steps.iter().all(|s| s.kind != TransitionKind::Unknown);
    let is_noop = steps.iter().all(|s| s.kind == TransitionKind::Noop);
    Ok(MigrationPlan {
        from,
        to,
        steps,
        fully_known,
        is_noop,
    })
}

/// Provenance for one applied step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppliedStep {
    pub transition: TransitionId,
    pub kind: TransitionKind,
    pub from_fingerprint: String,
    pub to_fingerprint: String,
    pub replacements: u64,
    pub provenance: String,
}

/// Record of migrating one file across a whole plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationRecord {
    pub relative: String,
    pub from: LanguageVersion,
    pub to: LanguageVersion,
    pub steps: Vec<AppliedStep>,
}

/// Doctor-owned mechanical header bump: rewrite the leading `mncs A.B;`
/// header to `mncs C.D;`. Classified REVIEW at the plan level because the
/// declared version gates semantics; the byte rewrite itself is exact.
pub fn bump_header(text: &str, to: LanguageVersion) -> Result<(String, bool), MigrationError> {
    let facts = scan_header(text);
    let Some((start, end)) = facts.span else {
        return Err(MigrationError::HeaderBump(
            "no parseable header to rewrite".to_owned(),
        ));
    };
    let replacement = format!("mncs {to};");
    let current = text.get(start..end).unwrap_or("");
    if current.trim() == replacement {
        return Ok((text.to_owned(), false));
    }
    let mut out = text.to_owned();
    out.replace_range(start..end, &replacement);
    Ok((out, true))
}

/// Apply one registry transition's transforms to `text`.
fn apply_transforms(
    text: &str,
    id: TransitionId,
    transition: &MigrationTransition,
) -> Result<(String, u64), MigrationError> {
    let mut out = text.to_owned();
    let mut replacements = 0u64;
    for step in &transition.transforms {
        match &step.op {
            Some(TransformOp::ReplaceAll {
                needle,
                replacement,
            }) => {
                let count = out.matches(needle.as_str()).count() as u64;
                out = out.replace(needle.as_str(), replacement.as_str());
                replacements += count;
            }
            None => {
                return Err(MigrationError::TransformFailed(
                    step.id.clone(),
                    "transform has no executable op".to_owned(),
                ));
            }
        }
    }
    let _ = id;
    Ok((out, replacements))
}

/// Apply a whole plan to one file's text.
///
/// Policy: `Noop` edges pass through; `Metadata` edges apply the mechanical
/// header bump; `Source` edges apply recorded transforms then bump the
/// header. `Unknown` edges always refuse. Edges whose transition
/// applicability is `Review`/`Manual` refuse unless `allow_review` is set.
pub fn apply_plan(
    relative: &str,
    text: &str,
    plan: &MigrationPlan,
    registry: &MigrationRegistry,
    allow_review: bool,
) -> Result<(String, MigrationRecord), MigrationError> {
    let mut current = text.to_owned();
    let mut record = MigrationRecord {
        relative: relative.to_owned(),
        from: plan.from,
        to: plan.to,
        steps: Vec::new(),
    };
    for step in &plan.steps {
        let transition = registry
            .get(step.transition.from, step.transition.to)
            .ok_or_else(|| MigrationError::NoPath {
                from: plan.from,
                to: plan.to,
                reason: format!("registry lacks edge {}", step.transition),
            })?;
        match transition.kind {
            TransitionKind::Unknown => {
                return Err(MigrationError::UnknownEdge(step.transition));
            }
            TransitionKind::Noop => {
                // A no-op edge requires no source changes, but the declared
                // header stamp still advances to the edge target; otherwise
                // the file would never reach the planned version.
                let from_fp = fingerprint(current.as_bytes());
                let (next, _) = bump_header(&current, step.transition.to)?;
                current = next;
                let to_fp = fingerprint(current.as_bytes());
                record.steps.push(AppliedStep {
                    transition: step.transition,
                    kind: step.kind,
                    from_fingerprint: from_fp,
                    to_fingerprint: to_fp,
                    replacements: 0,
                    provenance: transition.provenance.clone(),
                });
            }
            TransitionKind::Metadata | TransitionKind::Source => {
                if matches!(
                    transition.applicability,
                    Applicability::Review | Applicability::Manual
                ) && !allow_review
                {
                    return Err(MigrationError::ReviewRequired(step.transition));
                }
                let from_fp = fingerprint(current.as_bytes());
                if transition.kind == TransitionKind::Source {
                    let (next, n) =
                        apply_transforms(current.as_str(), step.transition, transition)?;
                    current = next;
                    let to_fp = fingerprint(current.as_bytes());
                    record.steps.push(AppliedStep {
                        transition: step.transition,
                        kind: step.kind,
                        from_fingerprint: from_fp,
                        to_fingerprint: to_fp,
                        replacements: n,
                        provenance: transition.provenance.clone(),
                    });
                    let from_fp = fingerprint(current.as_bytes());
                    let (next, _) = bump_header(&current, step.transition.to)?;
                    current = next;
                    let to_fp = fingerprint(current.as_bytes());
                    record.steps.push(AppliedStep {
                        transition: step.transition,
                        kind: TransitionKind::Metadata,
                        from_fingerprint: from_fp,
                        to_fingerprint: to_fp,
                        replacements: 0,
                        provenance: "doctor-mechanical: header bump".to_owned(),
                    });
                } else {
                    let (next, _) = bump_header(&current, step.transition.to)?;
                    current = next;
                    let to_fp = fingerprint(current.as_bytes());
                    record.steps.push(AppliedStep {
                        transition: step.transition,
                        kind: step.kind,
                        from_fingerprint: from_fp,
                        to_fingerprint: to_fp,
                        replacements: 0,
                        provenance: "doctor-mechanical: header bump".to_owned(),
                    });
                }
            }
        }
    }
    Ok((current, record))
}

/// Resolve a `--to` target: `latest`/`current` map to the registry current;
/// otherwise parse `X.Y`.
pub fn resolve_target(raw: &str) -> Result<LanguageVersion, MigrationError> {
    if raw.eq_ignore_ascii_case("latest") || raw.eq_ignore_ascii_case("current") {
        return Ok(current_version());
    }
    raw.parse::<LanguageVersion>()
        .map_err(|_| MigrationError::NoPath {
            from: current_version(),
            to: current_version(),
            reason: format!("unparseable target version {raw:?}"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(minor: u32) -> LanguageVersion {
        LanguageVersion::new(0, minor)
    }

    #[test]
    fn production_registry_covers_adjacent_edges_as_unknown() {
        let registry = default_registry();
        assert_eq!(registry.len(), 16);
        let t = registry.get(v(15), v(16)).expect("edge");
        assert_eq!(t.kind, TransitionKind::Unknown);
        assert!(!t.provenance.is_empty());
    }

    #[test]
    fn plan_builds_composed_path() {
        let registry = default_registry();
        let plan = plan(v(14), v(16), &registry).unwrap();
        assert_eq!(plan.steps.len(), 2);
        assert!(!plan.fully_known);
        assert!(!plan.is_noop);
    }

    #[test]
    fn same_version_plan_is_noop() {
        let registry = default_registry();
        let plan = plan(v(16), v(16), &registry).unwrap();
        assert!(plan.steps.is_empty() && plan.is_noop && plan.fully_known);
    }

    #[test]
    fn downgrade_and_gap_have_no_path() {
        let registry = default_registry();
        assert!(plan(v(16), v(15), &registry).is_err());
        assert!(plan(LanguageVersion::new(0, 99), v(16), &registry).is_err());
    }

    #[test]
    fn apply_refuses_unknown_edges() {
        let registry = default_registry();
        let plan = plan(v(15), v(16), &registry).unwrap();
        let text = "mncs 0.15;\nmodule a;\n";
        assert!(matches!(
            apply_plan("a.mncs", text, &plan, &registry, true),
            Err(MigrationError::UnknownEdge(_))
        ));
    }

    #[test]
    fn fixture_source_transition_applies_and_is_idempotent() {
        let registry = fixture_registry();
        let from = LanguageVersion::new(9, 0);
        let to = LanguageVersion::new(9, 2);
        let migration_plan = plan(from, to, &registry).unwrap();
        assert!(migration_plan.fully_known);
        assert!(!migration_plan.is_noop);
        let text = "mncs 9.0;\nmodule a;\nfn oldfn() {}\n";
        let (migrated, record) =
            apply_plan("a.mncs", text, &migration_plan, &registry, false).unwrap();
        assert!(migrated.contains("newfn") && !migrated.contains("oldfn"));
        // No-op edge recorded provenance without content change.
        assert_eq!(record.steps.len(), 3);
        // Migrate again: no changes.
        let plan2 = plan(LanguageVersion::new(9, 2), to, &registry).unwrap();
        let (again, _) = apply_plan("a.mncs", &migrated, &plan2, &registry, false).unwrap();
        assert_eq!(again, migrated);
    }

    #[test]
    fn header_bump_is_exact_and_stable() {
        let (out, changed) = bump_header("mncs 0.15;\nmodule a;\n", v(16)).unwrap();
        assert!(changed);
        assert!(out.starts_with("mncs 0.16;"));
        let (out2, changed2) = bump_header(&out, v(16)).unwrap();
        assert!(!changed2 && out2 == out);
        assert!(bump_header("module a;\n", v(16)).is_err());
    }

    #[test]
    fn target_resolution_handles_latest() {
        assert_eq!(resolve_target("latest").unwrap(), current_version());
        assert_eq!(resolve_target("0.8").unwrap(), v(8));
        assert!(resolve_target("bogus").is_err());
    }
}
