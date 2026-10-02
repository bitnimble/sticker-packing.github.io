import init, { preview, auto_outline } from './sticker_packer.js';
import { traceBase, traceFinish, type Traced, type BaseRaster } from './trace.js';
import { PackerPool } from './pool.js';
import { errorMessage, type PackArgs } from './types.js';

const $ = <T extends HTMLElement = HTMLElement>(id: string): T => document.getElementById(id) as T;
const setStatus = (msg: string, cls = ''): void => {
  const s = $('status');
  s.textContent = msg;
  s.className = 'status ' + cls;
};

interface BorderFile { text: string; url: string; }
interface ImageFile { bytes: Uint8Array; ext: string; url: string | null; name: string; }

let border: BorderFile | null = null; // active border (manual upload or auto-generated)
let manualBorder: BorderFile | null = null;
let image: ImageFile = { bytes: new Uint8Array(0), ext: '', url: null, name: '' };
let traced: Traced | null = null; // silhouette of the current art, for auto-outline
let base: BaseRaster | null = null; // cached rasterize+mask for the current art (radius-independent)
let previewReady = false;
let packerReady = false;
let packing = false;

// --- packing workers ------------------------------------------------------
const POOL_MAX_WORKERS = 8;
// Guarded so a worker-construction failure can't stop the file inputs from wiring up.
let pool: PackerPool | null = null;
try {
  pool = new PackerPool(Math.max(1, Math.min(POOL_MAX_WORKERS, (navigator.hardwareConcurrency || 2) - 1)));
  pool.ready.then(
    () => { packerReady = true; maybeReady(); },
    (e: unknown) => setStatus('Failed to load engine: ' + errorMessage(e), 'err'),
  );
} catch (e: unknown) {
  setStatus('Failed to start worker: ' + errorMessage(e), 'err');
}

init()
  .then(() => { previewReady = true; maybeReady(); updatePreview(); })
  .catch((e: unknown) => setStatus('Failed to load engine: ' + errorMessage(e), 'err'));

function maybeReady(): void {
  if (previewReady && packerReady) { maybeEnable(); setStatus('Ready.', 'ok'); }
}
function maybeEnable(): void {
  $<HTMLButtonElement>('run').disabled = !(previewReady && packerReady && border) || packing;
}

// --- live preview (main thread) ------------------------------------------
// Rendered as inline SVG (not <img src>) so a raster's `<image>` can reference the art's
// already-decoded blob URL -- <img>-hosted SVG runs in secure static mode and blocks external
// refs, forcing a slow base64 embed + re-decode instead.
const ART_HREF = '__ART_HREF__';
// Overlay the outline as a visible stroke on the clipped art (preview only): a thin dark cut line
// over transparent art can be invisible, so let the user recolour/thicken it. non-scaling-stroke
// keeps the width constant in screen px regardless of the art's viewBox scale.
function withOutlineStroke(svg: string): string {
  const d = svg.match(/<clipPath[^>]*>\s*<path d="([^"]*)"/)?.[1];
  if (!d) return svg;
  const color = $<HTMLInputElement>('previewOutlineColor').value;
  const w = $<HTMLInputElement>('previewOutlineWidth').value;
  const path = `<path id="previewStroke" d="${d}" fill="none" stroke="${color}" stroke-width="${w}" stroke-linejoin="round" vector-effect="non-scaling-stroke"/>`;
  return svg.replace('</svg>', path + '</svg>');
}
function updateOutlineStroke(): void {
  const p = document.getElementById('previewStroke');
  if (!p) return;
  p.setAttribute('stroke', $<HTMLInputElement>('previewOutlineColor').value);
  p.setAttribute('stroke-width', $<HTMLInputElement>('previewOutlineWidth').value);
}
function updatePreview(): void {
  if (!previewReady || !border) { $('previewPanel').style.display = 'none'; return; }
  const err = $('previewErr');
  const box = $('previewImg');
  try {
    let svg = preview(border.text, image.bytes, image.ext);
    if (image.url) svg = svg.replace(ART_HREF, image.url);
    box.innerHTML = withOutlineStroke(svg);
    box.style.display = '';
    err.style.display = 'none';
  } catch (e: unknown) {
    box.innerHTML = '';
    box.style.display = 'none';
    err.textContent = errorMessage(e);
    err.style.display = 'block';
  }
  $('previewPanel').style.display = 'block';
}
$('previewOutlineColor').addEventListener('input', updateOutlineStroke);
$('previewOutlineWidth').addEventListener('input', updateOutlineStroke);

