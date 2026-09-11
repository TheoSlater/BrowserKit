# BrowserKit

BrowserKit is a Rust framework for building custom desktop browsers using Chromium Embedded Framework.

## Architecture

`browserkit` is the small public builder API. `browserkit-runtime` owns the runtime lifecycle. `browserkit-cef` concentrates CEF initialization, subprocess dispatch, CEF Views window/browser creation, callbacks, and shutdown. `browserkit-types` is reserved for genuinely cross-layer, CEF-independent types.

The dependency direction is:

```text
browserkit-example → browserkit → browserkit-runtime → browserkit-cef → cef-rs
                                      └──────────────→ browserkit-types
```

## Building and running

```bash
cargo check --workspace
cargo run -p browserkit-example
```

The first build downloads and extracts the CEF archive through `cef-rs`'s `cef-dll-sys` build script. No CEF binaries are committed. By default the archive is placed below Cargo's build `OUT_DIR`; set `CEF_PATH` to a shared directory to cache it across builds. `cef-rs` also supports `cargo run -p export-cef-dir -- --force $HOME/.local/share/cef` from its own checkout.

This workspace pins the maintained `tauri-apps/cef-rs` source at commit `40c85f4` (crate `152.1.0+152.0.6` packaging CEF `152.0.6`, Chromium `152.0.7977.83`). The build also copies the Linux runtime files next to the executable as required by current cef-rs.

On Linux, the tested windowed CEF path selected direct Wayland (`XDG_SESSION_TYPE=wayland`, `WAYLAND_DISPLAY=wayland-0`). XWayland was not required. No WebKitGTK or Wayland workaround is used. The CEF Views window owns the event loop, handles normal input and resizing, and closes through CEF's browser lifecycle callbacks.

CEF's normal Linux sandbox and multi-process mode are enabled (`no_sandbox=0`), matching the upstream `cefsimple` sample. The CEF process type is detected from the launched executable's `--type` argument before `execute_process`; zygote, renderer, GPU, and utility children return immediately after CEF handles them and never enter BrowserKit initialization. The application entry point should also keep `app.run()` free of user-created worker threads or other long-running work before CEF's subprocess dispatch.

The Linux runtime needs the normal CEF desktop dependencies supplied by the host distribution, including GTK 3, GLib, X11/Wayland client libraries, Fontconfig, NSS, and GPU/EGL/Vulkan libraries. The downloaded CEF archive supplies `libcef.so`, `icudtl.dat`, locales, and `.pak` resources; `cef-rs` copies the required runtime files beside the executable.

## Linux compatibility diagnostics

Defaults leave CEF/Chromium acceleration and Ozone selection unchanged. The following opt-in diagnostics are parsed before CEF initialization and are also applied by the CEF command-line callback for child processes:

```text
BROWSERKIT_DISABLE_GPU=1       --disable-gpu --disable-gpu-compositing --disable-vulkan
BROWSERKIT_DISABLE_VULKAN=1    --disable-features=Vulkan
BROWSERKIT_NO_SANDBOX=1        --no-sandbox (temporary unsafe diagnostic only)
BROWSERKIT_OZONE_PLATFORM=wayland  --ozone-platform=wayland
BROWSERKIT_OZONE_PLATFORM=x11      --ozone-platform=x11
```

The Vulkan-only switch is intentionally distinct from full GPU disable. Invalid Ozone values are warned about and ignored. `BROWSERKIT_NO_SANDBOX` is never enabled by default and must not be used for production browser sessions. No missing `libcef.so` dependencies or relevant SELinux AVC denials were found during the Fedora diagnostics; the host may still reject the sandboxed GBM path depending on its graphics stack.

## Scope

M0-E contains one CEF Views window with a dedicated Alloy chrome BrowserView and multiple Alloy page BrowserViews. The chrome is loaded from the CEF-owned `browserkit://app/index.html` scheme and talks to the typed protocol with CEF's maintained `MessageRouter` (`cefQuery`). Page views are never given that bridge. The first page is active automatically; later pages are created and kept alive but hidden until `set_active_page` selects them. Closing the active page selects the page to its right, otherwise the page to its left, otherwise no page. Closing the final page leaves the window open. Alloy uses Chromium's content layer without Chrome browser UI. React, Vite, tabs UI, OSR, custom compositing, and frontend packages are not part of this milestone.

The temporary chrome validates handshake, page creation, activation, close, navigation, back/forward, reload, and stop through the same `ProtocolRouter` used by native callers. Enable the diagnostic software-rendering switches only when investigating a host GPU issue:

```bash
BROWSERKIT_DISABLE_GPU=1 cargo run -p browserkit-example
```

```rust
use browserkit::{BrowserKit, PageOptions};

fn main() -> browserkit::Result<()> {
    let app = BrowserKit::builder()
        .title("BrowserKit")
        .size(1200, 800)
        .build()?;
    let window = app.create_window()?;
    let first = window.create_page(PageOptions::new("https://example.com"))?;
    let _second = window.create_page(PageOptions::new("https://example.org"))?;
    window.set_active_page(first.id())?;
    app.run()
}
```
