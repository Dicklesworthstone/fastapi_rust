#!/usr/bin/env bash
#
# fastapi_rust installer
#
# fastapi_rust is a Rust *library* (crates.io package `fastapi-rust`), so there
# is no binary to download. This installer gets a machine from zero to a
# compiling fastapi_rust project:
#
#   1. Checks (and optionally installs) a Rust toolchain that meets the MSRV.
#   2. Resolves the latest published `fastapi-rust` version from crates.io.
#   3. Optionally scaffolds a new project (`--new NAME`) that builds and runs.
#   4. Installs the fastapi_rust skill for detected AI coding agents
#      (Claude Code, Codex CLI, Gemini/Antigravity CLI, Cursor).
#
# One-liner install (with cache buster):
#   curl -fsSL "https://raw.githubusercontent.com/Dicklesworthstone/fastapi_rust/main/install.sh?$(date +%s)" | bash
#
# Scaffold a project in one go:
#   curl -fsSL "https://raw.githubusercontent.com/Dicklesworthstone/fastapi_rust/main/install.sh?$(date +%s)" | bash -s -- --new my_api
#
# Options:
#   --new NAME         Scaffold a new fastapi_rust project in ./NAME (or --dir)
#   --dir DIR          Parent directory for --new (default: current directory)
#   --version X.Y.Z    Pin the fastapi-rust crate version (default: latest on crates.io)
#   --build            After scaffolding, run `cargo build` to prime the cache
#   --easy-mode        Install/upgrade Rust via rustup non-interactively when needed
#   --skill-only       Only install the AI-agent skill; skip toolchain/scaffold
#   --no-skill         Skip AI-agent skill installation
#   --offline          Skip network lookups (needs --version, or uses the pinned fallback)
#   --force            Overwrite an existing scaffold's Cargo.toml/src/main.rs and the skill
#   --quiet            Suppress non-error output
#   --no-gum           Disable gum formatting even if available
#   -h, --help         Show this help
#
set -euo pipefail
umask 022
shopt -s lastpipe 2>/dev/null || true

OWNER="${OWNER:-Dicklesworthstone}"
REPO="${REPO:-fastapi_rust}"
CRATE="fastapi-rust"
# Minimum rustc for the fastapi-rust 0.4.1+ dependency graph (asupersync 0.4 -> sysinfo 0.39).
MSRV="1.95.0"
# Last-resort version if crates.io cannot be reached and no --version was given.
FALLBACK_VERSION="0.4.2"
ASUPERSYNC_REQ="0.4"

VERSION="${VERSION:-}"
NEW_NAME=""
NEW_DIR="."
DO_BUILD=0
EASY=0
SKILL_ONLY=0
NO_SKILL=0
OFFLINE="${FASTAPI_RUST_OFFLINE:-0}"
FORCE=0
QUIET=0
NO_GUM=0
LOCK_DIR="${TMPDIR:-/tmp}/fastapi-rust-install.lock"
TEMP_DIR=""

# ── Output stack ─────────────────────────────────────────────────────────────

HAS_GUM=0
if command -v gum &>/dev/null && [ -t 1 ]; then
  HAS_GUM=1
fi

# Under `curl … | bash` stdin is the script pipe; prompts go through /dev/tty.
INSTALL_TTY=""
if ( : </dev/tty >/dev/tty ) 2>/dev/null; then
  INSTALL_TTY="/dev/tty"
fi
have_tty() { [ -n "$INSTALL_TTY" ]; }
PROMPT_TIMEOUT="${FASTAPI_RUST_PROMPT_TIMEOUT:-120}"

use_gum() { [ "$HAS_GUM" -eq 1 ] && [ "$NO_GUM" -eq 0 ]; }

log()  { [ "$QUIET" -eq 1 ] && return 0; echo -e "$@"; }
info() {
  [ "$QUIET" -eq 1 ] && return 0
  if use_gum; then gum style --foreground 39 "→ $*"; else echo -e "\033[0;34m→\033[0m $*"; fi
}
ok() {
  [ "$QUIET" -eq 1 ] && return 0
  if use_gum; then gum style --foreground 42 "✓ $*"; else echo -e "\033[0;32m✓\033[0m $*"; fi
}
warn() {
  [ "$QUIET" -eq 1 ] && return 0
  if use_gum; then gum style --foreground 214 "⚠ $*"; else echo -e "\033[1;33m⚠\033[0m $*"; fi
}
err() {
  if use_gum; then gum style --foreground 196 "✗ $*" >&2; else echo -e "\033[0;31m✗\033[0m $*" >&2; fi
}
die() { err "$@"; exit 1; }

