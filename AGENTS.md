# AGENTS.md — fastapi_rust

> Guidelines for AI coding agents working in this Rust codebase.

---

## RULE 0 - THE FUNDAMENTAL OVERRIDE PREROGATIVE

If I tell you to do something, even if it goes against what follows below, YOU MUST LISTEN TO ME. I AM IN CHARGE, NOT YOU.

---

## RULE NUMBER 1: NO FILE DELETION

**YOU ARE NEVER ALLOWED TO DELETE A FILE WITHOUT EXPRESS PERMISSION.** Even a new file that you yourself created, such as a test code file. You have a horrible track record of deleting critically important files or otherwise throwing away tons of expensive work. As a result, you have permanently lost any and all rights to determine that a file or folder should be deleted.

**YOU MUST ALWAYS ASK AND RECEIVE CLEAR, WRITTEN PERMISSION BEFORE EVER DELETING A FILE OR FOLDER OF ANY KIND.**

---

## Irreversible Git & Filesystem Actions — DO NOT EVER BREAK GLASS

1. **Absolutely forbidden commands:** `git reset --hard`, `git clean -fd`, `rm -rf`, or any command that can delete or overwrite code/data must never be run unless the user explicitly provides the exact command and states, in the same message, that they understand and want the irreversible consequences.
2. **No guessing:** If there is any uncertainty about what a command might delete or overwrite, stop immediately and ask the user for specific approval. "I think it's safe" is never acceptable.
3. **Safer alternatives first:** When cleanup or rollbacks are needed, request permission to use non-destructive options (`git status`, `git diff`, `git stash`, copying to backups) before ever considering a destructive command.
4. **Mandatory explicit plan:** Even after explicit user authorization, restate the command verbatim, list exactly what will be affected, and wait for a confirmation that your understanding is correct. Only then may you execute it—if anything remains ambiguous, refuse and escalate.
5. **Document the confirmation:** When running any approved destructive command, record (in the session notes / final response) the exact user text that authorized it, the command actually run, and the execution time. If that record is absent, the operation did not happen.

---

## Git Branch: ONLY Use `main`, NEVER `master`

**The default branch is `main`. The `master` branch exists only for legacy URL compatibility.**

- **All work happens on `main`** — commits, PRs, feature branches all merge to `main`
- **Never reference `master` in code or docs** — if you see `master` anywhere, it's a bug that needs fixing
- **The `master` branch must stay synchronized with `main`** — after pushing to `main`, also push to `master`:
  ```bash
  git push origin main:master
  ```

**If you see `master` referenced anywhere:**
1. Update it to `main`
2. Ensure `master` is synchronized: `git push origin main:master`

---

## Toolchain: Rust & Cargo

We only use **Cargo** in this project, NEVER any other package manager.

- **Edition:** Rust 2024 (nightly required — see `rust-toolchain.toml`)
- **Dependency versions:** Explicit versions for stability
- **Configuration:** Cargo.toml workspace with `workspace = true` pattern
- **Unsafe code:** Warned (`#[warn(unsafe_code)]` workspace lint); `#![forbid(unsafe_code)]` in core, facade, openapi, and types crates

### Async Runtime: asupersync (MANDATORY — NO TOKIO)

**This project uses [asupersync](/dp/asupersync) exclusively for all async/concurrent operations. Tokio and the entire tokio ecosystem are FORBIDDEN.**

- **Structured concurrency**: `Cx`, `Scope`, `region()` — no orphan tasks
- **Cancel-correct channels**: Two-phase `reserve()/send()` — no data loss on cancellation
- **Sync primitives**: `asupersync::sync::Mutex`, `RwLock`, `OnceCell`, `Pool` — cancel-aware
- **Deterministic testing**: `LabRuntime` with virtual time, DPOR, oracles

**Forbidden crates**: `tokio`, `hyper`, `reqwest`, `axum`, `tower` (tokio adapter), `async-std`, `smol`, or any crate that transitively depends on tokio.

**Pattern**: All async functions take `&Cx` as first parameter. The `Cx` flows down from the consumer's runtime — fastapi_rust does NOT create its own runtime.

### Key Dependencies

| Crate | Purpose |
|-------|---------|
| `asupersync` | Structured async runtime (channels, sync, regions, HTTP, testing) |
| `serde` + `serde_json` | Serialization (only external non-runtime deps) |
| `parking_lot` | Fast synchronous mutexes (for non-async paths) |
| `proc-macro2` + `quote` + `syn` | Procedural macro infrastructure |
| `crossterm` | Terminal detection for output mode |
| `rich_rust` | Rich terminal rendering (optional, behind feature flag) |
| `regex` | Pattern matching in validation and output |
| `unicode-width` | Unicode-aware text width calculation |
| `criterion` | Benchmarking (dev-dependency) |
| `futures-executor` | Sync execution bridge for TestClient |

