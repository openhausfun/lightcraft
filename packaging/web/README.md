# Hosting LightCraft for the web

> **Experimental.** The browser version keeps its library (catalog and imported photos) in the
> browser's own storage for the site. Browsers may clear that storage (site data cleared,
> private windows, storage pressure when persistent storage isn't granted). Tell your users to
> keep their original photos elsewhere and to use **File ▸ Back Up Library…**. See
> `docs/web.md` in the source for details.

`lightcraft-web-<version>.zip` (from the GitHub release, or `packaging/web/package.sh`) holds a
static site in `lightcraft-web-<version>/`:

| File | What it is |
|---|---|
| `index.html` | The page. It loads everything through relative URLs. |
| `lightcraft_web.js` | wasm-bindgen glue (generated, ES module) |
| `lightcraft_web_bg.wasm` | The app, about 13 MB (about 3 MB with brotli) |
| `worker.js` | Starts the render workers (each runs the same module) |
| `*.gz`, `*.br` | Precompressed copies of the files above (optional to serve) |
| `_headers`, `.htaccess` | Sample header rules for Netlify/Cloudflare Pages and Apache |

There is no server-side code. Upload the folder's contents anywhere that serves static files.

## Any path works

All URLs in `index.html` are relative, so the site works at a domain root
(`https://example.com/`), under a prefix (`https://example.com/tools/lightcraft/`) and from a
CDN bucket.

## Required server settings

- **MIME type:** serve `.wasm` as `application/wasm`. Browsers refuse to stream-compile it under
  any other type, and the app then loads slowly or not at all. Serve `.js` as `text/javascript`.
  Most hosts already do both. For nginx, check that `mime.types` has `application/wasm wasm;`.
- **Compression:** turn on gzip or Brotli for `.wasm`, `.js` and `.html`, or serve the
  precompressed `.br` / `.gz` files with `Content-Encoding` (nginx `brotli_static` /
  `gzip_static`). That takes the download from about 13 MB to about 3–5 MB.
- **Caching:** the file names don't change between versions, so **don't** mark them `immutable`:
  a browser would keep the old module after an upgrade, or mix old glue with a new module. Send
  `Cache-Control: no-cache` (revalidate with ETag / Last-Modified) on `index.html`, `.js` and
  `.wasm`, as the sample `_headers` and `.htaccess` do.
- **HTTPS:** browser storage for the library (OPFS), module workers and the clipboard need a
  secure context: `https://`, or `http://localhost` for testing. The app can't be opened from
  `file://`.
- **Isolation headers are optional.** The current build doesn't use `SharedArrayBuffer`, so
  `Cross-Origin-Opener-Policy` / `Cross-Origin-Embedder-Policy` aren't needed (a future
  wasm-threads build will need them; `cargo xtask web --serve` already sends them). If your site
  sends COEP `require-corp`, also send `Cross-Origin-Resource-Policy: same-origin` (or
  `cross-origin` when the files live on a CDN) on the app's files.

nginx example:

```nginx
location /lightcraft/ {
    types { application/wasm wasm; text/javascript js; text/html html; }
    gzip_static on;          # serves the .gz copies
    # brotli_static on;      # with ngx_brotli: serves the .br copies
    add_header Cache-Control "no-cache";
}
```

Local test: `python3 -m http.server 8765` inside the folder, then open http://localhost:8765/.

## Where users' data lives

Everything is stored in the browser, per site origin (scheme + host + port): the catalog and
preferences under `library/`, imported photos under `originals/`, thumbnails under `thumbs/`, in
the Origin Private File System, or IndexedDB where OPFS can't be written. Nothing is uploaded to
your server. Moving the app to another origin starts with an empty library there: users carry
their library over with **File ▸ Back Up Library…** and **File ▸ Restore Library from Backup…**.
One tab at a time can have the library open; a second tab shows a message instead.

## Embedding in a page (iframe)

```html
<iframe
  src="https://example.com/lightcraft/"
  title="LightCraft image editor"
  style="width: 100%; height: 720px; border: 0;"
  allow="fullscreen; clipboard-read; clipboard-write"
  allowfullscreen>
</iframe>
```

- The app fills the iframe and follows its size, so size the iframe and not the app.
- Keyboard shortcuts go to the iframe after the user clicks into it, as with any embedded app.
- **Cross-origin embeds** work, but browsers that partition or block third-party storage may
  give the embedded app separate or temporary storage: its library then differs from the one
  on the app's own site, or isn't kept between visits.
- **Sandboxed iframes** need at least
  `sandbox="allow-scripts allow-same-origin allow-downloads allow-popups allow-modals"`. Without
  `allow-same-origin` there's no storage; without `allow-downloads`, Export and Back Up Library
  (browser downloads) are blocked; without `allow-modals`, confirmations (restore, `?reset`)
  can't be shown and are treated as "no".
- Don't send `X-Frame-Options: DENY` or a `frame-ancestors` CSP that excludes the embedding page.

## Renderer and URL options

LightCraft draws its UI with WebGL2 (eframe's `glow` backend) and renders photos on the CPU in
Web Workers. A browser without WebGL2 gets a message in place of the app. URL options, also on
an iframe `src`:

| Option | Effect |
|---|---|
| `?workers=N` | Number of render workers (default: cores − 1, at most 4); `?workers=0` renders on the main thread |
| `?store=idb` | Keep the library in IndexedDB instead of OPFS |
| `?store=memory` | Keep nothing (a throwaway session) |
| `?reset` | Delete the library stored in this browser (asks first) |