run_with_spinner() {
  local title="$1"; shift
  if use_gum && [ "$QUIET" -eq 0 ]; then
    gum spin --spinner dot --title "$title" -- "$@"
  else
    info "$title"
    "$@"
  fi
}

# ask_yn "<prompt>" "<y|n default>" — yes/no on the operator TTY (works under curl|bash).
ask_yn() {
  local prompt="$1" default="${2:-n}" reply="" status=0
  if have_tty; then
    printf '%s ' "$prompt" >"$INSTALL_TTY" 2>/dev/null || true
    IFS= read -r -t "$PROMPT_TIMEOUT" reply <"$INSTALL_TTY" 2>/dev/null || status=$?
    if [ "$status" -ne 0 ]; then
      reply=""
      printf '\n' >"$INSTALL_TTY" 2>/dev/null || true
      warn "No reply (timeout after ${PROMPT_TIMEOUT}s or end of input); taking the default (${default})."
    fi
  fi
  [ -n "$reply" ] || reply="$default"
  if [ "$default" = "y" ]; then
    case "$reply" in n|N|no|No|NO) return 1 ;; *) return 0 ;; esac
  else
    case "$reply" in y|Y|yes|Yes|YES) return 0 ;; *) return 1 ;; esac
  fi
}

# draw_box "color" line... — double-line box with ANSI-aware width.
draw_box() {
  local color="$1"; shift
  local lines=("$@")
  local max_width=0 esc strip_ansi_sed line stripped len
  esc=$(printf '\033')
  strip_ansi_sed="s/${esc}\\[[0-9;]*m//g"
  for line in ${lines[@]+"${lines[@]}"}; do
    stripped=$(printf '%b' "$line" | LC_ALL=C sed "$strip_ansi_sed")
    len=${#stripped}
    [ "$len" -gt "$max_width" ] && max_width=$len
  done
  local inner_width=$((max_width + 4)) border="" i
  for ((i=0; i<inner_width; i++)); do border+="═"; done
  printf "\033[%sm╔%s╗\033[0m\n" "$color" "$border"
  for line in ${lines[@]+"${lines[@]}"}; do
    stripped=$(printf '%b' "$line" | LC_ALL=C sed "$strip_ansi_sed")
    len=${#stripped}
    local padding=$((max_width - len)) pad_str=""
    for ((i=0; i<padding; i++)); do pad_str+=" "; done
    printf "\033[%sm║\033[0m  %b%s  \033[%sm║\033[0m\n" "$color" "$line" "$pad_str" "$color"
  done
  printf "\033[%sm╚%s╝\033[0m\n" "$color" "$border"
}

usage() {
  cat <<'USAGE'
fastapi_rust installer — library bootstrapper (there is no binary to install)

Usage:
  curl -fsSL https://raw.githubusercontent.com/Dicklesworthstone/fastapi_rust/main/install.sh | bash -s -- [options]
  bash install.sh [options]

Options:
  --new NAME         Scaffold a new fastapi_rust project in ./NAME (or --dir)
  --dir DIR          Parent directory for --new (default: current directory)
  --version X.Y.Z    Pin the fastapi-rust crate version (default: latest on crates.io)
  --build            After scaffolding, run `cargo build` to prime the cache
  --easy-mode        Install/upgrade Rust via rustup non-interactively when needed
  --skill-only       Only install the AI-agent skill; skip toolchain/scaffold
  --no-skill         Skip AI-agent skill installation
  --offline          Skip network lookups (needs --version, or uses the pinned fallback)
  --force            Overwrite an existing scaffold's Cargo.toml/src/main.rs and the skill
  --quiet            Suppress non-error output
  --no-gum           Disable gum formatting even if available
  -h, --help         Show this help
USAGE
}

# ── Args ─────────────────────────────────────────────────────────────────────

while [ $# -gt 0 ]; do
  case "$1" in
    --new)        [ $# -ge 2 ] || die "--new requires a NAME"; NEW_NAME="$2"; shift 2 ;;
    --new=*)      NEW_NAME="${1#*=}"; shift ;;
    --dir)        [ $# -ge 2 ] || die "--dir requires a DIR"; NEW_DIR="$2"; shift 2 ;;
    --dir=*)      NEW_DIR="${1#*=}"; shift ;;
    --version)    [ $# -ge 2 ] || die "--version requires a value"; VERSION="$2"; shift 2 ;;
    --version=*)  VERSION="${1#*=}"; shift ;;
    --build)      DO_BUILD=1; shift ;;
    --easy-mode)  EASY=1; shift ;;
    --skill-only) SKILL_ONLY=1; shift ;;
    --no-skill)   NO_SKILL=1; shift ;;
    --offline)    OFFLINE=1; shift ;;
    --force)      FORCE=1; shift ;;
    --quiet)      QUIET=1; shift ;;
    --no-gum)     NO_GUM=1; shift ;;
    -h|--help)    usage; exit 0 ;;
    *)            die "Unknown option: $1 (see --help)" ;;
  esac
