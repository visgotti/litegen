// Password reset links on the Users and Members pages, kept free of React so
// they are unit-tested. No email is sent: an admin hands the link over.

/** The link to hand over: this dashboard's reset page for the token
 *  (`/reset-password/:token`, App.tsx), on the origin the admin is using. The
 *  server's `reset_url` is built from `server.public_base_url`, which can be
 *  another host than this dashboard; it is there for API callers. */
export function resetLinkOf(link: { token: string; reset_url?: string | null }, origin: string): string {
  return `${origin}/reset-password/${encodeURIComponent(link.token)}`;
}
