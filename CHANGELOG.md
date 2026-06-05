# Changelog

All notable changes to [fastapi_rust](https://github.com/Dicklesworthstone/fastapi_rust) are documented here.

This project is a Rust web framework inspired by Python's [FastAPI](https://fastapi.tiangolo.com/), built on [asupersync](https://github.com/Dicklesworthstone/asupersync) for structured concurrency. The workspace contains 8 crates: `fastapi` (facade), `fastapi-core`, `fastapi-http`, `fastapi-router`, `fastapi-macros`, `fastapi-openapi`, `fastapi-types`, and `fastapi-output`.

Commit links point to `https://github.com/Dicklesworthstone/fastapi_rust/commit/<hash>`.

---

## [Unreleased] (after v0.2.0)

> Commits on `main` since the v0.2.0 tag (`1170506`), as of 2026-03-21.
> HEAD: [`e9bccd3`](https://github.com/Dicklesworthstone/fastapi_rust/commit/e9bccd35309c001c786667fa56fe81ec3a399eca) (13 commits after v0.2.0)

### Server Connection Lifecycle Hardening (2026-03-07)

A focused reliability pass on the TCP server's connection management in `fastapi-http`, fixing multiple race conditions and panic-safety gaps:

- **RAII connection slot cleanup**: introduced `ConnectionSlotGuard` so connection slots are always freed -- even if a handler task panics. The guard is now created *before* spawning the async task, closing a slot-leak window ([`8b29fcb`](https://github.com/Dicklesworthstone/fastapi_rust/commit/8b29fcb89196c8a349f36c28d53c11bfd898a469), [`4a01487`](https://github.com/Dicklesworthstone/fastapi_rust/commit/4a01487a24b9d47f358035badfca0deaa357535b))
- **Panic isolation in connection tasks**: handler futures are wrapped so a panic produces a 500 response and clean drain instead of crashing the accept loop ([`24b7b94`](https://github.com/Dicklesworthstone/fastapi_rust/commit/24b7b94cdb7730d78d53384844cf5a55169d243e))
- **Shutdown-aware accept loop**: `poll_accept` now uses a timeout so the server notices shutdown signals promptly instead of blocking indefinitely ([`cf46b87`](https://github.com/Dicklesworthstone/fastapi_rust/commit/cf46b871d2f0b39a78ee56992e3eaf7972f0dbe4))
- **Race condition elimination**: fixed TOCTOU race in runtime task cleanup test ([`63d9778`](https://github.com/Dicklesworthstone/fastapi_rust/commit/63d9778976d951092cc5a3d8a27e16c6e7092087))
- **Refactored serve_concurrent**: now uses `RuntimeHandle::spawn` and exposes connection helpers for composability ([`04e3206`](https://github.com/Dicklesworthstone/fastapi_rust/commit/04e320647583dc5f143a34ee7d94fc80f1a3f5aa))

### License Update (2026-02-21)

- License changed from MIT to MIT with OpenAI/Anthropic Rider; README references updated ([`ab01e0e`](https://github.com/Dicklesworthstone/fastapi_rust/commit/ab01e0e1369d184e042f6a7099f5b3f45784083f), [`38ada79`](https://github.com/Dicklesworthstone/fastapi_rust/commit/38ada79c3c3071f2ac149c534988264e5a8d58bf))

### Dependency and CI Updates

- Upgrade workspace dependencies and update `Cargo.lock` for asupersync git source and windows-sys ([`4822d57`](https://github.com/Dicklesworthstone/fastapi_rust/commit/4822d57118e220900fae1a72f9111dbca48934ef), [`96fb6ec`](https://github.com/Dicklesworthstone/fastapi_rust/commit/96fb6ec5f77221e900419f1afdacddae5e04e7ec))
- Update `rich_rust` dependency to 0.2.0 in fastapi-output ([`827009a`](https://github.com/Dicklesworthstone/fastapi_rust/commit/827009ae1151cb72f1233ba6411976209c94dfe7))
- Pin asupersync rev for polling 3.11 reactor compatibility ([`9fe499e`](https://github.com/Dicklesworthstone/fastapi_rust/commit/9fe499ee03f39126f042d46af4eb710509c630ec))
- Dependabot: bump GitHub Actions group (multiple PRs on remote branches)

### Housekeeping

- Add missing ephemeral file patterns to `.gitignore` ([`7e8ee5f`](https://github.com/Dicklesworthstone/fastapi_rust/commit/7e8ee5f9c1c9289feb66acc49234f5d21b9a0ec4))
- Remove stale `a.out` and `.beads/issues.jsonl.bak` ([`7616b83`](https://github.com/Dicklesworthstone/fastapi_rust/commit/7616b8377dfc93ab694fcd4b6f2b78fb39289c19))
- Add cass (Cross-Agent Session Search) tool reference to AGENTS.md ([`fd488ab`](https://github.com/Dicklesworthstone/fastapi_rust/commit/fd488ab754b1844c22996c8b2adbeae14c29b899))

---

## [v0.2.0] -- 2026-02-15

> **Tag**: [`v0.2.0`](https://github.com/Dicklesworthstone/fastapi_rust/releases/tag/v0.2.0)
> **Tag commit**: [`1170506`](https://github.com/Dicklesworthstone/fastapi_rust/commit/1170506a1c467cbed4481e048d075f7771735c8e)
> **GitHub Release**: [fastapi_rust v0.2.0](https://github.com/Dicklesworthstone/fastapi_rust/releases/tag/v0.2.0) (published 2026-02-15)
> **Diff from v0.1.2**: `d12a9de..1170506` (107 commits)

Major hardening and feature release. All 8 workspace crates bumped from 0.1.x to 0.2.0. This release added three large subsystems (WebSocket, HTTP/2, multipart file uploads), removed the Tokio dependency, and completed a comprehensive security audit.

### WebSocket Support (RFC 6455) -- `bd-z09e`

Full RFC 6455 WebSocket protocol, integrated into the App builder and server:

- **Protocol core**: upgrade handshake, binary frame codec, masking, fragmentation, and UTF-8 text validation ([`8b2d273`](https://github.com/Dicklesworthstone/fastapi_rust/commit/8b2d2739f60370b0e7eba4cc5a7bf4d1a1d0b44f), [`870e903`](https://github.com/Dicklesworthstone/fastapi_rust/commit/870e903ca7646c8a0af06bb6af288b0bdce89f34))
- **Routing integration**: WebSocket routes registered alongside HTTP routes in `App` ([`8276ab5`](https://github.com/Dicklesworthstone/fastapi_rust/commit/8276ab519d470d0e89c1c7e07639b7dca29e52a7))
- **Ping/pong handling** with E2E tests ([`8afea7c`](https://github.com/Dicklesworthstone/fastapi_rust/commit/8afea7c889199a29311d4cf520fb79c965686b62), [`7e2bf41`](https://github.com/Dicklesworthstone/fastapi_rust/commit/7e2bf41a396a019a998ff4f269594e06c119b3d5))
- **Framing hardened**: close parsing, RSV validation, fragmented text handling ([`9d4cf15`](https://github.com/Dicklesworthstone/fastapi_rust/commit/9d4cf158a6e4a5ec9bab4dbe8bef4decf4b53596), [`846cf63`](https://github.com/Dicklesworthstone/fastapi_rust/commit/846cf632b5f7ad81501f36cc968e512e581a2824))
- **Token-list upgrade**: accept multi-token `Upgrade` headers per spec ([`1cf4a4f`](https://github.com/Dicklesworthstone/fastapi_rust/commit/1cf4a4fa1a56f8b480646edbcdd7ce10a2a91281), [`a3007b6`](https://github.com/Dicklesworthstone/fastapi_rust/commit/a3007b6f81f0fbfb1067f0c56b3e1570d499d745))
- **Convenience API**: `send_bytes`, `ping`, `close` methods; close handshake receive fix ([`c7e79e1`](https://github.com/Dicklesworthstone/fastapi_rust/commit/c7e79e1176a930ce6566f57c21c29c9e21430bbd))
- **RFC close codes**: send appropriate 1003/1007 codes before error returns ([`1600402`](https://github.com/Dicklesworthstone/fastapi_rust/commit/1600402b4f7a09912adeb690d9b6ba25f1776f56), [`6f1a895`](https://github.com/Dicklesworthstone/fastapi_rust/commit/6f1a89509166268c9ab8eb2fe71e2b8341fc553f))
- **MessageTooLarge** error variant with frame-length pre-check ([`b8d0832`](https://github.com/Dicklesworthstone/fastapi_rust/commit/b8d0832a627844734db0234f6de25ee79c32c1ce))
- **Subprotocol token validation** in handshake ([`87b1575`](https://github.com/Dicklesworthstone/fastapi_rust/commit/87b1575789dc3a91673becb7eed5942b294f6d31))
- **Handshake rejection tests** and helper extraction ([`f6432ed`](https://github.com/Dicklesworthstone/fastapi_rust/commit/f6432edc437ed3ce3764a1edde0fb4283af8a5e4))

### HTTP/2 Implementation (RFC 7540) -- `bd-2c9t`

Substantial HTTP/2 support built from the ground up:

- **h2c preface + HPACK** decoder with initial E2E test ([`564917f`](https://github.com/Dicklesworthstone/fastapi_rust/commit/564917fe9e41b7489e9ca6900d4fed1a34673e68))
- **Flow control**: full RFC 7540 receive-side flow control with WINDOW_UPDATE emission ([`1993983`](https://github.com/Dicklesworthstone/fastapi_rust/commit/199398373a701c3e08065cdd6e5c1b44f1f476c5)), send-side flow control wired into app/handler paths ([`6b5d281`](https://github.com/Dicklesworthstone/fastapi_rust/commit/6b5d2816a158cfeb0a8ea5b558411659ba3577ef), [`76667a8`](https://github.com/Dicklesworthstone/fastapi_rust/commit/76667a80ae5169ca06e91359bcb253dedb286c7f)), and GOAWAY frame support ([`6b5d281`](https://github.com/Dicklesworthstone/fastapi_rust/commit/6b5d2816a158cfeb0a8ea5b558411659ba3577ef))
- **Window overflow detection** per RFC 7540 section 6.9.1 ([`8b8b731`](https://github.com/Dicklesworthstone/fastapi_rust/commit/8b8b731ea85f195ffcd42bd0feb0fede316d2e87))
- **SETTINGS_INITIAL_WINDOW_SIZE delta overflow** detection ([`1e75de6`](https://github.com/Dicklesworthstone/fastapi_rust/commit/1e75de6da3ec2f91439fc1845b5f76c574ec2a00))
- **SETTINGS validation**: ENABLE_PUSH + all RFC 7540 settings recognized ([`a48e9db`](https://github.com/Dicklesworthstone/fastapi_rust/commit/a48e9db9b267ecf8e98078a9359f3bf8c61a4610), [`276b73e`](https://github.com/Dicklesworthstone/fastapi_rust/commit/276b73eaafa900f3c8da9e0eeb5c8cd47e9e0aad))
- **Interleaved frame handling**: WINDOW_UPDATE, PRIORITY, RST_STREAM, and extension frames handled correctly during body reads without dropping connections ([`a8d6dec`](https://github.com/Dicklesworthstone/fastapi_rust/commit/a8d6dec06308d7c43d8cbc0cbc088934dd8ecbd9), [`77753cb`](https://github.com/Dicklesworthstone/fastapi_rust/commit/77753cb0c359400d80ba06450922d64d294a5b49), [`720b16e`](https://github.com/Dicklesworthstone/fastapi_rust/commit/720b16ef264a58589b9d51ce584c6ca91e617ebf), [`b8af645`](https://github.com/Dicklesworthstone/fastapi_rust/commit/b8af6454e2a50d0b999cc8b67c6be8ae0feba52b))
- **GOAWAY/PUSH_PROMISE/DATA-on-stream-0 validation** + PING hardening ([`8c29794`](https://github.com/Dicklesworthstone/fastapi_rust/commit/8c297943bdebdb6c0accc3fbd4b37fec5d3b4d9f))
- **Frame size separation**: split max_frame_size into recv vs peer limits; honor dynamic peer max-frame-size during send waits ([`aa31156`](https://github.com/Dicklesworthstone/fastapi_rust/commit/aa311563c121ea622f1e69e39365b92dee9eafb2), [`b45da8d`](https://github.com/Dicklesworthstone/fastapi_rust/commit/b45da8d2b5926ba283494f268f542b7ad56eb77f))
- **Premature END_STREAM prevention** when flow control clamps send size ([`72b9602`](https://github.com/Dicklesworthstone/fastapi_rust/commit/72b96024bc053a41a4ff8d4a200e4136202e15f4))
- **Stream ID enforcement**, max concurrent streams advertisement, HPACK table cap ([`92e90b8`](https://github.com/Dicklesworthstone/fastapi_rust/commit/92e90b8952e7aa15c4597d3296c3c0a28a8ca99c))
- **CONTINUATION bomb prevention** with header block size limit ([`6a90aa5`](https://github.com/Dicklesworthstone/fastapi_rust/commit/6a90aa5ed44fbc6911b106a1a5afd13ee090c89b))
- **Reject idle DATA/CONTINUATION** frames on unexpected streams ([`330ebb7`](https://github.com/Dicklesworthstone/fastapi_rust/commit/330ebb7f91864866960ee2b074992146157b1bd1))
- E2E test suites for flow control, stream validation, and SETTINGS ([`8aa8ff6`](https://github.com/Dicklesworthstone/fastapi_rust/commit/8aa8ff60b0be94b15437f2e0d445d45e1b53da85), [`c1e9d0f`](https://github.com/Dicklesworthstone/fastapi_rust/commit/c1e9d0fa08ddb3c68dede5a57c42278038e9ff00), [`d14eb74`](https://github.com/Dicklesworthstone/fastapi_rust/commit/d14eb74b756041f6dcfaf79f3ca332370415d5f9))

### Multipart/Form-Data and File Uploads -- `bd-3ess`

- **Multipart parser** with per-field and total size limits ([`d055e7e`](https://github.com/Dicklesworthstone/fastapi_rust/commit/d055e7e87b5c0b7bb02e4dabde06293e171b90b9))
- **Boundary detection hardened** against edge-case payloads ([`771adab`](https://github.com/Dicklesworthstone/fastapi_rust/commit/771adabce2d0d65d2b2567b2b0ba18a264f1dc16))
- **Streaming multipart parser**, `UploadFile` async API (`read`/`write`/`seek`/`close`), and temp-file spooling ([`7af61f0`](https://github.com/Dicklesworthstone/fastapi_rust/commit/7af61f0b10454ae29a30b06c905eb0e9f2fcaec6))
- **Spool-backed file parts** propagated through `MultipartForm` without re-materializing bytes ([`22bfddf`](https://github.com/Dicklesworthstone/fastapi_rust/commit/22bfddfe9a1d7e0aad10845af59c319ca22ba70c))
- **413 extractor mapping** preserved for multipart payloads ([`e4372ca`](https://github.com/Dicklesworthstone/fastapi_rust/commit/e4372cac804c84295d730cef22e5efd84abe99f2))
- **Boundary length cap** to harden parser against abuse ([`afb4b7a`](https://github.com/Dicklesworthstone/fastapi_rust/commit/afb4b7a38da8de6a40868b396bcc2d16da325e37))
- Multipart extractor consolidated into core crate ([`ec35e01`](https://github.com/Dicklesworthstone/fastapi_rust/commit/ec35e013495fce561caef5882c5b8952c9881466))

### HTTP Digest Authentication (RFC 7616 / RFC 2617) -- `bd-gl3v`

- Full Digest auth implementation with RFC 7616 (SHA-256) and RFC 2617 (MD5) support, plus a dedicated extractor ([`3817c16`](https://github.com/Dicklesworthstone/fastapi_rust/commit/3817c16d6fe81a75c1a775e21f3f9f5ba5817821), [`0f2da2c`](https://github.com/Dicklesworthstone/fastapi_rust/commit/0f2da2c7e46fb0b95af3b3be8f0fec71a3f6aca5))

### Security Hardening -- `bd-uz2s`

- **Request smuggling prevention**: reject compound `Transfer-Encoding` headers ([`ee17294`](https://github.com/Dicklesworthstone/fastapi_rust/commit/ee17294c9b97d94de8cb1208089ff453edd43404))
- **Response header injection** protection ([`ad7c2d0`](https://github.com/Dicklesworthstone/fastapi_rust/commit/ad7c2d05154ddc2bac50923925b4d428ccc69dc2))
- **Routing and middleware hardening**: path traversal prevention, CORS origin validation, injection protections ([`17b0976`](https://github.com/Dicklesworthstone/fastapi_rust/commit/17b0976a755d2eeacac2bd9e0d146fff47a8f87e))
- **Content-type validation**, auth token length limits, and UTF-8 URL decoding fixes ([`c95e148`](https://github.com/Dicklesworthstone/fastapi_rust/commit/c95e148519affe54ecaacdda5861e5af102acc4c))
- **URL encoding path segment** fix, root_path normalization, constant_time_eq deduplication ([`cb10d95`](https://github.com/Dicklesworthstone/fastapi_rust/commit/cb10d955f689da3eac596904c44f432c613d170e))
- **Request parsing hardened** with handler path validation ([`db2a892`](https://github.com/Dicklesworthstone/fastapi_rust/commit/db2a892e9926aa85168d3c416cfcade6ca4dd267))
- **Accept-Ranges token** and **Expect token list** parsing tightened ([`7e5d624`](https://github.com/Dicklesworthstone/fastapi_rust/commit/7e5d62499e9c9f0c17a5fcd545f5b3e3dd375d8a), [`a761d5e`](https://github.com/Dicklesworthstone/fastapi_rust/commit/a761d5eaeb9cdf4c6768aeaca064018404162398))
- **Digest auth and multipart Content-Disposition parsing** hardened ([`620ace1`](https://github.com/Dicklesworthstone/fastapi_rust/commit/620ace1f8e764ed750fdc7401a4b06533bd14cad))
- Regression test suites for all parsing hardening work ([`e27818b`](https://github.com/Dicklesworthstone/fastapi_rust/commit/e27818ba31db16c35e8e4ed6e257c851faf950a0))

### Core Framework Improvements

- **Tokio fully removed**: replaced with asupersync-only chunked streaming reads ([`cfe6b87`](https://github.com/Dicklesworthstone/fastapi_rust/commit/cfe6b87290a8647c4a8e576e8216056ac89ec043))
- **Misleading placeholder stubs removed**: async chunked trailers fixed ([`d9afd91`](https://github.com/Dicklesworthstone/fastapi_rust/commit/d9afd91e474c5307c65a1bb060b90e5bb2694dfc))
- **Route macros made runtime-real** (not just compile-time stubs) ([`5f218b4`](https://github.com/Dicklesworthstone/fastapi_rust/commit/5f218b4f22b51c8a2fbebec752b98a928b0469ec))
- **Body size enforcement**: `max_size` limit enforced before completing on `expected_size` ([`886323e`](https://github.com/Dicklesworthstone/fastapi_rust/commit/886323eb79ae722f07883d82678525530639f2b9))
- **Validation parity**: OpenAPI camelCase support, body streaming fixes ([`926b824`](https://github.com/Dicklesworthstone/fastapi_rust/commit/926b824bed6b1e4f838976e26fd78f7384b0c5d4))
- **response_model options**: `by_alias`, `exclude_unset`, `exclude_defaults` fully implemented ([`68376b7`](https://github.com/Dicklesworthstone/fastapi_rust/commit/68376b72492131e25eeffbcb92d321dcc07a64b7))
- **Pre-body validators** wired in; multi-range parsing support added ([`cf1eb95`](https://github.com/Dicklesworthstone/fastapi_rust/commit/cf1eb9507d600560b345db1dbcb3e8e927631bfd))
- **Tuple/unit struct support** in `derive(Validate)` ([`bd23880`](https://github.com/Dicklesworthstone/fastapi_rust/commit/bd238804d36b0d04b0ec5467a211d5fbda810456))
- **Route consolidation**: `Route::new` replaces `Route::with_placeholder_handler` everywhere ([`280bf6b`](https://github.com/Dicklesworthstone/fastapi_rust/commit/280bf6bcffb0ab899fc24d1160d7b9bd2c792494))
- **Parity matrix** added to track feature coverage vs Python FastAPI ([`c827eed`](https://github.com/Dicklesworthstone/fastapi_rust/commit/c827eedf53bb5c52ac87564d580a5e6fb9d09831))
- **Coverage module**: replace hand-built JSON with serde_json for correct escaping ([`9b836e0`](https://github.com/Dicklesworthstone/fastapi_rust/commit/9b836e038ece04a4a250bea8b0de9138b91925f1))

### Release Mechanics

- All 8 workspace crates bumped to 0.2.0 ([`0077351`](https://github.com/Dicklesworthstone/fastapi_rust/commit/0077351f596b22c7e305f87ed9e26ada26786c1c))
- Fix `Debug` derive on `FieldValidation` for syn 2.x compatibility ([`1170506`](https://github.com/Dicklesworthstone/fastapi_rust/commit/1170506a1c467cbed4481e048d075f7771735c8e))
- Merge conflict resolution after workspace publish ([`f9660e2`](https://github.com/Dicklesworthstone/fastapi_rust/commit/f9660e2498c935621d89c52e72b753e6019f031e), [`40365f6`](https://github.com/Dicklesworthstone/fastapi_rust/commit/40365f61b7d90c683f0853917e6e97c67a00a7e6))
- Use local asupersync path to avoid version mismatch with consumers ([`6bae9e8`](https://github.com/Dicklesworthstone/fastapi_rust/commit/6bae9e8cdfd2165a4e56bd6f0b068e4241cb17ff))

---

## [v0.1.2] -- 2026-02-05 (crates.io initial publish)

> No git tag; version set in workspace Cargo.toml.
> **Key commit**: [`d12a9de`](https://github.com/Dicklesworthstone/fastapi_rust/commit/d12a9de34f3d41bc19a0b387eee010af1143258a) -- align workspace deps to 0.1.1 / fastapi-rust 0.1.2
> **Diff from project start**: `1fbd93f..d12a9de` (204 commits over 19 days)

First public crates.io release. The crate was renamed from `fastapi` to `fastapi-rust` (Rust import name `fastapi_rust`) to avoid crate name conflicts. Workspace internal crates versioned at 0.1.1, facade crate at 0.1.2.

### Publish Logistics

- Prepared metadata, docs, and categories for crates.io ([`6b933a4`](https://github.com/Dicklesworthstone/fastapi_rust/commit/6b933a4def3405ae45c01e927f8a7bbb962c88f2))
- Renamed package to `fastapi-rust` for crates.io ([`6631d60`](https://github.com/Dicklesworthstone/fastapi_rust/commit/6631d60dd415fb96def15c822c53c7daa95f5189), [`0c4728c`](https://github.com/Dicklesworthstone/fastapi_rust/commit/0c4728c9903451b43d5864928d9407ff802f6cc8))
- Workspace deps aligned to 0.1.1 (internal crates) and 0.1.2 (facade) ([`d12a9de`](https://github.com/Dicklesworthstone/fastapi_rust/commit/d12a9de34f3d41bc19a0b387eee010af1143258a))

---

## Pre-Release Development (2026-01-17 -- 2026-02-04)

> **Initial commit**: [`1fbd93f`](https://github.com/Dicklesworthstone/fastapi_rust/commit/1fbd93f0d25e1a53e1b96c46f9a5d572344e1afd) (2026-01-17)
> Rapid development phase: 204 commits in 19 days, building the framework from spec extraction through a comprehensive feature set ready for crates.io.

### Project Bootstrap (2026-01-17)

The project began with a FastAPI spec extraction exercise, producing an architecture document that maps Python idioms to Rust equivalents (decorators to proc macros, Pydantic to serde + compile-time validation, ASGI to structured concurrency regions).

- FastAPI spec extraction and Rust architecture design ([`1fbd93f`](https://github.com/Dicklesworthstone/fastapi_rust/commit/1fbd93f0d25e1a53e1b96c46f9a5d572344e1afd), [`d87e360`](https://github.com/Dicklesworthstone/fastapi_rust/commit/d87e36008099c19911866610461ad336020eaab0))
- asupersync integration architecture ([`40f6eed`](https://github.com/Dicklesworthstone/fastapi_rust/commit/40f6eedfced88628087e2878caa05bd1d728ef3b))
- Workspace initialized with `fastapi-core`, `fastapi-http`, `fastapi-router`, `fastapi-macros`, `fastapi-openapi` crates ([`5cb282f`](https://github.com/Dicklesworthstone/fastapi_rust/commit/5cb282fc38078dff1bd9e7c718daa3807ff60653))

### HTTP Parser and Server

Zero-copy HTTP/1.1 parser and TCP server, built directly on asupersync's I/O primitives with no Tokio/Hyper dependency:

- **Zero-copy HTTP/1.1 parser** with borrowed header types, lifetime-correct `get_all` ([`5cb282f`](https://github.com/Dicklesworthstone/fastapi_rust/commit/5cb282fc38078dff1bd9e7c718daa3807ff60653), [`04ddd37`](https://github.com/Dicklesworthstone/fastapi_rust/commit/04ddd3700ec91cc8e6ac77b3a3190034fc897fb4))
- **TCP server** connection handling with asupersync I/O ([`b098dfe`](https://github.com/Dicklesworthstone/fastapi_rust/commit/b098dfe02fe1d482260722c5cd78bc6eac5ce6e4))
- **Concurrent connection handling** with OpenAPI schema generation ([`605c58c`](https://github.com/Dicklesworthstone/fastapi_rust/commit/605c58c1916d576478231fe031e2ede4ab6dc722))
- **Keep-alive timeout** for idle connections ([`94aaf38`](https://github.com/Dicklesworthstone/fastapi_rust/commit/94aaf3890b01b070a6ea90c038795cb083fcbfc8))
- **Graceful shutdown controller** with coordinated server shutdown ([`021b058`](https://github.com/Dicklesworthstone/fastapi_rust/commit/021b0589e7254f313f1837c56e563e5b4b08ebc1))
- **HEAD request handling** per RFC 7231 ([`3af3b1a`](https://github.com/Dicklesworthstone/fastapi_rust/commit/3af3b1ab75e03089fc8ca0864d65417ee015a753))
- **Async streaming body readers** and security schemes ([`f1905a9`](https://github.com/Dicklesworthstone/fastapi_rust/commit/f1905a9d859dc313404ca660169f36d3e646bc1a))
- **Link header builder** and HTTP pipelining tests ([`3237924`](https://github.com/Dicklesworthstone/fastapi_rust/commit/32379246c645b71e4eb6029aaea00ad6d8f401dc))
- **API versioning** and HTTP trailers support ([`fbd2e63`](https://github.com/Dicklesworthstone/fastapi_rust/commit/fbd2e6385afe5da00fa0a7effdbb972442677270))
- **ETag conditionals** and trailing slash redirect ([`6158538`](https://github.com/Dicklesworthstone/fastapi_rust/commit/6158538290fe7e09e39e930cf4be5d7eea69e14d))
- **Static file serving** with caching support ([`48db8f3`](https://github.com/Dicklesworthstone/fastapi_rust/commit/48db8f3f6b929a3f54d1eba48cb808f6f7e8231a))
- **Server metrics** and response examples ([`12297e5`](https://github.com/Dicklesworthstone/fastapi_rust/commit/12297e59dfa8a349c4058e435a88520aa2f1d5c5))
- Replaced polling-based timeout with asupersync async timeout ([`f4fe3f0`](https://github.com/Dicklesworthstone/fastapi_rust/commit/f4fe3f02e9d10af14ea3024b97a4b3486d4a88d8))

### Routing

Radix trie router with compile-time route validation via proc macros:

- **Radix trie router** with path matching and conflict detection ([`7c35166`](https://github.com/Dicklesworthstone/fastapi_rust/commit/7c351667e4bc0e46920ab0dd22e65864fae57bfa))
- **Route priority scoring** and enhanced match semantics ([`9e0e06b`](https://github.com/Dicklesworthstone/fastapi_rust/commit/9e0e06b84eba51e92473f89e54eecae1e37d5c55))
- **Route registry** and centralized route management ([`169d8e1`](https://github.com/Dicklesworthstone/fastapi_rust/commit/169d8e18940ca5f0c60169971c350463266c7e38))
- **Route constraint validation** and trie enhancements ([`bccc5d5`](https://github.com/Dicklesworthstone/fastapi_rust/commit/bccc5d5e34a0488a0054ddf0d5684c94f300c991))
- **Zero-allocation path matching** with stack buffer ([`4812562`](https://github.com/Dicklesworthstone/fastapi_rust/commit/481256255031c22baac9eb2e3952ab757714c098))
- **308 Permanent Redirect** for route lookups ([`3dd5d5c`](https://github.com/Dicklesworthstone/fastapi_rust/commit/3dd5d5cff1c2b123cd687e8566a03fef0e92be7e))
- **Sub-application mounting** support ([`f0ed512`](https://github.com/Dicklesworthstone/fastapi_rust/commit/f0ed51225f89eee8cf6c91a220a9d20bd71fe54a))
- **URL generation** / reverse routing system ([`3199dce`](https://github.com/Dicklesworthstone/fastapi_rust/commit/3199dcec10d471e770fae5121442bfe325ac94bc))
- **Route-level security requirements** ([`adfd86f`](https://github.com/Dicklesworthstone/fastapi_rust/commit/adfd86f947115d8ce7e38dcc40adde978ccee0fb))
- **Trie optimization** and route macro improvements ([`3dccce0`](https://github.com/Dicklesworthstone/fastapi_rust/commit/3dccce0c1c7611f30c13ca89e34f537d2a8941cc), [`df1d547`](https://github.com/Dicklesworthstone/fastapi_rust/commit/df1d54758223030871178b3e13a0e9412a587b1a))

### Extractors

Type-driven extractors that declare parameter sources and let the framework handle extraction and validation:

- **Path\<T\>** extractor for typed path parameter extraction ([`8605732`](https://github.com/Dicklesworthstone/fastapi_rust/commit/8605732821f61c520680b8c9df7f4ad50d91d8de))
- **BearerToken** extractor for auth header parsing ([`fe4ab99`](https://github.com/Dicklesworthstone/fastapi_rust/commit/fe4ab99430d292e8d72f8960cb72bd5d61bf90b1))
- **Form body** extractor for URL-encoded data ([`285f012`](https://github.com/Dicklesworthstone/fastapi_rust/commit/285f012972d9708fa46ef7a512a0b704949acadb))
- **API key extractors**: `ApiKeyHeader`, `ApiKeyQuery`, `ApiKeyCookie` ([`99d8967`](https://github.com/Dicklesworthstone/fastapi_rust/commit/99d89672ec2a4afdffd2d1c99a5de5a10eb4db71), [`2d33a70`](https://github.com/Dicklesworthstone/fastapi_rust/commit/2d33a7076908fa3682a76ac2917cd3a07cbf64c1))
- **Raw body extractors**: `Bytes`, `StringBody` ([`92cec70`](https://github.com/Dicklesworthstone/fastapi_rust/commit/92cec70b7774888bcce3c764bf47bd439abbaa7d))
- **Range request** support ([`50aacd6`](https://github.com/Dicklesworthstone/fastapi_rust/commit/50aacd6c719ae4b89e2244913396ffa04d8b228f))
- **Content negotiation** system ([`fe107b1`](https://github.com/Dicklesworthstone/fastapi_rust/commit/fe107b185915f123240904242db301023a2cd030))
- **DigestAuth** extractor and fault injection module ([`2d1706c`](https://github.com/Dicklesworthstone/fastapi_rust/commit/2d1706cc1504a740f2bab1416174b5d5541c14bc))
- **Pagination helpers** and response wrapper ([`24c2cc6`](https://github.com/Dicklesworthstone/fastapi_rust/commit/24c2cc6dcb505f7191a1dd91711e4a35750edc5d))

### Middleware

Composable middleware with onion model execution (first registered runs first on the way in, last on the way out):

- **Middleware system** with before/after hooks ([`349e201`](https://github.com/Dicklesworthstone/fastapi_rust/commit/349e20185fde046cc81bbe366f971f00f7dca5a1))
- **CORS middleware** with configurable origins, methods, headers ([`867e8ae`](https://github.com/Dicklesworthstone/fastapi_rust/commit/867e8ae4df68362e365161efc97d3cfde72583ea))
- **CSRF middleware** with token validation ([`867e8ae`](https://github.com/Dicklesworthstone/fastapi_rust/commit/867e8ae4df68362e365161efc97d3cfde72583ea))
- **Cache-Control middleware** with builder API ([`40e618f`](https://github.com/Dicklesworthstone/fastapi_rust/commit/40e618fdb921d1fb38329fd68d0ac27c3d10698c))
- **Response interceptors and transformers** ([`175e543`](https://github.com/Dicklesworthstone/fastapi_rust/commit/175e54337a3f392a28b4506a426cc56629a2c177))
- **Response timing metrics** collection ([`978c981`](https://github.com/Dicklesworthstone/fastapi_rust/commit/978c9812e8dcaf23384a388c14c4893d0567daeb))
- **RequestId middleware** and request/response logger ([`349e201`](https://github.com/Dicklesworthstone/fastapi_rust/commit/349e20185fde046cc81bbe366f971f00f7dca5a1))

### Dependency Injection

- **Request-scoped DI** with caching: resolve once per request or per call ([`6e711b4`](https://github.com/Dicklesworthstone/fastapi_rust/commit/6e711b4f315752399da1146d7b88a5a03e8b6645), [`7892d0a`](https://github.com/Dicklesworthstone/fastapi_rust/commit/7892d0aa12545fa7fdd3e42a4812dbf33d4dd4ee))
- **Dependency override** integration for testing ([`64d94f3`](https://github.com/Dicklesworthstone/fastapi_rust/commit/64d94f3ee724d7099c014aa7883b2cd0cb064460), [`d47744b`](https://github.com/Dicklesworthstone/fastapi_rust/commit/d47744b655b859baee0f02f4eb52743a58a59ffc))
- **Scope constraint validation** (request, function, no-cache) ([`7a53822`](https://github.com/Dicklesworthstone/fastapi_rust/commit/7a538221394ce2c92a35e14e494e920dc6a951d9))
- **Circular dependency detection** with tests ([`63f5aa6`](https://github.com/Dicklesworthstone/fastapi_rust/commit/63f5aa61f12772436cf0a7b5c0892484a0716545))

### OpenAPI Specification Generation

- **OpenAPI 3.1 schema generation** from route metadata ([`605c58c`](https://github.com/Dicklesworthstone/fastapi_rust/commit/605c58c1916d576478231fe031e2ede4ab6dc722))
- **Validation constraints** embedded in schema ([`09ba510`](https://github.com/Dicklesworthstone/fastapi_rust/commit/09ba5101a710aaf06ed413b69b9b37a9dc65fed9))
- **Security schemes** support ([`f1905a9`](https://github.com/Dicklesworthstone/fastapi_rust/commit/f1905a9d859dc313404ca660169f36d3e646bc1a))
- **Swagger UI and ReDoc** documentation endpoints ([`8bd462f`](https://github.com/Dicklesworthstone/fastapi_rust/commit/8bd462f2e9a96274883119409ff1b892d62147a1))
- **Compile-time response type verification** for OpenAPI consistency ([`a173b98`](https://github.com/Dicklesworthstone/fastapi_rust/commit/a173b98f6c75a4b6960d11a15bfa307e14d88418))
- **Compile-time JSON validation** for schema examples ([`bece7c2`](https://github.com/Dicklesworthstone/fastapi_rust/commit/bece7c2fd635f19fed2c5c68d71c1a12dc18d938))
- **Auth indicators** and **schema depth limiting** in OpenAPI display ([`0e746be`](https://github.com/Dicklesworthstone/fastapi_rust/commit/0e746be9d8098d89417b766f2e3de66fc0c468cf), [`c75bf1d`](https://github.com/Dicklesworthstone/fastapi_rust/commit/c75bf1d7796cc792d536bb906414b9246ae8bc7f))
- **Response examples** in generated spec ([`12297e5`](https://github.com/Dicklesworthstone/fastapi_rust/commit/12297e59dfa8a349c4058e435a88520aa2f1d5c5))

### Procedural Macros

- **Route macros**: `#[get]`, `#[post]`, etc. generating real route registrations ([`5cb282f`](https://github.com/Dicklesworthstone/fastapi_rust/commit/5cb282fc38078dff1bd9e7c718daa3807ff60653))
- **`#[derive(Validate)]`** with email, phone, contains, starts_with, ends_with validators ([`b1b9c3f`](https://github.com/Dicklesworthstone/fastapi_rust/commit/b1b9c3f504a99f2f1839021328e12191bc1e449e), [`baa47d1`](https://github.com/Dicklesworthstone/fastapi_rust/commit/baa47d190b12f21ca45c4a972a2e18e7fcb16482), [`1a18c2a`](https://github.com/Dicklesworthstone/fastapi_rust/commit/1a18c2ac24a71c3109663bcacc4a3db37d8057f6))
- **`#[derive(JsonSchema)]`** with generic type support and improved error spans ([`3666ddf`](https://github.com/Dicklesworthstone/fastapi_rust/commit/3666ddfebf02ed5becb3d36d2ff589e5fccdd5fe))
- **Parameter macro** and OpenAPI spec enhancements ([`a777064`](https://github.com/Dicklesworthstone/fastapi_rust/commit/a77706445f58b8475a3d823fe52351cc9f8c2be6))

### Testing Infrastructure

- **TestClient** for in-process HTTP testing without network I/O ([`643605e`](https://github.com/Dicklesworthstone/fastapi_rust/commit/643605e96cfdfbeee63b160fe6a1a14681229cd7))
- **Deterministic testing** with seed-based execution ordering via asupersync Lab runtime
- **Test fixture framework** for reducing boilerplate ([`0d2ae4e`](https://github.com/Dicklesworthstone/fastapi_rust/commit/0d2ae4e326360f92603aab5cd05d41499a8d6ef7))
- **Property-based testing helpers** ([`ca3fc33`](https://github.com/Dicklesworthstone/fastapi_rust/commit/ca3fc33fa70ce8c0b267fd7e05ffa53f5c073c14))
- **Test coverage tracking and reporting** ([`2e31169`](https://github.com/Dicklesworthstone/fastapi_rust/commit/2e31169ba24b463dd1a6e12c649262b09b055dfa))
- **Visual regression tests** with insta snapshots ([`b404000`](https://github.com/Dicklesworthstone/fastapi_rust/commit/b40400086415b96d07240cf01a4e4de4745bf956))
- **HTTP security test suite** ([`bd9fab9`](https://github.com/Dicklesworthstone/fastapi_rust/commit/bd9fab9f498c51f88458c273723e06d9d766c6f9))
- **HTTP benchmarks** for parser performance ([`19cfc8e`](https://github.com/Dicklesworthstone/fastapi_rust/commit/19cfc8e3e591b93ad48f9cb6ccacdad8e5774903))

### Security and Cryptography

- **Password hashing** and load testing modules ([`b43516d`](https://github.com/Dicklesworthstone/fastapi_rust/commit/b43516df8e1e962d8abb8c4e745adc6ae7b368dc))
- **Timing-safe comparison** utilities ([`fe6a4a1`](https://github.com/Dicklesworthstone/fastapi_rust/commit/fe6a4a123095b2c7597d8a80119bb963865526f0))
- **CSRF token generation** fixed to use CSPRNG, weak fallback removed ([`6c6a55b`](https://github.com/Dicklesworthstone/fastapi_rust/commit/6c6a55b0938aa6813aaecd3afadf24dd7304479e), [`0237e83`](https://github.com/Dicklesworthstone/fastapi_rust/commit/0237e83600a069d85d96a9b1f033e37fe66b69de))
- **PBKDF2 iterations increased**, HTML escaping added, chunk sizes limited ([`346c885`](https://github.com/Dicklesworthstone/fastapi_rust/commit/346c8858ab70f7a8c613431b1fc670c2be23e233))
- **Password validation**: reject invalid base64 instead of silently skipping ([`b8b6be8`](https://github.com/Dicklesworthstone/fastapi_rust/commit/b8b6be81bfd211bbff8f2f3059c423d3af5f2a91))
- **Hidden path traversal** fix in static files, percent-decode, header casing ([`c7f0e54`](https://github.com/Dicklesworthstone/fastapi_rust/commit/c7f0e54b7d5c6f4bf537c5fb07521575deb6f5c6))
- **Multipart/query parsing hardened**, fail-fast on missing CSPRNG ([`8603bc0`](https://github.com/Dicklesworthstone/fastapi_rust/commit/8603bc0c67cd9f2b80204df9c57deaaa55f9853a))
- **Digest-auth substring match** prevention ([`20e08cb`](https://github.com/Dicklesworthstone/fastapi_rust/commit/20e08cbb5eb0674b8210169de707e5e2748d8f2a))
- **Integer overflow prevention** in `ByteRange::len` ([`35b0ed9`](https://github.com/Dicklesworthstone/fastapi_rust/commit/35b0ed9f0992354d1bfa21fbd8e094130395e6ae))
- **XSS fix** in coverage reports ([`6c6a55b`](https://github.com/Dicklesworthstone/fastapi_rust/commit/6c6a55b0938aa6813aaecd3afadf24dd7304479e))
- Critical vulnerabilities and hot-path optimizations ([`94467b6`](https://github.com/Dicklesworthstone/fastapi_rust/commit/94467b6757aac60ab301975a79e95602a7ea5104), [`64deafa`](https://github.com/Dicklesworthstone/fastapi_rust/commit/64deafa49a130051cb0668a1e22f10bd463beedd))
- Email and URL validation improved in macros ([`bf60b21`](https://github.com/Dicklesworthstone/fastapi_rust/commit/bf60b211915041a73ece68ffafe24c1aec4e57bf))
- Salt generation switched to `/dev/urandom` ([`c98fec5`](https://github.com/Dicklesworthstone/fastapi_rust/commit/c98fec559679d05443962facc98a86d67c613b19))

### Performance Optimizations

- **Zero-allocation header lookup** for lowercase names ([`108edc2`](https://github.com/Dicklesworthstone/fastapi_rust/commit/108edc21fe224f1bbdfb9cac5958a629b7b1bb81))
- **Regex pattern caching** with `OnceLock` and compile-time validation ([`580ac53`](https://github.com/Dicklesworthstone/fastapi_rust/commit/580ac531b800ed1f384e3221a5966ff459dd1149))
- **Zero-allocation Connection header parsing** for common tokens ([`9b8a5bf`](https://github.com/Dicklesworthstone/fastapi_rust/commit/9b8a5bfbb87bf0d5a64c4f17f911c423ba63d913))
- **Zero-copy header insertion** via `insert_from_slice` ([`f6089ec`](https://github.com/Dicklesworthstone/fastapi_rust/commit/f6089ec22788938a96738337808017a26245e283), [`907acff`](https://github.com/Dicklesworthstone/fastapi_rust/commit/907acff4f92e6d5568afa422397c1f190b04de1b))
- **Pre-lowercase host patterns** at config time ([`2bd2437`](https://github.com/Dicklesworthstone/fastapi_rust/commit/2bd24375f0311d7504e9b79a7dec5e8b05d02ce3))
- **Zero-allocation path matching** with stack buffer in router ([`4812562`](https://github.com/Dicklesworthstone/fastapi_rust/commit/481256255031c22baac9eb2e3952ab757714c098))
- Migrate from `std::sync::Mutex` to `parking_lot` across dependency, coverage, testing, rate limiter, and output modules ([`4ed3169`](https://github.com/Dicklesworthstone/fastapi_rust/commit/4ed316983e7d45e2c34b60a16ddb5064f93af370), [`c84c3c6`](https://github.com/Dicklesworthstone/fastapi_rust/commit/c84c3c6dbc7d75bbeda3a24935ec641e1c4bbaf9), [`fdb865f`](https://github.com/Dicklesworthstone/fastapi_rust/commit/fdb865f869b99b53d9e42135636415a0022acc1c), [`697a09c`](https://github.com/Dicklesworthstone/fastapi_rust/commit/697a09cf7bace6e1c1cff8f91aabc8c80034af66))
- Improved panic handling and reduced allocations in HTTP handling ([`01df1fa`](https://github.com/Dicklesworthstone/fastapi_rust/commit/01df1facbe71de1c13a80262b437f3053967232d))

### Structured Logging and Observability

- **Structured logging** with span-based tracing infrastructure ([`666047b`](https://github.com/Dicklesworthstone/fastapi_rust/commit/666047b1090df1ca191d8fd390b809edceb30571))
- **Output facade** with AI agent mode detection ([`af8457c`](https://github.com/Dicklesworthstone/fastapi_rust/commit/af8457ce9d7dd3114f4b63d38ea1a1b909509669))
- **fastapi-output crate** with theme system, icons, spacing, accessibility, and rich display components ([`63f5aa6`](https://github.com/Dicklesworthstone/fastapi_rust/commit/63f5aa61f12772436cf0a7b5c0892484a0716545), [`1d4b619`](https://github.com/Dicklesworthstone/fastapi_rust/commit/1d4b6195807b05a539c0e94808b985e86e810cd0), [`744deae`](https://github.com/Dicklesworthstone/fastapi_rust/commit/744deae592527b77c8633fb654221919161e71d5))
- **BackgroundTasks** improvements and timing tests ([`dff52c2`](https://github.com/Dicklesworthstone/fastapi_rust/commit/dff52c2ed7920580b1b97d062d47c952b794f003))

### Documentation and CI

- Comprehensive `ARCHITECTURE.md` ([`611beb5`](https://github.com/Dicklesworthstone/fastapi_rust/commit/611beb5b5d0b687b267b449bc4a95185fa9f35ec))
- README with hero illustration, CI badges, and quick example ([`361d1dc`](https://github.com/Dicklesworthstone/fastapi_rust/commit/361d1dca654258963e14087c84cd0ab0ebb0edcc), [`16d0050`](https://github.com/Dicklesworthstone/fastapi_rust/commit/16d0050c8525f826dde270fa8ec8826044046525))
- Cookbook, migration guide, and demo application in rustdoc ([`dbcd0a8`](https://github.com/Dicklesworthstone/fastapi_rust/commit/dbcd0a8eccc3ef0e01767cc7ee94a46dd7819b72))
- Comprehensive rustdoc for all major config structs ([`f3b8e68`](https://github.com/Dicklesworthstone/fastapi_rust/commit/f3b8e6892073ad470e22f9876d5f521a58c890d3))
- Feature flags and crate structure tables ([`b784313`](https://github.com/Dicklesworthstone/fastapi_rust/commit/b78431361f873afce32e9dd8e4946513c72283c2))
- All rustdoc broken link and HTML warnings resolved ([`0981b46`](https://github.com/Dicklesworthstone/fastapi_rust/commit/0981b46bcc549da6a7408c1203a243fd3c0f3e1d))
- GitHub Actions CI: build, test, clippy, rustfmt, docs ([`16d0050`](https://github.com/Dicklesworthstone/fastapi_rust/commit/16d0050c8525f826dde270fa8ec8826044046525), [`546b07e`](https://github.com/Dicklesworthstone/fastapi_rust/commit/546b07e3ace5843644bab5beebc74e07440370c1))
- MIT License added ([`2ae94b1`](https://github.com/Dicklesworthstone/fastapi_rust/commit/2ae94b1851dd8dcc9c0a2dec9eb093e48c1f879e))
- Auth example with bearer token authentication ([`d9b33d4`](https://github.com/Dicklesworthstone/fastapi_rust/commit/d9b33d44cbb7b875870b9dc3677674341ba9e613))

---

## Summary Statistics

| Metric | Value |
|--------|-------|
| First commit | 2026-01-17 |
| Total commits (main) | 325 |
| Tagged releases | 1 (`v0.2.0`) |
| GitHub releases | 1 (`v0.2.0`, 2026-02-15) |
| crates.io publishes | 1 (`v0.1.2`, 2026-02-05) |
| Workspace crates | 8 |
| Current version | 0.2.0 |
| Remote branches | 5 (main, master, 3 dependabot) |

---

*This changelog was reconstructed from git history on 2026-03-21.*
