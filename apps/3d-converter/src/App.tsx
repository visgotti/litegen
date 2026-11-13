// The dashboard's application shell (dashboard/src/App.tsx, AppShell): the
// branded sidebar with its navigation, and the page in the content column.
import { Box, BookOpen, ExternalLink, Lock, Package } from 'lucide-react';
import ConverterPage from './components/ConverterPage';
import './App.css';

const LINKS = [
  { href: 'https://www.npmjs.com/package/@litegen/3d-converter', icon: Package, label: 'npm package', short: 'npm' },
  { href: 'https://pypi.org/project/litegen-3d-converter/', icon: Package, label: 'PyPI package', short: 'PyPI' },
  { href: 'https://github.com/visgotti/litegen/tree/main/packages/3d-converter', icon: BookOpen, label: 'Source and docs', short: 'Docs' },
];

export default function App() {
  return (
    <div className="app">
      <nav className="sidebar" aria-label="LiteGen tools">
        <div className="sidebar-inner">
        <div className="sidebar-header">
          <h1>
            <img className="brand-mark" src="litegen-logo.png" alt="" /> LiteGen
          </h1>
          <span className="subtitle">Tools</span>
        </div>
        <ul className="nav-list">
          <li>
            <a className="active" href="./" aria-current="page">
              <Box size={18} aria-hidden="true" />
              <span>3D Converter</span>
            </a>
          </li>
        </ul>
        <p className="nav-section">Use it in code</p>
        <ul className="nav-list nav-list--links">
          {LINKS.map(({ href, icon: Icon, label, short }) => (
            <li key={href}>
              <a href={href} target="_blank" rel="noreferrer" aria-label={label}>
                <Icon size={18} aria-hidden="true" />
                <span className="nav-long">{label}</span>
                <span className="nav-short" aria-hidden="true">{short}</span>
                <ExternalLink size={12} aria-hidden="true" className="nav-external" />
              </a>
            </li>
          ))}
        </ul>
        <p className="sidebar-foot">
          <Lock size={13} aria-hidden="true" />
          Runs in your browser. Nothing is uploaded.
        </p>
        </div>
      </nav>
      <main className="content">
        <header className="page-head">
          <div>
            <h2 className="page-title">3D Converter</h2>
            <p className="page-subtitle">
              Convert between glb, gltf, obj, stl, ply, 3mf, dae, fbx, usdz, usda and usdc: every pair, both ways, with what each
              format can't keep spelled out.
            </p>
          </div>
          <span className="badge accent page-badge">
            <Lock size={12} aria-hidden="true" /> In your browser
          </span>
        </header>
        <ConverterPage />
        <footer className="foot">
          Built on <code>@litegen/3d-converter</code>, litegen's own converter compiled to WebAssembly. Every result is
          drawn by three.js's own loader for its format.
        </footer>
      </main>
    </div>
  );
}
