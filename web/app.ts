// Main-thread UI. Live preview runs here (fast, synchronous); packing runs in a Web Worker.
import init, { preview, auto_outline } from './sticker_packer.js';
import { traceArt, type Traced } from './trace.js';
import type { PackArgs, ProgressFn, WorkerOut, WorkerResult } from './types.js';

const $ = <T extends HTMLElement = HTMLElement>(id: string): T => document.getElementById(id) as T;
const setStatus = (msg: string, cls = ''): void => {
  const s = $('status');
  s.textContent = msg;
  s.className = 'status ' + cls;
};

interface BorderFile { text: string; url: string; }
interface ImageFile { bytes: Uint8Array; ext: string; url: string | null; }

let border: BorderFile | null = null; // active border (manual upload or auto-generated)
let manualBorder: BorderFile | null = null;
let image: ImageFile = { bytes: new Uint8Array(0), ext: '', url: null };
let traced: Traced | null = null; // silhouette of the current art, for auto-outline
let previewReady = false;
let workerReady = false;

// --- packing worker -------------------------------------------------------
// Guarded so a worker-construction failure can't stop the file inputs from wiring up.
let worker: Worker | null = null;
try {
  worker = new Worker('./worker.js', { type: 'module' });
  worker.addEventListener('message', (e: MessageEvent<WorkerOut>) => {
    if (e.data.type === 'ready') { workerReady = true; maybeReady(); }
    if (e.data.type === 'init-error') setStatus('Failed to load engine: ' + e.data.message, 'err');
  });
} catch (e: unknown) {
  setStatus('Failed to start worker: ' + String((e as Error)?.message ?? e), 'err');
}

function runInWorker(args: PackArgs, onProgress: ProgressFn): Promise<WorkerResult> {
  if (!worker) return Promise.reject(new Error('packing worker is unavailable'));
  const w = worker;
  return new Promise((resolve, reject) => {
    const handler = (e: MessageEvent<WorkerOut>) => {
      const m = e.data;
      if (m.type === 'progress') onProgress(m.stage, m.frac);
      else if (m.type === 'result') { w.removeEventListener('message', handler); resolve(m); }
      else if (m.type === 'error') { w.removeEventListener('message', handler); reject(new Error(m.message)); }
    };
    w.addEventListener('message', handler);
    w.postMessage({ type: 'pack', args });
  });
}

init()
  .then(() => { previewReady = true; maybeReady(); updatePreview(); })
  .catch((e: unknown) => setStatus('Failed to load engine: ' + e, 'err'));

function maybeReady(): void {
  if (previewReady && workerReady) { $<HTMLButtonElement>('run').disabled = !border; setStatus('Ready.', 'ok'); }
}
function maybeEnable(): void {
  $<HTMLButtonElement>('run').disabled = !(previewReady && workerReady && border);
}

// --- live preview (main thread) ------------------------------------------
// Rendered as inline SVG (not <img src>) so a raster's `<image>` can reference the art's
// already-decoded blob URL -- <img>-hosted SVG runs in secure static mode and blocks external
// refs, forcing a slow base64 embed + re-decode instead.
const ART_HREF = '__ART_HREF__';
function updatePreview(): void {
  if (!previewReady || !border) { $('previewPanel').style.display = 'none'; return; }
  const err = $('previewErr');
  const box = $('previewImg');
  try {
    let svg = preview(border.text, image.bytes, image.ext);
    if (image.url) svg = svg.replace(ART_HREF, image.url);
    box.innerHTML = svg;
    box.style.display = '';
    err.style.display = 'none';
  } catch (e: unknown) {
    box.innerHTML = '';
    box.style.display = 'none';
    err.textContent = String((e as Error)?.message ?? e);
    err.style.display = 'block';
  }
  $('previewPanel').style.display = 'block';
}

// --- file inputs: preview replaces the drop zone -------------------------
function wireDrop(dropId: string, inputId: string, onFile: (f: File) => void): void {
  const drop = $(dropId);
  const input = $<HTMLInputElement>(inputId);
  drop.addEventListener('click', () => input.click());
  drop.addEventListener('dragover', (e) => { e.preventDefault(); drop.classList.add('over'); });
  drop.addEventListener('dragleave', () => drop.classList.remove('over'));
  drop.addEventListener('drop', (e: DragEvent) => {
    e.preventDefault();
    drop.classList.remove('over');
    const f = e.dataTransfer?.files[0];
    if (f) onFile(f);
  });
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
}

wireDrop('borderDrop', 'borderFile', async (file) => {
  const text = await file.text();
  if (manualBorder?.url) URL.revokeObjectURL(manualBorder.url);
  manualBorder = { text, url: URL.createObjectURL(new Blob([text], { type: 'image/svg+xml' })) };
  showCard('border', manualBorder.url, file.name);
  if (!autoEnabled()) { border = manualBorder; maybeEnable(); updatePreview(); }
});
$('borderClear').addEventListener('click', () => {
  if (manualBorder?.url) URL.revokeObjectURL(manualBorder.url);
  manualBorder = null;
  clearCard('border');
  if (!autoEnabled()) { border = null; maybeEnable(); updatePreview(); }
});

