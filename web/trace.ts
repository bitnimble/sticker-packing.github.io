// Trace the opaque silhouette of the art (SVG or raster) into a simplified polygon in the art's
// viewBox coordinates -- the input to auto-outline generation. Runs entirely in the browser via a
// canvas, so no image decoder is needed on the Rust side.

export interface Traced {
  points: number[]; // flattened x,y pairs, viewBox units
  vb: [number, number, number, number]; // minx, miny, w, h
}

export interface ArtFile { bytes: Uint8Array; ext: string; url: string; }

const MAX_DIM = 800; // cap tracing raster resolution for speed
const ALPHA = 128; // opacity threshold

function readViewBox(svg: string): [number, number, number, number] | null {
  const m = svg.match(/viewBox\s*=\s*["']([^"']+)["']/i);
  if (!m) return null;
  const n = m[1].split(/[\s,]+/).map(Number).filter((v) => !isNaN(v));
  return n.length === 4 ? [n[0], n[1], n[2], n[3]] : null;
}

async function loadImage(url: string): Promise<HTMLImageElement> {
  const img = new Image();
  await new Promise<void>((res, rej) => {
    img.onload = () => res();
    img.onerror = () => rej(new Error('could not load art image'));
    img.src = url;
  });
  return img;
}

// Rasterize the art to a canvas; return its alpha mask plus the viewBox and canvas->viewBox scale.
async function rasterize(art: ArtFile): Promise<{ mask: Uint8Array; w: number; h: number; vb: [number, number, number, number]; scale: number }> {
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
  const px = ctx.getImageData(0, 0, w, h).data;
  const mask = new Uint8Array(w * h);
  for (let i = 0; i < w * h; i++) mask[i] = px[i * 4 + 3] >= ALPHA ? 1 : 0;
  return { mask, w, h, vb, scale: vb[2] / w };
}

// Keep only the largest 4-connected opaque component (drops stray specks / anti-alias noise).
function largestComponent(mask: Uint8Array, w: number, h: number): Uint8Array {
  const label = new Int32Array(w * h).fill(0);
  const stack: number[] = [];
  let best = 0, bestId = 0, id = 0;
  for (let i = 0; i < w * h; i++) {
    if (!mask[i] || label[i]) continue;
    id++;
    let size = 0;
    stack.push(i);
    label[i] = id;
    while (stack.length) {
      const p = stack.pop()!;
      size++;
      const x = p % w, y = (p / w) | 0;
      const nb = [x > 0 ? p - 1 : -1, x < w - 1 ? p + 1 : -1, y > 0 ? p - w : -1, y < h - 1 ? p + w : -1];
      for (const q of nb) if (q >= 0 && mask[q] && !label[q]) { label[q] = id; stack.push(q); }
    }
    if (size > best) { best = size; bestId = id; }
  }
  const out = new Uint8Array(w * h);
  for (let i = 0; i < w * h; i++) out[i] = label[i] === bestId ? 1 : 0;
  return out;
}

// Clockwise Moore-neighbour boundary trace (y-down), returning the ordered outer contour.
function mooreTrace(mask: Uint8Array, w: number, h: number): number[][] {
  const fg = (x: number, y: number) => x >= 0 && x < w && y >= 0 && y < h && mask[y * w + x] === 1;
  let sx = -1, sy = -1;
  scan: for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) if (mask[y * w + x]) { sx = x; sy = y; break scan; }
  if (sx < 0) return [];
  // clockwise neighbourhood offsets starting at "up"
  const cw = [[0, -1], [1, -1], [1, 0], [1, 1], [0, 1], [-1, 1], [-1, 0], [-1, -1]];
  const contour: number[][] = [[sx, sy]];
  let px = sx, py = sy;
  let bx = sx - 1, by = sy; // we entered the start from its left
  const guard = w * h * 4;
  for (let it = 0; it < guard; it++) {
    let start = cw.findIndex((d) => px + d[0] === bx && py + d[1] === by);
    if (start < 0) start = 0;
    let moved = false;
    for (let k = 1; k <= 8; k++) {
      const i = (start + k) % 8;
      const cx = px + cw[i][0], cy = py + cw[i][1];
      if (fg(cx, cy)) {
        const prev = (i + 7) % 8; // last background cell checked becomes the new backtrack
        bx = px + cw[prev][0]; by = py + cw[prev][1];
        px = cx; py = cy;
        moved = true;
        break;
      }
    }
    if (!moved) break; // isolated pixel
    if (px === sx && py === sy) break;
    contour.push([px, py]);
  }
  return contour;
}

// Douglas-Peucker polyline simplification.
function simplify(pts: number[][], eps: number): number[][] {
  if (pts.length < 3) return pts;
  const keep = new Uint8Array(pts.length);
  keep[0] = 1; keep[pts.length - 1] = 1;
  const stack: [number, number][] = [[0, pts.length - 1]];
  while (stack.length) {
    const [a, b] = stack.pop()!;
    const [ax, ay] = pts[a], [bx, by] = pts[b];
    const dx = bx - ax, dy = by - ay;
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

export async function traceArt(art: ArtFile): Promise<Traced> {
  const { mask, w, h, vb, scale } = await rasterize(art);
  const largest = largestComponent(mask, w, h);
  let contour = mooreTrace(largest, w, h);
  if (contour.length < 3) throw new Error('no opaque shape found in the art');
  contour = simplify(contour, 1.5);
  const points: number[] = [];
  for (const [x, y] of contour) { points.push(vb[0] + x * scale, vb[1] + y * scale); }
  return { points, vb };
}
