import { useState, useEffect } from 'react';
import { getApiKey, setApiKey, clearApiKey } from '../sdk-client';

export default function AuthBar() {
  const [inputKey, setInputKey] = useState('');
  const [savedKey, setSavedKey] = useState<string>('');

  useEffect(() => {
    setSavedKey(getApiKey());
  }, []);

  const handleSave = () => {
    const key = inputKey.trim();
    if (!key) return;
    setApiKey(key);
    setSavedKey(key);
    setInputKey('');
  };

  const handleSignOut = () => {
    clearApiKey();
    setSavedKey('');
    window.location.reload();
  };

  if (savedKey) {
    return (
      <div
        data-testid="auth-bar"
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 12,
          padding: '10px 16px',
          background: '#161b22',
          borderBottom: '1px solid #30363d',
          marginBottom: 16,
        }}
      >
        <span
          data-testid="auth-status"
          style={{
            display: 'inline-flex',
            alignItems: 'center',
            gap: 6,
            padding: '4px 12px',
            borderRadius: 999,
            background: '#1a4731',
            color: '#3fb950',
            fontWeight: 600,
            fontSize: 13,
          }}
        >
          Authenticated ✓
        </span>
        <button
          className="btn btn-secondary"
          onClick={handleSignOut}
          style={{ fontSize: 12, padding: '4px 12px' }}
        >
          Sign out
        </button>
      </div>
    );
  }

  return (
    <div
      data-testid="auth-bar"
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 8,
        padding: '10px 16px',
        background: '#161b22',
        borderBottom: '1px solid #30363d',
        marginBottom: 16,
      }}
    >
      <input
        className="input"
        data-testid="api-key-input"
        type="password"
        placeholder="Enter master API key..."
        value={inputKey}
        onChange={e => setInputKey(e.target.value)}
        onKeyDown={e => e.key === 'Enter' && handleSave()}
        style={{ flex: 1, maxWidth: 400 }}
      />
      <button
        className="btn btn-primary"
        data-testid="save-key-btn"
        onClick={handleSave}
        style={{ whiteSpace: 'nowrap' }}
      >
        Save key
      </button>
    </div>
  );
}