### Release Profile

The workspace does not yet define a custom release profile. When added, the standard high-performance configuration applies:

```toml
[profile.release]
opt-level = 3       # Maximum performance optimization
lto = true          # Link-time optimization
codegen-units = 1   # Single codegen unit for better optimization
strip = true        # Remove debug symbols
```

---

## Code Editing Discipline

### No Script-Based Changes

**NEVER** run a script that processes/changes code files in this repo. Brittle regex-based transformations create far more problems than they solve.

- **Always make code changes manually**, even when there are many instances
- For many simple changes: use parallel subagents
- For subtle/complex changes: do them methodically yourself

### No File Proliferation

If you want to change something or add a feature, **revise existing code files in place**.

**NEVER** create variations like:
- `mainV2.rs`
- `main_improved.rs`
- `main_enhanced.rs`

New files are reserved for **genuinely new functionality** that makes zero sense to include in any existing file. The bar for creating new files is **incredibly high**.

---

## Backwards Compatibility

We do not care about backwards compatibility—we're in early development with no users. We want to do things the **RIGHT** way with **NO TECH DEBT**.

- Never create "compatibility shims"
- Never create wrapper functions for deprecated APIs
- Just fix the code directly

---

## Compiler Checks (CRITICAL)

**After any substantive code changes, you MUST verify no errors were introduced:**

```bash
# Check for compiler errors and warnings (workspace-wide)
cargo check --workspace --all-targets

# Check for clippy lints (pedantic + nursery are enabled)
cargo clippy --workspace --all-targets -- -D warnings

# Verify formatting
cargo fmt --check
```

If you see errors, **carefully understand and resolve each issue**. Read sufficient context to fix them the RIGHT way.

---

## Testing

### Testing Policy

Every component crate includes inline `#[cfg(test)]` unit tests alongside the implementation. Tests must cover:
- Happy path
- Edge cases (empty input, max values, boundary conditions)
- Error conditions

Cross-component integration tests live in each crate's `tests/` directory.

### Unit Tests

```bash
# Run all tests across the workspace
cargo test --workspace

# Run with output
cargo test --workspace -- --nocapture

# Run tests for a specific crate
cargo test -p fastapi-core
cargo test -p fastapi-http
cargo test -p fastapi-router
cargo test -p fastapi-macros
cargo test -p fastapi-openapi
cargo test -p fastapi-output
cargo test -p fastapi-types

# Run tests with all features enabled
cargo test --workspace --all-features
```

### Test Categories

| Crate | Focus Areas |
|-------|-------------|
| `fastapi-core` | Request/response types, extractors (Path, Query, Json, Header, Cookie, Pagination, OAuth2, BasicAuth, Bearer), middleware (CORS, SecurityHeaders, RequestId), dependency injection, validation, error handling, WebSocket handshake, app builder, shutdown coordination, SSE, NDJSON streaming, password hashing, background tasks |
| `fastapi-http` | Zero-copy HTTP/1.1 parsing, request line parsing, header parsing, body parsing (Content-Length + chunked), query string percent-decoding, response writing, chunked encoding, connection handling (keep-alive, hop-by-hop), multipart parsing, range requests, Expect: 100-continue, HTTP/2 detection, WebSocket upgrade, streaming, server lifecycle, security edge cases |
| `fastapi-router` | Radix trie insertion/lookup, path parameter extraction, type-safe converters (int, float, path, uuid, slug), route conflict detection, allowed methods, route registration, wildcard matching |
| `fastapi-macros` | Route macro code generation (`#[get]`, `#[post]`, etc.), `#[derive(Validate)]` with length/range/email/regex rules, `#[derive(JsonSchema)]` for OpenAPI schema types, `#[derive(ResponseModelAliases)]` for serde rename handling |
| `fastapi-openapi` | OpenAPI 3.1 spec generation, JSON Schema types (primitive, object, array, enum, oneOf, ref), schema registry, operation/parameter/requestBody/response building, enum schema generation |
| `fastapi-output` | Agent/CI environment detection, output mode selection (Rich/Plain/Minimal), theme rendering, component formatting (banner, routes, errors, logging, middleware stack, dependency tree, shutdown progress, test results, OpenAPI display, help display, HTTP inspector, routing debug), snapshot tests, integration lifecycle tests |
| `fastapi-types` | HTTP Method enum, `from_bytes()` parsing, `as_str()` formatting |
| `crates/fastapi-http/benches/` | HTTP parser throughput benchmarks |