// --- file inputs: preview replaces the drop zone -------------------------
// The drop zone AND the filecard both accept drops, so a file can be swapped out by dropping a new
// one onto the card without clearing first (the drop zone is hidden while the card shows).
function wireDrop(dropId: string, inputId: string, cardId: string, onFile: (f: File) => void): void {
  const drop = $(dropId);
  const input = $<HTMLInputElement>(inputId);
  drop.addEventListener('click', () => input.click());
  for (const el of [drop, $(cardId)]) {
    el.addEventListener('dragover', (e) => { e.preventDefault(); el.classList.add('over'); });
    el.addEventListener('dragleave', () => el.classList.remove('over'));
    el.addEventListener('drop', (e: DragEvent) => {
      e.preventDefault();
      el.classList.remove('over');
      const f = e.dataTransfer?.files[0];
      if (f) onFile(f);
    });
  }
  input.addEventListener('change', () => { if (input.files?.[0]) onFile(input.files[0]); });
}
function showCard(kind: string, url: string, name: string): void {
  $(kind + 'Drop').style.display = 'none';
  $<HTMLImageElement>(kind + 'ThumbImg').src = url;
  $(kind + 'Name').textContent = name;
  $(kind + 'Card').style.display = 'flex';
}
function clearCard(kind: string): void {
  $(kind + 'Card').style.display = 'none';
  $(kind + 'Drop').style.display = 'block';
  $<HTMLInputElement>(kind + 'File').value = '';
  // Drop the (revoked) blob reference: leaving it on the img can leave it in an error state that a
  // later src assignment won't reliably reload from.
  $<HTMLImageElement>(kind + 'ThumbImg').removeAttribute('src');
}

wireDrop('borderDrop', 'borderFile', 'borderCard', async (file) => {
  const text = await file.text();
  const old = manualBorder?.url;
  manualBorder = { text, url: URL.createObjectURL(new Blob([text], { type: 'image/svg+xml' })) };
  showCard('border', manualBorder.url, file.name);
  if (!autoEnabled()) { border = manualBorder; maybeEnable(); updatePreview(); }
  if (old) URL.revokeObjectURL(old);
});
$('borderClear').addEventListener('click', () => {
  if (manualBorder?.url) URL.revokeObjectURL(manualBorder.url);
  manualBorder = null;
  clearCard('border');
  if (!autoEnabled()) { border = null; maybeEnable(); updatePreview(); }
});

wireDrop('imageDrop', 'imageFile', 'imageCard', async (file) => {
  const buf = new Uint8Array(await file.arrayBuffer());
  const ext = (file.name.split('.').pop() || '').toLowerCase();
  const old = image.url;
  const mime = ext === 'svg' ? 'image/svg+xml' : file.type || 'application/octet-stream';
  const url = URL.createObjectURL(new Blob([buf], { type: mime }));
  image = { bytes: buf, ext, url, name: file.name.replace(/\.[^.]*$/, '') };
  traced = null;
  base = null;
  previewCache.clear();
  showCard('image', url, file.name);
  await onArtChanged();
  if (old) URL.revokeObjectURL(old); // revoke only after the new art has rendered
});
$('imageClear').addEventListener('click', () => {
  if (image.url) URL.revokeObjectURL(image.url);
  image = { bytes: new Uint8Array(0), ext: '', url: null, name: '' };
  traced = null;
  base = null;
  previewCache.clear();
  clearCard('image');
  if (autoEnabled()) regenAuto();
  updatePreview();
});

// --- auto-outline (generate the border by dilating the art silhouette) ---
const autoEnabled = (): boolean => $<HTMLInputElement>('autoOutline').checked;
const currentStyle = (): string => (document.querySelector('input[name=autostyle]:checked') as HTMLInputElement | null)?.value ?? 'external';
const previewCache = new Map<string, string>();

