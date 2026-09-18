//! Building the test extensions in `tests/fixtures` for wasm32-wasip2.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Where fixtures build, outside the source tree.
pub fn fixture_target_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ext-fixtures")
}

/// The built component of the fixture `name`, or `None` (after a note) when
/// this machine cannot build wasm32-wasip2. `OKENA_REQUIRE_WASM` turns the
/// skip into a failure, so CI proves the tests ran.
pub fn fixture_wasm(name: &str) -> Option<PathBuf> {
    static BUILT: OnceLock<Mutex<std::collections::HashMap<String, Option<PathBuf>>>> = OnceLock::new();
    let mut built = BUILT.get_or_init(Default::default).lock().expect("lock");
    if let Some(done) = built.get(name) {
        return done.clone();
    }
    let result = build(name);
    built.insert(name.to_string(), result.clone());
    result
}

fn build(name: &str) -> Option<PathBuf> {
    let dir = fixtures_dir().join(name);
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let output = std::process::Command::new(cargo)
        .args(["build", "--release", "--target", "wasm32-wasip2", "--target-dir"])
        .arg(fixture_target_dir())
        .current_dir(&dir)
        .output()
        .expect("run cargo");
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let missing_target = stderr.contains("target may not be installed")
            || stderr.contains("can't find crate for `core`")
            || stderr.contains("can't find crate for `std`");
        assert!(
            missing_target && std::env::var_os("OKENA_REQUIRE_WASM").is_none(),
            "building fixture {name} failed:\n{stderr}"
        );
        eprintln!("skipping: the wasm32-wasip2 target is not installed (rustup target add wasm32-wasip2)");
        return None;
    }
    let crate_name = format!("okena_ext_{}", name.replace('-', "_"));
    Some(
        fixture_target_dir()
            .join("wasm32-wasip2/release")
            .join(format!("{crate_name}.wasm")),
    )
}
