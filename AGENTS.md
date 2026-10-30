# Reader implementation

- Keep the main GUI native Rust. The user approved a dedicated HTML/CSS/SVG book engine provided measured startup and page-turn performance stays fast; compose its surface inside the native GUI without a web UI shell. Dictionary definitions may use a WebView.
- Use a source locator shared by selection, search, bookmarks, hit-testing, and speech. Do not infer speech timing from character-count ratios.
- Japanese vertical text uses natural character advance (1 em by default), vertical forms, close ruby, and separate line spacing. Never rotate whole horizontal paragraphs to simulate vertical text.
- Default accent is blue, configurable in appearance settings. Support light, dark, and sepia.
- The bottom reading toolbar is icon-only except the voice name and speed. Controls need tooltips and keyboard access.
- Reading starts with only the book visible. Menus/toolbars overlay the fixed book surface and never reflow it. Center taps toggle chrome even with a drawer open. Do not call this a focus mode, and do not bind F11 by default.
- Library and reader windows are separate; opening another book preserves existing reader windows.
- Page click zones, arrow keys, and spread order follow the book's progression and writing direction. Each wheel notch is queued and painted as a separate page; never discard rapid repeated input. All actions expose configurable key bindings.
- Calibre, imported books, and dictionaries are read-only sources. Persist application state and caches in the app data directory.
- Deliver working behavior; clearly distinguish unavailable features from complete ones. Do not fabricate timestamps or claim untested platforms work.
- Agents own their assigned crate paths. Do not modify another agent's files without coordinating.
