# Third-party code

| Where | From | License |
|---|---|---|
| `crates/ghostty-sys/` | [herdr-app](https://github.com/herdrdev/herdr-app) `crates/ghostty-sys` | Apache-2.0 |
| `crates/lila-term/src/{vt,hc,sprites,links,win32_keys}.rs` | [herdr-app](https://github.com/herdrdev/herdr-app) `src/` (trimmed; each file says so at the top) | Apache-2.0 |
| `vendor/ghostty/` (git submodule) | [Ghostty](https://github.com/ghostty-org/ghostty) (libghostty-vt) | MIT |
| `crates/ghostty-sys/libghostty-vt/` (prebuilt library and headers) | [Ghostty](https://github.com/ghostty-org/ghostty) libghostty-vt release `ghostty-vt-<version>.xcframework.zip`; the library also contains simdutf (Apache-2.0 OR MIT), Highway (Apache-2.0 OR BSD-3-Clause) and Wuffs (Apache-2.0 OR MIT) | MIT |
| dependency `gpui-kit` and the `gpui-pre` crates it pins | [GPUI Kit](https://github.com/longbridge/gpui-kit), Zed's GPUI | Apache-2.0 |
