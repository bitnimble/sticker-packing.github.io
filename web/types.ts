// Shared types for the main-thread app and the packing worker.

export interface PackArgs {
  border: string;
  imageBytes: Uint8Array;
  imageExt: string;
  width: number;
  pageW: number;
  pageH: number;
  margin: number;
  spacing: number;
  method: string;
  rotations: number;
  maxCount: number;
  simplify: number;
  attempts: number;
  stroke: number;
  wantPdf: boolean;
  pdfBackground: boolean;
  regMarks: boolean;
  regLengthIn: number;
  regInsetLIn: number;
  regInsetTIn: number;
  regInsetRIn: number;
  regInsetBIn: number;
}

export type ProgressFn = (stage: string, frac: number) => void;

export interface WorkerResult {
  type: 'result';
  count: number;
  contentSvg: string;
  outlineSvg: string;
  contentPdf: Uint8Array;
  outlinePdf: Uint8Array;
}

export type WorkerOut =
  | { type: 'ready' }
  | { type: 'init-error'; message: string }
  | { type: 'progress'; stage: string; frac: number }
  | WorkerResult
  | { type: 'error'; message: string };

export type WorkerIn = { type: 'pack'; args: PackArgs };
