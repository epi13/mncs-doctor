#![forbid(unsafe_code)]

//! `mncs-doctor`: repository health inspection, automated repair, and
//! version-aware source migration for MNCS projects.
//!
//! Architectural boundary: `mncs-language` owns syntax, parsing, semantic
//! interpretation, diagnostics, structured fixes, canonicalization, and
//! version-transition knowledge. This crate owns repository/project concerns:
//! discovery, inventory, health analysis, migration planning and orchestration,
//! safe fix application, transactions, verification, and reporting.
//!
//! Nothing in this crate parses MNCS beyond a deliberately shallow,
//! well-documented header scan (see [`diagnostics`]). Full semantic analysis
//! is delegated to language backends through [`diagnostics::LanguageBackend`].

pub mod diagnostics;
pub mod discovery;
pub mod edits;
pub mod fix;
pub mod health;
pub mod migration;
pub mod mncs_runtime;
pub mod report;
pub mod toolchain;
pub mod transaction;
pub mod verify;
pub mod version;

/// Version of the doctor tool itself (mirrors crate version).
pub const DOCTOR_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Machine-readable report schema version emitted by [`report`].
pub const REPORT_SCHEMA_VERSION: &str = "0.1";
