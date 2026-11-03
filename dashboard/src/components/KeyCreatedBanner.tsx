/** A newly created API key's secret, shown once (the server never returns it again). */
export default function KeyCreatedBanner({ secret, publicId }: { secret: string; publicId?: string | null }) {
  return (
    <div className="alert alert-success" data-testid="key-created-banner">
      {publicId && (
        <>
          Public key id: <code data-testid="key-public-id" style={{ userSelect: 'all' }}>{publicId}</code>
          <br />
        </>
      )}
      Secret key: <code data-testid="key-secret" style={{ userSelect: 'all' }}>{secret}</code>
      <br />
      <small>Copy the secret now — it won't be shown again.</small>
    </div>
  );
}
