// Main-thread UI. Live preview runs here (fast, synchronous); packing runs in a Web Worker.
import init, { preview } from './sticker_packer.js';
import type { PackArgs, ProgressFn, WorkerOut, WorkerResult } from './types.js';

const $ = <T extends HTMLElement = HTMLElement>(id: string): T => document.getElementById(id) as T;
const setStatus = (msg: string, cls = ''): void => {
  const s = $('status');
  s.textContent = msg;
  s.className = 'status ' + cls;
};

interface BorderFile { text: string; url: string; }
interface ImageFile { bytes: Uint8Array; ext: string; url: string | null; }

let border: BorderFile | null = null;
let image: ImageFile = { bytes: new Uint8Array(0), ext: '', url: null };
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
let previewUrl: string | null = null;
function updatePreview(): void {
  if (!previewReady || !border) { $('previewPanel').style.display = 'none'; return; }
  const err = $('previewErr');
  const img = $<HTMLImageElement>('previewImg');
  try {
    const svg = preview(border.text, image.bytes, image.ext);
    if (previewUrl) URL.revokeObjectURL(previewUrl);
    previewUrl = URL.createObjectURL(new Blob([svg], { type: 'image/svg+xml' }));
    img.src = previewUrl;
    img.style.display = '';
    err.style.display = 'none';
  } catch (e: unknown) {
    img.style.display = 'none';
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
  if (border?.url) URL.revokeObjectURL(border.url);
  border = { text, url: URL.createObjectURL(new Blob([text], { type: 'image/svg+xml' })) };
  showCard('border', border.url, file.name);
  maybeEnable();
  updatePreview();
});
$('borderClear').addEventListener('click', () => {
  if (border?.url) URL.revokeObjectURL(border.url);
  border = null;
  clearCard('border');
  maybeEnable();
  updatePreview();
});

wireDrop('imageDrop', 'imageFile', async (file) => {
  const buf = new Uint8Array(await file.arrayBuffer());
  const ext = (file.name.split('.').pop() || '').toLowerCase();
  if (image.url) URL.revokeObjectURL(image.url);
  const mime = ext === 'svg' ? 'image/svg+xml' : file.type || 'application/octet-stream';
  const url = URL.createObjectURL(new Blob([buf], { type: mime }));
  image = { bytes: buf, ext, url };
  showCard('image', url, file.name);
  updatePreview();
});
$('imageClear').addEventListener('click', () => {
  if (image.url) URL.revokeObjectURL(image.url);
  image = { bytes: new Uint8Array(0), ext: '', url: null };
  clearCard('image');
  updatePreview();
});

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
