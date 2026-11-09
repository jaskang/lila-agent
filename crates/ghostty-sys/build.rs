//! Links the prebuilt libghostty-vt in `libghostty-vt/<target>/` and generates
//! bindings from the headers next to it (`libghostty-vt/include`, the same
//! release). `update-prebuilt.sh` refreshes both.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let target = std::env::var("TARGET").unwrap();
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    let prebuilt = manifest.join("libghostty-vt");
    println!("cargo:rerun-if-changed={}", prebuilt.display());

    // Only the static archive in the search path, so the linker can't pick a shared one.
    let name = if msvc { "ghostty-vt.lib" } else { "libghostty-vt.a" };
    let staged = prebuilt.join(&target).join(name);
    assert!(staged.is_file(), "no prebuilt libghostty-vt for {target}: add {} (see update-prebuilt.sh)", staged.display());
    let lib = out.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::copy(&staged, lib.join(name)).unwrap();
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
