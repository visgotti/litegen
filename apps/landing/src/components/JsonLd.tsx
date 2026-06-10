import { siteConfig } from '@/config/site';

/**
 * Structured data (JSON-LD) for rich search results: a single @graph describing
 * the organization, the website, and the SoftwareApplication. Rendered
 * server-side into the static HTML. All URLs derive from siteConfig.url so they
 * track the canonical domain automatically.
 */
export function JsonLd() {
  const description =
    'Open-source, OpenAI-compatible proxy for AI image and video generation with routing, caching, and cost tracking.';
  const data = {
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
        description,
        publisher: { '@id': `${siteConfig.url}/#organization` },
      },
      {
        '@type': 'SoftwareApplication',
        name: siteConfig.name,
        applicationCategory: 'DeveloperApplication',
        operatingSystem: 'Linux, macOS, Docker',
        description,
        url: siteConfig.url,
        sameAs: [siteConfig.githubUrl],
        offers: { '@type': 'Offer', price: '0', priceCurrency: 'USD' },
        license: siteConfig.licenseUrl,
      },
    ],
  };

  return (
    <script
      type="application/ld+json"
      dangerouslySetInnerHTML={{ __html: JSON.stringify(data) }}
    />
  );
}
