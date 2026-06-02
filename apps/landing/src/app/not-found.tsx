import Link from 'next/link';
import { routing } from '@/i18n/routing';

/**
 * Global 404 for non-localized paths. It renders outside the `[locale]` layout,
 * so it provides its own <html>/<body>.
 */
export default function NotFound() {
  return (
    <html lang={routing.defaultLocale}>
      <body
        style={{
          margin: 0,
          minHeight: '100vh',
          display: 'flex',
          flexDirection: 'column',
          alignItems: 'center',
          justifyContent: 'center',
          gap: 16,
          background: '#0f1117',
          color: '#e1e4e8',
          fontFamily: 'system-ui, sans-serif',
          textAlign: 'center',
          padding: 24,
        }}
      >
        <h1 style={{ fontSize: 48, margin: 0 }}>404</h1>
        <p style={{ color: '#9aa4b2', margin: 0 }}>This page could not be found.</p>
        <Link href={`/${routing.defaultLocale}`} style={{ color: '#7c8cff', fontWeight: 600 }}>
          ← Back to LiteGen
        </Link>
      </body>
    </html>
  );
}
