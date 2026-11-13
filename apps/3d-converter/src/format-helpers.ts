// A model's facts as the tool shows them: modelFacts and clipRows follow the
// dashboard's sceneFacts and clipRows (dashboard/src/components/
// model3d-assets.ts), over the converter package's ModelInfo instead of the
// API's scene manifest. Sizes, dimensions and the drawn note are the shared
// package's (@litegen/3d-viewer), so both apps read them the same.
import type { ModelInfo } from '@litegen/3d-converter';
import { formatDimensions } from '@litegen/3d-viewer';

const POSE: Record<string, string | null> = { t: 'T-pose', a: 'A-pose', unknown: null };

/** What the converter's reader measured, as label and value rows. */
export function modelFacts(info: ModelInfo): Array<[string, string]> {
  const rows: Array<[string, string]> = [
    ['Triangles / vertices', `${info.triangles.toLocaleString('en-US')} · ${info.vertices.toLocaleString('en-US')}`],
    ['Meshes / nodes', `${info.meshes} · ${info.nodes}`],
    ['Materials', `${info.materials.length} · ${info.uvSets} UV set${info.uvSets === 1 ? '' : 's'} · ${info.vertexColors ? 'vertex colours' : 'no vertex colours'}`],
  ];
  if (info.textures > 0) rows.push(['Textures', String(info.textures)]);
  if (info.bounds) {
    const { min, max } = info.bounds;
    rows.push(['Bounds', formatDimensions({ x: max[0] - min[0], y: max[1] - min[1], z: max[2] - min[2] })]);
  }
  if (info.skin) {
    const humanoid = info.skin.humanoid && `${info.skin.humanoid.naming} humanoid, ${info.skin.humanoid.bones.length} bones`;
    const parts = [`${info.skin.joints} joints`, info.skin.root, POSE[info.skin.bindPose], humanoid].filter(
      (p): p is string => typeof p === 'string' && p !== '',
    );
    rows.push(['Skin', parts.join(' · ')]);
  }
  return rows;
}

export interface ClipRow {
  name: string;
  duration: string;
  inPlace: string;
  travel: string;
}

/** Each animation clip: its name, length, whether it stays in place, and how
 *  far its root travels. */
export function clipRows(info: ModelInfo): ClipRow[] {
  return info.animations.map((a) => ({
    name: a.name ?? '—',
    duration: `${a.duration.toFixed(2)} s`,
    inPlace: a.inPlace ? 'yes' : 'no',
    travel: `${Math.hypot(...a.rootTravel).toFixed(2)} m`,
  }));
}