function clearAutoBorder(): void {
  if (border && border !== manualBorder && border.url) URL.revokeObjectURL(border.url);
  border = null;
}
function genOutline(style: string): string {
  if (!traced) throw new Error('no traced art');
  const marginPct = num('autoMargin', 5);
  const roundness = num('autoRound', 0);
  const key = style + ':' + marginPct + ':' + roundness;
  const hit = previewCache.get(key);
  if (hit) return hit;
  // Roundness (0-100): extra convex-corner rounding, as a fraction of the shape size.
  const roundRadius = (roundness / 100) * 0.12 * Math.min(traced.vb[2], traced.vb[3]);
  const flat: number[] = [];
  const lengths: number[] = [];
  for (const c of traced.contours) { lengths.push(c.length / 2); for (const v of c) flat.push(v); }
  const stroke = Math.max(traced.vb[2], traced.vb[3]) / 150;
  const svg = auto_outline(new Float64Array(flat), new Uint32Array(lengths), ...traced.vb, marginPct, roundRadius, style, stroke);
  previewCache.set(key, svg);
  return svg;
}
// Simplification (0-100) as a 0..1 amount: outward-only, amplitude-ordered smoothing of the
// silhouette -- shallow wiggles smooth away first, prominent notches survive, outline only grows.
function simplifyAmount(): number {
  return num('autoSimplify', 0) / 100;
}
async function traceCurrentArt(): Promise<void> {
  traced = null;
  previewCache.clear();
  if (!image.url) { base = null; return; }
  try {
    // Rasterize+mask is cached per art; only the close/label/trace re-runs when Simplification moves.
    if (!base) base = await traceBase({ bytes: image.bytes, ext: image.ext, url: image.url });
    traced = traceFinish(base, simplifyAmount());
  } catch { traced = null; }
}
function regenAuto(): void {
  if (!autoEnabled()) return;
  const err = $('autoErr');
  clearAutoBorder();
  try {
    if (!traced) throw new Error(image.url ? 'could not trace the art silhouette' : 'add art first to auto-create the outline');
    const svg = genOutline(currentStyle());
    border = { text: svg, url: URL.createObjectURL(new Blob([svg], { type: 'image/svg+xml' })) };
    err.style.display = 'none';
  } catch (e) {
    err.textContent = errorMessage(e);
    err.style.display = 'block';
  }
  maybeEnable();
  updatePreview();
}
async function onArtChanged(): Promise<void> {
  if (autoEnabled()) { await traceCurrentArt(); regenAuto(); }
  updatePreview();
}

$('autoOutline').addEventListener('change', async () => {
  const on = autoEnabled();
  $('borderManual').style.display = on ? 'none' : 'block';
  $('autoOpts').style.display = on ? 'block' : 'none';
  if (on) {
    if (!traced && image.url) await traceCurrentArt();
    regenAuto();
  } else {
    clearAutoBorder();
    border = manualBorder;
    $('autoErr').style.display = 'none';
    maybeEnable();
    updatePreview();
  }
});
// Sliders fire `input` continuously while dragging; coalesce so the trace/WASM pipeline runs once
// the drag settles instead of per-tick.
function debounce(fn: () => unknown, ms: number): () => void {
  let t: ReturnType<typeof setTimeout> | undefined;
  return () => { clearTimeout(t); t = setTimeout(fn, ms); };
}
$('autoMargin').addEventListener('input', regenAuto);
function widthRange(): [number, number] {
  return [num('widthMin', 50), num('widthMax', 70)];
}
function syncWidth(moved: HTMLInputElement): void {
  const lo = $<HTMLInputElement>('widthMin');
  const hi = $<HTMLInputElement>('widthMax');
  if (+lo.value > +hi.value) (moved === lo ? hi : lo).value = moved.value;
  const [a, b] = widthRange();
  $('widthReadout').textContent = a === b ? `${a} mm` : `${a}–${b} mm`;
  const pos = (v: number): number => (v - +lo.min) / (+lo.max - +lo.min);
  // equal thumbs stack: put the one with more room to move on top
  lo.style.zIndex = a === b && pos(a) > 0.5 ? '1' : '';
  const fill = $('widthFill');
  fill.style.left = `calc(8px + (100% - 16px) * ${pos(a)})`;
  fill.style.width = `calc((100% - 16px) * ${pos(b) - pos(a)})`;
}
for (const id of ['widthMin', 'widthMax']) {
  const el = $<HTMLInputElement>(id);
  el.addEventListener('input', () => syncWidth(el));
}
syncWidth($<HTMLInputElement>('widthMax'));
$('autoRound').addEventListener('input', debounce(regenAuto, 150));
$('autoSimplify').addEventListener('input', debounce(async () => { await traceCurrentArt(); regenAuto(); }, 150));
document.querySelectorAll('input[name=autostyle]').forEach((r) => r.addEventListener('change', regenAuto));

