# Notices

The Jaynes app reuses parts of other projects. Each keeps its own licence.

| What | From | Licence |
|---|---|---|
| The terminal: `src/hc.rs`, `src/vt.rs`, `src/terminal.rs`, `src/terminal/`, `src/links.rs`, `src/sprites.rs`, `src/spawn.rs`, `src/win32_keys.rs`, `src/settings.rs`, `src/core_bin.rs`, `src/bench.rs`; `crates/ghostty-sys` | [herdr-app](https://github.com/herdrdev/herdr-app) at 00a48db | Apache-2.0, `LICENSE-herdr-app` |
| GPUI snapshot: `vendor/gpui-pre`, `vendor/gpui-pre-macos`, `vendor/gpui-pre-apple`, `vendor/gpui-pre-wgpu` | [Zed](https://github.com/zed-industries/zed), as vendored and patched by herdr-app (patches marked `HERDR:`) | Apache-2.0, each folder's `LICENSE-APACHE` |
| `vendor/gpui-kit` (only `crates/base`) | [longbridge/gpui-kit](https://github.com/longbridge/gpui-kit), as vendored by herdr-app | Apache-2.0, `vendor/gpui-kit/LICENSE-APACHE` |
| `vendor/taffy` | [Taffy](https://github.com/DioxusLabs/taffy), as patched by herdr-app | MIT, `vendor/taffy/LICENSE` |
| `vendor/ghostty` (git submodule, libghostty-vt) | [Ghostty](https://github.com/ghostty-org/ghostty) at 291fd05 | MIT, `vendor/ghostty/LICENSE` |
| JetBrains Mono, Geist and Geist Mono fonts | `assets/fonts` | SIL Open Font License 1.1, `assets/fonts/OFL*.txt` |
| Symbols Nerd Font | `assets/fonts` | MIT, `assets/fonts/LICENSE-SymbolsNerdFont.txt` |

Files copied from herdr-app and changed here say so in their first lines (`JAYNES:`). The terminals themselves live in
[herdr-core](https://github.com/herdrdev/herdr-core) (`hc`, Apache-2.0), which the person installs; the app talks to it
and ships none of it.

Building needs Zig 0.16 for libghostty-vt: `ZIG=/opt/homebrew/opt/zig@0.16/bin/zig cargo build` (Homebrew's `zig@0.16`).
