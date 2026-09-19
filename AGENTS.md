# Agent contract

- Establish bounded family context and current language/architecture
  identities before changing validation behavior.
- Doctor validates repository health, freshness, drift, migration, and
  evidence; it does not become the semantic authority for language, Commons,
  testing, actions, debug, or RAVEL.
- Query `mncs-language` and Commons capability/pressure knowledge before
  adding host logic. Repair reusable missing capabilities upstream where
  practical; keep host code to filesystem/process/transport/report adapters.
- Every retained host boundary must be explicit and testable. Never silently
  fall back from a native-canonical path to host semantic behavior.
- Use the existing `mncs-family.repository-manifest/v0alpha1` contract and
  preserve `UNKNOWN` when facts or identities are stale or unavailable.
