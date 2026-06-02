/**
 * Pass-through root layout.
 *
 * The real <html>/<body> live in `[locale]/layout.tsx` (localized pages) and
 * `not-found.tsx` (the global 404), each of which renders a *complete* document.
 * This root layout exists only because Next requires every page — including the
 * root `not-found.tsx` and the synthetic `/_not-found` route — to have a root
 * layout. Without it, `next dev` fails to compile `/_not-found`
 * ("not-found.tsx doesn't have a root layout") and every route 500s (the dev
 * error overlay then surfaces a minified "a[d] is not a function").
 *
 * Returning `children` unchanged (no extra <html>/<body>) keeps the static
 * `output: 'export'` build — which prerenders one page per locale via
 * generateStaticParams — byte-for-byte unaffected.
 */
export default function RootLayout({ children }: { children: React.ReactNode }) {
  return children;
}
