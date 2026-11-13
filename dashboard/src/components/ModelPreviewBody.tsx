// The full 3D inspector for one generation's result: the shared inspector
// (@litegen/3d-viewer, packages/3d-viewer), the same one the 3D converter
// tool shows, filled from the API's assets and the delivered mesh's scene
// manifest. Loaded on first use by ModelPreview, so three.js stays out of the
// dashboard's first chunk.
import type { Model3dAsset } from '@litegen/sdk';
import { Download, ExternalLink } from 'lucide-react';
import {
  Chip, ClipTable, CopyUrlButton, DataTable, FactList, InspectorError, ModelInspector, NoteList, Thumbnails, formatBytes,
} from '@litegen/3d-viewer';
import {
  clipRows, isBrowserImage, meshOf, originalOf, previewOf, sceneDimensions, sceneFacts, sceneNotes, sceneTriangles, texturesOf, validAssets,
} from './model3d-assets';
import { fetchStoredFile, useFileSrc } from '../file-src';

const NO_MESH = 'This generation completed without a mesh asset. Every completed 3D generation must carry exactly one mesh, so this is a provider contract violation — there is nothing to render.';

function upper(format: string): string {
  return format ? format.toUpperCase() : '—';
}

/** Kind-appropriate secondary figure for the asset list. */
function assetDetail(a: Model3dAsset): string {
  if (a.polycount != null) return `${a.polycount.toLocaleString('en-US')} polys`;
  if (a.width != null && a.height != null) return `${a.width} × ${a.height}`;
  return '—';
}

function AssetTable({ assets, testId }: { assets: Model3dAsset[]; testId: string }) {
  return (
    <DataTable
      title="Assets"
      testId={`${testId}-assets`}
      columns={[{ label: 'Kind' }, { label: 'Format' }, { label: 'Size', numeric: true }, { label: 'Detail', numeric: true }, { label: 'Links', hidden: true }]}
      rows={assets.map((a, i) => ({
        key: `${i}:${a.url}`,
        testId: `${testId}-asset-${i}`,
        cells: [
          <Chip kind={a.kind}>{a.kind}</Chip>,
          upper(a.format),
          formatBytes(a.size_bytes),
          assetDetail(a),
          // Asset URLs are absolute and usually cross-origin, where browsers
          // ignore `download`; target=_blank keeps that case from navigating
          // the dashboard away.
          <>
            <a href={a.url} target="_blank" rel="noreferrer" data-testid={`${testId}-asset-${i}-open`} aria-label={`Open ${a.kind} ${upper(a.format)} in a new tab`}>
              <ExternalLink size={12} aria-hidden="true" />Open
            </a>
            <a href={a.url} download target="_blank" rel="noreferrer" data-testid={`${testId}-asset-${i}-download`} aria-label={`Download ${a.kind} ${upper(a.format)}`}>
              <Download size={12} aria-hidden="true" />Download
            </a>
          </>,
        ],
      }))}
    />
  );
}

/** A texture or preview thumbnail; a private app's file route is fetched with
 *  the dashboard's credentials first. */
function FileImg({ src, alt }: { src: string; alt: string }) {
  return <img src={useFileSrc(src)} alt={alt} loading="lazy" />;
}

export interface ModelPreviewProps {
  assets: Model3dAsset[];
  /** A failure the caller already knows about; replaces the viewer. */
  error?: string;
  testId?: string;
}

export default function ModelPreviewBody({ assets: rawAssets, error, testId = 'model-preview' }: ModelPreviewProps) {
  // The prop is untrusted too: Playground history replays response bodies from
  // localStorage without going through assetsOf, and a malformed entry must
  // not throw in render (there is no error boundary above this).
  const assets = validAssets(rawAssets);
  const mesh = meshOf(assets);
  const preview = previewOf(assets);
  const poster = useFileSrc(preview?.url);

  if (error || !mesh) {
    return (
      <section className="mp" data-testid={testId} data-state="error" aria-label="3D model inspector">
        <InspectorError testId={testId} title={error ? '3D generation failed' : 'No mesh in this result'} message={error ?? NO_MESH} />
        {assets.length > 0 && <AssetTable assets={assets} testId={testId} />}
      </section>
    );
  }

  const scene = mesh.scene;
  const original = originalOf(assets);
  const thumbs = [preview, ...texturesOf(assets)]
    .filter((a): a is Model3dAsset => a != null && isBrowserImage(a.format))
    .map((a) => ({ url: a.url, label: a.kind, alt: `${a.kind} (${upper(a.format)})` }));

  return (
    <ModelInspector
      testId={testId}
      source={{ url: mesh.url, poster: preview ? (poster ?? null) : null, fetch: fetchStoredFile }}
      // An unlabelled mesh (a row older than assets[]) is tried as glTF
      // Binary, as <model-viewer> tried it.
      format={mesh.format.toLowerCase() || 'glb'}
      size={mesh.size_bytes}
      // What litegen counted on delivery; else what the vendor reported.
      triangles={sceneTriangles(scene) ?? mesh.polycount}
      dimensions={sceneDimensions(scene)}
      actions={<CopyUrlButton url={mesh.url} testId={`${testId}-copy-url`} label="Copy mesh URL" />}
      // Several Generations rows can be open at once: each draws only on screen.
      mountWhenVisible
    >
      <FactList rows={sceneFacts(scene)} testId={testId} />
      <ClipTable rows={clipRows(scene)} testId={testId} />
      <NoteList title="Delivery notes" notes={sceneNotes(scene)} testId={`${testId}-notes`} />
      {original && (
        <p className="mp-original" data-testid={`${testId}-original`}>
          The vendor’s own file, kept beside litegen’s normalised mesh:{' '}
          <a href={original.url} target="_blank" rel="noreferrer">{upper(original.format)}</a>
          {original.size_bytes != null && ` (${formatBytes(original.size_bytes)})`}
        </p>
      )}
      <Thumbnails items={thumbs} testId={testId} image={FileImg} />
      <AssetTable assets={assets} testId={testId} />
    </ModelInspector>
  );
}