done

VERSION="${VERSION#v}"
if [ -n "$NEW_NAME" ] && ! [[ "$NEW_NAME" =~ ^[A-Za-z_][A-Za-z0-9_-]*$ ]]; then
  die "--new NAME must be a valid Cargo package name (got '$NEW_NAME')"
fi

# ── Locking + cleanup ────────────────────────────────────────────────────────

cleanup() {
  [ -n "$TEMP_DIR" ] && [ -d "$TEMP_DIR" ] && rm -rf "$TEMP_DIR"
  if [ -d "$LOCK_DIR" ] && [ "$(cat "$LOCK_DIR/pid" 2>/dev/null || echo)" = "$$" ]; then
    rm -rf "$LOCK_DIR"
  fi
  return 0
}
trap cleanup EXIT
# Never die silently: name the line when `set -e` trips on an unhandled failure.
trap 'err "Installer failed unexpectedly at line $LINENO (re-run with: bash -x install.sh ...)"' ERR

acquire_lock() {
  if mkdir "$LOCK_DIR" 2>/dev/null; then
    echo $$ > "$LOCK_DIR/pid"
    return 0
  fi
  local pid
  pid=$(cat "$LOCK_DIR/pid" 2>/dev/null || echo "")
  if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
    die "Another fastapi_rust install is in progress (PID $pid). Remove $LOCK_DIR if that is wrong."
  fi
  warn "Removing stale install lock"
  rm -rf "$LOCK_DIR"
  mkdir "$LOCK_DIR" || die "Could not create lock $LOCK_DIR"
  echo $$ > "$LOCK_DIR/pid"
}

# ── Proxy ────────────────────────────────────────────────────────────────────

PROXY_ARGS=()
setup_proxy() {
  if [ -n "${HTTPS_PROXY:-}" ]; then
    PROXY_ARGS=(--proxy "$HTTPS_PROXY"); info "Using HTTPS proxy: $HTTPS_PROXY"
  elif [ -n "${HTTP_PROXY:-}" ]; then
    PROXY_ARGS=(--proxy "$HTTP_PROXY"); info "Using HTTP proxy: $HTTP_PROXY"
  fi
}

# ── Platform ─────────────────────────────────────────────────────────────────

OS=""; ARCH=""; IS_WSL=0
detect_platform() {
  OS=$(uname -s | tr '[:upper:]' '[:lower:]')
  ARCH=$(uname -m)
  case "$ARCH" in
    x86_64|amd64)  ARCH="x86_64" ;;
    arm64|aarch64) ARCH="aarch64" ;;
  esac
  case "$OS" in
    linux|darwin) ;;
    mingw*|msys*|cygwin*) warn "Windows shell detected; use PowerShell + rustup.rs directly, or WSL. Continuing best-effort." ;;
    *) warn "Unrecognized OS '$OS'; continuing best-effort." ;;
  esac
  if [ "$OS" = "linux" ] && grep -qi microsoft /proc/version 2>/dev/null; then
    IS_WSL=1; export IS_WSL
    warn "WSL detected: keep the project on the Linux filesystem (not /mnt/c) for sane build times."
  fi
  info "Platform: ${OS}/${ARCH}"
}

# ── Preflight ────────────────────────────────────────────────────────────────

version_ge() { # version_ge A B  -> true if A >= B (dotted numerics)
  [ "$(printf '%s\n%s\n' "$2" "$1" | sort -t. -k1,1n -k2,2n -k3,3n | head -n1)" = "$2" ]
}

