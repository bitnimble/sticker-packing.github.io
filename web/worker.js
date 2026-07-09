// Packing worker: loads the wasm engine and runs pack() off the main thread, streaming
// progress back so the page stays responsive.
import init, { pack } from './sticker_packer.js';

const ready = init()
  .then(() => self.postMessage({ type: 'ready' }))
  .catch(e => self.postMessage({ type: 'init-error', message: String(e && e.message || e) }));

self.onmessage = async (e) => {
  if (e.data.type !== 'pack') return;
  const a = e.data.args;
  try {
    await ready;
    const onProgress = (stage, frac) => self.postMessage({ type: 'progress', stage, frac });
    const res = pack(
      a.border, a.imageBytes, a.imageExt, a.width, a.pageW, a.pageH,
      a.margin, a.spacing, a.method, a.rotations, a.maxCount, a.simplify,
      a.attempts, a.stroke, a.wantPdf, onProgress,
    );
    const out = {
      type: 'result',
      count: res.count,
      contentSvg: res.content_svg,
      outlineSvg: res.outline_svg,
      contentPdf: res.content_pdf,   // Uint8Array
      outlinePdf: res.outline_pdf,
    };
    res.free();
    // transfer the PDF buffers (avoids a copy back to the main thread)
    self.postMessage(out, [out.contentPdf.buffer, out.outlinePdf.buffer]);
  } catch (err) {
    self.postMessage({ type: 'error', message: String(err && err.message || err) });
  }
};
