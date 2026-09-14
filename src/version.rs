//! Language version model.
//!
//! `mncs-language` owns the authoritative profile registry
//! (`crates/mncs-syntax/src/profile.rs`, RFC 0036). This module mirrors only
//! the *registry shape* needed for repository-level planning: an ordered list
//! of known source profiles plus their lifecycle status. It performs no
//! parsing and encodes no semantic meaning of any profile.
//!
//! If upstream publishes a machine-readable profile registry, this table
//! should be generated from it (see `pressure/DOC-P-001.md`).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A source language version such as `0.17`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LanguageVersion {
    /// Major component (today always 0; `1.0` is reserved/unsupported upstream).
    pub major: u32,
    /// Minor component.
    pub minor: u32,
}

impl LanguageVersion {
    /// Construct a version without parsing.
    pub const fn new(major: u32, minor: u32) -> Self {
        Self { major, minor }
    }

    /// Short `X.Y` rendering used in headers and reports.
    pub fn short(&self) -> String {
        format!("{}.{}", self.major, self.minor)
    }
}

impl fmt::Display for LanguageVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("invalid language version {0:?}: expected X.Y with numeric components")]
pub struct VersionParseError(String);

impl FromStr for LanguageVersion {
    type Err = VersionParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let (major, minor) = s
            .split_once('.')
            .ok_or_else(|| VersionParseError(s.to_owned()))?;
        let major: u32 = major
            .trim()
            .parse()
            .map_err(|_| VersionParseError(s.to_owned()))?;
        let minor: u32 = minor
            .trim()
            .parse()
            .map_err(|_| VersionParseError(s.to_owned()))?;
        Ok(Self { major, minor })
    }
}

/// Lifecycle of a known source profile, mirroring upstream `ProfileStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProfileStatus {
    /// Frozen: still accepted, receives no changes.
    Sealed,
    /// The current toolchain profile.
    Current,
    /// Known but rejected by the toolchain (e.g. reserved `1.0`).
    Unsupported,
}

/// One row of the mirrored profile registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileRecord {
    pub version: LanguageVersion,
    pub status: ProfileStatus,
}

/// The mirrored registry. Single source of "current" inside doctor: exactly
/// one record carries [`ProfileStatus::Current`]; nothing else in the
/// codebase hardcodes a current version.
pub fn registry() -> Vec<ProfileRecord> {
    let mut records: Vec<ProfileRecord> = (1u32..=17)
        .map(|minor| ProfileRecord {
            version: LanguageVersion::new(0, minor),
            status: if minor == 17 {
                ProfileStatus::Current
            } else {
                ProfileStatus::Sealed
            },
        })
        .collect();
    records.push(ProfileRecord {
        version: LanguageVersion::new(1, 0),
        status: ProfileStatus::Unsupported,
    });
    records
}

/// The current toolchain profile per the mirrored registry.
pub fn current_version() -> LanguageVersion {
    LanguageVersion::new(0, 17)
}

/// Look up a version in the mirrored registry.
pub fn lookup(version: LanguageVersion) -> Option<ProfileStatus> {
    registry()
        .into_iter()
        .find(|record| record.version == version)
        .map(|record| record.status)
}

/// Classification of a declared version for planning purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionClass {
    /// Matches the current toolchain profile.
    Current,
    /// Known sealed profile: accepted, migration path may exist.
    Sealed,
    /// Known but rejected by the toolchain.
    Unsupported,
    /// Absent from the mirrored registry (too old, too new, or malformed).
    Unknown,
}

/// Classify a declared (or missing) version.
pub fn classify(declared: Option<LanguageVersion>) -> VersionClass {
    match declared {
        None => VersionClass::Unknown,
        Some(v) => match lookup(v) {
            Some(ProfileStatus::Current) => VersionClass::Current,
            Some(ProfileStatus::Sealed) => VersionClass::Sealed,
            Some(ProfileStatus::Unsupported) => VersionClass::Unsupported,
            None => VersionClass::Unknown,
        },
    }
}

/// A directed adjacent transition `from -> to` between profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TransitionId {
    pub from: LanguageVersion,
    pub to: LanguageVersion,
}

impl TransitionId {
    pub const fn new(from: LanguageVersion, to: LanguageVersion) -> Self {
        Self { from, to }
    }
}

impl fmt::Display for TransitionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} -> {}", self.from, self.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_short_versions() {
        assert_eq!("0.17".parse(), Ok(LanguageVersion::new(0, 17)));
        assert_eq!("  0.8 ".parse(), Ok(LanguageVersion::new(0, 8)));
        assert!("1.0".parse::<LanguageVersion>().is_ok());
        assert!("0".parse::<LanguageVersion>().is_err());
        assert!("0.x".parse::<LanguageVersion>().is_err());
        assert!("".parse::<LanguageVersion>().is_err());
    }

    #[test]
    fn ordering_follows_numeric_version() {
        assert!(LanguageVersion::new(0, 8) < LanguageVersion::new(0, 16));
        assert!(LanguageVersion::new(0, 16) < LanguageVersion::new(0, 17));
        assert!(LanguageVersion::new(0, 17) < LanguageVersion::new(1, 0));
    }

    #[test]
    fn registry_has_single_current() {
        let records = registry();
        assert_eq!(records.len(), 18);
        let current: Vec<_> = records
            .iter()
            .filter(|r| r.status == ProfileStatus::Current)
            .collect();
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].version, current_version());
    }

    #[test]
    fn classification_covers_all_cases() {
        assert_eq!(classify(Some(current_version())), VersionClass::Current);
        assert_eq!(
            classify(Some(LanguageVersion::new(0, 8))),
            VersionClass::Sealed
        );
        assert_eq!(
            classify(Some(LanguageVersion::new(1, 0))),
            VersionClass::Unsupported
        );
        assert_eq!(
            classify(Some(LanguageVersion::new(0, 99))),
            VersionClass::Unknown
        );
        assert_eq!(classify(None), VersionClass::Unknown);
    }
}
