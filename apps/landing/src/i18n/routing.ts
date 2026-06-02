import { defineRouting } from 'next-intl/routing';

/**
 * Locale routing config — the single place to add a language. Drop a new
 * `messages/<locale>.json` and append the code here; routes, the locale
 * switcher, and the sitemap/hreflang alternates all pick it up automatically.
 */
export const routing = defineRouting({
  locales: ['en', 'es'],
  defaultLocale: 'en',
  localePrefix: 'always',
});

export type Locale = (typeof routing.locales)[number];
