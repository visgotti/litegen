import { useState } from 'react';
import type { ChangeEvent } from 'react';
import { X } from 'lucide-react';
import type { MergedRole, RefEntry } from './types';
import { roleLabel } from './params';
import { MAX_INLINE_TOTAL_BYTES, MAX_UPLOAD_BYTES, formatMiB, inlineBytes, isFetchableUrl, readAsDataUrl } from './ref-images';

interface Props {
  roles: MergedRole[];
  refs: RefEntry[];
  /** The tightest `max_total` among the selected models. */
  maxTotal?: number;
  totalSelected: number;
  onChange: (refs: RefEntry[]) => void;
  disabled?: boolean;
}

let nextId = 0;
const newId = () => `ref-${Date.now().toString(36)}-${(nextId++).toString(36)}`;

/**
 * Reference images, one block per role the selected models accept — `init`
 * for image-to-3D / image-to-image, `view-*` for multiview. Each image is a URL
 * the server fetches or a file sent inline as base64; the server hands it to
 * the provider in whichever form that provider takes, so the picker never has
 * to know. Each model is sent only the roles it declares (see
 * `refsForModel`), so a role's applicability note says who it reaches.
 */
export default function RefImagesField({ roles, refs, maxTotal, totalSelected, onChange, disabled }: Props) {
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [errors, setErrors] = useState<Record<string, string>>({});
  if (roles.length === 0) return null;

  const atTotalCap = maxTotal != null && refs.length >= maxTotal;
  const setError = (role: string, msg: string) => setErrors(prev => ({ ...prev, [role]: msg }));

  const add = (entry: Omit<RefEntry, 'id'>) => {
    onChange([...refs, { ...entry, id: newId() }]);
    setError(entry.role, '');
  };

  const addUrl = (role: string) => {
    const url = (drafts[role] ?? '').trim();
    if (!isFetchableUrl(url)) {
      setError(role, 'Enter an http(s) URL the server can fetch.');
      return;
    }
    add({ role, type: 'url', value: url });
    setDrafts(prev => ({ ...prev, [role]: '' }));
  };

  const addFile = async (role: string, e: ChangeEvent<HTMLInputElement>) => {
    const input = e.target;
    const file = input.files?.[0];
    input.value = ''; // let the same file be picked again after a removal
    if (!file) return;
    if (!file.type.startsWith('image/')) {
      setError(role, `${file.name} is not an image.`);
      return;
    }
    if (file.size > MAX_UPLOAD_BYTES) {
      setError(role, `${file.name} is ${formatMiB(file.size)}; uploads are capped at ${formatMiB(MAX_UPLOAD_BYTES)}. Use a URL for larger images.`);
      return;
    }
    if (inlineBytes(refs) + file.size > MAX_INLINE_TOTAL_BYTES) {
      setError(role, `Uploads in one request are capped at ${formatMiB(MAX_INLINE_TOTAL_BYTES)} in total. Use a URL instead.`);
      return;
    }
    try {
      add({ role, type: 'base64', value: await readAsDataUrl(file), name: file.name });
    } catch {
      setError(role, `${file.name} could not be read.`);
    }
  };

  return (
    <div className="pg-refs" data-testid="pg-refs">
      <div className="pg-refs-title">
        Reference images
        {maxTotal != null && <span className="pg-param-applies" data-testid="pg-refs-total">{refs.length}/{maxTotal}</span>}
      </div>
      {roles.map(r => {
        const tid = `pg-ref-${r.role}`;
        const mine = refs.filter(x => x.role === r.role);
        const canAdd = !disabled && !atTotalCap && mine.length < r.maxCount;
        const applies = r.models.length === totalSelected ? 'all' : r.models.map(m => m.split('/').pop()).join(', ');
        const requiredFor = r.requiredBy.length === totalSelected
          ? 'required'
          : r.requiredBy.length ? `required by ${r.requiredBy.map(m => m.split('/').pop()).join(', ')}` : '';
        return (
          <div key={r.role} className="pg-ref-role" data-testid={tid}>
            <div className="pg-param-head">
              <span className="pg-param-label">
                {roleLabel(r.role)} <code className="pg-ref-key">{r.role}</code>
                {requiredFor && <span className="pg-ref-required" data-testid={`${tid}-required`}>{requiredFor}</span>}
              </span>
              <span className="pg-param-applies" data-testid={`${tid}-applies`}>{applies}</span>
            </div>
            {mine.length > 0 && (
              <ul className="pg-ref-list">
                {mine.map((x, i) => (
                  <li key={x.id} className="pg-ref-item" data-testid={`${tid}-item-${i}`}>
                    <img src={x.value} alt="" className="pg-ref-thumb" loading="lazy" />
                    <span className="pg-ref-name" title={x.type === 'url' ? x.value : x.name}>
                      {x.type === 'url' ? x.value : `${x.name ?? 'upload'} (inline)`}
                    </span>
                    <button type="button" className="pg-ref-remove" aria-label={`Remove ${roleLabel(r.role).toLowerCase()} ${i + 1}`}
                      data-testid={`${tid}-remove-${i}`} disabled={disabled}
                      onClick={() => onChange(refs.filter(y => y.id !== x.id))}>
                      <X size={12} aria-hidden="true" />
                    </button>
                  </li>
                ))}
              </ul>
            )}
            {canAdd && (
              <div className="pg-ref-add">
                <input type="url" className="input" placeholder="https://… image URL" data-testid={`${tid}-url`}
                  aria-label={`${roleLabel(r.role)} URL`}
                  value={drafts[r.role] ?? ''}
                  onChange={e => setDrafts(prev => ({ ...prev, [r.role]: e.target.value }))}
                  onKeyDown={e => { if (e.key === 'Enter') { e.preventDefault(); addUrl(r.role); } }} />
                <button type="button" className="btn btn-secondary" data-testid={`${tid}-add-url`}
                  disabled={!(drafts[r.role] ?? '').trim()} onClick={() => addUrl(r.role)}>Add</button>
                <label className="btn btn-secondary pg-ref-upload">
                  Upload
                  <input type="file" accept="image/*" data-testid={`${tid}-file`} className="pg-ref-file"
                    onChange={e => void addFile(r.role, e)} />
                </label>
              </div>
            )}
            {/* Only where the count says something: a single optional slot
                (each multiview view) needs no "0/1" under it. */}
            {(r.maxCount > 1 || r.minCount > 0) && (
              <span className="pg-param-description" data-testid={`${tid}-count`}>
                {mine.length}/{r.maxCount}
                {r.minCount > 0 && ` · at least ${r.minCount}`}
              </span>
            )}
            {errors[r.role] && <span className="pg-ref-error" role="alert" data-testid={`${tid}-error`}>{errors[r.role]}</span>}
          </div>
        );
      })}
    </div>
  );
}