check_disk_space() {
  local target="$1" need_kb="$2" avail_kb
  avail_kb=$(df -Pk "$target" 2>/dev/null | awk 'NR==2 {print $4}')
  if [ -n "$avail_kb" ] && [ "$avail_kb" -lt "$need_kb" ]; then
    die "Not enough disk space in $target: $((avail_kb/1024))MB free, need $((need_kb/1024))MB"
  fi
}

check_network() {
  [ "$OFFLINE" -eq 1 ] && return 0
  if ! curl -fsSL "${PROXY_ARGS[@]}" --connect-timeout 3 -o /dev/null "https://index.crates.io/config.json" 2>/dev/null; then
    warn "Cannot reach crates.io; switching to offline mode (version lookup skipped)"
    OFFLINE=1
  fi
}

RUSTC_VERSION=""
TOOLCHAIN_STATUS="" # ok|installed|missing|too-old
# Print the rustc semver (e.g. 1.95.0 or 1.100.0-nightly); empty/non-zero if unusable.
rustc_version() {
  command -v rustc &>/dev/null || return 1
  local out
  out=$(timeout 15 rustc --version 2>/dev/null) || out=$(rustc --version 2>/dev/null) || return 1
  printf '%s' "$out" | awk '{print $2}'
}
check_toolchain() {
  if ! command -v cargo &>/dev/null || ! command -v rustc &>/dev/null; then
    # rustup may be installed but not on PATH in this shell (fresh install).
    if [ -x "$HOME/.cargo/bin/cargo" ]; then
      export PATH="$HOME/.cargo/bin:$PATH"
    fi
  fi
  # `rustc` may exist only as a rustup proxy with no toolchain installed, in
  # which case `rustc --version` fails; treat that the same as "no toolchain".
  RUSTC_VERSION=$(rustc_version || true)
  if [ -n "$RUSTC_VERSION" ]; then
    local numeric="${RUSTC_VERSION%%-*}"
    if version_ge "$numeric" "$MSRV"; then
      TOOLCHAIN_STATUS="ok"
      ok "rustc $RUSTC_VERSION (MSRV $MSRV satisfied)"
      return 0
    fi
    TOOLCHAIN_STATUS="too-old"
    warn "rustc $RUSTC_VERSION is older than the MSRV $MSRV"
    if command -v rustup &>/dev/null; then
      if [ "$EASY" -eq 1 ] || ask_yn "Run 'rustup update stable' now? [y/N]" "n"; then
        run_with_spinner "Updating stable toolchain via rustup" rustup update stable
        RUSTC_VERSION=$(rustc_version || true)
        TOOLCHAIN_STATUS="ok"
        ok "rustc $RUSTC_VERSION"
        return 0
      fi
    fi
    die "Upgrade Rust to $MSRV or newer (https://rustup.rs) and re-run."
  fi

  TOOLCHAIN_STATUS="missing"
  warn "No Rust toolchain found (cargo/rustc not on PATH)"
  [ "$OFFLINE" -eq 1 ] && die "Offline and no toolchain available; install Rust from https://rustup.rs first."
  if [ "$EASY" -eq 1 ] || ask_yn "Install Rust via rustup (https://rustup.rs) now? [y/N]" "n"; then
    local rustup_init="$TEMP_DIR/rustup-init.sh"
    curl -fsSL "${PROXY_ARGS[@]}" --proto '=https' --tlsv1.2 https://sh.rustup.rs -o "$rustup_init" \
      || die "Failed to download rustup-init"
    run_with_spinner "Installing Rust (stable) via rustup" sh "$rustup_init" -y --profile minimal --default-toolchain stable --no-modify-path
    export PATH="$HOME/.cargo/bin:$PATH"
    RUSTC_VERSION=$(rustc_version || true)
    [ -n "$RUSTC_VERSION" ] || die "rustup finished but rustc is not usable yet; open a new shell and re-run."
    TOOLCHAIN_STATUS="installed"
    ok "Installed rustc $RUSTC_VERSION"
    return 0
  fi
  die "A Rust toolchain (>= $MSRV) is required. Install it from https://rustup.rs and re-run."
}

# ── Version resolution ───────────────────────────────────────────────────────

