/// <reference lib="webworker" />
import init, { pack, pack_count, PackOptions } from './sticker_packer.js';
import { errorMessage, type PackArgs, type WorkerIn, type WorkerOut } from './types.js';

const ctx = self as unknown as DedicatedWorkerGlobalScope;
const post = (m: WorkerOut, transfer?: Transferable[]) =>
  transfer ? ctx.postMessage(m, transfer) : ctx.postMessage(m);

const ready = init()
  .then(() => post({ type: 'ready' }))
  .catch((e: unknown) => post({ type: 'init-error', message: errorMessage(e) }));

const options = (a: PackArgs): PackOptions => new PackOptions(
  a.widthMin, a.widthMax, a.pageW, a.pageH,
  a.margin, a.spacing, a.method, a.rotations, a.maxCount, a.simplify,
  a.attempts, a.stroke, a.wantPdf, a.pdfBackground,
  a.regMarks, a.regDraw, a.regDrawOutline, a.regLengthIn, a.regThicknessIn,
  a.regInsetLIn, a.regInsetTIn, a.regInsetRIn, a.regInsetBIn,
);

ctx.onmessage = async (e: MessageEvent<WorkerIn>) => {
  const msg = e.data;
  const a = msg.args;
  try {
    await ready;
    const opts = options(a);
    try {
      if (msg.type === 'count') {
        post({ type: 'count', count: pack_count(a.border, msg.width, opts) });
        return;
      }
      const onProgress = (stage: string, frac: number) => post({ type: 'progress', stage, frac });
      const res = pack(a.border, a.imageBytes, a.imageExt, opts, onProgress);
      const counts = res.sweep_counts;
      const out: WorkerOut = {
        type: 'result',
        count: res.count,
        width: res.width,
        sweep: Array.from(res.sweep_widths, (w, i): [number, number] => [w, counts[i]]),
        contentSvg: res.content_svg,
        outlineSvg: res.outline_svg,
        contentPdf: res.content_pdf,
        outlinePdf: res.outline_pdf,
      };
      res.free();
      post(out, [out.contentPdf.buffer, out.outlinePdf.buffer]);
    } finally {
      opts.free();
    }
  } catch (err: unknown) {
    post({ type: 'error', message: errorMessage(err) });
  }
};