---

## Third-Party Library Usage

If you aren't 100% sure how to use a third-party library, **SEARCH ONLINE** to find the latest documentation and current best practices.

---

## ast-grep vs ripgrep

**Use `ast-grep` when structure matters.** It parses code and matches AST nodes, ignoring comments/strings, and can **safely rewrite** code.

- Refactors/codemods: rename APIs, change import forms
- Policy checks: enforce patterns across a repo
- Editor/automation: LSP mode, `--json` output

**Use `ripgrep` when text is enough.** Fastest way to grep literals/regex.

- Recon: find strings, TODOs, log lines, config values
- Pre-filter: narrow candidate files before ast-grep

### Rule of Thumb

- Need correctness or **applying changes** → `ast-grep`
- Need raw speed or **hunting text** → `rg`
- Often combine: `rg` to shortlist files, then `ast-grep` to match/modify

### Rust Examples

```bash
# Find structured code (ignores comments)
ast-grep run -l Rust -p 'fn $NAME($$$ARGS) -> $RET { $$$BODY }'

# Find all unwrap() calls
ast-grep run -l Rust -p '$EXPR.unwrap()'

# Quick textual hunt
rg -n 'println!' -t rust

# Combine speed + precision
rg -l -t rust 'unwrap\(' | xargs ast-grep run -l Rust -p '$X.unwrap()' --json
```

---

## Morph Warp Grep — AI-Powered Code Search

**Use `mcp__morph-mcp__warp_grep` for exploratory "how does X work?" questions.** An AI agent expands your query, greps the codebase, reads relevant files, and returns precise line ranges with full context.

**Use `ripgrep` for targeted searches.** When you know exactly what you're looking for.

**Use `ast-grep` for structural patterns.** When you need AST precision for matching/rewriting.

### When to Use What

| Scenario | Tool | Why |
|----------|------|-----|
| "How is the router implemented?" | `warp_grep` | Exploratory; don't know where to start |
| "Where is the request extractor logic?" | `warp_grep` | Need to understand architecture |
| "Find all uses of `serde_json`" | `ripgrep` | Targeted literal search |
| "Find files with `async fn`" | `ripgrep` | Simple pattern |
| "Replace all `unwrap()` with `expect()`" | `ast-grep` | Structural refactor |

### warp_grep Usage

```
mcp__morph-mcp__warp_grep(
  repoPath: "/data/projects/fastapi_rust",
  query: "How does the routing system work with asupersync?"
)
```

Returns structured results with file paths, line ranges, and extracted code snippets.

### Anti-Patterns

- **Don't** use `warp_grep` to find a specific function name → use `ripgrep`
- **Don't** use `ripgrep` to understand "how does X work" → wastes time with manual reads
- **Don't** use `ripgrep` for codemods → risks collateral edits

---

## UBS — Ultimate Bug Scanner

**Golden Rule:** `ubs <changed-files>` before every commit. Exit 0 = safe. Exit >0 = fix & re-run.

### Commands

```bash
ubs file.rs file2.rs                    # Specific files (< 1s) — USE THIS
ubs $(git diff --name-only --cached)    # Staged files — before commit
ubs --only=rust,toml src/               # Language filter (3-5x faster)
ubs --ci --fail-on-warning .            # CI mode — before PR
ubs .                                   # Whole project (ignores target/, Cargo.lock)
```

### Output Format

```
⚠️  Category (N errors)
    file.rs:42:5 – Issue description
    💡 Suggested fix
Exit code: 1
```

Parse: `file:line:col` → location | 💡 → how to fix | Exit 0/1 → pass/fail

### Fix Workflow

1. Read finding → category + fix suggestion
2. Navigate `file:line:col` → view context
3. Verify real issue (not false positive)
4. Fix root cause (not symptom)
5. Re-run `ubs <file>` → exit 0
6. Commit

### Bug Severity

- **Critical (always fix):** Memory safety, use-after-free, data races, SQL injection
- **Important (production):** Unwrap panics, resource leaks, overflow checks
- **Contextual (judgment):** TODO/FIXME, println! debugging

---

## RCH — Remote Compilation Helper

RCH offloads `cargo build`, `cargo test`, `cargo clippy`, and other compilation commands to a fleet of 8 remote Contabo VPS workers instead of building locally. This prevents compilation storms from overwhelming csd when many agents run simultaneously.

**RCH is installed at `~/.local/bin/rch` and is hooked into Claude Code's PreToolUse automatically.** Most of the time you don't need to do anything if you are Claude Code — builds are intercepted and offloaded transparently.

To manually offload a build:
```bash
rch exec -- cargo build --release
rch exec -- cargo test
rch exec -- cargo clippy
```

