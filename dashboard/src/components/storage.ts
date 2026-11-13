// An app's own bucket (docs/storage-access.md): the form behind the
// Organization page's storage panel, kept free of React so it is unit-tested.
import type { PutAppStorageRequest } from '@litegen/sdk';

/** The storage form as typed. */
export interface StorageForm {
  bucket: string;
  region: string;
  endpoint: string;
  publicUrl: string;
  prefix: string;
  accessKeyId: string;
  secret: string;
  /** How litegen reaches the bucket: a key pair, or a cross-account IAM role. */
  credentialMode: 'access_key' | 'assume_role';
  roleArn: string;
  /** Whether litegen's own store keeps the app's files while the bucket can't be reached. */
  fallback: boolean;
}

/** The request body for the form, or why it can't be sent. */
export function storageBody(form: StorageForm): { body?: PutAppStorageRequest; error?: string } {
  const bucket = form.bucket.trim();
  if (!bucket) return { error: 'A bucket name is required.' };
  const optional = (v: string) => v.trim() || undefined;
  const body: PutAppStorageRequest = {
    bucket_name: bucket,
    region: form.region.trim() || 'us-east-1',
    endpoint_url: optional(form.endpoint),
    custom_public_url: optional(form.publicUrl),
    path_prefix: optional(form.prefix),
    credential_mode: form.credentialMode,
    fallback_to_litegen_store: form.fallback,
  };
  for (const k of ['endpoint_url', 'custom_public_url', 'path_prefix'] as const) {
    if (body[k] === undefined) delete body[k];
  }
  if (form.credentialMode === 'assume_role') {
    const arn = form.roleArn.trim();
    if (!/^arn:aws[a-z-]*:iam::\d{12}:role\/.+/.test(arn)) {
      return { error: 'A role ARN is required: arn:aws:iam::{12-digit account}:role/{name}.' };
    }
    body.role_arn = arn;
  } else if (form.accessKeyId.trim() && form.secret.trim()) {
    body.access_key_id = form.accessKeyId.trim();
    body.secret_access_key = form.secret.trim();
  }
  return { body };
}
