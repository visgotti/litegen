import { describe, it, expect } from 'vitest';
import { storageBody, type StorageForm } from './storage';

const FORM: StorageForm = {
  bucket: ' media ',
  region: '',
  endpoint: '',
  publicUrl: '',
  prefix: '',
  accessKeyId: '',
  secret: '',
  credentialMode: 'access_key',
  roleArn: '',
  fallback: true,
};

describe('storageBody', () => {
  it('needs a bucket', () => {
    expect(storageBody({ ...FORM, bucket: '  ' }).error).toMatch(/bucket/i);
  });

  it('sends a key pair only when both halves are given', () => {
    expect(storageBody(FORM).body).toEqual({ bucket_name: 'media', region: 'us-east-1', credential_mode: 'access_key', fallback_to_litegen_store: true });
    expect(storageBody({ ...FORM, accessKeyId: 'AKIA1', secret: 's3cret' }).body).toMatchObject({ access_key_id: 'AKIA1', secret_access_key: 's3cret' });
    expect(storageBody({ ...FORM, accessKeyId: 'AKIA1' }).body?.access_key_id).toBeUndefined();
  });

  it('sends a role, and no keys, in role mode', () => {
    const body = storageBody({ ...FORM, credentialMode: 'assume_role', roleArn: ' arn:aws:iam::123456789012:role/litegen ', accessKeyId: 'AKIA1', secret: 's' }).body!;
    expect(body).toMatchObject({ credential_mode: 'assume_role', role_arn: 'arn:aws:iam::123456789012:role/litegen' });
    expect(body.access_key_id).toBeUndefined();
    expect(body.secret_access_key).toBeUndefined();
  });

  it('refuses role mode without a role ARN', () => {
    expect(storageBody({ ...FORM, credentialMode: 'assume_role' }).error).toMatch(/role ARN/);
    expect(storageBody({ ...FORM, credentialMode: 'assume_role', roleArn: 'my-bucket' }).error).toMatch(/role ARN/);
  });

  it('says whether litegen\'s store may stand in', () => {
    expect(storageBody({ ...FORM, fallback: false }).body?.fallback_to_litegen_store).toBe(false);
  });
});
