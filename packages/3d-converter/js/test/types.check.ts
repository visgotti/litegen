// Never run, only compiled, with `tsc -p test/tsconfig.json` (strict, exactOptionalPropertyTypes):
// the valid uses must type-check, and every @ts-expect-error line must be an
// error, or tsc fails ("Unused '@ts-expect-error' directive").
import {
  convert, readModel, detectFormat, formats, ConvertError,
  type Conversion, type ConvertOptions, type DaeMorphs, type ErrorCode, type FileRole, type Format, type FormatInfo,
  type Loss, type LossKind, type ModelInfo, type ModelInput, type OptionName, type ReadOptions, type UsdClips, type UsdzRoot,
  type WriteOptions,
} from '@litegen/3d-converter';

const bytes = new Uint8Array(0);

const c: Conversion = convert(bytes, { to: 'obj', package: 'zip', units: 'mm', up: 'z' });
const f: Format = c.format;
const data: Uint8Array<ArrayBuffer> = c.files[0]!.data;
// Owned bytes: their buffer is a real ArrayBuffer, which a Blob or a
// transfer list takes, not an ArrayBufferLike.
const buffer: ArrayBuffer = data.buffer;
const role: FileRole = c.files[0]!.role;
const kind: LossKind | undefined = c.losses[0]?.kind;
const lights: LossKind = 'lights';
const keepsLights: boolean = formats()[0]!.keeps.lights && formats()[0]!.keepsInPackage.cameras;
const points: LossKind = 'points';
const lines: LossKind = 'lines';
const keepsPoints: boolean = formats()[0]!.keeps.points && formats()[0]!.keepsInPackage.lines;
void lights;
void keepsLights;
void points;
void lines;
void keepsPoints;
const loss: Loss | undefined = c.losses[0];

const input: ModelInput = { 'a.gltf': bytes, 'a.bin': bytes };
const m = readModel(input, { from: 'gltf', fromUnits: 'cm' });
const info: ModelInfo = m.info;
const triangles: number = info.triangles;
const duration: number | undefined = info.animations[0]?.duration;
const w: Conversion = m.write({ to: 'fbx', fbxColorSpace: 'linear', fbxLightUnits: 'blender', fps: 24 });
const usd: Conversion = convert(bytes, { to: 'usdz', usdLightUnits: 'photometric' });
const morphs: DaeMorphs = 'keep';
const dae: Conversion = m.write({ to: 'dae', daeMorphs: morphs });
const baked: Conversion = convert(bytes, { to: 'dae', daeMorphs: 'bake' });
const root: UsdzRoot = 'usdc';
const clips: UsdClips = 'all';
const crateRooted: Conversion = m.write({ to: 'usdz', usdzRoot: root, usdClips: 'first' });
const crate: Conversion = convert(bytes, { to: 'usdc', usdClips: clips, usdLightUnits: 'blender' });
const usdcFormat: Format = 'usdc';
m.free();
{
  using disposed = readModel(bytes);
  void disposed.format;
}

const detected: Format | undefined = detectFormat(bytes, 'x.glb');
const all: FormatInfo[] = formats();
const options: OptionName[] = all[0]!.writeOptions;

const read: ReadOptions = { from: 'stl', fromUp: 'z' };
const write: WriteOptions = { to: 'usdz' };
const both: ConvertOptions = { ...read, ...write };

try {
  convert(bytes, both);
} catch (e) {
  if (e instanceof ConvertError) {
    const code: ErrorCode = e.code;
    const param: string | undefined = e.param;
    void code;
    void param;
  }
}

// @ts-expect-error: not a format
convert(bytes, { to: 'xyz' });
// @ts-expect-error: `to` is required
convert(bytes, {});
// @ts-expect-error: not a unit
convert(bytes, { to: 'stl', units: 'km' });
// @ts-expect-error: not an option
convert(bytes, { to: 'stl', scale: 2 });
// @ts-expect-error: not a colour space
convert(bytes, { to: 'fbx', fbxColorSpace: 'rgb' });
// @ts-expect-error: not a light unit
convert(bytes, { to: 'fbx', fbxLightUnits: 'watts' });
// @ts-expect-error: not a morph choice
convert(bytes, { to: 'dae', daeMorphs: 'baked' });
// @ts-expect-error: not a USDZ root
convert(bytes, { to: 'usdz', usdzRoot: 'usd' });
// @ts-expect-error: not a clip choice
convert(bytes, { to: 'usda', usdClips: 'none' });
// @ts-expect-error: not a packaging
convert(bytes, { to: 'obj', package: 'tar' });
// @ts-expect-error: the file's data is bytes
const asText: string = c.files[0]!.data;
// @ts-expect-error: a string is not a model input
convert('model.glb', { to: 'obj' });
// @ts-expect-error: write takes write options only
m.write({ to: 'obj', fromUnits: 'mm' });
// @ts-expect-error: there is no such loss kind
const colour: LossKind = 'colour';

void [f, buffer, role, kind, loss, triangles, duration, w, usd, dae, baked, crateRooted, crate, usdcFormat, detected, options, asText, colour];
