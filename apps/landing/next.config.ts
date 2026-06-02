import type { NextConfig } from 'next';
import createNextIntlPlugin from 'next-intl/plugin';

// Points next-intl at the per-request config (locale + messages).
const withNextIntl = createNextIntlPlugin('./src/i18n/request.ts');

// GitHub Pages serves a project site from a subpath (e.g. /litegen), so that
// build sets NEXT_PUBLIC_BASE_PATH=/litegen to prefix routes + assets. It's a
// NEXT_PUBLIC_ var so client code (e.g. the Redoc spec fetch) can read it too.
// Cloudflare Pages serves at the domain root and sets nothing → no prefix.
const basePath = process.env.NEXT_PUBLIC_BASE_PATH?.trim().replace(/\/$/, '') || '';

const nextConfig: NextConfig = {
  reactStrictMode: true,
  poweredByHeader: false,
  // Fully static build for Cloudflare Pages (no server/middleware at runtime).
  // The site is already SSG (generateStaticParams + setRequestLocale), so this
  // emits plain HTML/JS/CSS into ./out. The `/` -> `/en` redirect the old
  // middleware did is handled by public/_redirects on CF Pages.
  output: 'export',
  // Subpath prefix for GitHub Pages (empty for Cloudflare root hosting).
  ...(basePath ? { basePath, assetPrefix: basePath } : {}),
  // next/image isn't used; disable the optimizer so export needs no loader.
  images: { unoptimized: true },
  // Pin the tracing root to this app (a stray lockfile above the repo would
  // otherwise make Next infer the wrong workspace root).
  outputFileTracingRoot: import.meta.dirname,
};

export default withNextIntl(nextConfig);
