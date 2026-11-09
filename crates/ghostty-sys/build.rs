//! Links libghostty-vt and generates bindings from its headers.
//!
//! The library comes prebuilt from `libghostty-vt/<target>/` when there is one
//! (macOS arm64; refresh with `update-prebuilt.sh`). Other targets build it from
//! vendor/ghostty with Zig 0.17. The headers are always the ones in
//! `libghostty-vt/include`, the same release as the library.

use std::path::{Path, PathBuf};
use std::process::Command;

// Zig builds for the machine it runs on unless told otherwise: its CPU's
// newest instructions, which crash older CPUs, and never another target's.
fn zig_target(target: &str) -> &'static str {
    match target {
        "x86_64-unknown-linux-gnu" => "x86_64-linux-gnu",
        "aarch64-unknown-linux-gnu" => "aarch64-linux-gnu",
        "x86_64-apple-darwin" => "x86_64-macos",
        "aarch64-apple-darwin" => "aarch64-macos",
        "x86_64-pc-windows-msvc" => "x86_64-windows-msvc",
        "aarch64-pc-windows-msvc" => "aarch64-windows-msvc",
        other => panic!("ghostty-sys: no Zig target for {other}"),
    }
}

fn zig() -> String {
    std::env::var("ZIG").unwrap_or_else(|_| "zig".into())
}

/// Builds the static library with Zig into `out`; returns where it landed.
fn build_with_zig(manifest: &Path, out: &Path, target: &str, msvc: bool) -> PathBuf {
    let src = manifest
        .join("../../vendor/ghostty")
        .canonicalize()
        .unwrap_or_else(|_| panic!("no prebuilt libghostty-vt for {target}, and vendor/ghostty is missing: run `git submodule update --init --depth 1`"));
    // Windows' canonical form is `\\?\C:\...`, which Zig can't always resolve.
    let src = PathBuf::from(src.to_string_lossy().trim_start_matches(r"\\?\").to_owned());
    for path in ["src", "build.zig", "build.zig.zon"] {
        println!("cargo:rerun-if-changed={}", src.join(path).display());
    }
    let zig = zig();
    let status = Command::new(&zig)
        .current_dir(&src)
        // Only the static library is linked. Ghostty builds the xcframework whenever it finds
        // xcodebuild, and with only the Command Line Tools /usr/bin/xcodebuild exists but fails.
        .args(["build", "-Demit-lib-vt", "-Demit-xcframework=false", "-Doptimize=ReleaseFast"])
        .arg(format!("-Dtarget={}", zig_target(target)))
        .arg("--prefix")
        .arg(out.join("zig-out"))
        .arg("--cache-dir")
        .arg(out.join("zig-cache"))
        .status()
        .unwrap_or_else(|e| panic!("no prebuilt libghostty-vt for {target}, and {zig:?} can't run (Zig 0.17 required): {e}"));
    assert!(status.success(), "zig build of libghostty-vt failed: {status}");
    out.join("zig-out/lib").join(if msvc { "ghostty-vt-static.lib" } else { "libghostty-vt.a" })
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let target = std::env::var("TARGET").unwrap();
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    let prebuilt = manifest.join("libghostty-vt");
    println!("cargo:rerun-if-changed={}", prebuilt.display());
    println!("cargo:rerun-if-env-changed=ZIG");

    // Only the static archive in the search path, so the linker can't pick a shared one.
    let name = if msvc { "ghostty-vt.lib" } else { "libghostty-vt.a" };
    let staged = prebuilt.join(&target).join(name);
    let built = if staged.is_file() { staged } else { build_with_zig(&manifest, &out, &target, msvc) };
    let lib = out.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::copy(&built, lib.join(name)).unwrap();
    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo:rustc-link-lib=static=ghostty-vt");

    let include = prebuilt.join("include");
    let mut builder = bindgen::Builder::default()
        .header(include.join("ghostty/vt.h").to_string_lossy())
        .clang_arg(format!("-I{}", include.display()));
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        // Apple's libclang has its builtin headers but needs the SDK for the C library.
        if let Ok(sdk) = Command::new("xcrun").args(["--show-sdk-path"]).output() {
            builder = builder.clang_arg(format!("-isysroot{}", String::from_utf8_lossy(&sdk.stdout).trim()));
        }
    } else if Command::new("clang").arg("--version").output().is_err() {
        // libclang without a full clang install lacks its builtin headers; Zig ships the
        // same ones. With a clang installed, bindgen asks it for its own, which match its
        // libclang (Zig's newer ones break older ones: Ubuntu 22.04's clang 14).
        let zig_env = Command::new(zig()).arg("env").output().expect("zig env");
        let zig_env = String::from_utf8_lossy(&zig_env.stdout);
        let zig_lib = zig_env
            .lines()
            .find_map(|l| l.trim().strip_prefix(".lib_dir = \""))
            .and_then(|l| l.strip_suffix("\","))
            .expect("zig env has no lib_dir");
        builder = builder.clang_arg(format!("-isystem{zig_lib}/include"));
    }
    builder
        .allowlist_function("ghostty_.*")
        .allowlist_type("Ghostty.*")
        .allowlist_var("GHOSTTY_.*")
        .prepend_enum_name(false)
        .derive_default(true)
        .layout_tests(false)
        .generate()
        .expect("bindgen failed on ghostty/vt.h")
        .write_to_file(out.join("bindings.rs"))
        .unwrap();
}
