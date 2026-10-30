# Dictionary popup helper

The reader and bookshelf remain native Rust. This process uses the OS WebView only for MDX definitions. This includes dictionary-authored CSS and JavaScript. It cannot serve book files through its resource protocol.

Start beside the reader executable:

```text
reader-dictionary-popup --data-dir APP_DATA_DIR --query WORD --x 400 --y 250 --theme light --accent 2563eb
```

Coordinates are physical screen coordinates. Windows also accepts `--parent-hwnd HWND` to own the borderless popup to the reader window. Linux positioning depends on the compositor; Wayland may ignore requested positions.

Keep stdin open and send UTF-8 JSON, one object per line, to reuse the process:

```json
{"type":"lookup","query":"猫","x":400,"y":250,"theme":"sepia","accent":"2563eb"}
{"type":"hide"}
{"type":"close"}
```

The optional appearance keys preserve their previous values when omitted. Escape and the close button hide the popup, including while pinned. Loss of foreground activation hides an unpinned popup. `close` or stdin EOF exits the helper. Stdout emits `ready`, `results`, and `error` JSON events. `--dismiss-on-blur false` keeps the window open for interaction testing. The production default is `true`.

A background worker searches enabled dictionaries in order. Results form consecutive sections, with a dictionary index on the right. Results appear as soon as each dictionary finishes; definition frames load near the viewport. See [dictionary behavior and browser regressions](../../docs/dictionaries.md).

MDX/MDD sources are opened read-only. Keys and derived indexes are cached under `APP_DATA_DIR/dictionaries`. Resources load lazily from MDD volumes; the whole archive is never unpacked. The current parser supports MDict v1/v2. Unsupported versions and encodings remain unavailable, with diagnostics. Passcode-protected files also need configured credentials.

An original tiny corpus for local tests can be generated with:

```text
cargo run -p reader-dictionary-popup --example fixture -- QA_APP_DATA_DIR
```

Windows uses the installed WebView2 Runtime; macOS uses WKWebView. Linux builds require the native GTK/WebKitGTK development dependencies described by Wry. Only Windows runtime behavior has been exercised in this workspace.
