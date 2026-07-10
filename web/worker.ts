/// <reference lib="webworker" />
// Packing worker: runs pack() off the main thread, streaming progress.
import init, { pack } from './sticker_packer.js';
import type { WorkerIn, WorkerOut } from './types.js';

const ctx = self as unknown as DedicatedWorkerGlobalScope;
const post = (m: WorkerOut, transfer?: Transferable[]) =>
  transfer ? ctx.postMessage(m, transfer) : ctx.postMessage(m);

const ready = init()
  .then(() => post({ type: 'ready' }))
  .catch((e: unknown) => post({ type: 'init-error', message: String((e as Error)?.message ?? e) }));

ctx.onmessage = async (e: MessageEvent<WorkerIn>) => {
  if (e.data.type !== 'pack') return;
  const a = e.data.args;
  try {
    await ready;
    const onProgress = (stage: string, frac: number) => post({ type: 'progress', stage, frac });
    const res = pack(
      a.border, a.imageBytes, a.imageExt, a.width, a.pageW, a.pageH,
      a.margin, a.spacing, a.method, a.rotations, a.maxCount, a.simplify,
      a.attempts, a.stroke, a.wantPdf,
      a.regMarks, a.regLengthIn, a.regInsetLIn, a.regInsetTIn, a.regInsetRIn, a.regInsetBIn,
      onProgress,
    );
    const out: WorkerOut = {
      type: 'result',
      count: res.count,
      contentSvg: res.content_svg,
      outlineSvg: res.outline_svg,
      contentPdf: res.content_pdf,
      outlinePdf: res.outline_pdf,
    };
    res.free();
    post(out, [out.contentPdf.buffer, out.outlinePdf.buffer] as Transferable[]);
  } catch (err: unknown) {
    post({ type: 'error', message: String((err as Error)?.message ?? err) });
  }
};
