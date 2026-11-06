//! Language version model.
//!
//! `mncs-language` owns the authoritative profile registry
//! (`crates/mncs-syntax/src/profile.rs`, RFC 0036). This module retains the
//! host-facing version value and the registry shape needed for
//! repository-level planning. Production `X.Y` text parsing and classification
//! run through Doctor's verified MNCS version policy; the host keeps only the
//! byte/string boundary and registry data.
//!
//! This remains a compatibility fallback for shallow header planning. When a
//! project root is available, toolchain probing consumes the authoritative
//! `mncs-language/docs/language-capabilities.json` projection instead.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Native parser widths; small tokens use fewer interpreter lanes.
pub(crate) const VERSION_TEXT_SMALL_BYTES: usize = 16;
pub(crate) const VERSION_TEXT_CHUNK_BYTES: usize = 64;

/// A source language version such as `0.18`.
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
pub enum VersionParseError {
    #[error("invalid language version {0:?}: expected X.Y with numeric components")]
    Invalid(String),
    #[error("native language version parser failed: {0}")]
    Runtime(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeVersionParseOutcome {
    Parsed(LanguageVersion),
    Invalid,
}

impl FromStr for LanguageVersion {
    type Err = VersionParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let runtime = crate::mncs_runtime::DoctorMncsRuntime::production()
            .map_err(|error| VersionParseError::Runtime(error.to_string()))?;
        match runtime
            .parse_version_text(s)
            .map_err(|error| VersionParseError::Runtime(error.to_string()))?
        {
            NativeVersionParseOutcome::Parsed(version) => Ok(version),
            NativeVersionParseOutcome::Invalid => Err(VersionParseError::Invalid(s.to_owned())),
        }
    }
}

/// Independent host oracle retained only for parity tests. It preserves the
/// previous parser, including its unbounded input syntax, as an independent
/// reference for native behavior.
#[cfg(test)]
pub(crate) fn parse_language_version_reference(s: &str) -> Option<LanguageVersion> {
    let s = s.trim();
    let (major, minor) = s.split_once('.')?;
    Some(LanguageVersion {
        major: major.trim().parse().ok()?,
        minor: minor.trim().parse().ok()?,
    })
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
    let mut records: Vec<ProfileRecord> = (1u32..=18)
        .map(|minor| ProfileRecord {
            version: LanguageVersion::new(0, minor),
            status: if minor == 18 {
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
    LanguageVersion::new(0, 18)
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
    fn native_text_parser_matches_independent_host_reference() {
        let whitespace = [
            '\u{0009}', '\u{000A}', '\u{000B}', '\u{000C}', '\u{000D}', '\u{0020}', '\u{0085}',
            '\u{00A0}', '\u{1680}', '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}',
            '\u{2005}', '\u{2006}', '\u{2007}', '\u{2008}', '\u{2009}', '\u{200A}', '\u{2028}',
            '\u{2029}', '\u{202F}', '\u{205F}', '\u{3000}',
        ];
        let exclusions = [
            '\u{0084}', '\u{00A1}', '\u{1681}', '\u{200B}', '\u{2027}', '\u{202A}', '\u{2060}',
            '\u{FEFF}',
        ];
        let mut cases = vec![
            "0.17".to_owned(),
            " +0 . +17 ".to_owned(),
            "00.0008".to_owned(),
            "1.0".to_owned(),
            "4294967295.4294967295".to_owned(),
            "0".to_owned(),
            "0.".to_owned(),
            ".1".to_owned(),
            "0.17.1".to_owned(),
            "-0.17".to_owned(),
            "0.-1".to_owned(),
            "4294967296.1".to_owned(),
            "0.4294967296".to_owned(),
            "0.١".to_owned(),
            "\u{200B}0.17".to_owned(),
            "0.17\u{FEFF}".to_owned(),
            "+ 0.17".to_owned(),
            "0. +17".to_owned(),
        ];
        for character in whitespace {
            cases.push(format!("{character}0.17{character}"));
            cases.push(format!("0{character}.{character}17"));
        }
        for character in exclusions {
            cases.push(format!("{character}0.17"));
            cases.push(format!("0.17{character}"));
            cases.push(format!("0{character}.17"));
        }
        cases.push(format!("{}0.17", " ".repeat(300)));
        cases.push(format!("{}.{}", "0".repeat(300), "0".repeat(300)));
        cases.push(format!("{}0.17", " ".repeat(63) + "\u{2003}"));
        cases.push(format!("0.{}17", " ".repeat(61) + "\u{2003}"));

        for input in cases {
            let expected = parse_language_version_reference(&input);
            let actual = match input.parse::<LanguageVersion>() {
                Ok(version) => Some(version),
                Err(VersionParseError::Invalid(_)) => None,
                Err(error) => panic!("native parser failed for {input:?}: {error}"),
            };
            assert_eq!(actual, expected, "input {input:?}");
        }

        let runtime = crate::mncs_runtime::DoctorMncsRuntime::production()
            .expect("production policy runtime");
        assert!(runtime
            .provenance()
            .expect("native version parse provenance")
            .entrypoints
            .iter()
            .any(|entrypoint| {
                entrypoint == "doctor.version.v1::parse_chunk16"
                    || entrypoint == "doctor.version.v1::parse_chunk64"
            }));
        assert!(runtime
            .provenance()
            .expect("native version finalization provenance")
            .entrypoints
            .iter()
            .any(|entrypoint| entrypoint == "doctor.version.v1::finish_parse"));
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
        assert_eq!(records.len(), 19);
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
