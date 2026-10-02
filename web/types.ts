// Shared types for the main-thread app and the packing worker.

export interface AutoOutlineArgs {
  points: Float64Array;
  lengths: Uint32Array;
  vb: [number, number, number, number];
  marginMm: number;
  roundRadius: number;
  style: string;
  stroke: number;
}

export interface PackArgs {
  border: string;
  auto: AutoOutlineArgs | null; // replaces `border`, regenerated per swept width
  imageBytes: Uint8Array;
  imageExt: string;
  widthMin: number;
  widthMax: number;
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
  regDraw: boolean;
  regLengthIn: number;
  regThicknessIn: number;
  regInsetLIn: number;
  regInsetTIn: number;
  regInsetRIn: number;
  regInsetBIn: number;
}

export const errorMessage = (e: unknown): string => (e instanceof Error ? e.message : String(e));

export type ProgressFn =(stage: string, frac: number) => void;

export interface WorkerResult {
  type: 'result';
  count: number;
  width: number | undefined;
  sweep: Array<[width: number, count: number]>;
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
  | { type: 'count'; count: number }
  | { type: 'error'; message: string };

export type WorkerIn =
  | { type: 'pack'; args: PackArgs }
  | { type: 'count'; args: PackArgs; width: number };