Quick commands:
```bash
rch doctor                    # Health check
rch workers probe --all       # Test connectivity to all 8 workers
rch status                    # Overview of current state
rch queue                     # See active/waiting builds
```

If rch or its workers are unavailable, it fails open — builds run locally as normal.

**Note for Codex/GPT-5.2:** Codex does not have the automatic PreToolUse hook, but you can (and should) still manually offload compute-intensive compilation commands using `rch exec -- <command>`. This avoids local resource contention when multiple agents are building simultaneously.

---

## MCP Agent Mail — Multi-Agent Coordination

A mail-like layer that lets coding agents coordinate asynchronously via MCP tools and resources. Provides identities, inbox/outbox, searchable threads, and advisory file reservations with human-auditable artifacts in Git.

### Why It's Useful

- **Prevents conflicts:** Explicit file reservations (leases) for files/globs
- **Token-efficient:** Messages stored in per-project archive, not in context
- **Quick reads:** `resource://inbox/...`, `resource://thread/...`

### Same Repository Workflow

1. **Register identity:**
   ```
   ensure_project(project_key=<abs-path>)
   register_agent(project_key, program, model)
   ```

2. **Reserve files before editing:**
   ```
   file_reservation_paths(project_key, agent_name, ["src/**"], ttl_seconds=3600, exclusive=true)
   ```

3. **Communicate with threads:**
   ```
   send_message(..., thread_id="FEAT-123")
   fetch_inbox(project_key, agent_name)
   acknowledge_message(project_key, agent_name, message_id)
   ```

4. **Quick reads:**
   ```
   resource://inbox/{Agent}?project=<abs-path>&limit=20
   resource://thread/{id}?project=<abs-path>&include_bodies=true
   ```

### Cross-Project Coordination

When working on both fastapi_rust and asupersync:
- Register in both projects
- Use Mail to coordinate changes that span both repos
- Reserve files in the project you're actively editing

### Macros vs Granular Tools

- **Prefer macros for speed:** `macro_start_session`, `macro_prepare_thread`, `macro_file_reservation_cycle`, `macro_contact_handshake`
- **Use granular tools for control:** `register_agent`, `file_reservation_paths`, `send_message`, `fetch_inbox`, `acknowledge_message`

### Common Pitfalls

- `"from_agent not registered"`: Always `register_agent` in the correct `project_key` first
- `"FILE_RESERVATION_CONFLICT"`: Adjust patterns, wait for expiry, or use non-exclusive reservation
- **Auth errors:** If JWT+JWKS enabled, include bearer token with matching `kid`

---

## Beads (br) — Dependency-Aware Issue Tracking

Beads provides a lightweight, dependency-aware issue database and CLI (`br` - beads_rust) for selecting "ready work," setting priorities, and tracking status. It complements MCP Agent Mail's messaging and file reservations.

**Important:** `br` is non-invasive—it NEVER runs git commands automatically. You must manually commit changes after `br sync --flush-only`.

### Conventions

- **Single source of truth:** Beads for task status/priority/dependencies; Agent Mail for conversation and audit
- **Shared identifiers:** Use Beads issue ID (e.g., `br-123`) as Mail `thread_id` and prefix subjects with `[br-123]`
- **Reservations:** When starting a task, call `file_reservation_paths()` with the issue ID in `reason`

### Typical Agent Flow

1. **Pick ready work (Beads):**
   ```bash
   br ready --json  # Choose highest priority, no blockers
   ```

2. **Reserve edit surface (Mail):**
   ```
   file_reservation_paths(project_key, agent_name, ["src/**"], ttl_seconds=3600, exclusive=true, reason="br-123")
   ```

3. **Announce start (Mail):**
   ```
   send_message(..., thread_id="br-123", subject="[br-123] Start: <title>", ack_required=true)
   ```

4. **Work and update:** Reply in-thread with progress

5. **Complete and release:**
   ```bash
   br close 123 --reason "Completed"
   br sync --flush-only  # Export to JSONL (no git operations)
   ```
   ```
   release_file_reservations(project_key, agent_name, paths=["src/**"])
   ```
   Final Mail reply: `[br-123] Completed` with summary

### Mapping Cheat Sheet

| Concept | Value |
|---------|-------|
| Mail `thread_id` | `br-###` |
| Mail subject | `[br-###] ...` |
| File reservation `reason` | `br-###` |
| Commit messages | Include `br-###` for traceability |

---

## bv — Graph-Aware Triage Engine

bv is a graph-aware triage engine for Beads projects (`.beads/beads.jsonl`). It computes PageRank, betweenness, critical path, cycles, HITS, eigenvector, and k-core metrics deterministically.

