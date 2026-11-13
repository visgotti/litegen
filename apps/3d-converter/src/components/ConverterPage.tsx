// The converter: a model in, shown through its own format's loader; a target
// and its options; the converted files out, shown the same way, with what
// the target could not keep. Every format fact (what a file is, which file
// is the model, which options apply, what a format keeps, what was lost)
// comes from @litegen/3d-converter, which runs in a worker.
import { useEffect, useMemo, useRef, useState } from 'react';
import { Box, MousePointerClick } from 'lucide-react';
import { unzipSync } from 'three/addons/libs/fflate.module.js';
import type { Conversion, Format, FormatInfo, ModelInfo, ReadOptions, WriteOptions } from '@litegen/3d-converter';
import { ConverterClient, type ClientError } from '../converter/client';
import type { ReadResult, WriteResult } from '../converter/protocol';
import { ModelInspector, Thumbnails, type Dimensions, type ViewerFile } from '@litegen/3d-viewer';
import { useTextureThumbnails } from '../texture-thumbnails';
import { viewerFilesOf } from '../viewer-files';
import DropZone from './DropZone';
import SampleGallery, { loadSample, type Sample } from './SampleGallery';
import FormatPicker from './FormatPicker';
import OptionsPanel, { type Options } from './OptionsPanel';
import Downloads from './Downloads';
import ModelFacts from './ModelFacts';
import LossList from './LossList';
import FileList from './FileList';

interface Input {
  files: Record<string, Uint8Array>;
  /** Which file is the model, when the user had to say. */
  fileName?: string;
  stem: string;
  sampleId: string | null;
  read: ReadResult;
  readKey: string;
}

interface Output {
  /** What the user asked for: the download. */
  result: WriteResult;
  /** The same model as separate files, for the viewer (a zip is not shown). */
  display: Conversion;
  displayInfo: ModelInfo | undefined;
  /** The packaging the result was written with. */
  packaging: 'file' | 'zip' | 'files';
  /** The up axis it was written with, so a changed option never turns an
   *  older output while the new one is on its way. */
  up: WriteOptions['up'];
}

const stemOf = (name: string) => name.split('/').pop()!.replace(/\.[^.]+$/, '') || 'model';

const dimensionsOf = (info: ModelInfo | undefined): Dimensions | null | undefined => {
  if (!info) return undefined;
  const b = info.bounds;
  return b ? { x: b.max[0] - b.min[0], y: b.max[1] - b.min[1], z: b.max[2] - b.min[2] } : null;
};

/** The read options that apply to the source format (its readOptions). */
function readOptionsFor(o: Options, source: FormatInfo | null): ReadOptions {
  const r: ReadOptions = {};
  if (source?.readOptions.includes('fromUnits') && o.fromUnits) r.fromUnits = o.fromUnits;
  if (source?.readOptions.includes('fromUp') && o.fromUp) r.fromUp = o.fromUp;
  if (source?.readOptions.includes('fbxColorSpace') && o.fbxColorSpace) r.fbxColorSpace = o.fbxColorSpace;
  if (source?.readOptions.includes('fbxLightUnits') && o.fbxLightUnits) r.fbxLightUnits = o.fbxLightUnits;
  if (source?.readOptions.includes('usdLightUnits') && o.usdLightUnits) r.usdLightUnits = o.usdLightUnits;
  return r;
}

/** The write options that apply to the target format (its writeOptions). */
function writeOptionsFor(to: Format, o: Options, target: FormatInfo | null): WriteOptions {
  const w: WriteOptions = { to };
  if (target?.writesSidecars && o.package !== 'file') w.package = o.package;
  if (target?.writeOptions.includes('units') && o.units) w.units = o.units;
  if (target?.writeOptions.includes('up') && o.up) w.up = o.up;
  if (target?.writeOptions.includes('fbxColorSpace') && o.fbxColorSpace) w.fbxColorSpace = o.fbxColorSpace;
  if (target?.writeOptions.includes('fbxLightUnits') && o.fbxLightUnits) w.fbxLightUnits = o.fbxLightUnits;
  if (target?.writeOptions.includes('usdLightUnits') && o.usdLightUnits) w.usdLightUnits = o.usdLightUnits;
  if (target?.writeOptions.includes('daeMorphs') && o.daeMorphs) w.daeMorphs = o.daeMorphs;
  if (target?.writeOptions.includes('usdzRoot') && o.usdzRoot) w.usdzRoot = o.usdzRoot;
  if (target?.writeOptions.includes('usdClips') && o.usdClips) w.usdClips = o.usdClips;
  if (target?.writeOptions.includes('fps') && o.fps) w.fps = o.fps;
  return w;
}

/** The input's files as the viewer takes them. A lone zip that the
 *  converter unpacked is unpacked here too (three's bundled fflate), so the
 *  viewer gets the same files the converter read. */
function inputViewerFiles(input: Input): ViewerFile[] {
  const names = Object.keys(input.files);
  const listed = new Map(input.read.files.map((f) => [f.name, f.role]));
  if (names.length === 1 && !listed.has(names[0]!)) return viewerFilesOf(unzipSync(input.files[names[0]!]!), listed);
  return Object.entries(input.files).map(([name, data]) => ({ name, data, role: listed.get(name) }));
}

