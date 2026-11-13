// The messages between the page and the converter worker. The worker holds
// one read model; reads replace it, writes write it.
import type { Conversion, Format, FormatInfo, InputFileInfo, ModelInfo, ReadOptions, WriteOptions } from '@litegen/3d-converter';

export type Request =
  | { id: number; op: 'formats' }
  | { id: number; op: 'read'; files: Record<string, Uint8Array>; options?: ReadOptions }
  | { id: number; op: 'write'; options: WriteOptions };

export interface WireError {
  name: string;
  code: string;
  message: string;
  param?: string;
  /** With `input_ambiguous`: the files that could be the model. */
  candidates?: string[];
}

export type Reply = { id: number; ok: true; value: unknown } | { id: number; ok: false; error: WireError };

export interface ReadResult {
  format: Format;
  /** What the converter measured; absent when it could not measure the
   *  model (infoError says why), which still converts. */
  info?: ModelInfo;
  infoError?: string;
  warnings: string[];
  files: InputFileInfo[];
}

export interface WriteResult {
  conversion: Conversion;
  /** How long the write took in the worker, in milliseconds. */
  ms: number;
  /** What the converter's own reader finds in the written files, for the
   *  output's stats; absent when it cannot read them back. */
  info?: ModelInfo;
}

export type Replies = { formats: FormatInfo[]; read: ReadResult; write: WriteResult };
