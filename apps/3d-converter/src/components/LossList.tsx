// What a conversion could not keep, and what reading the input dropped: the
// package's losses (kind, what, why) and the reader's warnings, in the
// shared inspector's table and note list (@litegen/3d-viewer).
import type { Loss } from '@litegen/3d-converter';
import { Chip, DataTable, NoteList } from '@litegen/3d-viewer';

export default function LossList({ losses, warnings, testId }: { losses: Loss[]; warnings: string[]; testId: string }) {
  return (
    <>
      <DataTable
        title="Not kept"
        testId={`${testId}-losses`}
        columns={[{ label: 'Kind' }, { label: 'What' }, { label: 'Why' }]}
        rows={losses.map((l, i) => ({
          key: `${i}:${l.kind}:${l.what}`,
          cells: [<Chip kind="loss">{l.kind.replace(/_/g, ' ')}</Chip>, l.what, <span style={{ whiteSpace: 'normal' }}>{l.why}</span>],
        }))}
        empty="Nothing: this format holds everything the model has."
      />
      <NoteList title="Reading it" notes={warnings} testId={`${testId}-warnings`} />
    </>
  );
}
