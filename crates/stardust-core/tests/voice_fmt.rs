//! Formatting regressions for the voice lane.
//!
//! CI runs `cargo fmt --all -- --check`, which applies the rustfmt style of the
//! workspace edition. These tests run the same style check on this crate's sources.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The `edition` in the workspace `Cargo.toml`. `cargo fmt` formats with its style.
const WORKSPACE_EDITION: &str = "2021";

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Sorted `.rs` files directly inside `dir` whose file names satisfy `keep`.
fn rust_files(dir: &Path, keep: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(&keep)
        })
        .collect();
    files.sort();
    files
}

fn assert_workspace_style(files: &[PathBuf]) {
    assert!(!files.is_empty(), "no Rust files selected");
    let rustfmt = std::env::var_os("RUSTFMT").unwrap_or_else(|| OsString::from("rustfmt"));
    let output = Command::new(rustfmt)
        .args(["--edition", WORKSPACE_EDITION, "--check"])
        .args(files)
        .output()
        .expect("rustfmt must be installed; rust-toolchain.toml lists the rustfmt component");
    assert!(
        output.status.success(),
        "rustfmt --edition {WORKSPACE_EDITION} --check reported style drift:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn core_modules_outside_voice_keep_workspace_rustfmt_style() {
    // lib.rs is left out because rustfmt would also descend into the voice module.
    let files = rust_files(&crate_dir().join("src"), |name| {
        name != "lib.rs" && name != "voice.rs"
    });
    assert_workspace_style(&files);
}

#[test]
fn voice_sources_keep_workspace_rustfmt_style() {
    let src = crate_dir().join("src");
    let files = [
        vec![src.join("voice.rs")],
        rust_files(&src.join("voice"), |_| true),
        rust_files(&crate_dir().join("tests"), |name| {
            name.starts_with("voice_")
        }),
    ]
    .concat();
    assert_workspace_style(&files);
}