const stylePrev = $('stylePreview');
$('styleOpts').querySelectorAll('label').forEach((lab) => {
  const val = (lab.querySelector('input') as HTMLInputElement).value;
  lab.addEventListener('mouseenter', () => {
    if (!traced || !image.url) { stylePrev.style.display = 'none'; return; }
    try {
      const vb = traced.vb;
      const art = `<image href="${image.url}" x="${vb[0]}" y="${vb[1]}" width="${vb[2]}" height="${vb[3]}" preserveAspectRatio="none"/>`;
      const outline = genOutline(val)
        .replace('<path', art + '<path')
        .replace(/stroke="#000000"/g, 'stroke="#4c9be8"')
        .replace(/stroke-width="[^"]*"/g, `stroke-width="${Math.max(vb[2], vb[3]) / 55}"`);
      stylePrev.innerHTML = outline;
      stylePrev.style.display = 'block';
    } catch { stylePrev.style.display = 'none'; }
  });
});
$('styleOpts').addEventListener('mouseleave', () => { stylePrev.style.display = 'none'; });

// --- options -------------------------------------------------------------
function num(id: string, fallback: number): number {
  const el = document.getElementById(id) as HTMLInputElement | null;
  if (!el) return fallback;
  const v = parseFloat(el.value);
  return isNaN(v) ? fallback : v;
}
$('pagesize').addEventListener('change', () => {
  $('customPage').style.display = $<HTMLSelectElement>('pagesize').value === 'custom' ? 'grid' : 'none';
});
$('regMarks').addEventListener('change', () => {
  const on = $<HTMLInputElement>('regMarks').checked;
  $('regOpts').style.display = on ? 'grid' : 'none';
  // Registration inset defines the cut border, superseding the page margin -- disable it.
  const marginEl = $<HTMLInputElement>('margin');
  marginEl.disabled = on;
  (marginEl.closest('label') as HTMLElement).style.opacity = on ? '0.5' : '';
});
function pageDims(): [number, number] {
  const v = $<HTMLSelectElement>('pagesize').value;
  if (v !== 'custom') { const [w, h] = v.split('x').map(Number); return [w, h]; }
  let w = num('pw', 210);
  let h = num('ph', 297);
  if ($<HTMLSelectElement>('pageunit').value === 'in') { w *= 25.4; h *= 25.4; }
  return [w, h];
}

function setLink(container: HTMLElement, filename: string, blob: Blob): void {
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  a.textContent = '↓ ' + filename;
  container.appendChild(a);
}

// --- run -----------------------------------------------------------------
// Drop the previous sheets (and free their blob URLs) so a repack doesn't leave stale output on
// screen while the new sheets render.
function clearResults(): void {
  $('results').classList.remove('show');
  for (const id of ['contentImg', 'outlineImg']) {
    const img = $<HTMLImageElement>(id);
    if (img.src.startsWith('blob:')) URL.revokeObjectURL(img.src);
    img.removeAttribute('src');
  }
  for (const a of document.querySelectorAll<HTMLAnchorElement>('#contentDl a, #outlineDl a')) {
    if (a.href.startsWith('blob:')) URL.revokeObjectURL(a.href);
  }
  $('contentDl').innerHTML = '';
  $('outlineDl').innerHTML = '';
  $('sweep').innerHTML = '';
}

