import { ImageResponse } from 'next/og';
import { routing } from '@/i18n/routing';

// Social share card, generated at build time (no network/font fetch needed).
export const alt = 'LiteGen — One API for every AI image & video model';
export const size = { width: 1200, height: 630 };
export const contentType = 'image/png';

// Prebuild one card per locale (required by `output: 'export'`).
export const dynamic = 'force-static';
export function generateStaticParams() {
  return routing.locales.map((locale) => ({ locale }));
}

export default function OpengraphImage() {
  return new ImageResponse(
    (
      <div
        style={{
          height: '100%',
          width: '100%',
          display: 'flex',
          flexDirection: 'column',
          justifyContent: 'center',
          padding: '80px',
          background: '#0f1117',
          color: '#e1e4e8',
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: 22 }}>
          <div
            style={{
              width: 60,
              height: 60,
              borderRadius: 16,
              background: 'linear-gradient(135deg, #7c8cff, #a855f7)',
            }}
          />
          <div style={{ fontSize: 38, fontWeight: 700 }}>LiteGen</div>
        </div>
        <div style={{ display: 'flex', fontSize: 68, fontWeight: 800, marginTop: 44, maxWidth: 940, lineHeight: 1.1 }}>
          One API for every AI image &amp; video model
        </div>
        <div style={{ display: 'flex', fontSize: 30, color: '#9aa4b2', marginTop: 28 }}>
          The universal proxy for AI image &amp; video generation.
        </div>
      </div>
    ),
    { ...size },
  );
}
