// The conversion's options, each shown only where the package's formats()
// says it applies: the source's readOptions, the target's writeOptions.
import type { DaeMorphs, FbxColorSpace, FormatInfo, LightUnits, OptionName, Packaging, Unit, UpAxis, UsdClips, UsdzRoot } from '@litegen/3d-converter';

export interface Options {
  package: Packaging;
  units?: Unit;
  up?: UpAxis;
  fromUnits?: Unit;
  fromUp?: UpAxis;
  fbxColorSpace?: FbxColorSpace;
  fbxLightUnits?: LightUnits;
  usdLightUnits?: LightUnits;
  daeMorphs?: DaeMorphs;
  usdzRoot?: UsdzRoot;
  usdClips?: UsdClips;
  fps?: number;
}

// Labels for the closed sets: typed by the package's unions, so a value the
// package adds fails this file's build until it is labelled.
const UNITS: Record<Unit, string> = { m: 'metres', cm: 'centimetres', mm: 'millimetres', in: 'inches', ft: 'feet' };
const AXES: Record<UpAxis, string> = { y: '+Y up (glTF)', z: '+Z up (CAD, slicers, Blender)' };
const COLOUR_SPACES: Record<FbxColorSpace, string> = { srgb: 'sRGB (three.js)', linear: 'linear (Blender, Unity, Unreal)' };
const LIGHT_UNITS: Record<LightUnits, string> = { photometric: 'candela and lux (glTF, three.js)', blender: 'watts (Blender)' };
// three.js, which draws this page's preview, cannot load Collada morph
// controllers, so `keep` says so where it is chosen.
const DAE_MORPHS: Record<DaeMorphs, string> = {
  bake: 'at each node\'s weights (every reader)',
  keep: 'as morph controllers (SceneKit, Godot; this preview can\'t draw them)',
};
const USDZ_ROOTS: Record<UsdzRoot, string> = {
  usda: 'USDA text (model-viewer, AR Quick Look)',
  usdc: 'binary crate (as Apple\'s packagers write it)',
};
const USD_CLIPS: Record<UsdClips, string> = {
  all: 'every clip, as variants (a reader without them plays the first)',
  first: 'the first clip only',
};
const PACKAGING: Record<Packaging, string> = { file: 'One file', zip: 'Zip', files: 'Separate files' };

function Select<T extends string>({ name, label, value, choices, onChange, busy }: {
  name: OptionName;
  label: string;
  value: T | undefined;
  choices: Record<T, string>;
  onChange: (v: T | undefined) => void;
  busy: boolean;
}) {
  return (
    <label className="option">
      <span>{label}</span>
      <select data-testid={`option-${name}`} value={value ?? ''} disabled={busy} onChange={(e) => onChange((e.target.value || undefined) as T | undefined)}>
        <option value="">default</option>
        {(Object.entries(choices) as Array<[T, string]>).map(([v, text]) => (
          <option key={v} value={v}>{`${v} — ${text}`}</option>
        ))}
      </select>
    </label>
  );
}

export default function OptionsPanel({ source, target, value, onChange, busy }: {
  source: FormatInfo | null;
  target: FormatInfo | null;
  value: Options;
  onChange: (o: Options) => void;
  busy: boolean;
}) {
  const reads = (o: OptionName) => source?.readOptions.includes(o) ?? false;
  const writes = (o: OptionName) => target?.writeOptions.includes(o) ?? false;
  const set = (patch: Partial<Options>) => onChange({ ...value, ...patch });
  const showFbx = reads('fbxColorSpace') || writes('fbxColorSpace');
  const showFbxLights = reads('fbxLightUnits') || writes('fbxLightUnits');
  const showUsdLights = reads('usdLightUnits') || writes('usdLightUnits');
  const any =
    target?.writesSidecars || reads('fromUnits') || reads('fromUp') || writes('units') || writes('up') || showFbx || showFbxLights || showUsdLights ||
    writes('daeMorphs') || writes('usdzRoot') || writes('usdClips') || writes('fps');
  if (!any) return null;
  return (
    <div className="options" data-testid="options">
      {target?.writesSidecars && (
        <div className="option">
          <span>Package</span>
          <div className="seg" role="group" aria-label="Package">
            {(Object.entries(PACKAGING) as Array<[Packaging, string]>).map(([p, text]) => (
              <button key={p} type="button" className="btn btn-secondary seg-btn" aria-pressed={value.package === p} data-testid={`package-${p}`} disabled={busy} onClick={() => set({ package: p })}>
                {text}
              </button>
            ))}
          </div>
        </div>
      )}
      {reads('fromUnits') && <Select name="fromUnits" label={`Input is in (${source!.format})`} value={value.fromUnits} choices={UNITS} busy={busy} onChange={(v) => set({ fromUnits: v })} />}
      {reads('fromUp') && <Select name="fromUp" label="Input's up axis" value={value.fromUp} choices={AXES} busy={busy} onChange={(v) => set({ fromUp: v })} />}
      {writes('units') && <Select name="units" label={`Write in (${target!.format})`} value={value.units} choices={UNITS} busy={busy} onChange={(v) => set({ units: v })} />}
      {writes('up') && <Select name="up" label="Write with up axis" value={value.up} choices={AXES} busy={busy} onChange={(v) => set({ up: v })} />}
      {showFbx && <Select name="fbxColorSpace" label="FBX colour space" value={value.fbxColorSpace} choices={COLOUR_SPACES} busy={busy} onChange={(v) => set({ fbxColorSpace: v })} />}
      {showFbxLights && <Select name="fbxLightUnits" label="FBX light units" value={value.fbxLightUnits} choices={LIGHT_UNITS} busy={busy} onChange={(v) => set({ fbxLightUnits: v })} />}
      {showUsdLights && <Select name="usdLightUnits" label="USD light units" value={value.usdLightUnits} choices={LIGHT_UNITS} busy={busy} onChange={(v) => set({ usdLightUnits: v })} />}
      {writes('daeMorphs') && <Select name="daeMorphs" label="Collada morph targets" value={value.daeMorphs} choices={DAE_MORPHS} busy={busy} onChange={(v) => set({ daeMorphs: v })} />}
      {writes('usdzRoot') && <Select name="usdzRoot" label="USDZ root layer" value={value.usdzRoot} choices={USDZ_ROOTS} busy={busy} onChange={(v) => set({ usdzRoot: v })} />}
      {writes('usdClips') && <Select name="usdClips" label="USD animation clips" value={value.usdClips} choices={USD_CLIPS} busy={busy} onChange={(v) => set({ usdClips: v })} />}
      {writes('fps') && (
        <label className="option">
          <span>Animation keys per second</span>
          <input
            type="number"
            min={1}
            max={240}
            step={1}
            placeholder="30"
            data-testid="option-fps"
            value={value.fps ?? ''}
            disabled={busy}
            onChange={(e) => {
              const n = Number(e.target.value);
              set({ fps: e.target.value === '' || !Number.isInteger(n) || n < 1 ? undefined : n });
            }}
          />
        </label>
      )}
    </div>
  );
}
