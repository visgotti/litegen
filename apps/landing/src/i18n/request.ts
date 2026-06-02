import { getRequestConfig } from 'next-intl/server';
import { routing, type Locale } from './routing';

/**
 * Resolves the active locale per request and loads its message dictionary.
 * Falls back to the default locale for anything unrecognised.
 */
export default getRequestConfig(async ({ requestLocale }) => {
  let locale = await requestLocale;

  if (!locale || !routing.locales.includes(locale as Locale)) {
    locale = routing.defaultLocale;
  }

  return {
    locale,
    messages: (await import(`../../messages/${locale}.json`)).default,
  };
});
