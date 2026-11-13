// The target formats, each saying what it keeps (the package's formats()).
import type { Format, FormatInfo, Keeps } from '@litegen/3d-converter';

const MATERIALS: Record<Keeps['materials'], string> = {
  none: 'no materials',
  vertex_color: 'vertex colour',
  color_only: 'colours',
  phong: 'Phong materials',
  pbr: 'PBR materials',
};

/** A one-line summary of what a format keeps, most telling first. */
export function keepsSummary(k: Keeps): string {
  const parts = [MATERIALS[k.materials]];
  if (k.textures) parts.push('textures');
  if (k.skins) parts.push('skins');
  if (k.animation) parts.push('animation');
  if (k.morphTargets) parts.push('morphs');
  if (k.lights) parts.push('lights');
  if (k.cameras) parts.push('cameras');
  if (!k.hierarchy && k.materials !== 'none') parts.push('flattened');
  return parts.join(' · ');
}

export default function FormatPicker({
  formats,
  value,
  onChange,
  packaged,
  busy,
}: {
  formats: FormatInfo[];
  value: Format | null;
  onChange: (f: Format) => void;
  packaged: boolean;
  busy: boolean;
}) {
  return (
    <div className="targets" role="radiogroup" aria-label="Convert to">
      {formats.map((f) => {
        const keeps = packaged && f.writesSidecars ? f.keepsInPackage : f.keeps;
        return (
          <button
            key={f.format}
            type="button"
            role="radio"
            aria-checked={value === f.format}
            className="target"
            data-testid={`target-${f.format}`}
            disabled={busy}
            onClick={() => onChange(f.format)}
            title={`${f.label}: keeps ${keepsSummary(keeps)}`}
          >
            <span className="target-ext">.{f.format}</span>
            <span className="target-label">{f.label}</span>
            <span className="target-keeps">{keepsSummary(keeps)}</span>
          </button>
        );
      })}
    </div>
  );
}
