# Dependency Upgrade Log

## 2026-10-08 refresh (v0.5.0 release)

`cargo update` within existing requirements: 14 lockfile packages advanced. A sweep of
every direct registry dependency against crates.io found all of them on their latest
major (asupersync 0.5.0, rich_rust 0.2.3, syn 3, criterion 0.8, insta 1.49, proptest 1,
serial_test 4.0.1, ...), so no manifest change was needed. Gated by the v0.5.0 release
lanes (workspace fmt, clippy `-D warnings`, workspace tests on RCH).

## 2026-10-04–05 refresh (bd-3ffo)

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
No open GitHub issues or PRs were found, including the October 5 recheck.
The preservation push at e942b6e passed formatting and macOS/Windows tests,
then [Clippy](https://github.com/Dicklesworthstone/fastapi_rust/actions/runs/37242247058/job/111553476317)
reported two `collapsible_if` errors in compression middleware. The nested
conditions are now equivalent short-circuit let chains; `-D warnings` stays
unchanged. Final remote Clippy passed with default and all features, and
the strict hosted Clippy job passed at a8c265a. The complete prior-graph
hosted result at 6b358bd is recorded below.
The scheduled workflow now uses explicit `+nightly` / `+1.95.0` selectors:
otherwise the repository pin silently overrides the toolchain installed for
the named latest-nightly or MSRV check. Existing warning gates and optional
job policies are preserved.

Hosted CI at 62bc0d6 passed Clippy, formatting, documentation generation and
security audit, but all three OS test jobs failed the newly runnable Header
doctest: its import used the private `extract` module. The example now uses
the public `fastapi_core::Header` re-export. Assertions and runnable status
are preserved; final full tests and all three hosted OS jobs subsequently
verified this correction at the prior dependency graph.
[Ubuntu failure](https://github.com/Dicklesworthstone/fastapi_rust/actions/runs/37251098519/job/111580578309).

Baseline: RCH `cargo test --workspace --all-features --locked` at dfaa5a1 passed.
The flate2 and futures upgrades passed their first workspace tests, but external
checkout resets at 20:39:55 and 20:41:07 UTC removed those lockfile changes and
the uncommitted source/doc edits. Original handwritten patches were recovered
from the session log; dependency changes were reapplied and sequentially retested.
Those earlier runs do not certify the recovered final checkout.

Transitive updates are researched from published manifests, changelogs, and
tagged source before mutation. Each update gets affected-crate tests before the
next update; the final workspace suite checks integration. Target-specific
dependencies receive native regression checks, not cross-platform certification.
No snapshots are regenerated. Prior-graph workspace checks and the refreshed
dependency audit are recorded below.
The initial 46 researched package/family entries have passing affected-crate
tests. The final missed compatible patch, clap_lex 1.1.1 (published September
14), passed 528 HTTP all-targets/all-feature tests. Benchmark harnesses ran
in test mode; this is no throughput claim. Final `cargo update --dry-run`
now proposes only the held seven-package wasm family and its forbidden Tokio
addition. All 46 tested target versions remain locked; none of the seven
forbidden runtime crates occurs in the 312-package lockfile.
The refreshed `cargo audit --json` passed with zero known vulnerabilities,
no advisory ignores, and the existing unmaintained bincode 1.3.3
(RUSTSEC-2025-0141) / yaml-rust 0.4.5 (RUSTSEC-2024-0320) notices.
All seven prior-graph workspace gates, including actual Rust 1.95, passed as
recorded below; all 13 hosted jobs subsequently passed at 6b358bd.
Scoped UBS static scans were run before this commit. The final changed Rust
file (`extract.rs`) returned exit 1: eight critical pattern matches are four
unchanged test-only panic assertions, three test-only request-header setup
calls mistaken for response sinks, and public cookie-name comparison mistaken
for secret comparison. Source and test-module boundaries were inspected;
assertions were retained and no scanner suppression was added. The broader
five-file scan also returned 1 (161 critical pattern matches); sampled real
middleware policy/redirect gaps are recorded in the existing architecture
coverage matrix. This is not a clean UBS or comprehensive security result.
UBS Cargo phases were explicitly not evaluated; compilation runs through RCH.

The first final MSRV attempt stalled in source transfer on vmi1149989. At the
observed `sync_up` phase, `execution_started=false`; the later recovered lease
records execution as started, so no claim is made that Cargo never ran during
cancellation/recovery. Exact wrapper
`rchw-77ef5ef7-4dc3-4afb-9dbf-f53c91e98bbd` / build `30050444235506058`
was recovered with terminal acknowledgement, exit 137, and a dead wrapper
before retry on vmi1153651. The caller returned 143; neither result passes
the MSRV gate. Two subsequent attempts on vmi1153651 exhausted the unchanged
1800-second limit and were acknowledged terminal with exit 137 before retry.
The identical check then passed on admissible vmi1264463 in 235.1 seconds.
Compiler limits and the requested check remained unchanged.

### Prior-graph workspace verification

Code and lockfile revision: `a8c265a93acc4147fa65e94992fbd5cbeb9c5726`.
All compiler commands below ran remotely through strict RCH, serialized with
`--jobs 2`. These final commands use the normal development/test profiles;
the earlier `profile.test.debug=0` scopes do not substitute for this suite.

| Gate | Result |
|---|---|
| `cargo check --workspace --all-targets --locked --jobs 2` | Passed. |
| `cargo clippy --workspace --all-targets --locked --jobs 2 -- -D warnings` | Passed. |
| `cargo check --workspace --all-targets --all-features --locked --jobs 2` | Passed. |
| `cargo clippy --workspace --all-targets --all-features --locked --jobs 2 -- -D warnings` | Passed. |
| `cargo test --workspace --all-features --locked --quiet --jobs 2` | 2601 passed, zero failed; 188 existing ignores, including doctests. Includes the nine macro consumers and runnable Header example. |
| `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --all-features --locked --jobs 2` | Passed; flags forwarded through `RCH_ENV_ALLOWLIST=RUSTDOCFLAGS`. |
| `cargo +1.95.0 check --workspace --all-targets --all-features --locked --jobs 2` | Passed remotely on vmi1264463 after the acknowledged failed attempts above. |
| `cargo fmt --check` and `git diff --check` | Passed locally after the Header import correction. |

[Hosted CI at a8c265a](https://github.com/Dicklesworthstone/fastapi_rust/actions/runs/37257327749)
was superseded by a metadata-only preservation push at
`6b358bd8d7a2c312a542685b58c0961950a17bde`; source, lockfile and workflows
were identical. Normal cancellation did not release the queued replacement;
the exact superseded run was force-cancelled through GitHub's documented API,
retaining its logs. [Replacement CI at 6b358bd](https://github.com/Dicklesworthstone/fastapi_rust/actions/runs/37261029752)
completed with all 13 jobs successful, including all five release targets.
Raw Ubuntu, macOS and Windows logs show all nine named macro consumers
passing with no ignored consumers, and the Header example passing in both
core doctest runs (26 passed; 135/131 existing ignores). Parent and reviewer
retrievals of the same logs count as one hosted execution per OS.

### October 5 registry recheck

While the prior graph was completing validation, powerfmt 0.2.1 (09:01 UTC)
and unicase 2.10.0 (13:11 UTC) were published. Both were researched from primary
registry metadata and upstream source before mutation, then updated separately;
each passed 333 affected output tests through strict RCH before the next update.
All 18 direct dependencies and seven Actions release selectors were rechecked
and remained current. The complete upgrade record now covers 48 researched
package/family entries: 46 sequential-table rows plus the initial flate2 and
direct syn entries. The current lockfile differs from the prior certified
graph in these two releases. All seven workspace gates subsequently passed
on the final dependency graph, with the same commands, profiles, and warning
limits listed above: the full suite ran 2601 passing tests, zero failures,
and 188 existing ignores. The source and lockfile at `fc61caa` and `7c53067`
are identical; the latter changes upgrade-log wording only. The rustdoc
compiler returned 0, but its original caller returned 102 after artifact
retrieval exhausted local disk space. Native RCH recovery of the same wrapper
`rchw-a8104335-a66e-49fe-9bc9-8f8965b21215` / build
`30050444235506615` returned 0 and delivered all eight documentation indexes.
The original failure and staging remain retained; no compiler replay or
deletion was used to manufacture a passing result.

Current-graph hosted CI remains pending. At `7c53067`, all seven initial
jobs (audit, three OS test jobs, formatting, Clippy, docs) passed. The x86_64
macOS release binary compiled successfully, then artifact creation failed
with `ENOTFOUND`; this is not a successful release job or an all-green CI run.
[Current-graph CI](https://github.com/Dicklesworthstone/fastapi_rust/actions/runs/37357680149).
The subsequently implemented typed OpenAPI and HTTPS redirect capabilities
(`fr-z7jj`, `fr-e7f9`) passed fresh source validation on October 6 UTC, with
the seven workspace gates: default and all-feature all-target checks and
strict Clippy, the full all-feature suite, documentation, and actual Rust
1.95. Normal profiles and compiler deadlines remain unchanged. The full
suite ran 2,575 ordinary tests and 51 doctests successfully, zero failures,
and 188 existing ignores. All 14 macro consumers, the expanded raw-identifier
query case, all three real TCP redirect regressions, and the mismatched
response-model compile-fail doctest passed. A separate `--show-output`
doctest confirmed the intended `ResponseProduces<u32>` mismatch was the
sole compiler error. All eight documentation indexes were retrieved. An
additional documentation run passed with `-D warnings` set explicitly using
Cargo's forced `env.RUSTDOCFLAGS` configuration; the remote command and
terminal receipt make that strict gate directly reviewable.
The first full-suite attempt failed seven stale converter/wildcard path
lookups. Primary OpenAPI rules and independent source review justified
canonical document keys; route constructors and all existing semantic
assertions remain, with additional raw-key rejection and mandatory path
parameter checks. Earlier compiler, Clippy, deadline, cancellation, and
disk-write failures remain retained; they are not passing attempts.
Fresh hosted CI was still required at that checkpoint. The completed run below
fulfills `bd-3ffo`'s original requirement without waiving any job.

### October 6 completed hosted validation

[CI at `cfc29fc`](https://github.com/Dicklesworthstone/fastapi_rust/actions/runs/37455186231)
completed successfully on October 6 at 13:27 UTC: all 13 jobs passed, including
all five release targets and their artifact uploads. Each of Linux, macOS and
Windows passed 2,632 all-feature tests with zero failures, 188 existing ignores
and zero filtered tests; each also ran a separate default doctest suite with
53 passes. Formatting, strict Clippy and warnings-denied documentation passed.
The audit checked the same 312-package graph and reported no known
vulnerabilities, retaining the existing unmaintained bincode and yaml-rust
warnings. Independent review checked the actual raw logs, not just job labels.

The 48 researched update entries remain in the accepted graph. Cargo.toml,
Cargo.lock, CI and toolchain are unchanged between that hosted revision and
the subsequent authentication integration at `da27a27`. That later feature
passed its own seven strict remote RCH workspace gates, including actual Rust
1.95 and 2,649 tests; the older hosted run is evidence for its cited revision,
not a hosted result for the later authentication source. Earlier failed,
cancelled and disk-recovery attempts remain recorded above and in Beads.

The current-graph resolver dry run proposes only the held
seven-package wasm family plus Tokio; all 48 tested targets remain in the
312-package lockfile, with none of the seven forbidden runtime crates.
The October 5 audit again passed with zero known vulnerabilities, no advisory
ignores, and the same two unmaintained notices. Local formatting and whitespace
checks passed. UBS scoped to the changed lockfile/log/bead files returned 0
(one detected Rust input, four informational items, no critical/warning
matches). No Rust source changed in that dependency-only continuation; its targeted result
does not supersede the earlier source scan findings or certify security.
The subsequent 16-file capability source scan returned 1: 171 critical,
2,340 warning, and 821 informational pattern matches. Changed redirect
authority/target sinks and test-only matches were inspected; no scanner
suppression was added. This is neither an exhaustive finding audit nor a
clean security certificate. Compilation checks ran exclusively through RCH.

The latest generic-array 0.14.9 is excluded by the upstream
[crypto-common 0.1.7 manifest](https://docs.rs/crate/crypto-common/0.1.7/source/Cargo.toml),
which requires exactly `=0.14.7`. No dependency override or upstream patch is
introduced to defeat that constraint. The seven-package wasm family remains
held for the project's explicit Tokio ban, as documented above.

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
The log update was refused on ovh-a after its disk-pressure threshold was
crossed; no tests ran there. The same locked update passed 333 tests on
vmi1149989. Later regex-automata synchronization stalled for over twelve
minutes on vmi1149989 with `execution_started=false`. Cancellation initially
remained unconfirmed; recovery acknowledged exact wrapper
`rchw-ced97431-9d61-4afb-8510-c396c8d9f787` / build `30050444235505946`
as finished with exit 1 and no execution before retry on another worker.
No later dependency changed while that validation was unresolved.

The latest wasm-bindgen-futures (0.4.79) adds a normal Tokio dependency under an
Emscripten cfg, which Cargo records even on Linux. The project forbids this.
Use the preceding Tokio-free family: futures 0.4.78, bindgen 0.2.128, and
js-sys/web-sys 0.3.105. [Published manifest](https://raw.githubusercontent.com/wasm-bindgen/wasm-bindgen/0.2.129/crates/futures/Cargo.toml).
The first futures-only resolver attempt failed because locked web-sys required
js-sys 0.3.103. Selecting both futures and web-sys with precise futures 0.4.78
lets Cargo update the exactly coupled family atomically; its dry run confirmed
all seven intended versions without Tokio. No manual checksum edits were needed.

### Sequential transitive validation

Smallvec also reaches asupersync and parking_lot_core. Before the next update, RCH `cargo test --workspace --all-features --lib --locked --quiet --config profile.test.debug=0 --jobs 2` passed all 2110 library tests on vmi1227854; this supplements the output-only check.

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
| num-integer | 0.1.46 → 0.1.47 | [Source](https://static.crates.io/crates/num-integer/num-integer-0.1.47.crate): u128 square-root fixes. | RCH `cargo test --workspace --all-features --lib --locked --quiet --config profile.test.debug=0 --jobs 2` passed. |
| pest / derive / generator / meta | 2.8.8 → 2.9.2 | [Source](https://github.com/pest-parser/pest/releases/tag/v2.9.2): Coupled generator/derive/meta; Unicode 18. | RCH `cargo test -p fastapi-output --all-features --locked --quiet --config profile.test.debug=0 --jobs 2` passed. |
| pkg-config | 0.3.33 → 0.3.34 | [Source](https://static.crates.io/crates/pkg-config/pkg-config-0.3.34.crate): MSRV 1.63; cflags controls. | RCH `cargo test -p fastapi-output --all-features --locked --quiet --config profile.test.debug=0 --jobs 2` passed. |
| plist | 1.10.0 → 1.10.1 | [Source](https://static.crates.io/crates/plist/plist-1.10.1.crate): Migrated quick-xml 0.42 and base64 0.23 internally. | RCH `cargo test -p fastapi-output --all-features --locked --quiet --config profile.test.debug=0 --jobs 2` passed. |
| rand | 0.8.7 → 0.8.8 | [Source](https://static.crates.io/crates/rand/rand-0.8.8.crate): serde1 feature fix. | RCH `cargo test --workspace --all-features --lib --locked --quiet --config profile.test.debug=0 --jobs 2` passed. |
| regex-automata | 0.4.16 → 0.4.18 (already locked) | [Source](https://github.com/rust-lang/regex/compare/regex-automata-0.4.16...regex-automata-0.4.18): Configurable pool capacity. | RCH `cargo test -p fastapi-output --all-features --locked --quiet --config profile.test.debug=0 --jobs 2` passed. |
| rustix | 1.1.4 → 1.1.5 | [Source](https://github.com/bytecodealliance/rustix/compare/v1.1.4...v1.1.5): MSRV 1.65; libc statx and timeout changes. | RCH `cargo test -p fastapi-output --all-features --locked --quiet --config profile.test.debug=0 --jobs 2` passed. |
| signal-hook | 0.4.4 → 0.4.5 | [Source](https://static.crates.io/crates/signal-hook/signal-hook-0.4.5.crate): Close-on-exec; drops Android below API 21. | RCH `cargo test -p fastapi-output --all-features --locked --quiet --config profile.test.debug=0 --jobs 2` passed. |
| smallvec | 1.15.2 → 1.16.2 | [Source](https://github.com/servo/rust-smallvec/releases/tag/v1.16.2): Retain/drop safety fixes. | RCH `cargo test -p fastapi-output --all-features --locked --quiet --config profile.test.debug=0 --jobs 2` passed. |
| thiserror | 2.0.19 → 2.0.21 | [Source](https://github.com/dtolnay/thiserror/releases/tag/2.0.21): Coupled derive generic parsing fix. | RCH `cargo test --workspace --all-features --lib --locked --quiet --config profile.test.debug=0 --jobs 2` passed. |
| tinyvec | 1.12.0 → 1.13.3 | [Source](https://github.com/Lokathor/tinyvec/blob/v1.13.3/changelog.md): Initialization and alloc fixes. | RCH `cargo test -p fastapi-output --all-features --locked --quiet --config profile.test.debug=0 --jobs 2` passed. |
| unicode-ident | 1.0.24 → 1.0.26 | [Source](https://github.com/dtolnay/unicode-ident/releases/tag/1.0.26): Unicode 18 identifier handling. | RCH `cargo test -p fastapi-macros --locked --quiet --config profile.test.debug=0 --jobs 2` passed. |
| zerocopy | 0.8.55 → 0.8.59 | [Source](https://github.com/google/zerocopy/releases/tag/v0.8.59): Layout/read/transmute and paired derive fixes. | RCH `cargo test --workspace --all-features --lib --locked --quiet --config profile.test.debug=0 --jobs 2` passed. |
| clap_lex | 1.1.0 → 1.1.1 | [Source](https://github.com/clap-rs/clap/compare/clap_lex-v1.1.0...clap_lex-v1.1.1): Missed compatible patch found in final dry-run; MSRV 1.85 unchanged; published-source comparison found internal OsStr spelling and packaging/lint maintenance, with no consumer API break identified. | RCH `cargo test -p fastapi-http --all-targets --all-features --locked --quiet --config profile.test.debug=0 --jobs 2` passed. |
| powerfmt | 0.2.0 → 0.2.1 | [Source](https://github.com/jhpratt/powerfmt/blob/3dd45162a7e64d6b354675e4282d0550dce8bfc8/CHANGELOG.md): Published Oct5 09:01 UTC; MSRV rises to 1.79; fixes the always-exported `padded_width_of!` alternate flag; only optional procedural macros remain disabled through time. | RCH `cargo test -p fastapi-output --all-features --locked --quiet --config profile.test.debug=0 --jobs 2` passed. |
| unicase | 2.9.0 → 2.10.0 | [Source](https://github.com/seanmonstar/unicase/releases/tag/v2.10.0): Published Oct5 13:11 UTC; Unicode18 and three-character folding-order fixes change some equality results; MSRV undeclared; no normal dependencies. | RCH `cargo test -p fastapi-output --all-features --locked --quiet --config profile.test.debug=0 --jobs 2` passed. |


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
