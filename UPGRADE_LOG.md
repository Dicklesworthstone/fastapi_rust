# Dependency Upgrade Log

**Date:** 2026-02-19  |  **Project:** fastapi_rust  |  **Language:** Rust

## Summary
- **Updated:** 4 spec bumps + 14 lock-file updates  |  **Skipped:** 7 (already at latest within spec)  |  **Failed:** 0  |  **Needs attention:** 0

## Lock-File Updates (cargo update)

All within existing semver specs — no Cargo.toml changes needed:

| Dependency | Old | New |
|---|---|---|
| bitflags | 2.10.0 | 2.11.0 |
| bumpalo | 3.19.1 | 3.20.2 |
| cc | 1.2.55 | 1.2.56 |
| clap | 4.5.57 | 4.5.60 |
| clap_builder | 4.5.57 | 4.5.60 |
| clap_lex | 0.7.7 | 1.0.0 |
| deranged | 0.5.5 | 0.5.6 |
| futures-core | 0.3.31 | 0.3.32 |
| futures-executor | 0.3.31 | 0.3.32 |
| futures-task | 0.3.31 | 0.3.32 |
| futures-util | 0.3.31 | 0.3.32 |
| syn | 2.0.114 | 2.0.116 |
| unicode-ident | 1.0.23 | 1.0.24 |
| zmij | 1.0.20 | 1.0.21 |

## Spec-Level Updates

### crossterm: 0.28 -> 0.29 (fastapi-output)
- **Breaking:** Rustix default backend, cursor 0-based, Event no longer Copy, terminal::size() returns error
- **Impact:** Only `IsTty` trait used — none of the breaking changes apply
- **Tests:** Passed

### criterion: 0.5 -> 0.8 (fastapi-http dev-dep)
- **Breaking:** async-std removed, deprecated APIs deleted, MSRV bumped
- **Impact:** None — benchmark code uses standard BenchmarkGroup API only
- **Tests:** Passed (compilation verified; bench harness not executed)

### insta: 1.34 -> 1.46 (fastapi-output dev-dep)
- **Breaking:** None (minor version bumps)
- **Tests:** Passed

### serial_test: 3.2 -> 3.3 (fastapi-output dev-dep)
- **Breaking:** None (aligned with fastapi-core's existing 3.3.1 spec)
- **Tests:** Passed

## Already Latest (no change needed)

These specs already cover the latest stable versions:

| Dependency | Spec | Latest Resolved |
|---|---|---|
| serde | "1" | 1.0.228 |
| serde_json | "1" | 1.0.149 |
| parking_lot | "0.12" | 0.12.5 |
| futures-executor | "0.3" | 0.3.32 |
| regex | "1" / "1.12" | 1.12.3 |
| proc-macro2 | "1" | 1.0.106 |
| quote | "1" | 1.0.44 |
| syn | "2" | 2.0.116 |
| unicode-width | "0.2" | 0.2.2 |
| proptest | "1" | 1.10.0 |
| serial_test (core) | "3.3.1" | 3.3.1 |

## Path/Git Dependencies (not updated)

| Dependency | Type | Notes |
|---|---|---|
| asupersync | git + path override | Own project, updated separately |
| rich_rust | crates.io + path override | Own project, updated separately |

## Failed

_(none)_

## Needs Attention

_(none)_
