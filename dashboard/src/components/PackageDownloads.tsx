import type { Model3dAsset } from '@litegen/sdk';
import { packageDownloads, packageLabel } from './model3d-assets';

/** Download links for a 3D result's zip packages (`output_packages: true`),
 *  in request order. Renders nothing when there are none. The zips are linked,
 *  never opened. */
export default function PackageDownloads({ assets, testId }: { assets: Model3dAsset[]; testId: string }) {
  const packages = packageDownloads(assets);
  if (packages.length === 0) return null;
  return (
    <ul className="package-downloads" data-testid={testId} style={{ listStyle: 'none', padding: 0, margin: '8px 0 0' }}>
      {packages.map(p => (
        <li key={p.url}>
          <a href={p.url} download data-testid={`${testId}-${p.format.toLowerCase()}`} rel="noopener noreferrer">
            {packageLabel(p)}
          </a>
        </li>
      ))}
    </ul>
  );
}