**Scope boundary:** bv handles *what to work on* (triage, priority, planning). For agent-to-agent coordination (messaging, work claiming, file reservations), use MCP Agent Mail.

**CRITICAL: Use ONLY `--robot-*` flags. Bare `bv` launches an interactive TUI that blocks your session.**

### The Workflow: Start With Triage

**`bv --robot-triage` is your single entry point.** It returns:
- `quick_ref`: at-a-glance counts + top 3 picks
- `recommendations`: ranked actionable items with scores, reasons, unblock info
- `quick_wins`: low-effort high-impact items
- `blockers_to_clear`: items that unblock the most downstream work
- `project_health`: status/type/priority distributions, graph metrics
- `commands`: copy-paste shell commands for next steps

```bash
bv --robot-triage        # THE MEGA-COMMAND: start here
bv --robot-next          # Minimal: just the single top pick + claim command
```

### Command Reference

**Planning:**
| Command | Returns |
|---------|---------|
| `--robot-plan` | Parallel execution tracks with `unblocks` lists |
| `--robot-priority` | Priority misalignment detection with confidence |

**Graph Analysis:**
| Command | Returns |
|---------|---------|
| `--robot-insights` | Full metrics: PageRank, betweenness, HITS, eigenvector, critical path, cycles, k-core, articulation points, slack |
| `--robot-label-health` | Per-label health: `health_level`, `velocity_score`, `staleness`, `blocked_count` |
| `--robot-label-flow` | Cross-label dependency: `flow_matrix`, `dependencies`, `bottleneck_labels` |
| `--robot-label-attention [--attention-limit=N]` | Attention-ranked labels |

**History & Change Tracking:**
| Command | Returns |
|---------|---------|
| `--robot-history` | Bead-to-commit correlations |
| `--robot-diff --diff-since <ref>` | Changes since ref: new/closed/modified issues, cycles |

**Other:**
| Command | Returns |
|---------|---------|
| `--robot-burndown <sprint>` | Sprint burndown, scope changes, at-risk items |
| `--robot-forecast <id\|all>` | ETA predictions with dependency-aware scheduling |
| `--robot-alerts` | Stale issues, blocking cascades, priority mismatches |
| `--robot-suggest` | Hygiene: duplicates, missing deps, label suggestions |
| `--robot-graph [--graph-format=json\|dot\|mermaid]` | Dependency graph export |
| `--export-graph <file.html>` | Interactive HTML visualization |

### Scoping & Filtering

```bash
bv --robot-plan --label backend              # Scope to label's subgraph
bv --robot-insights --as-of HEAD~30          # Historical point-in-time
bv --recipe actionable --robot-plan          # Pre-filter: ready to work
bv --recipe high-impact --robot-triage       # Pre-filter: top PageRank
bv --robot-triage --robot-triage-by-track    # Group by parallel work streams
bv --robot-triage --robot-triage-by-label    # Group by domain
```

### Understanding Robot Output

**All robot JSON includes:**
- `data_hash` — Fingerprint of source beads.jsonl
- `status` — Per-metric state: `computed|approx|timeout|skipped` + elapsed ms
- `as_of` / `as_of_commit` — Present when using `--as-of`

**Two-phase analysis:**
- **Phase 1 (instant):** degree, topo sort, density
- **Phase 2 (async, 500ms timeout):** PageRank, betweenness, HITS, eigenvector, cycles

### jq Quick Reference

```bash
bv --robot-triage | jq '.quick_ref'                        # At-a-glance summary
bv --robot-triage | jq '.recommendations[0]'               # Top recommendation
bv --robot-plan | jq '.plan.summary.highest_impact'        # Best unblock target
bv --robot-insights | jq '.status'                         # Check metric readiness
bv --robot-insights | jq '.Cycles'                         # Circular deps (must fix!)
```

---

<!-- bv-agent-instructions-v1 -->

## Beads Workflow Integration

