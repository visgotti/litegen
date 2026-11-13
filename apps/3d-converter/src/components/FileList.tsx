// A model's files and the part each plays (the package's file roles), in
// the shared inspector's table (@litegen/3d-viewer), as the dashboard lists
// a delivered mesh's assets.
import type { FileRole } from '@litegen/3d-converter';
import { Chip, DataTable, formatBytes } from '@litegen/3d-viewer';

export interface ListedFile {
  name: string;
  role: FileRole;
  size: number;
}

const ROLE_LABEL: Record<FileRole, string> = {
  model: 'model',
  material_library: 'materials',
  buffer: 'buffer',
  texture: 'texture',
  layer: 'USD layer',
  unused: 'unused',
};

export default function FileList({ files, title, testId }: { files: ListedFile[]; title: string; testId: string }) {
  return (
    <DataTable
      title={title}
      testId={`${testId}-files`}
      columns={[{ label: 'Role' }, { label: 'File' }, { label: 'Size', numeric: true }]}
      rows={files.map((f) => ({
        key: f.name,
        cells: [<Chip kind={f.role}>{ROLE_LABEL[f.role]}</Chip>, f.name, formatBytes(f.size)],
      }))}
    />
  );
}
