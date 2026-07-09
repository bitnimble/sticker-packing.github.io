# Sticker Packer

Tessellate as many rotated copies of a sticker as possible onto a page, then export two
**registered** sheets for a Print-then-Cut workflow (Cricut / Silhouette):

- **Content sheet**, the artwork, tessellated and clipped to the sticker shape (print this).
- **Outline sheet**, the border cut-lines at the same positions (feed this to the cutter).

Packing is driven by a **border** SVG; the **art** (SVG or raster) is rendered separately and
clipped to the border, so the two can be different shapes / off-centre. Output is SVG + PDF
(native vector, sRGB).

The same engine runs as a native CLI and as a fully client-side **WebAssembly web app** (no
backend, good for static hosting like GitHub Pages or S3).

## Web app

Everything runs locally in the browser via WASM. Build and serve:

```sh
wasm-pack build --target web --release --no-default-features --features pdf
cp pkg/sticker_packer.js pkg/sticker_packer_bg.wasm web/
python3 -m http.server -d web 8000   # then open http://localhost:8000
```

It's single-threaded (no `SharedArrayBuffer`), so no COOP/COEP headers are required, plain
static hosting works.

### Deploy to GitHub Pages

A workflow (`.github/workflows/pages.yml`) builds the wasm and deploys on every push to
`main`. One-time setup: in the repo, **Settings → Pages → Build and deployment → Source →
"GitHub Actions"**. After that, pushes publish automatically to
`https://<user>.github.io/<repo>/`.

The generated `web/sticker_packer*.{js,wasm}` are git-ignored. CI rebuilds them.

## Native CLI

```sh
cargo build --release
./target/release/pack_stickers \
  --border border.svg --image art.svg \
  --sticker-width 46 --rotations 180 --spacing 1 --margin 2 \
  --method both --page a4 --out sheet
# -> sheet_content.svg/.pdf and sheet_outline.svg/.pdf
```

Omit `--image` for single-SVG mode (the border is the art). Key flags:

| flag | meaning |
|------|---------|
| `--border` / `--image` | packing outline / artwork (SVG or png/jpg/bmp/gif/webp) |
| `--sticker-width` | scale each sticker to N mm (default: use SVG units) |
| `--page` | `a3\|a4\|a5\|a6\|letter\|legal\|tabloid` (or `--page-width`/`--page-height` in mm) |
| `--landscape` | swap page dimensions |
| `--margin` / `--spacing` | page bleed / gap between stickers (mm) |
| `--method` | `both` (default) · `greedy` · `lattice` |
| `--rotations` | orientations to try |
| `--greedy-attempts` | multi-start count (default 16) |
| `--max-count` | cap sticker count |
| `--stroke` | cut-line width on the outline sheet (mm) |
| `--svg-only` | skip PDF output |

The border and art SVGs **must share the same `viewBox`** so the art aligns to the border
(the tool errors otherwise).

## How it works

- **Greedy**: multi-start No-Fit-Polygon bottom-fill (8 directions + forced first-piece
  orientations), keeps the best, robust against the non-monotonicity of a single greedy pass.
- **Lattice**: densest single + double (antiparallel) lattice packing.
- Minkowski sums via triangulation; buffering via Minkowski-with-a-disk; parallelised with
  `rayon` natively (single-threaded on wasm).

## Layout

```
src/geom.rs      geometry core (Minkowski, buffer, collision body, affine)
src/greedy.rs    multi-start NFP fill
src/lattice.rs   single + double lattice
src/svgio.rs     SVG parse / viewBox handling
src/output.rs    content + outline sheet SVG, SVG->PDF
src/engine.rs    filesystem-free pipeline (shared by CLI + wasm)
src/wasm_api.rs  wasm-bindgen entry
src/main.rs      native CLI
web/index.html   web app (inline CSS/JS)
```
