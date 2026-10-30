# Dictionary popup

The main reader remains native. Only dictionary definitions use the isolated `reader-dictionary-popup` WebView process. Keep that executable beside `voice-over-book-reader`.

- Enabled dictionaries appear in their configured order, with an index on the right. Click a name to scroll to its definitions; the active name follows the visible section. Dictionaries with no match or a pending search stay in the index with their status.
- Names and pending sections appear immediately. A background worker searches dictionaries in order using existing MDX disk indexes. It adds each result when ready. Clicking a pending dictionary prioritizes it after the current search. Changing the word cancels the old queue. Building a dictionary's first index can take longer. Input and existing definition resources stay available.
- Definitions share one outer vertical scroll. Only nearby articles create WebView frames and request CSS, images, and scripts. With more than 24 live frames, distant frames can be released. Their measured height is retained. Returning to an article reloads it and its scripts. This bounds off-screen WebView work without removing dictionary images. Very large authored widgets may retain their own intentional scroll area.
- Completed searches patch the existing sections instead of reloading the popup. The visible article and scroll anchor are preserved when results above arrive. The local protocol resolves dictionary CSS, JavaScript, images, entry redirects, and MDD audio. The reader theme colors the chrome; it does not replace a dictionary's authored definition colors.
- The query is editable; Enter searches it. Entry links prioritize the originating dictionary in the new query while searching the others too. Use Up/Down or Home/End in the dictionary index to move between sections. F6 moves between the index and definitions; Alt+Up/Down moves to the previous/next dictionary. Section jumps respect the OS reduced-motion preference.
- The pin button keeps the popup open while reading or changing pages. Close and Esc explicitly dismiss it, including while pinned. Unpinned outside focus is debounced to ignore the WebView's transient child-focus changes.
- Drag the header grip to move the window. All eight edges/corners resize it; the bottom-right grip makes that action visible. User size and position are retained in `dictionary-popup.json` inside the chosen app-data directory.
- Initial placement and moved/resized bounds are clamped to the monitor's work area. Windows uses `rcWork`, excluding the taskbar. Other compositors may impose their own placement policy. The popup does not resize itself on every lookup.
- Windows native decoration/shadow and DWM border color are disabled. The HTML surface fills the client area; there is no transparent outer pad or arrow.

Missing titles, HTML-marked titles, and placeholders fall back to the MDX filename stem. One such placeholder is `Title (No HTML code allowed)`. Existing catalogs receive the same normalization when opened. MDX/MDD source files are never modified.

## Parent protocol

The helper accepts one JSON object per stdin line:

```json
{"type":"lookup","query":"猫","x":100,"y":200,"theme":"sepia","accent":"2563eb","paper":"F5EFDF","ink":"463E31"}
{"type":"hide"}
{"type":"force_hide"}
{"type":"set_pinned","pinned":true}
{"type":"close"}
```

`hide` is a reading-surface dismissal and respects the pin. `force_hide` is an explicit close/Esc and overrides it. `close` terminates the helper. Coordinates are physical desktop pixels. Fields omitted from a reused lookup retain their existing appearance/anchor. `paper` and `ink` are optional six-digit hex colors without `#`; only valid values affect the chrome. The launch CLI accepts the same colors as `--paper` and `--ink`. A user-positioned or pinned popup retains its position.

Stdout emits newline-delimited JSON. The parent should use these events to synchronize visibility and pin state. A hide request alone does not confirm visibility:

```json
{"event":"visibility","visible":true,"pinned":false}
{"event":"pinned","visible":true,"pinned":true}
{"event":"results","query":"猫","dictionary_id":"…","count":1,"errors":[]}
```

The existing `ready` and `error` events remain available.

## Local QA corpus

```powershell
cargo run -p reader-dictionary-popup --example fixture -- APP_DATA_DIR
```

This writes three small, original dictionaries under the supplied directory:

- Japanese definitions with MDD CSS, SVG, JavaScript, and audio.
- A matching example dictionary.
- An English-only dictionary with no match for `猫`.

Use a separate scratch app-data folder. Do not point this fixture generator at a user's dictionary directory.

## Browser regressions

Rust tests cover section metadata, queue cancellation and priorities, and safe query serialization. They also cover custom colors, resources, and window geometry.

An optional Chromium test checks narrow layouts and 120 dictionaries. It also checks lazy frames, scrollspy, keyboard navigation, and incremental updates. Further cases cover a 32,000 px definition, forwarded wheel scrolling, and frame messages:

```powershell
cargo run -p reader-dictionary-popup --example render_fixture | Out-File -Encoding utf8 popup-fixture.html
python crates/reader-dictionary-popup/tests/popup_browser.py --html popup-fixture.html
```

This requires Python Playwright and Chromium. Pass `--channel msedge` to use an installed Microsoft Edge instead. The test serves original fixtures for local dictionary URLs. It makes no dictionary network requests. Browser tests do not replace native popup drag/resize/pin QA. Only Windows native behavior has been exercised in this workspace.
