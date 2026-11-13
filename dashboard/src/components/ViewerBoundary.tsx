// Keeps a failure inside the lazily loaded 3D viewer from taking the
// dashboard down. The dashboard has no error boundary of its own, so a
// rejected lazy import would unmount the whole app. The usual cause is a tab
// left open across a deploy: the old chunk names are deleted, and the server
// answers them with index.html. The mesh stays downloadable.
import { Component, type ReactNode } from 'react';

interface Props {
  /** The mesh, still downloadable when the viewer is not. */
  meshUrl?: string;
  /** The viewer's test ID; the fallback's is `${testId}-fallback`. */
  testId?: string;
  children: ReactNode;
}

export default class ViewerBoundary extends Component<Props, { failed: boolean }> {
  state = { failed: false };

  static getDerivedStateFromError() {
    return { failed: true };
  }

  render() {
    if (!this.state.failed) return this.props.children;
    const { meshUrl, testId } = this.props;
    return (
      <div
        role="alert"
        data-testid={testId ? `${testId}-fallback` : undefined}
        data-reason="import"
        style={{
          display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', gap: 8,
          minHeight: 160, padding: 16, textAlign: 'center', boxSizing: 'border-box', width: '100%',
          background: '#0d1117', border: '1px dashed #30363d', borderRadius: 6, color: '#8b949e', fontSize: 13,
        }}
      >
        <span>The 3D viewer could not be loaded — reload the page to try again.</span>
        {meshUrl && (
          // target=_blank: the URL is absolute and usually cross-origin, where
          // `download` is ignored, and must not navigate the dashboard away.
          <a
            href={meshUrl}
            download
            target="_blank"
            rel="noopener noreferrer"
            data-testid={testId ? `${testId}-fallback-link` : undefined}
            style={{ color: '#58a6ff' }}
          >
            Download mesh
          </a>
        )}
      </div>
    );
  }
}
