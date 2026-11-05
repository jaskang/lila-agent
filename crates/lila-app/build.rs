//! macOS: link as if against the 15.5 SDK. AppKit gives windows built with a
//! newer SDK the larger, wider-spaced window buttons; with 15.5 they match
//! Herdr's (and the space the sidebar leaves for them, `ui::TRAFFIC_LIGHTS`).

fn main() {
    println!("cargo:rerun-if-env-changed=MACOSX_DEPLOYMENT_TARGET");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let min = std::env::var("MACOSX_DEPLOYMENT_TARGET").unwrap_or_else(|_| "11.0".into());
        println!("cargo:rustc-link-arg-bins=-Wl,-platform_version,macos,{min},15.5");
    }
}