function sweepSummary(sweep: Array<[number, number]>, best: number | undefined): string {
  const runs: Array<[number, number, number]> = [];
  for (const [w, count] of sweep) {
    const last = runs[runs.length - 1];
    if (last && last[2] === count) last[1] = w;
    else runs.push([w, w, count]);
  }
  return runs
    .map(([from, to, count]) => {
      const text = `${from === to ? from : `${from}–${to}`} mm: ${count}`;
      return best != null && from <= best && best <= to ? `<b>${text}</b>` : text;
    })
    .join(' · ');
}

$('run').addEventListener('click', async () => {
  if (!border || packing) return;
  packing = true;
  maybeEnable();
  clearResults();
  setStatus('');
  $('progress').style.display = 'block';
  $('bar').style.width = '0%';
  $('progText').textContent = 'Starting…';
  const t0 = performance.now();
  try {
    const wantPdf = $<HTMLInputElement>('pdf').checked;
    const [pageW, pageH] = pageDims();
    const regInset = num('regInset', 0.4);
    const [widthMin, widthMax] = widthRange();
    const args: PackArgs = {
      border: border.text,
      imageBytes: image.bytes,
      imageExt: image.ext,
      widthMin,
      widthMax,
      pageW,
      pageH,
      margin: num('margin', 5),
      spacing: num('spacing', 1.5),
      method: $<HTMLSelectElement>('method').value,
      rotations: Math.max(1, Math.round(num('rotations', 72))),
      maxCount: $<HTMLInputElement>('maxcount').value === '' ? -1 : Math.round(num('maxcount', -1)),
      simplify: 0.4,
      attempts: Math.max(1, Math.round(num('attempts', 8))),
      stroke: num('stroke', 0.1),
      wantPdf,
      pdfBackground: $<HTMLInputElement>('pdfBg').checked,
      regMarks: $<HTMLInputElement>('regMarks').checked,
      regDraw: $<HTMLInputElement>('regDraw').checked,
      regLengthIn: num('regLength', 0.4),
      regThicknessIn: num('regThickness', 0.02),
      // per-side fields (advanced) are blank by default and inherit the single Inset value
      regInsetLIn: num('regInsetL', regInset),
      regInsetTIn: num('regInsetT', regInset),
      regInsetRIn: num('regInsetR', regInset),
      regInsetBIn: num('regInsetB', regInset),
    };
    if (!pool) throw new Error('packing worker is unavailable');
    const res = await pool.pack(args, (stage, frac) => {
      $('bar').style.width = Math.round(frac * 100) + '%';
      $('progText').textContent = stage + '…';
    });
    const secs = ((performance.now() - t0) / 1000).toFixed(1);
    $('progress').style.display = 'none';
    const swept = res.sweep.length > 1;
    setStatus(`Packed ${res.count} stickers${swept ? ` at ${res.width} mm` : ''} in ${secs}s.`, 'ok');
    $('sweep').innerHTML = swept ? 'Per sheet by width: ' + sweepSummary(res.sweep, res.width) : '';

    const contentBlob = new Blob([res.contentSvg], { type: 'image/svg+xml' });
    const outlineBlob = new Blob([res.outlineSvg], { type: 'image/svg+xml' });
    $<HTMLImageElement>('contentImg').src = URL.createObjectURL(contentBlob);
    $<HTMLImageElement>('outlineImg').src = URL.createObjectURL(outlineBlob);
    const stem = image.name || 'stickers';
    setLink($('contentDl'), stem + '_content.svg', contentBlob);
    setLink($('outlineDl'), stem + '_outline.svg', outlineBlob);
    if (wantPdf) {
      setLink($('contentDl'), stem + '_content.pdf', new Blob([res.contentPdf as BlobPart], { type: 'application/pdf' }));
      setLink($('outlineDl'), stem + '_outline.pdf', new Blob([res.outlinePdf as BlobPart], { type: 'application/pdf' }));
    }
    $('results').classList.add('show');
  } catch (e: unknown) {
    $('progress').style.display = 'none';
    setStatus('Error: ' + errorMessage(e), 'err');
  }
  packing = false;
  maybeEnable();
});
