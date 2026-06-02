'use client';

/**
 * Embedded API reference. Renders the OpenAPI spec (served statically at
 * `/openapi.json`, copied from `sdks/openapi.json`) with Redoc's standalone
 * bundle. The bundle is loaded client-side via next/script so it works with the
 * site's static `output: export` build; `Redoc.init` mounts into our container
 * once the script is ready.
 */
import Script from 'next/script';
import { useRef } from 'react';
import styles from './ApiReference.module.css';

declare global {
  interface Window {
    Redoc?: { init: (specUrl: string, options: object, element: HTMLElement | null) => void };
  }
}

const REDOC_SRC = 'https://cdn.redocly.com/redoc/latest/bundles/redoc.standalone.js';

export function ApiReference() {
  const ref = useRef<HTMLDivElement>(null);

  return (
    <>
      <div id="redoc-container" ref={ref} className={styles.container} />
      <Script
        src={REDOC_SRC}
        strategy="afterInteractive"
        onLoad={() => {
          window.Redoc?.init(
            '/openapi.json',
            {
              hideDownloadButton: false,
              expandResponses: '200,201',
              theme: {
                colors: { primary: { main: '#6b7bff' } },
                typography: { fontFamily: 'inherit' },
              },
            },
            ref.current,
          );
        }}
      />
    </>
  );
}
