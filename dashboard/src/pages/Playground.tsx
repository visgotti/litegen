import { useState } from 'react';
import SingleMode from '../playground/SingleMode';
import CompareMode from '../playground/CompareMode';
import { useCan } from '../components/useCan';

type Mode = 'single' | 'compare';

export default function Playground() {
  const [mode, setMode] = useState<Mode>('single');
  const can = useCan();
  // A Viewer reads (README roles): its generations and quotes are refused,
  // so the page offers neither, and polls nothing.
  if (!can('generation:create')) {
    return (
      <div>
        <h2 className="page-title">Playground</h2>
        <p className="mg-hint" data-testid="playground-viewer-note">
          Your role reads generations: making one, or asking what one would cost, takes a member&apos;s role.
          Every generation of this organization&apos;s you may read is under Generations.
        </p>
      </div>
    );
  }
  return (
    <div>
      <h2 className="page-title">Playground</h2>
      <div className="pg-mode-toggle" data-testid="pg-mode-toggle">
        <button className={`playground-tab${mode === 'single' ? ' active' : ''}`}
          data-testid="pg-mode-single" onClick={() => setMode('single')}>Single</button>
        <button className={`playground-tab${mode === 'compare' ? ' active' : ''}`}
          data-testid="pg-mode-compare" onClick={() => setMode('compare')}>Compare</button>
      </div>
      {mode === 'single' ? <SingleMode /> : <CompareMode />}
    </div>
  );
}