wireDrop('imageDrop', 'imageFile', async (file) => {
  const buf = new Uint8Array(await file.arrayBuffer());
  const ext = (file.name.split('.').pop() || '').toLowerCase();
  if (image.url) URL.revokeObjectURL(image.url);
  const mime = ext === 'svg' ? 'image/svg+xml' : file.type || 'application/octet-stream';
  const url = URL.createObjectURL(new Blob([buf], { type: mime }));
  image = { bytes: buf, ext, url };
  showCard('image', url, file.name);
  await onArtChanged();
});
$('imageClear').addEventListener('click', () => {
  if (image.url) URL.revokeObjectURL(image.url);
  image = { bytes: new Uint8Array(0), ext: '', url: null };
  traced = null;
  previewCache.clear();
  clearCard('image');
  if (autoEnabled()) regenAuto();
  updatePreview();
});

// --- auto-outline (generate the border by dilating the art silhouette) ---
const autoEnabled = (): boolean => $<HTMLInputElement>('autoOutline').checked;
const currentStyle = (): string => (document.querySelector('input[name=autostyle]:checked') as HTMLInputElement | null)?.value ?? 'external';
const previewCache = new Map<string, string>(); // "style:margin" -> outline SVG for the current art

function clearAutoBorder(): void {
  if (border && border !== manualBorder && border.url) URL.revokeObjectURL(border.url);
  border = null;
}
function genOutline(style: string): string {
  if (!traced) throw new Error('no traced art');
  // autoMargin is in mm; the silhouette is in the art's viewBox units. Convert via the sticker
  // width (blank => viewBox units are treated as mm 1:1 by the packer).
  const marginMm = num('autoMargin', 2);
  const stickerW = num('width', 0);
  const margin = stickerW > 0 ? marginMm * (traced.vb[2] / stickerW) : marginMm;
  // Roundness (0-100): extra convex-corner rounding, as a fraction of the shape size.
  const roundness = num('autoRound', 0);
  const roundRadius = (roundness / 100) * 0.12 * Math.min(traced.vb[2], traced.vb[3]);
  const key = style + ':' + marginMm + ':' + stickerW + ':' + roundness;
  const hit = previewCache.get(key);
  if (hit) return hit;
  const flat: number[] = [];
  const lengths: number[] = [];
  for (const c of traced.contours) { lengths.push(c.length / 2); for (const v of c) flat.push(v); }
  const stroke = Math.max(traced.vb[2], traced.vb[3]) / 150;
  const svg = auto_outline(new Float64Array(flat), new Uint32Array(lengths), traced.vb[0], traced.vb[1], traced.vb[2], traced.vb[3], margin, roundRadius, style, stroke);
  previewCache.set(key, svg);
  return svg;
}
// Simplification (0-100): Douglas-Peucker tolerance on the traced silhouette, in trace pixels.
function simplifyPx(): number {
  return 1 + Math.pow(num('autoSimplify', 0) / 100, 1.5) * 60;
}
async function traceCurrentArt(): Promise<void> {
  traced = null;
  previewCache.clear();
  if (!image.url) return;
  try { traced = await traceArt({ bytes: image.bytes, ext: image.ext, url: image.url }, simplifyPx()); } catch { traced = null; }
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
    err.textContent = String((e as Error)?.message ?? e);
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
$('autoMargin').addEventListener('input', () => { previewCache.clear(); regenAuto(); });
$('width').addEventListener('input', () => { previewCache.clear(); regenAuto(); });
$('autoRound').addEventListener('input', () => { previewCache.clear(); regenAuto(); });
$('autoSimplify').addEventListener('input', async () => { await traceCurrentArt(); regenAuto(); });
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
  a.textContent = '↓ ' + filename.replace('stickers_', '');
  container.appendChild(a);
}

// --- run -----------------------------------------------------------------
$('run').addEventListener('click', async () => {
  if (!border) return;
  const runBtn = $<HTMLButtonElement>('run');
  runBtn.disabled = true;
  setStatus('');
  $('progress').style.display = 'block';
  $('bar').style.width = '0%';
  $('progText').textContent = 'Starting…';
  const t0 = performance.now();
  try {
    const wantPdf = $<HTMLInputElement>('pdf').checked;
    const [pageW, pageH] = pageDims();
    const args: PackArgs = {
      border: border.text,
      imageBytes: image.bytes,
      imageExt: image.ext,
      width: num('width', 0) || 0,
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
    };
    const res = await runInWorker(args, (stage, frac) => {
      $('bar').style.width = Math.round(frac * 100) + '%';
      $('progText').textContent = stage + '…';
    });
    const secs = ((performance.now() - t0) / 1000).toFixed(1);
    $('progress').style.display = 'none';
    setStatus(`Packed ${res.count} stickers in ${secs}s.`, 'ok');

    const contentBlob = new Blob([res.contentSvg], { type: 'image/svg+xml' });
    const outlineBlob = new Blob([res.outlineSvg], { type: 'image/svg+xml' });
    $<HTMLImageElement>('contentImg').src = URL.createObjectURL(contentBlob);
    $<HTMLImageElement>('outlineImg').src = URL.createObjectURL(outlineBlob);
    $('contentDl').innerHTML = '';
    $('outlineDl').innerHTML = '';
    setLink($('contentDl'), 'stickers_content.svg', contentBlob);
    setLink($('outlineDl'), 'stickers_outline.svg', outlineBlob);
    if (wantPdf) {
      setLink($('contentDl'), 'stickers_content.pdf', new Blob([res.contentPdf as BlobPart], { type: 'application/pdf' }));
      setLink($('outlineDl'), 'stickers_outline.pdf', new Blob([res.outlinePdf as BlobPart], { type: 'application/pdf' }));
    }
    $('results').classList.add('show');
  } catch (e: unknown) {
    $('progress').style.display = 'none';
    setStatus('Error: ' + ((e as Error)?.message ?? e), 'err');
  }
  runBtn.disabled = false;
});
