// Invitations on the Members and Users pages, kept free of React so they are
// unit-tested. No email is sent for one: the inviter hands its link over.

/** The link to hand over: this dashboard's accept page for the token
 *  (`/invite/:token`, App.tsx), on the origin the inviter is using. The
 *  server's `invite_url` is built from `server.public_base_url`, which can be
 *  another host than this dashboard; it is there for API callers. */
export function inviteLinkOf(invite: { token: string; invite_url?: string | null }, origin: string): string {
  return `${origin}/invite/${encodeURIComponent(invite.token)}`;
}