VERSION_SOURCE=""
resolve_version() {
  if [ -n "$VERSION" ]; then
    VERSION_SOURCE="pinned (--version)"
    return 0
  fi
  if [ "$OFFLINE" -eq 0 ]; then
    # Sparse index: one JSON line per version; take the newest non-yanked one.
    local index_url="https://index.crates.io/fa/st/${CRATE}" line v
    if line=$(curl -fsSL "${PROXY_ARGS[@]}" --connect-timeout 5 "$index_url" 2>/dev/null | grep '"yanked":false' | tail -n1); then
      v=$(printf '%s' "$line" | sed -nE 's/.*"vers":"([^"]+)".*/\1/p')
      if [ -n "$v" ]; then
        VERSION="$v"; VERSION_SOURCE="crates.io index"
        return 0
      fi
    fi
    # Fallback: latest GitHub release tag.
    if v=$(curl -fsSL "${PROXY_ARGS[@]}" --connect-timeout 5 "https://api.github.com/repos/${OWNER}/${REPO}/releases/latest" 2>/dev/null \
          | sed -nE 's/.*"tag_name": *"v?([^"]+)".*/\1/p' | head -n1) && [ -n "$v" ]; then
      VERSION="$v"; VERSION_SOURCE="GitHub releases"
      return 0
    fi
    warn "Could not resolve the latest version online"
  fi
  VERSION="$FALLBACK_VERSION"; VERSION_SOURCE="installer fallback"
}

# ── Scaffold ─────────────────────────────────────────────────────────────────

