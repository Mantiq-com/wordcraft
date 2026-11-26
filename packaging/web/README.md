# Hosting WordCraft for the web

`wordcraft-web-<version>.zip` (from the GitHub release, or `packaging/web/package.sh`) holds a
static site in `wordcraft-web-<version>/`:

| File | What it is |
|---|---|
| `index.html` | The page. It loads everything through relative URLs. |
| `wordcraft-web-<hash>.js` | wasm-bindgen glue (generated, ES module) |
| `wordcraft-web-<hash>_bg.wasm` | The app, about 13 MB, or 5 MB with compression |
| `_headers`, `.htaccess` | Sample header rules for Netlify/Cloudflare Pages and Apache |

There is no server-side code. Upload the folder's contents anywhere that serves static files.

## Any path works

All URLs in `index.html` are relative (`public_url = "./"` in `apps/wordcraft-web/Trunk.toml`),
so the site works at a domain root (`https://example.com/`), under a prefix
(`https://example.com/tools/wordcraft/`) and from a CDN bucket. The asset names carry a content
hash, so they can be cached forever. Only `index.html` needs revalidation.

## Required server settings

- **MIME type:** serve `.wasm` as `application/wasm`. Browsers refuse to stream-compile it under
  any other type, and the app then loads slowly or not at all. Serve `.js` as `text/javascript`.
  Most hosts already do both. For nginx, check that `mime.types` has `application/wasm wasm;`.
- **Compression:** turn on gzip or Brotli for `.wasm`, `.js` and `.html`. That takes the
  download from about 13 MB to about 5 MB. You can also precompress (`brotli -k *.wasm`) and let
  the server send `Content-Encoding: br`.
- **Caching:** `Cache-Control: public, max-age=31536000, immutable` on the hashed `.wasm` and
  `.js` files, and `no-cache` on `index.html`.
- **HTTPS:** WebGPU (and the clipboard) only work in a secure context, which means `https://`
  or `http://localhost`. Over plain HTTP elsewhere, the app falls back to WebGL2.
- **No special isolation headers:** WordCraft doesn't use `SharedArrayBuffer`, so it doesn't
  need `Cross-Origin-Opener-Policy` or `Cross-Origin-Embedder-Policy`. If your site already sends
  COEP `require-corp`, also send `Cross-Origin-Resource-Policy: same-origin` (or `cross-origin`
  when the files live on a CDN) on the app's files.

nginx example:

```nginx
location /wordcraft/ {
    types { application/wasm wasm; text/javascript js; text/html html; }
    gzip on;
    gzip_types application/wasm text/javascript text/html;
    location ~* \.(wasm|js)$ { add_header Cache-Control "public, max-age=31536000, immutable"; }
    location ~* index\.html$ { add_header Cache-Control "no-cache"; }
}
```

Local test: `python3 -m http.server 8765` inside the folder, then open http://localhost:8765/.

## Embedding in a page (iframe)

```html
<iframe
  src="https://example.com/wordcraft/"
  title="WordCraft word processor"
  style="width: 100%; height: 720px; border: 0;"
  allow="fullscreen; clipboard-read; clipboard-write"
  allowfullscreen>
</iframe>
```

- The app fills the iframe and follows its size, so size the iframe and not the app.
- Keyboard shortcuts go to the iframe after the user clicks into it, as with any embedded app.
- **Cross-origin embeds** work. Preferences are kept in the iframe's `localStorage`. Browsers
  that partition or block third-party storage may forget them between visits, and the app
  then starts with defaults.
- **Sandboxed iframes** need at least
  `sandbox="allow-scripts allow-same-origin allow-downloads allow-popups"`. Without
  `allow-same-origin` there's no storage. Without `allow-downloads`, Save and Export (browser
  downloads) are blocked.
- Don't send `X-Frame-Options: DENY` or a `frame-ancestors` CSP that excludes the embedding page.

## The parent bridge (`?host=parent`)

A page framing WordCraft in a **same-origin** iframe can add `?host=parent` to the iframe's URL
(`https://example.com/wordcraft/?host=parent`) and have WordCraft hand its files over instead of
downloading them. The two sides talk `postMessage` with plain window messages that carry a `type`:

