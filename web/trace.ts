// Trace the content silhouette of the art (SVG or raster) into simplified polygons in the art's
// viewBox coordinates -- the input to auto-outline generation. Runs entirely in the browser via a
// canvas, so no image decoder is needed on the Rust side.
//
// Masking picks one of two strategies per image:
//  - alpha: when the image has a band of fully-transparent pixels around its edge, content = the
//    non-transparent pixels (every island, not just the biggest).
//  - background colour: otherwise (opaque / no transparent border), find the dominant edge colour
//    and content = the pixels that differ from it.

export interface Traced {
  contours: number[][]; // each a flat x,y list in viewBox units
  vb: [number, number, number, number]; // minx, miny, w, h
}

export interface ArtFile { bytes: Uint8Array; ext: string; url: string; }

const MAX_DIM = 800; // cap tracing raster resolution for speed
const ALPHA = 24; // opacity threshold -- low, so faint/wispy edges (hair) count as content
const BG_TOL2 = 60 * 60; // squared RGB distance from background that counts as content

function readViewBox(svg: string): [number, number, number, number] | null {
  const m = svg.match(/viewBox\s*=\s*["']([^"']+)["']/i);
  if (!m) return null;
  const n = m[1].split(/[\s,]+/).map(Number).filter((v) => !isNaN(v));
  return n.length === 4 ? [n[0], n[1], n[2], n[3]] : null;
}

async function loadImage(url: string): Promise<HTMLImageElement> {
  const img = new Image();
  img.src = url;
  // decode() (not onload) guarantees the pixels are ready before we drawImage; onload can fire
  // early on large images, sampling a partially-decoded bitmap and giving non-deterministic masks.
  await img.decode();
  return img;
}

async function rasterize(art: ArtFile): Promise<{ px: Uint8ClampedArray; w: number; h: number; vb: [number, number, number, number]; scale: number }> {
  const img = await loadImage(art.url);
  let vb: [number, number, number, number];
  if (art.ext === 'svg') {
    vb = readViewBox(new TextDecoder().decode(art.bytes)) ?? [0, 0, img.naturalWidth || 100, img.naturalHeight || 100];
  } else {
    vb = [0, 0, img.naturalWidth, img.naturalHeight];
  }
  const s = Math.min(1, MAX_DIM / Math.max(vb[2], vb[3]));
  const w = Math.max(1, Math.round(vb[2] * s));
  const h = Math.max(1, Math.round(vb[3] * s));
  const canvas = document.createElement('canvas');
  canvas.width = w;
  canvas.height = h;
  const ctx = canvas.getContext('2d', { willReadFrequently: true });
  if (!ctx) throw new Error('no 2d canvas context');
  ctx.drawImage(img, 0, 0, w, h);
  return { px: ctx.getImageData(0, 0, w, h).data, w, h, vb, scale: vb[2] / w };
}

// True when a majority of the edge pixels are fully transparent -- the subject sits on a
// transparent background, so alpha is the reliable content mask.
function hasTransparentBorder(px: Uint8ClampedArray, w: number, h: number): boolean {
  let clear = 0, total = 0;
  const check = (x: number, y: number) => { total++; if (px[(y * w + x) * 4 + 3] < 16) clear++; };
  for (let x = 0; x < w; x++) { check(x, 0); check(x, h - 1); }
  for (let y = 1; y < h - 1; y++) { check(0, y); check(w - 1, y); }
  return total > 0 && clear / total > 0.5;
}

// Dominant edge colour (modal quantised bucket, refined to the mean of its members).
function backgroundColour(px: Uint8ClampedArray, w: number, h: number): [number, number, number] {
  const bucket = new Map<number, { n: number; r: number; g: number; b: number }>();
  const add = (x: number, y: number) => {
    const i = (y * w + x) * 4;
    const key = ((px[i] >> 4) << 8) | ((px[i + 1] >> 4) << 4) | (px[i + 2] >> 4);
    const e = bucket.get(key) ?? { n: 0, r: 0, g: 0, b: 0 };
    e.n++; e.r += px[i]; e.g += px[i + 1]; e.b += px[i + 2];
    bucket.set(key, e);
  };
  for (let x = 0; x < w; x++) { add(x, 0); add(x, h - 1); }
  for (let y = 1; y < h - 1; y++) { add(0, y); add(w - 1, y); }
  let best = { n: 0, r: 255 * 1, g: 255 * 1, b: 255 * 1 };
  for (const e of bucket.values()) if (e.n > best.n) best = e;
  return [best.r / best.n, best.g / best.n, best.b / best.n];
}

function contentMask(px: Uint8ClampedArray, w: number, h: number): Uint8Array {
  const mask = new Uint8Array(w * h);
  if (hasTransparentBorder(px, w, h)) {
    for (let i = 0; i < w * h; i++) mask[i] = px[i * 4 + 3] >= ALPHA ? 1 : 0;
    return mask;
  }
  const [br, bg, bb] = backgroundColour(px, w, h);
  for (let i = 0; i < w * h; i++) {
    const j = i * 4;
    // transparent pixels are background regardless of colour
    if (px[j + 3] < ALPHA) continue;
    const dr = px[j] - br, dg = px[j + 1] - bg, db = px[j + 2] - bb;
    if (dr * dr + dg * dg + db * db > BG_TOL2) mask[i] = 1;
  }
  return mask;
}