PROJECT_PATH=""
SCAFFOLD_STATUS="" # created|skipped|failed
scaffold_project() {
  [ -n "$NEW_NAME" ] || return 0
  mkdir -p "$NEW_DIR" || die "Cannot create --dir $NEW_DIR"
  PROJECT_PATH="$(cd "$NEW_DIR" && pwd)/$NEW_NAME"
  if [ -e "$PROJECT_PATH" ]; then
    if [ "$FORCE" -eq 0 ]; then
      die "$PROJECT_PATH already exists (use --force to overwrite its Cargo.toml and src/main.rs)"
    fi
    warn "Overwriting Cargo.toml and src/main.rs in existing $PROJECT_PATH"
  fi
  # A bare scaffold is tiny; a first `cargo build` of the dependency graph is not.
  if [ "$DO_BUILD" -eq 1 ]; then
    check_disk_space "$(dirname "$PROJECT_PATH")" $((1536 * 1024))
  else
    check_disk_space "$(dirname "$PROJECT_PATH")" $((10 * 1024))
  fi
  mkdir -p "$PROJECT_PATH/src"
  local crate_ident="${NEW_NAME//-/_}"

  cat > "$PROJECT_PATH/Cargo.toml" <<EOF
[package]
name = "${NEW_NAME}"
version = "0.1.0"
edition = "2024"
rust-version = "${MSRV%.*}"

[dependencies]
fastapi-rust = "${VERSION}"
asupersync = { version = "${ASUPERSYNC_REQ}", default-features = false }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
EOF

  cat > "$PROJECT_PATH/src/main.rs" <<'EOF'
//! Generated by the fastapi_rust installer.
//!
//! Run it:        cargo run
//! Try it:        curl -i http://127.0.0.1:8000/
//!                curl -i http://127.0.0.1:8000/health
//! Test it:       cargo test
//!
//! Docs: https://docs.rs/fastapi-rust — the `prelude` re-exports the extractor
//! and macro surface (`#[get]`, `Json<T>`, `Path<T>`, `Query<T>`, ...).

use fastapi_rust::core::{App, Request, RequestContext, Response, ResponseBody};

/// GET / — plain-text greeting.
fn hello(_ctx: &RequestContext, _req: &mut Request) -> std::future::Ready<Response> {
    std::future::ready(Response::ok().body(ResponseBody::Bytes(b"Hello from fastapi_rust!".to_vec())))
}

/// GET /health — JSON liveness probe.
fn health(_ctx: &RequestContext, _req: &mut Request) -> std::future::Ready<Response> {
    std::future::ready(
        Response::ok().body(ResponseBody::Bytes(br#"{"status":"healthy"}"#.to_vec())),
    )
}

fn build_app() -> App {
    App::builder().get("/", hello).get("/health", health).build()
}

fn main() {
    let addr = std::env::var("FASTAPI_ADDR").unwrap_or_else(|_| "127.0.0.1:8000".to_string());
    let app = build_app();
    println!("fastapi_rust listening on http://{addr} ({} routes)", app.route_count());

    // fastapi_rust runs on asupersync, a cancel-correct structured-concurrency runtime.
    let runtime = asupersync::runtime::RuntimeBuilder::current_thread()
        .build()
        .expect("asupersync runtime must build");
    if let Err(e) = runtime.block_on(fastapi_rust::serve(app, addr)) {
        eprintln!("server error: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastapi_rust::core::TestClient;

    #[test]
    fn hello_route_responds() {
        let client = TestClient::new(build_app());
        let response = client.get("/").send();
        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(response.text(), "Hello from fastapi_rust!");
    }

    #[test]
    fn health_route_is_json() {
        let client = TestClient::new(build_app());
        let response = client.get("/health").send();
        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(response.text(), r#"{"status":"healthy"}"#);
    }

    #[test]
    fn unknown_route_is_404() {
        let client = TestClient::new(build_app());
        assert_eq!(client.get("/nope").send().status().as_u16(), 404);
    }
}
EOF

  if [ ! -f "$PROJECT_PATH/.gitignore" ]; then
    printf '/target\n' > "$PROJECT_PATH/.gitignore"
  fi
  ok "Scaffolded $PROJECT_PATH (fastapi-rust ${VERSION}, crate ident ${crate_ident})"
  SCAFFOLD_STATUS="created"

  if [ "$DO_BUILD" -eq 1 ]; then
    [ "$OFFLINE" -eq 1 ] && warn "--build with --offline relies on a warm cargo registry cache"
    info "Building ${NEW_NAME} (the first build compiles the whole dependency graph; cargo shows progress)"
    local -a quiet_flag=()
    [ "$QUIET" -eq 1 ] && quiet_flag=(-q)
    if cargo build --manifest-path "$PROJECT_PATH/Cargo.toml" "${quiet_flag[@]}"; then
      ok "cargo build succeeded"
    else
      SCAFFOLD_STATUS="failed"
      die "cargo build failed in $PROJECT_PATH (see output above)"
    fi
  fi
}

# ── AI agent skill ───────────────────────────────────────────────────────────

DETECTED_AGENTS=()
SKILL_STATUS=() # "agent:installed|already|failed|skipped"
detect_agents() {
  DETECTED_AGENTS=()
  { [ -d "$HOME/.claude" ] || command -v claude &>/dev/null; } && DETECTED_AGENTS+=("claude:$HOME/.claude/skills")
  { [ -d "$HOME/.codex" ]  || command -v codex  &>/dev/null; } && DETECTED_AGENTS+=("codex:$HOME/.codex/skills")
  { [ -d "$HOME/.gemini" ] || command -v gemini &>/dev/null || command -v agy &>/dev/null; } && DETECTED_AGENTS+=("gemini:$HOME/.gemini/skills")
  { [ -d "$HOME/.cursor" ] || command -v cursor &>/dev/null; } && DETECTED_AGENTS+=("cursor:$HOME/.cursor/skills")
  return 0
}

write_inline_skill() {
  local dest="$1"
  mkdir -p "$dest"
  cat > "$dest/SKILL.md" <<EOF
---
name: fastapi-rust
description: Build HTTP APIs in Rust with fastapi_rust (crates.io \`fastapi-rust\`), the FastAPI-inspired framework on the asupersync structured-concurrency runtime. Use when adding routes, extractors, middleware, OpenAPI, or tests to a fastapi_rust project, or when a Cargo.toml depends on fastapi-rust.
---

# fastapi_rust

Installed by the fastapi_rust installer for \`fastapi-rust\` ${VERSION} (MSRV ${MSRV}).

## Orientation

- Package \`fastapi-rust\`; crate/import name \`fastapi_rust\` (\`use fastapi_rust::prelude::*;\`).
- Runtime is **asupersync**, never Tokio. Handlers receive \`&Cx\`; call \`cx.checkpoint()\` in loops and long work (it returns \`Result<(), asupersync::Error>\`).
- Submodules: \`fastapi_rust::core\` (App, Request, Response, middleware, extractors, TestClient),
  \`fastapi_rust::server\` (serve, ServerConfig, graceful shutdown), \`fastapi_rust::openapi\`,
  \`fastapi_rust::macros\` (\`#[get]\`, \`#[post]\`, \`JsonSchema\`, \`Validate\`).
- Build on stable >= ${MSRV}: depend on \`asupersync\` with \`default-features = false\` — its default \`nightly-outcome-try\` feature requires nightly.

## Patterns

\`\`\`rust
use fastapi_rust::prelude::*;

#[derive(Serialize, Deserialize, JsonSchema)]
struct Item { id: i64, name: String }

#[get("/items/{id}")]
async fn get_item(cx: &Cx, id: Path<i64>) -> Json<Item> {
    // cx.checkpoint() -> Result<(), asupersync::Error>. Map it yourself: there is
    // no From<asupersync::Error> for HttpError, so a bare \`?\` will not compile.
    if cx.checkpoint().is_err() {
        return Json(Item { id: id.0, name: "cancelled".into() });
    }
    Json(Item { id: id.0, name: "Widget".into() })
}

let app = App::builder()
    .title("My API").version("1.0.0")
    .route_entry(get_item_route())          // generated by #[get]
    .middleware(RequestIdMiddleware::new())
    .build();
runtime.block_on(fastapi_rust::serve(app, "0.0.0.0:8000"));
\`\`\`

- Extractors: \`Path<T>\`, \`Query<T>\`, \`Json<T>\`, \`Header<T>\`, \`State<T>\`, \`Depends<T>\`.
  Validation errors return FastAPI-compatible 422 JSON.
- Tests: \`TestClient::new(app).get("/").send()\` — no sockets, deterministic.
- Server: \`serve(app, addr)\` / \`serve_with_config\`; \`ShutdownController\` for graceful drain.

## Gotchas

- \`App::builder().get(path, handler)\` takes plain fns \`fn(&RequestContext, &mut Request) -> impl Future<Output = Response>\`;
  attribute-macro routes register via \`.route_entry(<name>_route())\`.
- Wildcard / \`{name:path}\` segments must be the final segment.
- Do not add Tokio/Hyper/Axum; the framework is dependency-disciplined by design.

## References

- Repo + CHANGELOG: https://github.com/${OWNER}/${REPO}
- API docs: https://docs.rs/fastapi-rust/${VERSION}
- Examples: https://github.com/${OWNER}/${REPO}/tree/main/crates/fastapi/examples
EOF
}

install_skill() {
  [ "$NO_SKILL" -eq 1 ] && { info "Skipping AI-agent skill (--no-skill)"; return 0; }
  info "Scanning for AI coding agents (Claude Code, Codex, Gemini/Antigravity, Cursor)…"
  detect_agents
  if [ ${#DETECTED_AGENTS[@]} -eq 0 ]; then
    info "No AI coding agents detected; skill not installed"
    return 0
  fi

  # Primary: a skill tarball attached to the GitHub release; fallback: inline.
  local tarball="" skill_src=""
  if [ "$OFFLINE" -eq 0 ]; then
    tarball="$TEMP_DIR/fastapi-rust-skill.tar.gz"
    if curl -fsSL "${PROXY_ARGS[@]}" --connect-timeout 5 \
        "https://github.com/${OWNER}/${REPO}/releases/download/v${VERSION}/fastapi-rust-skill.tar.gz" -o "$tarball" 2>/dev/null \
       && tar -tzf "$tarball" >/dev/null 2>&1; then
      mkdir -p "$TEMP_DIR/skill" && tar -xzf "$tarball" -C "$TEMP_DIR/skill"
      local skill_md
      skill_md=$(find "$TEMP_DIR/skill" -maxdepth 3 -name SKILL.md | head -n1)
      [ -n "$skill_md" ] && skill_src=$(dirname "$skill_md")
    fi
  fi

  local entry agent base dest
  for entry in "${DETECTED_AGENTS[@]}"; do
    agent="${entry%%:*}"; base="${entry#*:}"; dest="$base/fastapi-rust"
    if [ -f "$dest/SKILL.md" ] && [ "$FORCE" -eq 0 ] && grep -q "fastapi-rust\` ${VERSION} " "$dest/SKILL.md" 2>/dev/null; then
      SKILL_STATUS+=("$agent:already"); continue
    fi
    if ! mkdir -p "$dest" 2>/dev/null; then
      SKILL_STATUS+=("$agent:failed"); warn "Cannot write $dest"; continue
    fi
    if [ -n "$skill_src" ]; then
      cp -R "$skill_src"/. "$dest"/ && SKILL_STATUS+=("$agent:installed") || SKILL_STATUS+=("$agent:failed")
    else
      write_inline_skill "$dest" && SKILL_STATUS+=("$agent:installed") || SKILL_STATUS+=("$agent:failed")
    fi
  done
  ok "Skill processed for ${#DETECTED_AGENTS[@]} agent(s)"
}

# ── Summary ──────────────────────────────────────────────────────────────────

show_summary() {
  [ "$QUIET" -eq 1 ] && return 0
  local lines=()
  lines+=("\033[1;32mfastapi_rust is ready\033[0m")
  lines+=("")
  case "$TOOLCHAIN_STATUS" in
    ok)        lines+=("Toolchain:  rustc ${RUSTC_VERSION} (>= ${MSRV})") ;;
    installed) lines+=("Toolchain:  installed rustc ${RUSTC_VERSION} via rustup")
               lines+=("            new shells: source \"\$HOME/.cargo/env\" (PATH was not modified)") ;;
    "")        lines+=("Toolchain:  not checked (--skill-only)") ;;
  esac
  lines+=("Crate:      fastapi-rust ${VERSION} (${VERSION_SOURCE})")
  case "$SCAFFOLD_STATUS" in
    created) lines+=("Project:    ${PROJECT_PATH}") ;;
    *)       lines+=("Project:    none (use --new NAME to scaffold one)") ;;
  esac
  if [ "$NO_SKILL" -eq 1 ]; then
    lines+=("Skill:      skipped (--no-skill)")
  elif [ ${#SKILL_STATUS[@]} -eq 0 ]; then
    lines+=("Skill:      no AI agents detected")
  else
    local s
    for s in "${SKILL_STATUS[@]}"; do
      case "${s#*:}" in
        installed) lines+=("Skill:      ${s%%:*} — installed") ;;
        already)   lines+=("Skill:      ${s%%:*} — already current") ;;
        failed)    lines+=("Skill:      ${s%%:*} — FAILED") ;;
      esac
    done
  fi
  lines+=("")
  if [ "$SCAFFOLD_STATUS" = "created" ]; then
    lines+=("Next:       cd ${PROJECT_PATH} && cargo run")
    lines+=("            curl -i http://127.0.0.1:8000/health")
  else
    lines+=("Add to Cargo.toml:  fastapi-rust = \"${VERSION}\"   asupersync = { version = \"${ASUPERSYNC_REQ}\", default-features = false }")
  fi
  lines+=("Docs:       https://docs.rs/fastapi-rust/${VERSION}")
  lines+=("")
  lines+=("\033[0;90mUndo: delete the project directory and ~/.{claude,codex,gemini,cursor}/skills/fastapi-rust\033[0m")
  lines+=("\033[0;90mrustup (if installed here): rustup self uninstall\033[0m")

  if use_gum; then
    local esc stripped=()
    esc=$(printf '\033')
    local l
    for l in "${lines[@]}"; do stripped+=("$(printf '%b' "$l" | LC_ALL=C sed "s/${esc}\\[[0-9;]*m//g")"); done
    gum style --border double --border-foreground 42 --padding "0 2" --margin "1 0" "${stripped[@]}"
  else
    echo
    draw_box "0;32" "${lines[@]}"
    echo
  fi
}

# ── Main ─────────────────────────────────────────────────────────────────────

main() {
  if [ "$QUIET" -eq 0 ]; then
    if use_gum; then
      gum style --border normal --border-foreground 39 --padding "0 1" --margin "1 0" \
        "$(gum style --foreground 42 --bold 'fastapi_rust installer')" \
        "$(gum style --foreground 245 'FastAPI-inspired Rust web framework on the asupersync runtime')"
    else
      echo -e "\n\033[1;32mfastapi_rust installer\033[0m"
      echo -e "\033[0;90mFastAPI-inspired Rust web framework on the asupersync runtime\033[0m\n"
    fi
  fi

  TEMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/fastapi-rust-install.XXXXXX")
  acquire_lock
  setup_proxy
  detect_platform
  command -v curl &>/dev/null || [ "$OFFLINE" -eq 1 ] || die "curl is required"
  check_network
  resolve_version
  info "fastapi-rust version: ${VERSION} (${VERSION_SOURCE})"

  if [ "$SKILL_ONLY" -eq 0 ]; then
    check_toolchain
    scaffold_project
  fi
  install_skill
  show_summary
}

main "$@"