| Message | Direction | Fields |
|---|---|---|
| `wordcraft:ready` | app → page | `version`, `appVersion`: the app is running and listening; send a file now |
| `wordcraft:open` | page → app | `name`, `bytes` (an `ArrayBuffer` or `Uint8Array`): open like File › Open |
| `wordcraft:save` | app → page | `name`, `saveAs`, `bytes` (a transferred `ArrayBuffer`): every file the app would otherwise download (Save, Save As, Export as PDF, Export Page as PNG). The page stores it and reports failures itself. `saveAs` says whether the file becomes the document's file: `false` after Save (the page's own file, written under its name) and for exports that are only copies (PDF, Markdown, plain text); `true` after Save As and for exports to a format that keeps everything (.docx, .odt, .rtf), after which the next Save carries that name with `false` |
| `wordcraft:dirty` | app → page | `dirty`: unsaved changes appeared or went away |
| `wordcraft:failed` | app → page | `error`: the app couldn't start (no WebGPU or WebGL2, for example) |

In host mode the app opens no sample document, so the framing page should send the file to edit
and guard leaving the page while changes are unsaved. `&author=` on the iframe's URL names the
user for the comments and tracked changes the app creates. Opening `?host=parent` outside an
iframe gives the normal app with downloads.

## Same-document host mode (no iframe)

A host application can also run WordCraft from its own document: it loads the wasm-bindgen module
itself and gives WordCraft one of its own canvases, so there is no iframe and no `?host=parent`.
It opts in with `data-host="page"` on that canvas — the exact value `page` — and the canvas keeps
the id the app looks up (`id="wordcraft_canvas"`).

```html
<canvas id="wordcraft_canvas" data-host="page" style="width: 100%; height: 720px; display: block;"></canvas>
```