// Label 4-connected components, returning per-component pixel counts.
function label(mask: Uint8Array, w: number, h: number): { labels: Int32Array; sizes: number[] } {
  const labels = new Int32Array(w * h);
  const sizes: number[] = [0]; // index 0 = background
  const stack: number[] = [];
  for (let i = 0; i < w * h; i++) {
    if (!mask[i] || labels[i]) continue;
    const id = sizes.length;
    let size = 0;
    labels[i] = id;
    stack.push(i);
    while (stack.length) {
      const p = stack.pop() as number;
      size++;
      const x = p % w, y = (p / w) | 0;
      if (x > 0 && mask[p - 1] && !labels[p - 1]) { labels[p - 1] = id; stack.push(p - 1); }
      if (x < w - 1 && mask[p + 1] && !labels[p + 1]) { labels[p + 1] = id; stack.push(p + 1); }
      if (y > 0 && mask[p - w] && !labels[p - w]) { labels[p - w] = id; stack.push(p - w); }
      if (y < h - 1 && mask[p + w] && !labels[p + w]) { labels[p + w] = id; stack.push(p + w); }
    }
    sizes.push(size);
  }
  return { labels, sizes };
}

// Fill interior holes: flood the background inward from the border; any background not reached is
// enclosed by content (e.g. white clothing/highlights inside the subject) so make it content.
function fillHoles(mask: Uint8Array, w: number, h: number): void {
  const outside = new Uint8Array(w * h);
  const stack: number[] = [];
  const push = (i: number) => { if (!mask[i] && !outside[i]) { outside[i] = 1; stack.push(i); } };
  for (let x = 0; x < w; x++) { push(x); push((h - 1) * w + x); }
  for (let y = 0; y < h; y++) { push(y * w); push(y * w + w - 1); }
  while (stack.length) {
    const p = stack.pop() as number;
    const x = p % w, y = (p / w) | 0;
    if (x > 0) push(p - 1);
    if (x < w - 1) push(p + 1);
    if (y > 0) push(p - w);
    if (y < h - 1) push(p + w);
  }
  for (let i = 0; i < w * h; i++) if (!mask[i] && !outside[i]) mask[i] = 1;
}

// Chamfer distance transform: distance from each pixel to the nearest pixel where mask==src.
function distField(mask: Uint8Array, w: number, h: number, src: number): Float32Array {
  const INF = 1e9, D2 = Math.SQRT2;
  const d = new Float32Array(w * h);
  for (let i = 0; i < w * h; i++) d[i] = mask[i] === src ? 0 : INF;
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) {
    const i = y * w + x; let v = d[i];
    if (x > 0) v = Math.min(v, d[i - 1] + 1);
    if (y > 0) v = Math.min(v, d[i - w] + 1);
    if (x > 0 && y > 0) v = Math.min(v, d[i - w - 1] + D2);
    if (x < w - 1 && y > 0) v = Math.min(v, d[i - w + 1] + D2);
    d[i] = v;
  }
  for (let y = h - 1; y >= 0; y--) for (let x = w - 1; x >= 0; x--) {
    const i = y * w + x; let v = d[i];
    if (x < w - 1) v = Math.min(v, d[i + 1] + 1);
    if (y < h - 1) v = Math.min(v, d[i + w] + 1);
    if (x < w - 1 && y < h - 1) v = Math.min(v, d[i + w + 1] + D2);
    if (x > 0 && y < h - 1) v = Math.min(v, d[i + w - 1] + D2);
    d[i] = v;
  }
  return d;
}

// Morphological closing (dilate by r, erode by r): fills concave dents narrower than 2r with
// smooth outward arcs. Only ever grows (unioned with the original), so the outline never comes in.
// Runs on a background-padded buffer so out-of-canvas counts as background: without the pad, the
// dilation grows toward each canvas edge and the erosion (whose distance-to-background is unbounded
// past the edge) can't eat it back, leaving cardinal-direction spikes wherever the shape sits near
// an edge.
function close(mask: Uint8Array, w: number, h: number, r: number): Uint8Array {
  if (r < 1) return mask;
  const p = Math.ceil(r) + 2;
  const pw = w + 2 * p, ph = h + 2 * p;
  const pad = new Uint8Array(pw * ph);
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) if (mask[y * w + x]) pad[(y + p) * pw + (x + p)] = 1;
  const dOn = distField(pad, pw, ph, 1);
  const dil = new Uint8Array(pw * ph);
  for (let i = 0; i < pw * ph; i++) dil[i] = dOn[i] <= r ? 1 : 0;
  const dOff = distField(dil, pw, ph, 0);
  const out = new Uint8Array(w * h);
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) {
    const i = y * w + x;
    out[i] = mask[i] || dOff[(y + p) * pw + (x + p)] >= r ? 1 : 0;
  }
  return out;
}

