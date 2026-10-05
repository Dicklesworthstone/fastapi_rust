# Dependency Upgrade Log

## 2026-10-04 refresh (bd-3ffo)

The user explicitly requested `library-updater`. This section records registry
research and per-dependency validation; it does not claim a performance win.
Historical entries below are retained. The recovery file
`claude-upgrade-progress.json` becomes inactive once validation is complete.

All 18 direct external dependencies were checked against official crates.io
stable, non-yanked versions. Four need lockfile updates; their manifest ranges
already allow the target versions. Internal path dependencies and the pinned
nightly toolchain are preserved.

| Dependency | Locked before | Target | Research / compatibility notes | Validation |
|---|---|---|---|---|
| flate2 | 1.1.9 | 1.1.10 | [Release notes](https://github.com/rust-lang/flate2-rs/releases/tag/1.1.10): decoder/header fixes; miniz backend moves to 0.9. | Reapplied: RCH core all-feature library tests passed (1197 passed, 1 existing ignored). |
| futures-executor | 0.3.33 | 0.3.34 | [Release notes](https://github.com/rust-lang/futures-rs/releases/tag/0.3.34): waker fix; requires companion futures core/task/util 0.3.34. | Reapplied: RCH workspace all-feature library tests passed. |
| syn (direct) | 3.0.3 | 3.0.6 | [Release notes](https://github.com/dtolnay/syn/releases/tag/3.0.6): foreign safe-fn parsing, error spans, interpolated lifetimes. The separate transitive syn 2 remains upstream-owned. | RCH workspace all-feature tests passed. |
| insta | 1.48.0 | 1.49.0 | [Release notes](https://github.com/mitsuhiko/insta/releases/tag/1.49.0): enum-key serialization and macro warning fixes. Existing snapshots checked without regeneration. | RCH output all-feature tests passed. |

Already current: asupersync 0.5.0, serde 1.0.229, serde_json 1.0.151,
parking_lot 0.12.5, getrandom 0.4.3, regex 1.13.1, serial_test 4.0.1,
criterion 0.8.2, proc-macro2 1.0.107, quote 1.0.47, crossterm 0.29.0,
unicode-width 0.2.2, rich_rust 0.2.3, proptest 1.11.0.

GitHub Actions stable release tags and major aliases were checked against their
upstream Git refs: checkout v7.0.1, upload-artifact v7.0.1,
download-artifact v8.0.1, github-script v9.0.0, rust-cache v2.9.2,
cargo-deny-action v2.1.1, action-gh-release v3.0.3. All active workflow
selectors already point to these releases. Intentional rust-toolchain selectors
are preserved. CI now explicitly installs the repository's pinned nightly in
each job: the prior fmt/clippy jobs installed components on floating nightly,
then rustup selected the pinned nightly without those components. Their checks
never ran. The separate rustdoc failure was an obsolete `AppBuilder::docs`
link, corrected to `enable_docs`. Warning gates remain unchanged. Evidence:
[fmt job](https://github.com/Dicklesworthstone/fastapi_rust/actions/runs/35760150887/job/106855879709),
[clippy job](https://github.com/Dicklesworthstone/fastapi_rust/actions/runs/35760150887/job/106855879160),
[docs job](https://github.com/Dicklesworthstone/fastapi_rust/actions/runs/35760150887/job/106855879582).
No open GitHub issues or PRs were found. Post-push CI verification is pending.
The preservation push at e942b6e passed formatting and macOS/Windows tests,
then [Clippy](https://github.com/Dicklesworthstone/fastapi_rust/actions/runs/37242247058/job/111553476317)
reported two `collapsible_if` errors in compression middleware. The nested
conditions are now equivalent short-circuit let chains; `-D warnings` stays
unchanged. Final remote Clippy and the next pushed CI run remain pending.
The scheduled workflow now uses explicit `+nightly` / `+1.95.0` selectors:
otherwise the repository pin silently overrides the toolchain installed for
the named latest-nightly or MSRV check. Existing warning gates and optional
job policies are preserved.

Baseline: RCH `cargo test --workspace --all-features --locked` at dfaa5a1 passed.
The flate2 and futures upgrades passed their first workspace tests, but external
checkout resets at 20:39:55 and 20:41:07 UTC removed those lockfile changes and
the uncommitted source/doc edits. Original handwritten patches were recovered
from the session log; dependency changes are being reapplied and retested.
Those earlier runs do not certify the recovered final checkout.

Transitive updates are researched from published manifests, changelogs, and
tagged source before mutation. Each update gets affected-crate tests before the
next update; the final workspace suite checks integration. Target-specific
dependencies receive native regression checks, not cross-platform certification.
No snapshots are regenerated. Final checks and security audit are pending.

The crossbeam-epoch validation was initially refused by RCH because worker
vmi1264463 had critical memory pressure (exit 103). No tests ran and no local
fallback occurred. Validation resumes on an admissible remote worker before
another dependency changes; this refusal is not a passing test result.
The replacement worker passed epoch validation, then refused queue validation
for the same pressure condition. Subsequent builds use RCH's admissible-worker
selection and Cargo `--jobs 2` to reduce peak compiler memory. Test scope,
assertions, and warning gates are unchanged; neither refusal ran tests.
The data-encoding and hermit-abi runs hit source-sync timeouts on hz4 before
remote Cargo started; hybrid-array hit the same timeout on hz3. RCH retried
on another worker and each run passed 2110 workspace library tests. Failed
syncs are not test evidence; no local compilation fallback occurred. Final
checks use `RCH_SYNC_TIMEOUT_MS=120000` for source transfers after these
observed 35000-ms failures. Compiler/test limits, assertions, and warning gates
are unchanged; this transport allowance does not certify any product behavior.
Indexmap validation also failed before remote Cargo started, and RCH retained
unconfirmed ownership. `rch jobs recover` acknowledged wrapper
`rchw-ae839b0a-ec54-4d19-a6ad-f6f496aafe32` / build `30050444235505885`
as terminal with exit 1 before retry. No tests ran in that attempt. Resumed
dependency checks also use the 120000-ms source-transfer allowance.
The next IndexMap attempt reached remote Cargo on hz4 but exhausted the
existing 1800-second execution budget while compiler threads waited on filesystem
I/O. RCH killed and verified the remote process group; exact wrapper
`rchw-a6cc9d2c-4495-4373-829d-9927dc4a79c4` / build `30050444235505890`
was recovered with terminal acknowledgement and exit 137. No passing result
was inferred from that attempt. Retrying the same locked update on admissible
worker ovh-a passed 333 affected tests; later dependencies changed only after
that result. No daemon restart, other-agent cancellation, or local fallback.

The latest wasm-bindgen-futures (0.4.79) adds a normal Tokio dependency under an
Emscripten cfg, which Cargo records even on Linux. The project forbids this.
Use the preceding Tokio-free family: futures 0.4.78, bindgen 0.2.128, and
js-sys/web-sys 0.3.105. [Published manifest](https://raw.githubusercontent.com/wasm-bindgen/wasm-bindgen/0.2.129/crates/futures/Cargo.toml).
The first futures-only resolver attempt failed because locked web-sys required
js-sys 0.3.103. Selecting both futures and web-sys with precise futures 0.4.78
lets Cargo update the exactly coupled family atomically; its dry run confirmed
all seven intended versions without Tokio. No manual checksum edits were needed.

### Sequential transitive validation

| Package / coupled family | Before → after | Research | Tests |
|---|---|---|---|
| futures-executor | 0.3.33 → 0.3.34 | [Source](https://github.com/rust-lang/futures-rs/releases/tag/0.3.34): Waker identity fix; coupled core/task/util upgrades. | RCH `cargo test --workspace --all-features --lib --locked --quiet --config profile.test.debug=0` passed. |
| insta | 1.48.0 → 1.49.0 | [Source](https://github.com/mitsuhiko/insta/releases/tag/1.49.0): Enum-key serialization and macro warning fixes; preserve snapshots. | RCH `cargo test -p fastapi-output --all-features --locked --quiet --config profile.test.debug=0` passed. |
| chacha20 | 0.10.1 → 0.10.2 | [Source](https://github.com/RustCrypto/stream-ciphers/pull/580): Replace yanked release; fix SSE2 backend intrinsics. | RCH `cargo test --workspace --all-features --lib --locked --quiet --config profile.test.debug=0` passed. |
| wasm-bindgen-futures / wasm-bindgen / js-sys / web-sys | 0.4.76 / 0.2.126 / 0.3.103 → 0.4.78 / 0.2.128 / 0.3.105 | [Source](https://github.com/wasm-bindgen/wasm-bindgen/blob/0.2.128/CHANGELOG.md): Newest Tokio-free coupled bindgen/js/web family; native regression only. | RCH workspace all-feature library tests passed; native only. |
| aes | 0.9.2 → 0.9.3 | [Source](https://static.crates.io/crates/aes/aes-0.9.3.crate): MSRV 1.89; VAES default and backend cfg changes. | RCH `cargo test --workspace --all-features --lib --locked --quiet --config profile.test.debug=0` passed. |
| aes-gcm | 0.11.0 → 0.11.1 | [Source](https://static.crates.io/crates/aes-gcm/aes-gcm-0.11.1.crate): Internal ctutils migration. | RCH `cargo test --workspace --all-features --lib --locked --quiet --config profile.test.debug=0` passed. |
| aho-corasick | 1.1.4 → 1.1.5 | [Source](https://github.com/BurntSushi/aho-corasick/compare/1.1.4...1.1.5): Checked offsets. | RCH `cargo test -p fastapi-output --all-features --locked --quiet --config profile.test.debug=0` passed. |
| bitflags | 2.13.1 → 2.13.2 | [Source](https://github.com/bitflags/bitflags/releases/tag/2.13.2): Generated const placement fix. | RCH `cargo test -p fastapi-output --all-features --locked --quiet --config profile.test.debug=0` passed. |
| cc | 1.4.0 → 1.6.0 | [Source](https://github.com/rust-lang/cc-rs/blob/cc-v1.6.0/CHANGELOG.md): MSRV 1.65; build flag/cache changes; paired MSVC tools. | RCH `cargo test -p fastapi-output --all-features --locked --quiet --config profile.test.debug=0` passed. |
| cfg-if | 1.0.4 → 1.0.5 | [Source](https://github.com/rust-lang/cfg-if/releases/tag/v1.0.5): Documentation update. | RCH `cargo test -p fastapi-output --all-features --locked --quiet --config profile.test.debug=0` passed. |
| clap | 4.6.4 → 4.6.7 | [Source](https://github.com/clap-rs/clap/blob/v4.6.7/CHANGELOG.md): Criterion parser family. | RCH `cargo test -p fastapi-http --all-targets --all-features --locked --quiet --config profile.test.debug=0` passed. |
| console | 0.16.4 → 0.16.6 | [Source](https://github.com/console-rs/console/releases/tag/0.16.6): Terminal escape and UTF-8 width fixes. | RCH `cargo test -p fastapi-output --all-features --locked --quiet --config profile.test.debug=0` passed. |
| cpufeatures | 0.3.0 → 0.3.1 | [Source](https://static.crates.io/crates/cpufeatures/cpufeatures-0.3.1.crate): AVX/Miri detection fixes. | RCH `cargo test --workspace --all-features --lib --locked --quiet --config profile.test.debug=0` passed. |
| crc32fast | 1.5.0 → 1.5.2 | [Source](https://github.com/srijs/rust-crc32fast/compare/v1.5.0...v1.5.2): CRC backend changes; no speed claim. | RCH `cargo test -p fastapi-output --all-features --locked --quiet --config profile.test.debug=0` passed. |
| crossbeam-deque | 0.8.7 → 0.8.8 | [Source](https://static.crates.io/crates/crossbeam-deque/crossbeam-deque-0.8.8.crate): Wider indexes and TSan support. | RCH `cargo test -p fastapi-output --all-features --locked --quiet --config profile.test.debug=0` passed. |
| crossbeam-epoch | 0.9.20 → 0.9.21 (already locked) | [Source](https://static.crates.io/crates/crossbeam-epoch/crossbeam-epoch-0.9.21.crate): Const null and TSan support. | RCH `cargo test -p fastapi-output --all-features --locked --quiet --config profile.test.debug=0` passed. |
| crossbeam-queue | 0.3.13 → 0.3.14 (already locked) | [Source](https://static.crates.io/crates/crossbeam-queue/crossbeam-queue-0.3.14.crate): Wider indexes. | RCH `cargo test --workspace --all-features --lib --locked --quiet --config profile.test.debug=0 --jobs 2` passed. |
| crossbeam-utils | 0.8.22 → 0.8.23 | [Source](https://static.crates.io/crates/crossbeam-utils/crossbeam-utils-0.8.23.crate): ShardedLock guard and TSan fixes. | RCH `cargo test -p fastapi-output --all-features --locked --quiet --config profile.test.debug=0 --jobs 2` passed. |
| data-encoding | 2.11.0 → 2.11.1 | [Source](https://github.com/ia0/data-encoding/compare/v2.11.0...v2.11.1): Metadata-only source comparison. | RCH `cargo test --workspace --all-features --lib --locked --quiet --config profile.test.debug=0 --jobs 2` passed. |
| either | 1.17.0 → 1.18.0 | [Source](https://static.crates.io/crates/either/either-1.18.0.crate): Tuple iterator implementations. | RCH `cargo test -p fastapi-output --all-features --locked --quiet --config profile.test.debug=0 --jobs 2` passed. |
| find-msvc-tools | 0.1.9 → 0.1.14 (already locked) | [Source](https://github.com/rust-lang/cc-rs/blob/find-msvc-tools-v0.1.14/find-msvc-tools/CHANGELOG.md): MSRV 1.65 and cc integration. | RCH `cargo test -p fastapi-output --all-features --locked --quiet --config profile.test.debug=0` passed in coupled cc validation. |
| futures-io | 0.3.33 → 0.3.34 | [Source](https://github.com/rust-lang/futures-rs/releases/tag/0.3.34): Companion futures release. | RCH `cargo test --workspace --all-features --lib --locked --quiet --config profile.test.debug=0 --jobs 2` passed. |
| hermit-abi | 0.5.2 → 0.5.3 | [Source](https://github.com/hermit-os/hermit-rs/compare/356f491b8c68aaa893a4450b10b9c080f6d2e63a...c0b97d12a2f1f65610ba84e6f23fbb69fc70c0f7): fsync and docs; native regression only. | RCH `cargo test --workspace --all-features --lib --locked --quiet --config profile.test.debug=0 --jobs 2` passed. |
| hybrid-array | 0.4.13 → 0.4.15 | [Source](https://static.crates.io/crates/hybrid-array/hybrid-array-0.4.15.crate): New array sizes. | RCH `cargo test --workspace --all-features --lib --locked --quiet --config profile.test.debug=0 --jobs 2` passed. |
| indexmap | 2.14.0 → 2.14.2 (already locked) | [Source](https://github.com/indexmap-rs/indexmap/blob/2.14.2/RELEASES.md): Macro hygiene and initialization. | RCH `cargo test -p fastapi-output --all-features --locked --quiet --config profile.test.debug=0 --jobs 2` passed. |
| lazy_static | 1.5.0 → 1.5.1 | [Source](https://github.com/rust-lang-nursery/lazy-static.rs/compare/be7c1c43f264699f956b70ce8e29941bd1e61bde...4c1b9a170c157d679592e2682b13cc780c1db814): Docs/metadata; maintenance status. | RCH `cargo test -p fastapi-output --all-features --locked --quiet --config profile.test.debug=0 --jobs 2` passed. |
| libc | 0.2.189 → 0.2.190 | [Source](https://static.crates.io/crates/libc/libc-0.2.190.crate): Platform bindings; native Linux checks. | RCH `cargo test --workspace --all-features --lib --locked --quiet --config profile.test.debug=0 --jobs 2` passed. |
| log | 0.4.33 → 0.4.34 (already locked) | [Source](https://github.com/rust-lang/log/releases/tag/0.4.34): Boxed logger alloc support. | RCH `cargo test -p fastapi-output --all-features --locked --quiet --config profile.test.debug=0 --jobs 2` passed. |
| lru | 0.18.2 → 0.18.5 | [Source](https://static.crates.io/crates/lru/lru-0.18.5.crate): Sparse constructor and retain. | RCH `cargo test --workspace --all-features --lib --locked --quiet --config profile.test.debug=0 --jobs 2` passed. |
| mio | 1.2.2 → 1.2.4 | [Source](https://static.crates.io/crates/mio/mio-1.2.4.crate): Named-pipe UAF and Unix readiness fixes. | RCH `cargo test --workspace --all-features --lib --locked --quiet --config profile.test.debug=0 --jobs 2` passed. |


## Historical upgrade record

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