The page and the app then talk with DOM `CustomEvent`s on the canvas instead of `postMessage`.
The app dispatches the same messages it would post to a parent — `wordcraft:ready`,
`wordcraft:save`, `wordcraft:dirty`, `wordcraft:failed` — with the message fields (without the
`type`) as the event's `detail`, and it opens a document when the page dispatches a
`wordcraft:open` event with `detail: { name, bytes }`. Events do not bubble, so listen on the
canvas itself (two editors on one page can't hear each other).

Add the listeners before `init`: the app announces itself with `wordcraft:ready` while `init` is
still running, and a file sent in reply would otherwise be missed.

```js
import init from "./wordcraft-web-<hash>.js"; // the wasm-bindgen glue shipped with the app

const canvas = document.getElementById("wordcraft_canvas");

canvas.addEventListener("wordcraft:ready", async () => {
  // The app is listening; hand it a document now, if you have one.
  const bytes = await documentBytes(); // an ArrayBuffer or Uint8Array
  canvas.dispatchEvent(new CustomEvent("wordcraft:open", {
    detail: { name: "Report.docx", bytes },
  }));
});

// Every file WordCraft would otherwise download (Save, Save As, Export as PDF) arrives here.
canvas.addEventListener("wordcraft:save", (event) => {
  const { name, saveAs, bytes } = event.detail; // bytes is an ArrayBuffer
  // Store the file under `name`; `saveAs` says whether it becomes the document's file.
  // Report your own errors.
  storeFile(name, bytes, saveAs);
});
canvas.addEventListener("wordcraft:dirty", (event) => {
  unsaved = event.detail.dirty; // warn before leaving, as you like
});
canvas.addEventListener("wordcraft:failed", (event) => {
  showError(event.detail.error); // the app couldn't start
});

await init({ module_or_path: "./wordcraft-web-<hash>_bg.wasm" });
```

- Saves are not downloads in this mode: the `bytes` in `wordcraft:save` hold the saved document,
  and the host decides where the file goes.
- `?author=` on the page's URL still names the user for new comments and tracked changes.
- An unframed page that uses `?host=parent` keeps the normal app with downloads.
  `data-host="page"` turns this mode on only when `?host=parent` is absent: if `?host=parent` is
  present it takes precedence, so a same-origin framed page uses the parent bridge, while an
  unframed or cross-origin page gets no bridge and keeps normal downloads.

The messages and both transports are the shared contract of [`craft-web-host`](https://github.com/storytold/craft-libs/tree/main/crates/web-host) (storytold/craft-libs#420), the same for every Craft app: its README lists every message, including `saved`, with the compatibility notes. In `wordcraft:ready`, `version` is the contract's (`"1"`) and `appVersion` is this app's release. WordCraft reads bytes, so a host sends `bytes`, not a `File`.

### Trust and limits

- **Who is trusted.** In page host mode the page and WordCraft share one document, so any script on the page can drive the app: open files in it and receive what it saves. Use this mode only for a page you control. The iframe mode (`?host=parent`) answers only the parent window, on the page's own origin.
- **Files from the host are untrusted input.** An opened file goes through the same reading as one picked with File ▸ Open or dropped on the window, with the same format limits; before it is copied into the app's memory, its size is checked against the host open limit (512 MiB) and a larger file is refused, and a name is cut to 255 characters.
- **The event and message names are the interface.** `wordcraft:ready`, `wordcraft:open`, `wordcraft:save`, `wordcraft:dirty` and `wordcraft:failed`, and the fields they carry (the same as the `?host=parent` messages), stay stable; changing one is a breaking change.

### Driving it from the page (`window.wordcraft`)

Every WordCraft web build puts `window.wordcraft` on the page, from [`craft-web-api`](https://github.com/storytold/craft-libs/tree/main/crates/web-api) (storytold/craft-libs#413), the same shape in every Craft app. `wordcraft.request(method, params)` calls any method of the control channel the desktop app serves (`docs/control-protocol.md`), and `execute(command, params)`, `commands()`, `docInspect(params)`, `uiInspect(params)` and `screenshot(params)` are shortcuts for the common ones. Each returns a Promise that resolves with the method's result and rejects with an `Error`; a call the app does not answer within 30 seconds rejects. WordCraft's control requests cannot be withdrawn, so a call that times out in the page still runs when the app reaches it, as it does for a desktop client.

```js
const doc = await wordcraft.docInspect();                  // a summary of the open document
await wordcraft.execute("text.insert", { text: "Hello" });
const { data } = await wordcraft.screenshot();            // the canvas as PNG (base64)
```

The API is there in every mode, with or without a host. Opening a file, handing a saved or exported file to the page, the file name and the unsaved flag stay with the host messages above.

An embedding page gives the editor its own look with the `ui.hostTheme` command:

```js
await wordcraft.execute("ui.hostTheme", { light: { accent: "#885f1a" }, radius: 8, share: false });
```

Its fields are `{"light"?: {token: "#rrggbb"}, "dark"?: {..}, "radius"?: 0-16, "appearance"?: "light|dark|system", "community"?: bool, "share"?: bool, "hideTabs"?: [tab], "font"?: base64, "fontMedium"?: base64, "fontSemibold"?: base64}`. The tokens map onto the interface palette in camel case; an accent alone also tints the selection, checked buttons and the text on it. `community` and `share` say whether the community button and the Share button show, `hideTabs` names ribbon tabs to hide (`"Zotero"` is accepted as a legacy alias for the renamed `"Citations"` tab), and the fonts are the regular, medium and semibold interface faces as TTF, OTF, WOFF or WOFF2 files. The look lasts for the visit and is never saved with the preferences; `{}` returns to WordCraft's own.

## Fork source and remaining extensions

This fork incorporates upstream `main` at
[`62391d8aa3884f0a04da5285e5f7f244c35fa1a0`](https://github.com/storytold/wordcraft/commit/62391d8aa3884f0a04da5285e5f7f244c35fa1a0),
retrieved on 2026-11-26 (`2026-11-26T18:56:09Z`). The latest stable release at retrieval is
[WordCraft v0.19.1](https://github.com/storytold/wordcraft/releases/tag/v0.19.1) at
`5d658dc033f3139926839bffd6ae0eef558c210c` (cut from pre-UI-port `09faec42ec`); this fork
continues tracking upstream `main` (post-UI-port, workspace version `0.18.1`) and retains
three extensions for embedded editing:

| Extension | Why it remains | Requirement for removal |
|---|---|---|
| Per-command `author` on `engine.execute` | The upstream [control handler](https://github.com/storytold/wordcraft/blob/62391d8aa3884f0a04da5285e5f7f244c35fa1a0/crates/ui-egui/src/control/handler.rs) executes with the session's author. `file.setAuthor` changes that shared setting. | An upstream command must sign its comments and tracked changes with the requested author, then restore the session's author and initials on success or error, including commands addressed to another document. |
| `data` and `originalData` on `review.compare` and `review.combine` | The upstream [comparison loader](https://github.com/storytold/wordcraft/blob/62391d8aa3884f0a04da5285e5f7f244c35fa1a0/crates/engine/src/cmd/tools/compare.rs) opens document paths. Browser hosts hold document bytes without a filesystem. | Both documents must be accepted as bytes with format-identifying names, preserving comparison options and tracked changes through save and reopen. |
| `ui.hostTheme` | The upstream [UI commands](https://github.com/storytold/wordcraft/blob/62391d8aa3884f0a04da5285e5f7f244c35fa1a0/crates/ui-egui/src/app/commands.rs) select WordCraft's appearance but do not accept a host palette, corner radius, hidden tabs/buttons or interface fonts. | Upstream must provide the host colours, fonts, radius, appearance, button visibility and hidden ribbon tabs described above, without saving them to preferences; resetting the theme must restore WordCraft's defaults. |

Retiring an extension requires the upstream web bundle to pass its embedding checks, including
open, edit, save and reopen, as well as the behaviour in the table. A source comparison or a
successful build alone does not establish that compatibility. Before releasing a fork update,
run the repository's full `cargo xtask ci` gate and qualify the resulting web bundle.

### Native validation on the selected source

With Rust `1.102.0-nightly (6c709eb42 2026-11-25)`, nextest `0.9.152`, `CRAFT_I18N_DIR` on
`craft-i18n` commit `e56d26244bbac099bfd42e23fb12f78a4be418a7` (`craft-i18n.rev`) and
`CRAFT_FONTS_DIR` on `craft-fonts` commit `baf0f2ccd1745893aa6337a35141ec12a23d4fc1`:

- `cargo fmt --all -- --check`, `cargo clippy --workspace --lib --bins -- -D warnings`,
  `cargo xtask assets`, `cargo xtask layers`, `cargo xtask wasm`, `cargo xtask language`,
  `cargo test --workspace --doc`, and `cargo xtask i18n-report` (1/1) all pass.
- Under nightly `1.102.0-nightly`, `cargo clippy --workspace --all-targets -- -D warnings`
  reports 39 `clippy::unnecessary_to_owned` errors solely in unchanged upstream
  `wordcraft-engine` test code; zero errors occur in fork-modified files.
- All targeted fork tests (`theme::host`, `request_author`, `compare` and `wordcraft-mcp`)
  pass.
- Running the full workspace nextest suite with `--no-fail-fast` completes 4,966 tests:
  4,960 passed, 6 failed, 6 skipped:
  - `wordcraft-engine` `tests::page_down_and_up_move_by_a_page_in_the_continuous_views`
  - `wordcraft-engine` `tests::page_down_and_up_never_stay_on_a_line_in_the_continuous_views`
  - Four `wordcraft-ui-egui` `egui_kittest::wgpu` tests
    (`objects::tests::a_dragged_shape_leaves_the_text_behind_it`,
    `objects::tests::a_level_line_dragged_by_its_middle_moves_and_stays_level`,
    `objects::tests::rotation_drag_turns_picture_pixels_live_and_commits_once_or_cancels`,
    `theme::legacy::tests::dark_interface_text_is_only_a_little_heavier_than_light_interface_text`),
    which fail with `No adapter found` on a headless host without a GPU adapter.
- An independent run of all six failing tests on the unmodified selected upstream commit
  `62391d8aa3884f0a04da5285e5f7f244c35fa1a0` reproduces all six failures identically
  (6 run, 0 passed, 6 failed). All assertions remain unchanged.

### Browser validation on the selected source

`packaging/web/package.sh` builds the browser app from source merge
`58031efa09e33ea49dfa3b272dcd1d535dacaa19` with Rust `1.102.0-nightly (6c709eb42 2026-11-25)`,
Trunk `0.22.2` and `wasm-bindgen` `0.2.129` (removing `.trunk-lock` before zipping). Its local
archive, `wordcraft-web-0.18.1.zip` (38 MB), has SHA-256
`5395cd61c508ebddf690ec337dbd2587f4f4c3529157d6566252cc8b3dce5e70` and serves
`wordcraft-web-feec29f759815884_bg.wasm` (SHA-256
`62e3ea7e84b5ca2daf347a5201cb8d156255927171992c11cdde46fc7bbd8f2a`).

Hosted headless Chrome (`158.0.8127.36`, Linux) checks through the real parent-page bridge
under a CSP blocking external connections passed DOCX creation, editing, undo/redo, Save As
(`6,945` bytes, `saveAs: true`), ordinary Save (`8,969` bytes, `saveAs: false`), and
reopening. Saved DOCX XML retains tracked insertions by `Revision Assistant` and comments by
`Review Assistant` and the session author `Local Reviewer` (confirming a failing authored
command restores the session author). `review.compare` and `review.combine` accept both
inputs as base64 bytes (`originalData` and `data`) and preserve tracked insertions/deletions
attributed to `Compare Author` through save (`7,049` bytes) and reopen. Reviewed and compared
documents round-trip in both directions with the published `host-theme-4` bundle.

`ui.hostTheme` apply (dark appearance, accent `#885f1a`, 8 pt corner radius, hidden Discord
and Share buttons, hidden `Citations` and `Developer` tabs via `hideTabs: ["Zotero", "Developer"]`,
and host TTF interface font) was visually verified in a screenshot, and `{}` reset completed
cleanly. PDF export returns `%PDF-1.7` (`10,042` bytes), and `review.restrict` (`readOnly`)
refuses edits. Origin/source spoofs are ignored, disposal clears the iframe `src` and rejects
later requests, and all runtime assets load from the host origin with zero console errors or
page exceptions. Protected-range per-command author behavior was not re-tested on this build;
assume the pre-existing requirement for a trusted command caller still applies. These synthetic
checks do not qualify authenticated application storage, autosave acknowledgements or
permissions.

### Previous validation on the 2026-11-21 main revision

The release package built from `f8ec9e59a1a850bc3dbd661450f863164741aab3` on upstream
`8a047c333b239e5c5d5c9d91e2dae2cc84185439` (`wordcraft-web-0.18.1.zip`, SHA-256
`ffb76e715b24e60b250aef6f3401e7cd67a4e558ba93b782313f4cc177d02da8`, published as
`host-theme-4`). Those results do not qualify the `62391d8a` source above.

### Previous validation on the 2026-11-17 main revision

The following results apply to upstream `211f62231d32502dce1c2015f603436d75099fbf`
and do not qualify the newer source above.

`cargo xtask ci` passes formatting and Clippy with the pinned font and
translation inputs. With nextest 0.9.150, the native stage stops after 1,986
passes, two failures and nine skipped tests. Another 3,085 tests and the later
gate stages do not run. A repeat gate reaches 1,987 passes with the same two
failures and nine skipped tests; 3,084 tests remain unrun.

Both failures are continuous-view Page Up/Page Down tests in
`crates/engine/src/tests.rs`. At line 13362, Draft view moves to paragraph 24
where the test expects paragraph 25, the position reached in Print Layout.
At line 13397, the third Page Up remains at offset 1 in the first paragraph
where the test requires the caret to move backward. The unmodified selected
upstream target reproduces both failures with the same toolchain and pinned
inputs: its `cargo xtask test` run reports 1,985 passes, two failures and nine
skipped tests before stopping. The assertions remain unchanged.

`packaging/web/package.sh` builds the browser app from source merge
`50ebd3874272e4eb8b571d8f3c56389019784a3c`. Its local archive,
`wordcraft-web-0.18.1.zip`, has SHA-256
`6e74d27d256188531e902251b521957a89fe8e2f1b3b96b55ba7c8f2891ff105`.
The tested page serves `wordcraft-web-dc187636931f4a47_bg.wasm`, and its
About dialog reports `0.18.1 (50ebd387)`.

Hosted DOCX creation, editing, undo/redo, ordinary Save, Save As and reopening
pass. Ordinary Save preserves the filename and clears the unsaved flag.
Comments and tracked insertions retain their requested authors in the saved
XML, and an unsuccessful command restores the session author. Compare and
Combine accept both inputs as document bytes. Reviewed and compared documents
retain their text, comments and tracked changes through a round trip from this
build to the published `host-theme-3` bundle and back.

Host theme apply/reset, PDF export and read-only command rejection pass.
Messages from another origin or source are ignored, and disposal unloads the
iframe and rejects later requests. Observed runtime assets come from the host
origin under a content policy that blocks external connections. These checks
use synthetic documents; authenticated storage, autosave acknowledgements and
application permissions remain unqualified. The earlier protected-range author
issue below has not been requalified on this revision.

The browser build reports two existing warnings: unused imports in
`crates/ui-egui/src/dialogs.rs:38` and the unused `system_arabic` parameter in
`crates/ui-egui/src/theme.rs:885`. Neither file changed from the previous fork
revision. No warning or test was suppressed.

### Previous validation on the 2026-11-15 main revision

The following results apply to upstream `bc0e51ee4db1f23940ac30fc626da02c09541adc`
and do not qualify the newer source above.

The native `cargo xtask ci` run on fork commit
[`2e2dae6c688dc63fa6ea474c3756f0724b2e6128`](https://github.com/Mantiq-com/wordcraft/commit/2e2dae6c688dc63fa6ea474c3756f0724b2e6128)
passed formatting and Clippy, then stopped at six `fit_one_page` test failures:
1,808 tests passed and nine were skipped. Nextest stopped before running another
3,169 tests. The later gate stages did not run. All six failures also occur on
the unmodified previous upstream commit with the same toolchain and pinned inputs.
Its `cargo xtask test` run reached five of them before stopping; a separate run
of `fit_one_page_bounds_fine_step_loop` reproduced the sixth. The assertions
remain unchanged.

The release web build passes hosted DOCX creation, editing, Save As and reopening.
Comments and tracked insertions retain their requested authors in the saved XML;
an unsuccessful command restores the session author. Comparison accepts both DOCX
inputs as bytes and preserves the changes after saving and reopening. Host theme
apply/reset and PDF export pass. The published `host-theme-3` bundle opens these
reviewed and compared documents, and a document it saves reopens in this build.
Messages from another origin or source are ignored, and disposing the bridge
unloads the frame. These synthetic checks do not qualify authenticated storage
or permissions in a consuming application.

The per-command author also affects document protection: an insertion refused
for the session author succeeds when the request names the author assigned to a
protected range. This occurs in both this build and `host-theme-3` and remains
unresolved. The command API therefore requires a trusted caller; authorship
metadata must not be treated as independent of document-protection checks.

Historical results from the earlier v0.18.1-based fork do not qualify this source.
That native gate passed formatting and Clippy with Rust 1.99.1 and nextest 0.9.150,
using the release's pinned font and language inputs. Two navigation tests failed
in both that fork and the unmodified upstream release:
`page_down_and_up_move_by_a_page_in_the_continuous_views` reaches paragraph 24 in
Draft instead of paragraph 25 in Print Layout, and
`page_down_and_up_never_stay_on_a_line_in_the_continuous_views` stops moving on its
third Page Up. Those assertions remained unchanged, and that gate stopped at those
failures. The current source still needs a complete gate and application qualification.

## Renderer selection and fallback flags

WordCraft renders with wgpu. It uses **WebGPU** when the browser has it and falls back to
**WebGL2** on its own. URL query flags override this, and they work on the iframe `src` too:

| Flag | Effect |
|---|---|
| *(none)* | WebGPU if available, otherwise WebGL2 |
| `?webgl` | Force the WebGL2 backend (useful when a WebGPU driver misbehaves) |

For example: `<iframe src="https://example.com/wordcraft/?webgl" ...>`.

A browser with neither WebGPU nor WebGL2 gets a message in place of the app.
