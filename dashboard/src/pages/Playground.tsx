import { useState } from 'react';
import SingleMode from '../playground/SingleMode';
import CompareMode from '../playground/CompareMode';

type Mode = 'single' | 'compare';

export default function Playground() {
  const [mode, setMode] = useState<Mode>('single');
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
