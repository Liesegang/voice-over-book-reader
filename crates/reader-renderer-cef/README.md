# Chromium book surface — alpha integration

Only the book document uses Chromium. Library, settings, menus, dictionaries, speech controls and window chrome remain native Rust. This crate is standalone, with `[workspace]` in its manifest. Remove that section when adding it to the application workspace. Stable 0.2.1 has no dependency on this crate.

## Versions and ownership

- Rust 1.97.1 MSVC minimum; the probe used `1.97.1-x86_64-pc-windows-msvc`.
- CEF Rust bindings `154.5.0+154.0.34`, Chromium `154.0.8037.98`.
- wgpu `30.0.1`, matching eframe/egui-wgpu 0.37. Windows DX12 was measured.
- Vendored welding 0.15.0 from commit `7f04a38bb77696a8ca584565529f7c81fd8fddd4` of <https://github.com/merely-made/wgpu-weld>. `vendor/welding/LICENSE` is MPL-2.0. The local change registers `vobr` in every subprocess via `WeldApp::on_register_custom_schemes`. Flags are STANDARD, SECURE, CORS_ENABLED, and FETCH_ENABLED. CSP bypass, local-file privilege and sandbox-disable switches are not enabled. Manifest workspace references were made explicit and grafting pinned to reviewed 0.6.0. A second Windows-only change uses CEF's global RequestContext when profile=None. This enables globally registered native factories. profile=Some retains upstream's independent RequestContext. Register factories in that context too.
- Wrapper/protocol/navigator sources are this project's MIT code.
- This crate contains the renderer wrapper. App bootstrap, resource handling, and workspace integration belong to the host application.

## Sandboxed Windows entry and lifetime

Use CEF's matching `bootstrap.exe` and a client DLL exporting `RunWinMain`. The supplied `instance` and `sandbox_info` must remain unchanged. Call `SandboxBootstrap::dispatch_subprocess` before creating any GUI. Return child exit codes immediately. The browser process may keep `SandboxBootstrap`. Call `initialize(cache_root)` after the native shell is visible. Initialization returns `Runtime<'a>` and is performed only once. Keep it across books.

`Runtime` shares one CEF instance through `Rc`. Never clone welding's underlying `CefRuntime`. Its derived Clone and unconditional Drop would each call shutdown. The wrapper clones only the Rc and shuts CEF down once. A `Runtime<'static>` in the FFI entry requires its unsafe lifetime contract. Drop all surfaces and runtime references before `RunWinMain` returns. Close each surface and pump until `is_closed()` before dropping the final runtime.

```rust,ignore
let bootstrap = unsafe {
    SandboxBootstrap::<'static>::from_entrypoint(instance, sandbox_info, cef_dir)?
};
if let Some(code) = bootstrap.dispatch_subprocess()? { return code; }
let runtime = bootstrap.initialize(app_cache_root)?;
// Register resources here, then create native app and GPU surface.
```

For packaging, keep stable binaries untouched. Build a separate alpha client `cdylib` depending on the native app `rlib` plus this crate. Copy CEF bootstrap to `voice-over-book-reader-alpha.exe`. Copy the client DLL to the matching name `voice-over-book-reader-alpha.dll`. Keep matching CEF libraries, locale, and pak resources beside them. The bootstrap/client DLL basename must match. For a separate bundler, enable the CEF crate's `build-util` feature. It provides `cef::build_util::win::bundle`. The runtime API does not require that build-only feature. Set `CEF_PATH` to the pinned SDK directory while building. Validation used `C:\path\to\llvm\bin` for `LIBCLANG_PATH`.

## Native book resource factory

The application provides lazy ZIP resources without expanding the whole book. Use `vobr://<unique-book-token>/<raw-zip-path>`; host tokens isolate book origins. Serve XHTML as `application/xhtml+xml` and SVG as `image/svg+xml`. Use proper CSS and font MIME types too. Preserve original markup and relative paths. Register the handler **after** Runtime.initialize, **before** Surface.new:

```rust,ignore
use reader_renderer_cef::cef;
use cef::*; // required by generated wrap_* implementations

cef::wrap_scheme_handler_factory! {
    struct BookFactory { /* Arc book registry */ }
    impl SchemeHandlerFactory {
        fn create(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>,
            scheme_name: Option<&CefString>, request: Option<&mut Request>)
            -> Option<ResourceHandler> {
            // Resolve request.url() against registered book token/path.
            // Return an owned ResourceHandler for immutable resource bytes.
            todo!()
        }
    }
}
let scheme: CefString = "vobr".into();
let domain: CefString = token.into();
let mut factory = BookFactory::new();
let ok = cef::register_scheme_handler_factory(
    Some(&scheme), Some(&domain), Some(&mut factory));
assert_eq!(ok, 1);
```

CEF154 handler method signatures:

```rust,ignore
fn open(&self, request: Option<&mut Request>, handle_request: Option<&mut i32>,
    callback: Option<&mut Callback>) -> i32;
fn response_headers(&self, response: Option<&mut Response>,
    response_length: Option<&mut i64>, redirect_url: Option<&mut CefString>);
fn read(&self, data_out: *mut u8, bytes_to_read: i32,
    bytes_read: Option<&mut i32>, callback: Option<&mut ResourceReadCallback>) -> i32;
fn cancel(&self);
```

For synchronous immutable bytes, `open` sets `handle_request = 1` and returns 1.

