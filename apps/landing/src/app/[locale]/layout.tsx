import type { Metadata } from 'next';
import { NextIntlClientProvider } from 'next-intl';
import { getMessages, getTranslations, setRequestLocale } from 'next-intl/server';
import { notFound } from 'next/navigation';
import { routing, type Locale } from '@/i18n/routing';
import { siteConfig } from '@/config/site';
import '@/styles/globals.css';

type LocaleParams = { params: Promise<{ locale: string }> };

/** Pre-render one static page per supported locale. */
export function generateStaticParams() {
  return routing.locales.map((locale) => ({ locale }));
}

export async function generateMetadata({ params }: LocaleParams): Promise<Metadata> {
  const { locale } = await params;
  const t = await getTranslations({ locale, namespace: 'meta' });

  // hreflang alternates so search engines serve the right language.
  const languages = Object.fromEntries(routing.locales.map((l) => [l, `/${l}`]));

  return {
    metadataBase: new URL(siteConfig.url),
    title: { default: t('title'), template: `%s · ${siteConfig.name}` },
    description: t('description'),
    applicationName: siteConfig.name,
    keywords: [
      'AI image generation',
      'AI video generation',
      'OpenAI compatible',
      'image generation proxy',
      'LiteLLM for images',
      'DALL-E',
      'Stable Diffusion',
      'Replicate',
      'Fal',
      'self-hosted AI gateway',
    ],
    authors: [{ name: 'LiteGen' }],
    alternates: {
      canonical: `/${locale}`,
      languages,
    },
    openGraph: {
      type: 'website',
      siteName: siteConfig.name,
      title: t('title'),
      description: t('description'),
      url: `${siteConfig.url}/${locale}`,
      locale,
    },
    twitter: {
      card: 'summary_large_image',
      title: t('title'),
      description: t('description'),
    },
    robots: {
      index: true,
      follow: true,
      googleBot: { index: true, follow: true, 'max-image-preview': 'large' },
    },
  };
}

export default async function LocaleLayout({
  children,
  params,
}: LocaleParams & { children: React.ReactNode }) {
  const { locale } = await params;
  if (!routing.locales.includes(locale as Locale)) {
    notFound();
  }

  // Enables static rendering for this locale.
  setRequestLocale(locale);
  const messages = await getMessages();
  const t = await getTranslations({ locale, namespace: 'meta' });

  // Structured data (schema.org) so search engines can build rich results and
  // understand LiteGen as a free, open-source developer tool. All URLs derive
  // from siteConfig.url so they track the canonical domain automatically.
  const jsonLd = {
    '@context': 'https://schema.org',
    '@graph': [
      {
        '@type': 'Organization',
        '@id': `${siteConfig.url}/#organization`,
        name: siteConfig.name,
        url: siteConfig.url,
        logo: `${siteConfig.url}/icon.svg`,
        sameAs: [siteConfig.githubUrl],
      },
      {
        '@type': 'WebSite',
        '@id': `${siteConfig.url}/#website`,
        url: siteConfig.url,
        name: siteConfig.name,
        description: t('description'),
        publisher: { '@id': `${siteConfig.url}/#organization` },
        inLanguage: locale,
      },
      {
        '@type': 'SoftwareApplication',
        name: siteConfig.name,
        applicationCategory: 'DeveloperApplication',
        operatingSystem: 'Any',
        url: siteConfig.url,
        description: t('description'),
        offers: { '@type': 'Offer', price: '0', priceCurrency: 'USD' },
        softwareHelp: siteConfig.docsUrl,
      },
    ],
  };

  return (
    <html lang={locale}>
      <body>
        <script
          type="application/ld+json"
          dangerouslySetInnerHTML={{ __html: JSON.stringify(jsonLd) }}
        />
        <NextIntlClientProvider messages={messages}>{children}</NextIntlClientProvider>
      </body>
    </html>
  );
}
