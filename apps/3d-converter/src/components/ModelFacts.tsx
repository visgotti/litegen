// What the converter's reader measured in a model, in the shared
// inspector's fact grid and clips table (@litegen/3d-viewer), as the
// dashboard shows a delivered mesh's scene manifest.
import type { ModelInfo } from '@litegen/3d-converter';
import { ClipTable, FactList } from '@litegen/3d-viewer';
import { clipRows, modelFacts } from '../format-helpers';

export default function ModelFacts({ info, testId }: { info: ModelInfo; testId: string }) {
  return (
    <>
      <FactList rows={modelFacts(info)} testId={testId} />
      <ClipTable rows={clipRows(info)} testId={testId} />
    </>
  );
}