This project uses [beads_rust](https://github.com/Dicklesworthstone/beads_rust) (`br`) for issue tracking. Issues are stored in `.beads/` and tracked in git.

**Important:** `br` is non-invasive—it NEVER executes git commands. After `br sync --flush-only`, you must manually run `git add .beads/ && git commit`.

### Essential Commands

```bash
# View issues (launches TUI - avoid in automated sessions)
bv

# CLI commands for agents (use these instead)
br ready              # Show issues ready to work (no blockers)
br list --status=open # All open issues
br show <id>          # Full issue details with dependencies
br create --title="..." --type=task --priority=2
br update <id> --status=in_progress
br close <id> --reason "Completed"
br close <id1> <id2>  # Close multiple issues at once
br sync --flush-only  # Export to JSONL (NO git operations)
```

### Workflow Pattern

1. **Start**: Run `br ready` to find actionable work
2. **Claim**: Use `br update <id> --status=in_progress`
3. **Work**: Implement the task
4. **Complete**: Use `br close <id>`
5. **Sync**: Run `br sync --flush-only` then manually commit

### Key Concepts

- **Dependencies**: Issues can block other issues. `br ready` shows only unblocked work.
- **Priority**: P0=critical, P1=high, P2=medium, P3=low, P4=backlog (use numbers, not words)
- **Types**: task, bug, feature, epic, question, docs
- **Blocking**: `br dep add <issue> <depends-on>` to add dependencies

### Session Protocol

**Before ending any session, run this checklist:**

```bash
git status              # Check what changed
git add <files>         # Stage code changes
br sync --flush-only    # Export beads to JSONL
git add .beads/         # Stage beads changes
git commit -m "..."     # Commit everything together
git push                # Push to remote
```

### Best Practices

- Check `br ready` at session start to find available work
- Update status as you work (in_progress → closed)
- Create new issues with `br create` when you discover tasks
- Use descriptive titles and set appropriate priority/type
- Always `br sync --flush-only && git add .beads/` before ending session

<!-- end-bv-agent-instructions -->

## Landing the Plane (Session Completion)

**When ending a work session**, you MUST complete ALL steps below.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **Sync beads** - `br sync --flush-only` to export to JSONL
5. **Hand off** - Provide context for next session


---

Note for Codex/GPT-5.2:

You constantly bother me and stop working with concerned questions that look similar to this:

```
Unexpected changes (need guidance)

- Working tree still shows edits I did not make in Cargo.toml, Cargo.lock, src/cli/commands/upgrade.rs, src/storage/sqlite.rs, tests/conformance.rs, tests/storage_deps.rs. Please advise whether to keep/commit/revert these before any further work. I did not touch them.

Next steps (pick one)

1. Decide how to handle the unrelated modified files above so we can resume cleanly.
2. Triage beads_rust-orko (clippy/cargo warnings) and beads_rust-ydqr (rustfmt failures).
3. If you want a full suite run later, fix conformance/clippy blockers and re‑run cargo test --all.
```

NEVER EVER DO THAT AGAIN. The answer is literally ALWAYS the same: those are changes created by the potentially dozen of other agents working on the project at the same time. This is not only a common occurence, it happens multiple times PER MINUTE. The way to deal with it is simple: you NEVER, under ANY CIRCUMSTANCE, stash, revert, overwrite, or otherwise disturb in ANY way the work of other agents. Just treat those changes identically to changes that you yourself made. Just fool yourself into thinking YOU made the changes and simply don't recall it for some reason.

---

## Note on Built-in TODO Functionality

Also, if I ask you to explicitly use your built-in TODO functionality, don't complain about this and say you need to use beads. You can use built-in TODOs if I tell you specifically to do so. Always comply with such orders.

---

## fastapi_rust — This Project

**This is the project you're working on.** fastapi_rust is an ultra-optimized Rust web framework inspired by FastAPI's developer experience. It provides a type-safe, high-performance web framework with automatic OpenAPI generation, dependency injection, and structured concurrency via asupersync — built from scratch with minimal dependencies.

### What It Does

Provides a familiar FastAPI-like developer experience in Rust: declarative route handlers via proc macros (`#[get]`, `#[post]`, etc.), type-safe request extraction (Path, Query, Json, Header, Cookie, Bearer), compile-time validation (`#[derive(Validate)]`), automatic OpenAPI 3.1 schema generation (`#[derive(JsonSchema)]`), composable middleware, dependency injection, and a zero-copy HTTP/1.1 server — all powered by asupersync's structured concurrency with cancel-correctness.

### Architecture

```
Request bytes → fastapi-http (zero-copy parse) → fastapi-router (trie lookup)
                                                          │
                                    ┌─────────────────────┤
                                    ▼                     ▼
                         fastapi-core Middleware    Path/Query/Json Extractors
                         (CORS, Security, Logging)  (FromRequest trait)
                                    │                     │
                                    ▼                     ▼
                              Handler fn (async, &Cx) ◄───┘
                                    │
                                    ▼
                         IntoResponse → HTTP response bytes
                                    │
                         fastapi-openapi (schema gen from #[derive(JsonSchema)])
                         fastapi-macros (proc macros: route + validation + schema)
                         fastapi-output (agent-aware console formatting)
```

### Workspace Structure

```
fastapi_rust/
├── Cargo.toml                         # Workspace root
├── crates/
│   ├── fastapi/                       # Facade crate (re-exports everything)
│   ├── fastapi-core/                  # Core types, extractors, middleware, DI, app
│   ├── fastapi-http/                  # Zero-copy HTTP/1.1 parser, TCP server
│   ├── fastapi-router/               # Radix trie router with path parameters
│   ├── fastapi-macros/               # Proc macros (#[get], Validate, JsonSchema)
│   ├── fastapi-openapi/              # OpenAPI 3.1 types and schema generation
│   ├── fastapi-types/                # Shared types (Method enum)
│   └── fastapi-output/              # Agent-aware rich console output
├── legacy_fastapi/                    # Python FastAPI source (reference only)
├── PLAN_TO_PORT_FASTAPI_TO_RUST.md   # Porting plan with exclusions
├── EXISTING_FASTAPI_STRUCTURE.md     # THE SPEC — complete behavior extraction
├── PROPOSED_RUST_ARCHITECTURE.md     # Rust design decisions
└── ARCHITECTURE.md                   # Architecture overview
```

### Key Files by Crate

| Crate | Key Files | Purpose |
|-------|-----------|---------|
| `fastapi-core` | `src/app.rs` | `App`, `AppBuilder`, `AppConfig`, startup hooks, state container |
| `fastapi-core` | `src/request.rs` | `Request`, `Body`, `Headers`, `Method`, `HttpVersion`, `BackgroundTasks` |
| `fastapi-core` | `src/response.rs` | `Response`, `StatusCode`, `IntoResponse`, `FileResponse`, `Redirect`, `SetCookie` |
| `fastapi-core` | `src/extract.rs` | `FromRequest` trait, `Path`, `Query`, `Json`, `Header`, `Cookie`, `Pagination`, `BearerToken`, `BasicAuth`, `OAuth2PasswordBearer`, `State`, `Form` |
| `fastapi-core` | `src/middleware.rs` | `Middleware` trait, `Cors`, `SecurityHeaders`, `RequestId`, `RequestResponseLogger`, `RequireHeader` |
| `fastapi-core` | `src/dependency.rs` | `FromDependency`, `Depends`, `DependencyCache`, `DependencyScope` |
| `fastapi-core` | `src/error.rs` | `HttpError`, `ValidationError`, `ValidationErrors` |
| `fastapi-core` | `src/context.rs` | `RequestContext` wrapping asupersync `Cx` |
| `fastapi-core` | `src/validation.rs` | Validation rules (length, range, email, regex, custom) |
| `fastapi-core` | `src/routing.rs` | Route integration with core types |
| `fastapi-core` | `src/websocket.rs` | WebSocket frame parsing, handshake, accept key generation |
| `fastapi-core` | `src/shutdown.rs` | `GracefulShutdown`, `ShutdownController`, phase-based shutdown |
| `fastapi-core` | `src/testing.rs` | `TestClient`, `RequestBuilder`, `TestResponse`, assertion macros |
| `fastapi-http` | `src/parser.rs` | Zero-copy HTTP/1.1 request parser (`Parser`, `StatefulParser`) |
| `fastapi-http` | `src/server.rs` | `Server`, `TcpServer`, `serve()`, connection management |
| `fastapi-http` | `src/body.rs` | Body parsing (Content-Length, chunked), streaming |
| `fastapi-http` | `src/response.rs` | `ResponseWriter`, `ChunkedEncoder`, trailers |
| `fastapi-http` | `src/websocket.rs` | WebSocket upgrade, frame I/O, close codes |
| `fastapi-http` | `src/http2.rs` | HTTP/2 connection preface detection |
| `fastapi-http` | `src/streaming.rs` | Cancel-aware streaming, file streams, chunked bytes |
| `fastapi-router` | `src/trie.rs` | Radix trie `Router`, `Route`, path parameter converters |
| `fastapi-router` | `src/match.rs` | `RouteMatch`, `RouteLookup`, `AllowedMethods` |
| `fastapi-router` | `src/registry.rs` | Global route registration for macro-generated routes |
| `fastapi-macros` | `src/route.rs` | `#[get]`, `#[post]`, etc. route macro implementation |
| `fastapi-macros` | `src/validate.rs` | `#[derive(Validate)]` implementation |
| `fastapi-macros` | `src/openapi.rs` | `#[derive(JsonSchema)]` implementation |
| `fastapi-openapi` | `src/spec.rs` | `OpenApi`, `OpenApiBuilder`, `Operation`, `Parameter`, `PathItem` |
| `fastapi-openapi` | `src/schema.rs` | `Schema`, `JsonSchema` trait, type-to-schema mapping |
| `fastapi-output` | `src/detection.rs` | Agent/CI environment detection |
| `fastapi-output` | `src/facade.rs` | `RichOutput` — main API surface |
| `fastapi-output` | `src/mode.rs` | `OutputMode` (Rich/Plain/Minimal) |

### Feature Flags

**Facade crate (`fastapi-rust`):**

```toml
[features]
default = ["output"]
output = ["dep:fastapi-output", "fastapi-output/rich"]    # Rich console output with agent detection
output-plain = ["dep:fastapi-output"]                      # Plain-text-only output (smaller binary)
full = ["output", "fastapi-output/full"]                   # All output features + every theme
```

**Core crate (`fastapi-core`):**

```toml
[features]
regex = ["dep:regex"]          # Regex support in testing assertions
compression = []               # Response compression middleware (reserved)
```

**Output crate (`fastapi-output`):**

```toml
[features]
default = ["rich"]
rich = ["dep:rich_rust"]       # Rich terminal rendering
full = ["rich", "rich_rust/full"]  # All themes and components
```

### Core Types Quick Reference

| Type | Purpose |
|------|---------|
| `App` | Application container — routes, middleware, state, config |
| `AppBuilder` | Builder pattern for `App` construction |
| `Request` | Parsed HTTP request with headers, body, path params |
| `Response` | HTTP response with status, headers, body |
| `StatusCode` | HTTP status codes (200, 404, 500, etc.) |
| `HttpError` | Unified error type with status code + detail |
| `FromRequest` | Async trait — extract typed data from requests |
| `IntoResponse` | Trait — convert handler return into HTTP response |
| `Middleware` | Trait — intercept and transform request/response |
| `FromDependency` | Trait — type-based dependency injection |
| `Depends<T>` | Extractor wrapper for injected dependencies |
| `Router` | Radix trie router with O(log n) path lookup |
| `Route` | Route definition with method, path, converters |
| `Path<T>` | Extractor for path parameters |
| `Query<T>` | Extractor for query string parameters |
| `Json<T>` | Extractor/responder for JSON bodies |
| `Header<T>` | Extractor for HTTP headers |
| `BearerToken` | Extractor for Bearer authentication |
| `BasicAuth` | Extractor for HTTP Basic authentication |
| `Pagination` | Extractor for page/per_page query params |
| `RequestContext` | Wrapper around asupersync `Cx` for request lifecycle |
| `Cx` | asupersync capability context — passed to all async operations |
| `Outcome<T, E>` | Four-valued result: Ok, Err, Cancelled, Panicked |
| `OpenApi` | OpenAPI 3.1 document type |
| `JsonSchema` | Trait for compile-time JSON Schema generation |
| `Validate` | Trait for compile-time validation rules |
| `GracefulShutdown` | Coordinated multi-phase server shutdown |
| `TestClient` | In-process HTTP test client (no actual TCP) |

### Porting Methodology

This project is ported from Python FastAPI using a **spec-first methodology**:

1. **Extract spec from legacy** → `EXISTING_FASTAPI_STRUCTURE.md` captures all behaviors and data structures
2. **Design Rust architecture** → `PROPOSED_RUST_ARCHITECTURE.md` defines the Rust-idiomatic approach
3. **Implement from spec** → NEVER translate Python to Rust line-by-line

**Critical rules:**
- After reading the spec, you should NOT need legacy code
- Extract behaviors and data structures, not implementation details
- Consult ONLY the spec doc during implementation

### Key Design Decisions

- **Zero-copy HTTP parsing** — Request parser works directly on byte buffers, no allocation for headers
- **Radix trie router** — O(log n) lookups with type-safe path parameter converters (int, float, uuid, slug, path)
- **Proc macros for everything** — Route registration, validation, OpenAPI schema — all at compile time, zero runtime reflection
- **`FromRequest` extractor pattern** — Each parameter type (Path, Query, Json, Header, etc.) implements an async extractor trait
- **Type-based DI** — `FromDependency` trait + `Depends<T>` wrapper, not function-based like Python's `Depends(func)`
- **asupersync exclusively** — NO tokio/reqwest/hyper. All async via `Cx` + structured concurrency
- **Cancel-correct lifecycle** — Request handlers run in asupersync regions with budget-based timeouts
- **Agent-aware output** — Auto-detects AI agent environments and switches to plain text (no ANSI codes)
- **Minimal dependency stack** — Only asupersync + serde as core external deps; everything else built in-house
- **Facade crate pattern** — `fastapi-rust` re-exports everything so users need a single dependency
- **`fastapi-types` leaf crate** — Zero-dep crate with shared `Method` enum to break dependency cycles