export default function ConverterPage({ client: given }: { client?: ConverterClient }) {
  const client = useMemo(() => given ?? new ConverterClient(), [given]);
  useEffect(() => () => {
    if (!given) client.dispose();
  }, [client, given]);

  const [formats, setFormats] = useState<FormatInfo[]>([]);
  const [input, setInput] = useState<Input | null>(null);
  const [target, setTarget] = useState<Format | null>(null);
  const [options, setOptions] = useState<Options>({ package: 'file' });
  const [output, setOutput] = useState<Output | null>(null);
  const [busy, setBusy] = useState<'reading' | 'converting' | null>(null);
  const [error, setError] = useState<ClientError | null>(null);
  // Files that could be read several ways, and the candidates the converter named.
  const [ambiguous, setAmbiguous] = useState<{ files: Record<string, Uint8Array>; sampleId: string | null; candidates: string[] } | null>(null);
  // Each read or write takes a number; a reply for an older one is dropped.
  const run = useRef(0);

  useEffect(() => {
    // A client disposed before it answered (React's dev double mount) is not
    // an error to show.
    let live = true;
    client.formats().then(
      (f) => {
        if (live) setFormats(f);
      },
      (e: ClientError) => {
        if (live) setError(e);
      },
    );
    return () => {
      live = false;
    };
  }, [client]);

  const infoOf = (f: Format | null | undefined) => formats.find((x) => x.format === f) ?? null;
  const source = infoOf(input?.read.format);
  const targetInfo = infoOf(target);
  const readKey = JSON.stringify(readOptionsFor(options, source));
  const writeOptions = target ? writeOptionsFor(target, options, targetInfo) : null;
  const writeKey = JSON.stringify(writeOptions);

  const open = async (files: Record<string, Uint8Array>, sampleId: string | null, fileName?: string) => {
    const id = ++run.current;
    setBusy('reading');
    setError(null);
    setAmbiguous(null);
    setOutput(null);
    try {
      const read = await client.read(files, fileName ? { fileName } : undefined);
      if (id !== run.current) return;
      // Read-side options belong to a source format; a new model starts clean.
      setOptions((o) => ({ ...o, fromUnits: undefined, fromUp: undefined }));
      const main = read.files.find((f) => f.role === 'model')?.name ?? Object.keys(files)[0] ?? 'model';
      setInput({ files, ...(fileName ? { fileName } : {}), stem: stemOf(main), sampleId, read, readKey: JSON.stringify({}) });
    } catch (e) {
      if (id !== run.current) return;
      const err = e as ClientError;
      setError(err);
      setInput(null);
      if (err.code === 'input_ambiguous' && err.candidates?.length) setAmbiguous({ files, sampleId, candidates: err.candidates });
    } finally {
      if (id === run.current) setBusy(null);
    }
  };

  // A read option changed: read again with it.
  useEffect(() => {
    if (!input || readKey === input.readKey) return;
    const id = ++run.current;
    setBusy('reading');
    setError(null);
    client
      .read(input.files, { ...(JSON.parse(readKey) as ReadOptions), ...(input.fileName ? { fileName: input.fileName } : {}) })
      .then((read) => {
        if (id === run.current) setInput({ ...input, read, readKey });
      }, (e: ClientError) => {
        if (id === run.current) setError(e);
      })
      .finally(() => {
        if (id === run.current) setBusy(null);
      });
  }, [client, input, readKey]);

  // The model, the target or a write option changed: convert.
  useEffect(() => {
    if (!input || !writeOptions || readKey !== input.readKey) return;
    const id = ++run.current;
    setBusy('converting');
    setError(null);
    (async () => {
      const result = await client.write(writeOptions);
      let display = result.conversion;
      let displayInfo = result.info;
      if (writeOptions.package === 'zip') {
        const files = await client.write({ ...writeOptions, package: 'files' });
        display = files.conversion;
        displayInfo = files.info ?? displayInfo;
      }
      if (id === run.current) setOutput({ result, display, displayInfo, packaging: writeOptions.package ?? 'file', up: writeOptions.up });
    })()
      .catch((e: ClientError) => {
        if (id !== run.current) return;
        setError(e);
        setOutput(null);
      })
      .finally(() => {
        if (id === run.current) setBusy(null);
      });
    // writeKey stands for writeOptions, which is rebuilt every render.
  }, [client, input, writeKey]);

  // Separate files whose names have folders, saved as the one zip that keeps them.
  const downloadZip = async () => {
    if (!writeOptions || !input) return;
    try {
      const zipped = await client.write({ ...writeOptions, package: 'zip' });
      const file = zipped.conversion.files[0]!;
      const url = URL.createObjectURL(new Blob([file.data], { type: file.mediaType }));
      const a = document.createElement('a');
      a.href = url;
      a.download = `${input.stem}.${zipped.conversion.format}.zip`;
      a.click();
      setTimeout(() => URL.revokeObjectURL(url), 60_000);
    } catch (e) {
      setError(e as ClientError);
    }
  };

  const inputFiles = useMemo(() => (input ? inputViewerFiles(input) : []), [input]);
  const outputFiles = useMemo<ViewerFile[]>(() => (output ? output.display.files.map((f) => ({ name: f.name, data: f.data, role: f.role })) : []), [output]);
  const inputThumbs = useTextureThumbnails(inputFiles);
  const outputThumbs = useTextureThumbnails(outputFiles);
  const size = (files: { data: Uint8Array }[]) => files.reduce((n, f) => n + f.data.length, 0);

  return (
    <div className="playground-layout converter">
      <section className="playground-panel pane" aria-label="Your model">
        <h2 className="pane-title"><span className="step">1</span> Your model</h2>
        <DropZone busy={busy === 'reading'} onFiles={(files) => void open(files, null)} />
        {ambiguous && (
          <div className="choices" role="group" aria-label="Which file is the model?" data-testid="model-choices">
            <p className="hint">Several of these files could be the model. Which one is it?</p>
            {ambiguous.candidates.map((name) => (
              <button
                type="button"
                key={name}
                className="btn btn-secondary"
                data-testid={`model-choice-${name}`}
                onClick={() => void open(ambiguous.files, ambiguous.sampleId, name)}
              >
                {name}
              </button>
            ))}
          </div>
        )}
        <SampleGallery
          busy={busy === 'reading'}
          current={input?.sampleId ?? null}
          onPick={(s: Sample) => {
            loadSample(s).then((files) => open(files, s.id), (e: Error) => setError({ code: 'format_unrecognized', message: e.message }));
          }}
        />
        {input && source && (
          <ModelInspector
            title="Input"
            testId="input"
            source={{ files: inputFiles }}
            format={input.read.format}
            formatLabel={source.label}
            storedFrame={source.storedFrame}
            {...(options.fromUp ? { up: options.fromUp } : {})}
            size={size(inputFiles)}
            triangles={input.read.info?.triangles}
            dimensions={dimensionsOf(input.read.info)}
          >
            {input.read.info ? (
              <ModelFacts info={input.read.info} testId="input" />
            ) : (
              <p className="hint" data-testid="input-unmeasured">Not measured: {input.read.infoError}</p>
            )}
            {input.read.warnings.length > 0 && <LossList losses={[]} warnings={input.read.warnings} testId="input" />}
            <Thumbnails items={inputThumbs} testId="input" />
            <FileList title="Files" testId="input" files={input.read.files.map((f) => ({ name: f.name, role: f.role, size: f.size }))} />
          </ModelInspector>
        )}
      </section>

      <section className="playground-panel pane" aria-label="Convert to">
        <h2 className="pane-title"><span className="step">2</span> Convert to</h2>
        {formats.length > 0 && (
          <FormatPicker formats={formats} value={target} packaged={options.package !== 'file'} busy={busy !== null || !input} onChange={setTarget} />
        )}
        <OptionsPanel source={source} target={targetInfo} value={options} busy={busy !== null} onChange={setOptions} />
        {error && (
          <div className="alert alert-error error" role="alert" data-testid="error" data-code={error.code}>
            <code className="error-code">{error.code}</code>
            <span>{error.message}</span>
          </div>
        )}
        {busy === 'converting' && (
          <p className="loading" data-testid="converting">
            <span className="spinner" aria-hidden="true" /> Converting in your browser…
          </p>
        )}
        {!output && !error && busy !== 'converting' && (
          <div className="output-empty">
            {input ? <MousePointerClick size={26} aria-hidden="true" /> : <Box size={26} aria-hidden="true" />}
            <p>{input ? 'Pick a format above: it converts here, in your browser.' : 'Choose a model, then a format to convert it to. The result shows here.'}</p>
          </div>
        )}
        {output && targetInfo && input && (
          // What this output is, once written: the format and packaging it was
          // written with (a test waits on these, not on the request).
          <div data-testid="output-done" data-format={output.result.conversion.format} data-package={output.packaging}>
          <ModelInspector
            title="Output"
            testId="output"
            source={{ files: outputFiles }}
            format={output.display.format}
            // The output's own format, not the target just picked: the output
            // on screen stays as it was until the new one is written.
            formatLabel={infoOf(output.display.format)?.label}
            storedFrame={infoOf(output.display.format)?.storedFrame}
            {...(output.up ? { up: output.up } : {})}
            size={size(output.result.conversion.files)}
            triangles={output.displayInfo?.triangles}
            dimensions={dimensionsOf(output.displayInfo)}
          >
            <p className="took" data-testid="took">Converted in {Math.max(1, Math.round(output.result.ms))} ms, in your browser.</p>
            <Downloads conversion={output.result.conversion} stem={input.stem} onZip={() => void downloadZip()} />
            <LossList losses={output.result.conversion.losses} warnings={[]} testId="output" />
            <Thumbnails items={outputThumbs} testId="output" />
            <FileList title="Files written" testId="output" files={output.display.files.map((f) => ({ name: f.name, role: f.role, size: f.data.length }))} />
          </ModelInspector>
          </div>
        )}
      </section>
    </div>
  );
}
