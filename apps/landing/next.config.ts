import type { NextConfig } from 'next';
import createNextIntlPlugin from 'next-intl/plugin';

// Points next-intl at the per-request config (locale + messages).
const withNextIntl = createNextIntlPlugin('./src/i18n/request.ts');

const nextConfig: NextConfig = {
  reactStrictMode: true,
  poweredByHeader: false,
  // Fully static build for Cloudflare Pages (no server/middleware at runtime).
  // The site is already SSG (generateStaticParams + setRequestLocale), so this
  // emits plain HTML/JS/CSS into ./out. The `/` -> `/en` redirect the old
  // middleware did is handled by public/_redirects on CF Pages.
  output: 'export',
  // next/image isn't used; disable the optimizer so export needs no loader.
  images: { unoptimized: true },
  // Pin the tracing root to this app (a stray lockfile above the repo would
  // otherwise make Next infer the wrong workspace root).
  outputFileTracingRoot: import.meta.dirname,
};

export default withNextIntl(nextConfig);
