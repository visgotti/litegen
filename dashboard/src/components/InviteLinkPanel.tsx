import OneTimeLinkPanel from './OneTimeLinkPanel';

/**
 * A new invitation's link, shown once in the invite modal. litegen sends no
 * email: the inviter copies the link and gives it to the invitee. The server
 * shows the token only in its answer to the invite, so the link can't be
 * shown again; to send a lost one, revoke the invitation and invite anew.
 */
export default function InviteLinkPanel({ email, link, expiresAt, onClose }: {
  email: string;
  link: string;
  expiresAt: string;
  onClose: () => void;
}) {
  return (
    <OneTimeLinkPanel
      tid="invite"
      heading={`Invitation created for ${email}`}
      note={`No email is sent: copy this link and give it to them. It opens until ${new Date(expiresAt).toLocaleString()}, once, and is not shown again.`}
      link={link}
      onClose={onClose}
    />
  );
}