// Clockwise Moore-neighbour boundary trace of the component with the given label.
function mooreTrace(labels: Int32Array, w: number, h: number, id: number, start: number): number[][] {
  const fg = (x: number, y: number) => x >= 0 && x < w && y >= 0 && y < h && labels[y * w + x] === id;
  const sx = start % w, sy = (start / w) | 0;
  const cw = [[0, -1], [1, -1], [1, 0], [1, 1], [0, 1], [-1, 1], [-1, 0], [-1, -1]];
  const contour: number[][] = [[sx, sy]];
  let px = sx, py = sy, bx = sx - 1, by = sy;
  const guard = w * h * 4;
  for (let it = 0; it < guard; it++) {
    let s = cw.findIndex((d) => px + d[0] === bx && py + d[1] === by);
    if (s < 0) s = 0;
    let moved = false;
    for (let k = 1; k <= 8; k++) {
      const i = (s + k) % 8;
      const cx = px + cw[i][0], cy = py + cw[i][1];
      if (fg(cx, cy)) {
        const prev = (i + 7) % 8;
        bx = px + cw[prev][0]; by = py + cw[prev][1];
        px = cx; py = cy;
        moved = true;
        break;
      }
    }
    if (!moved) break;
    if (px === sx && py === sy) break;
    contour.push([px, py]);
  }
  return contour;
}

// Douglas-Peucker on an open polyline (endpoints always kept).
function dp(pts: number[][], eps: number): number[][] {
  if (pts.length < 3) return pts;
  const keep = new Uint8Array(pts.length);
  keep[0] = 1; keep[pts.length - 1] = 1;
  const stack: [number, number][] = [[0, pts.length - 1]];
  while (stack.length) {
    const [a, b] = stack.pop() as [number, number];
    const ax = pts[a][0], ay = pts[a][1];
    const dx = pts[b][0] - ax, dy = pts[b][1] - ay;
    const len = Math.hypot(dx, dy) || 1;
    let far = -1, fd = eps;
    for (let i = a + 1; i < b; i++) {
      const d = Math.abs((pts[i][0] - ax) * dy - (pts[i][1] - ay) * dx) / len;
      if (d > fd) { fd = d; far = i; }
    }
    if (far >= 0) { keep[far] = 1; stack.push([a, far], [far, b]); }
  }
  return pts.filter((_, i) => keep[i]);
}

// Closed-ring simplification: split at two extreme anchors so the trace's start seam isn't left
// as a spurious near-duplicate vertex pair (which the fillet would otherwise bump).
function simplify(pts: number[][], eps: number): number[][] {
  const n = pts.length;
  if (n < 4) return pts;
  let far = 0, fd = -1;
  for (let i = 1; i < n; i++) {
    const d = (pts[i][0] - pts[0][0]) ** 2 + (pts[i][1] - pts[0][1]) ** 2;
    if (d > fd) { fd = d; far = i; }
  }
  const a = dp(pts.slice(0, far + 1), eps);
  const b = dp(pts.slice(far).concat([pts[0]]), eps);
  return a.slice(0, -1).concat(b.slice(0, -1));
}

export async function traceArt(art: ArtFile, simplifyRadius = 0): Promise<Traced> {
  const { px, w, h, vb, scale } = await rasterize(art);
  let mask = contentMask(px, w, h);
  fillHoles(mask, w, h); // solid interior, so bright/white regions inside the subject don't punch holes
  mask = close(mask, w, h, simplifyRadius); // Simplification: fill dents smoothly, grow-only
  const { labels, sizes } = label(mask, w, h);
  const firstPixel = new Int32Array(sizes.length).fill(-1);
  for (let i = 0; i < w * h; i++) { const l = labels[i]; if (l && firstPixel[l] < 0) firstPixel[l] = i; }

  const minSize = Math.max(4, Math.round(w * h * 1e-5)); // keep small elements, drop stray noise
  const kept = Array.from({ length: sizes.length - 1 }, (_, k) => k + 1)
    .filter((id) => sizes[id] >= minSize)
    .sort((a, b) => sizes[b] - sizes[a])
    .slice(0, 400);
  if (!kept.length) throw new Error('no content found in the art');

  // Trace every kept component (subject, text, watermark, particles) -- nothing is dropped. The
  // offset stage dilates and unions them, so adjacent pieces (e.g. neighbouring letters) fuse while
  // separate elements each keep their own outline.
  const contours: number[][] = [];
  for (const id of kept) {
    const ring = simplify(mooreTrace(labels, w, h, id, firstPixel[id]), 1.5);
    if (ring.length < 3) continue;
    const flat: number[] = [];
    for (const [x, y] of ring) flat.push(vb[0] + x * scale, vb[1] + y * scale);
    contours.push(flat);
  }
  if (!contours.length) throw new Error('no content found in the art');
  return { contours, vb };
}
