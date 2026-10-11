//! Dependency policy for the workspace lock.
//!
//! The project forbids Tokio and its ecosystem (AGENTS.md, "Async Runtime:
//! asupersync"). Cargo records target-specific dependencies for every target,
//! so a Tokio entry in Cargo.lock means some dependency path can pull it in.
//! The v0.5.0 lock refresh did exactly that through wasm-bindgen-futures
//! 0.4.79, which adds Tokio under an Emscripten cfg (issue #32).

use std::path::Path;

#[test]
fn workspace_lock_contains_no_tokio_packages() {
    let lock = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock");
    let text = std::fs::read_to_string(&lock).expect("read the workspace Cargo.lock");
    let packages: Vec<&str> = text
        .lines()
        .filter_map(|line| line.strip_prefix("name = \"")?.strip_suffix('"'))
        .collect();
    assert!(
        packages.contains(&"asupersync"),
        "expected to read the workspace lock at {}",
        lock.display()
    );
    let forbidden: Vec<&str> = packages
        .iter()
        .copied()
        .filter(|name| *name == "tokio" || name.starts_with("tokio-"))
        .collect();
    assert!(
        forbidden.is_empty(),
        "Cargo.lock contains forbidden Tokio packages {forbidden:?}; see AGENTS.md and issue #32"
    );
}