Headers set status, MIME type, and length. `read` copies at most bytes_to_read and advances a synchronized cursor. It writes bytes_read, returning 1 while data remains and 0 at EOF. The host resource handler owns path decoding, byte lifetime, and font deobfuscation. It also owns status codes and origin isolation. This crate does not access ZIPs.

## GPU integration

Use the existing eframe wgpu device:

```text
GpuBookSurface::new(&runtime, state.device.clone(), state.queue.clone(), options)
``` Set `SurfaceOptions.physical_size` to the full native reading rectangle. Set `scale_factor` to pixels-per-point. Native menu visibility must not change that rectangle. `poll()` imports native GPU frames and returns true on a new one. Register or update `frame().view` with these methods:

```text
egui_wgpu::Renderer::{register_native_texture, update_egui_texture_from_wgpu_texture}
``` Keep the surface/frame alive while used. Do not convert this texture to a CPU ColorImage. CEF uses GPU copies/shared handles; diagnostic CPU readback is confined to the performance probe.

CEF emits Bgra8Unorm gamma-encoded pixels. The probe used a non-sRGB native target so they are not gamma-encoded twice. Honor the app renderer's texture color flags when registering the external view. A naive blit to an sRGB target makes text pale. Windows DX12 is the supported measured path. Other OS/backends have not been tested by this integration.

## Commands, locations and pagination

`SurfaceEvent::DocumentLoaded` means raw XHTML loaded. Call install_navigator, then `Command::Configure`. `NodeBinding=[]` snapshots the original DOM before native style insertion. documentElement is 0. Every ELEMENT and decoded TEXT counts in preorder, including head/style/script/rt/rp. Comments, PI and doctype do not count. Consecutive TEXT/CDATA/TEXT parts form one virtual source node, matching roxmltree. Comments and PI separate runs. DOM is not normalized or wrapped. Explicit NodeBinding path/text pairs can validate the native facade mapping.

Offsets in SourceLocator/SourceRange are UTF-8 bytes. The bridge converts UTF-8 offsets to/from UTF-16 DOM Range indices. This includes astral and combining characters. Ruby annotation nodes do not become page-start speech text. Range rectangles and CSS Highlights use original DOM geometry and never reflow the page.

Configure carries a host revision and viewport/typography options. After fonts and images settle, the bridge emits Pagination, then Position. Pagination gives the exact current-layout page count and source start for each page. Unmapped pages have a null start, never an invented locator. Page commands address confirmed indices, not chapter fractions; old revision commands are ignored. Changing geometry or resources requires a new host revision; discard stale events. Native global pagination combines these per-chapter counts. It is independent of the old native layout engine's revision and cache. Do not substitute chapter length or image count.

`Invalidated { revision, reason }` reports viewport changes and author DOM/CSS mutations. It also reports late font/image loading. It pauses Page commands until the host sends Configure with a new revision and receives Pagination. The host can then Locate its saved source position. Native transforms and highlights do not invalidate pagination. Resize does not silently jump to page zero.

The initial reflow adapter uses CSS columns on the original body. It retains author stylesheets, SVG, embedded fonts, ruby, relative resources, and scripts. Pagination controls the body's viewport box. Unusual scripted or floating layouts need book testing. Pre-paginated mode uses the author viewport and uniform scaling. It applies no user typography overrides. Source IDs are captured at navigator installation. If scripts already changed the DOM, validate native sources instead of guessing numbering.

The host queues wheel/key commands in FIFO order. Paint each requested page before dispatching the next command. A JS Position event alone does not prove GPU presentation.

Poll returns nonblocking events; the current alpha drains the async channel at 16 ms while serviced. Event-driven CEF repaint wakeups can reduce idle work later.

## Validation

`node navigator.test.mjs` checks UTF conversions and source-run numbering. It also checks CDATA, comment separators, and revision rejection. Rust protocol tests cover revisioned counts, byte offsets and malformed/oversize events. A real-GPU sandbox probe tested Japanese vertical text, ruby, and inline SVG. It also measured geometry under overlays, 30 confirmed page transitions, and warm chapter loads. Development receipts were recorded under `work/wgpu-weld-probe/receipts`. These measurements do not prove arbitrary EPUB layouts or final app bootstrap integration.

The Chromium navigator regression also passed on CEF154. It recorded 23 pages and 1,271 original source nodes. It verified page-start navigation and glyph-rect/hit-test source identity. Other checks covered CSS speech highlights and selection text preservation. Virtual Text/CDATA/Text node 10 mapped UTF-8 emoji bytes 3..7 correctly. Author mutation invalidation and new-revision repagination also passed. Repeat it with:

```text
tests/run-chromium-regression.ps1 -Bundle <probe release bundle> -Work <scratch directory>
```

Release probe measurements used Windows / RTX3090 / 1280×800. They measured GPU marker readback and Queue.present calls, excluding display-photon latency:

| Text | Confirmed pages | Page median / p95 | Warm document load |
| --- | ---: | ---: | ---: |
| 8,972 chars | 18 | 49.4 / 50.7 ms | 82–99 ms |
| 112,012 chars | 223 | 49.5 / 66.1 ms | 316–449 ms |

Each run painted all 30 forward/backward pages. The first five queued pages were 1,2,3,4,5, each confirmed separately on GPU.

The short fixture's five-page burst took 166 to 199 ms.

A fresh-profile process launched and opened the short book in 2.88 seconds. A later process with a warm profile took 1.15 seconds.

Sample counts are small, and OS file caches were not reset. Keeping CEF alive allows faster book/chapter changes. Native overlay geometry was identical before/after visibility changes. The staged release bundle excluding PDBs was about 445 MB before compression.
