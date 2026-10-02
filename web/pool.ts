import { best_sweep_index, sweep_widths } from './sticker_packer.js';
import type { PackArgs, ProgressFn, WorkerIn, WorkerOut, WorkerResult } from './types.js';

const SWEEP_SHARE = 0.85;

/** Requires the main-thread wasm module to be initialised. */
export class PackerPool {
  private readonly workers: PackWorker[];

  constructor(private readonly maxSize: number) {
    this.workers = [new PackWorker()];
  }

  get ready(): Promise<void> {
    return this.workers[0].ready;
  }

  async pack(args: PackArgs, onProgress: ProgressFn): Promise<WorkerResult> {
    const widths = Array.from(sweep_widths(args.widthMin, args.widthMax));
    if (widths.length < 2) return this.render(args, onProgress);
    const counts = await this.countAll(args, widths, onProgress);
    const best = best_sweep_index(Uint32Array.from(counts));
    // nothing packed: the full pack reports why
    if (best == null) return this.render(args, onProgress);
    const width = widths[best];
    const res = await this.render(
      { ...args, widthMin: width, widthMax: width },
      (stage, frac) => onProgress(`${stage}, ${width} mm`, SWEEP_SHARE + (1 - SWEEP_SHARE) * frac),
    );
    return { ...res, width, sweep: widths.map((w, i) => [w, counts[i]]) };
  }

  private async render(args: PackArgs, onProgress: ProgressFn): Promise<WorkerResult> {
    const m = await this.workers[0].request({ type: 'pack', args }, onProgress);
    if (m.type !== 'result') throw new Error('unexpected worker reply ' + m.type);
    return m;
  }

  private async countAll(args: PackArgs, widths: number[], onProgress: ProgressFn): Promise<number[]> {
    while (this.workers.length < Math.min(this.maxSize, widths.length)) this.workers.push(new PackWorker());
    const countArgs: PackArgs = { ...args, imageBytes: new Uint8Array(0) };
    const counts: number[] = new Array(widths.length).fill(0);
    let next = 0;
    let done = 0;
    let failed = false;
    onProgress(`Packing ${widths.length} widths`, 0);
    await Promise.all(this.workers.slice(0, widths.length).map(async (w) => {
      while (!failed && next < widths.length) {
        const i = next++;
        try {
          const m = await w.request({ type: 'count', args: countArgs, width: widths[i] });
          if (m.type !== 'count') throw new Error('unexpected worker reply ' + m.type);
          counts[i] = m.count;
        } catch (e) {
          failed = true;
          throw e;
        }
        done++;
        onProgress(`Packed ${widths[i]} mm (${done}/${widths.length})`, SWEEP_SHARE * done / widths.length);
      }
    }));
    return counts;
  }
}

class PackWorker {
  readonly ready: Promise<void>;
  private readonly worker = new Worker('./worker.js', { type: 'module' });
  private queue: Promise<unknown> = Promise.resolve();

  constructor() {
    this.ready = new Promise((resolve, reject) => {
      const onMessage = (e: MessageEvent<WorkerOut>): void => {
        if (e.data.type === 'ready') resolve();
        else if (e.data.type === 'init-error') reject(new Error(e.data.message));
        else return;
        this.worker.removeEventListener('message', onMessage);
      };
      this.worker.addEventListener('message', onMessage);
    });
  }

  /** Requests run one at a time: replies carry no id, so only one may be in flight. */
  request(msg: WorkerIn, onProgress?: ProgressFn): Promise<WorkerOut> {
    const reply = this.queue.then(() => this.ready).then(() => this.send(msg, onProgress));
    this.queue = reply.catch(() => {});
    return reply;
  }

  private send(msg: WorkerIn, onProgress?: ProgressFn): Promise<WorkerOut> {
    return new Promise((resolve, reject) => {
      const done = (): void => {
        this.worker.removeEventListener('message', onMessage);
        this.worker.removeEventListener('error', onError);
      };
      const onMessage = (e: MessageEvent<WorkerOut>): void => {
        const m = e.data;
        if (m.type === 'progress') { onProgress?.(m.stage, m.frac); return; }
        done();
        if (m.type === 'error') reject(new Error(m.message));
        else resolve(m);
      };
      const onError = (e: ErrorEvent): void => {
        done();
        reject(new Error(e.message || 'packing worker crashed'));
      };
      this.worker.addEventListener('message', onMessage);
      this.worker.addEventListener('error', onError);
      this.worker.postMessage(msg);
    });
  }
}
