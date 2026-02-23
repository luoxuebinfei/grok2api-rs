const PUBLIC_KEY_STORAGE = 'grok2api_public_key';
const PUBLIC_KEY_ENC_PREFIX = 'enc:v1:';
const PUBLIC_KEY_XOR_PREFIX = 'enc:xor:';
const PUBLIC_KEY_SECRET = 'grok2api-public-key';
let cachedPublicKey = null;

const _textEncoder = new TextEncoder();
const _textDecoder = new TextDecoder();

function _toBase64(bytes) {
  let binary = '';
  bytes.forEach(b => { binary += String.fromCharCode(b); });
  return btoa(binary);
}

function _fromBase64(base64) {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

function _xorCipher(bytes, keyBytes) {
  const out = new Uint8Array(bytes.length);
  for (let i = 0; i < bytes.length; i++) {
    out[i] = bytes[i] ^ keyBytes[i % keyBytes.length];
  }
  return out;
}

function _xorEncrypt(plain) {
  const data = _textEncoder.encode(plain);
  const key = _textEncoder.encode(PUBLIC_KEY_SECRET);
  const cipher = _xorCipher(data, key);
  return `${PUBLIC_KEY_XOR_PREFIX}${_toBase64(cipher)}`;
}

function _xorDecrypt(stored) {
  if (!stored.startsWith(PUBLIC_KEY_XOR_PREFIX)) return stored;
  const payload = stored.slice(PUBLIC_KEY_XOR_PREFIX.length);
  const data = _fromBase64(payload);
  const key = _textEncoder.encode(PUBLIC_KEY_SECRET);
  const plain = _xorCipher(data, key);
  return _textDecoder.decode(plain);
}

async function _derivePublicKey(salt) {
  const keyMaterial = await crypto.subtle.importKey(
    'raw', _textEncoder.encode(PUBLIC_KEY_SECRET), 'PBKDF2', false, ['deriveKey']
  );
  return crypto.subtle.deriveKey(
    { name: 'PBKDF2', salt, iterations: 100000, hash: 'SHA-256' },
    keyMaterial,
    { name: 'AES-GCM', length: 256 },
    false,
    ['encrypt', 'decrypt']
  );
}

async function encryptPublicKey(plain) {
  if (!plain) return '';
  if (!crypto?.subtle) return _xorEncrypt(plain);
  const salt = crypto.getRandomValues(new Uint8Array(16));
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const key = await _derivePublicKey(salt);
  const cipher = await crypto.subtle.encrypt(
    { name: 'AES-GCM', iv }, key, _textEncoder.encode(plain)
  );
  return `${PUBLIC_KEY_ENC_PREFIX}${_toBase64(salt)}:${_toBase64(iv)}:${_toBase64(new Uint8Array(cipher))}`;
}

async function decryptPublicKey(stored) {
  if (!stored) return '';
  if (stored.startsWith(PUBLIC_KEY_XOR_PREFIX)) return _xorDecrypt(stored);
  if (!stored.startsWith(PUBLIC_KEY_ENC_PREFIX)) return stored;
  if (!crypto?.subtle) return '';
  const parts = stored.split(':');
  if (parts.length !== 5) return '';
  const salt = _fromBase64(parts[2]);
  const iv = _fromBase64(parts[3]);
  const cipher = _fromBase64(parts[4]);
  const key = await _derivePublicKey(salt);
  const plain = await crypto.subtle.decrypt({ name: 'AES-GCM', iv }, key, cipher);
  return _textDecoder.decode(plain);
}

async function getStoredPublicKey() {
  const stored = localStorage.getItem(PUBLIC_KEY_STORAGE) || '';
  if (!stored) return '';
  try {
    return await decryptPublicKey(stored);
  } catch (e) {
    clearPublicKey();
    return '';
  }
}

async function storePublicKey(publicKey) {
  if (!publicKey) { clearPublicKey(); return; }
  const encrypted = await encryptPublicKey(publicKey);
  localStorage.setItem(PUBLIC_KEY_STORAGE, encrypted || '');
}

function clearPublicKey() {
  localStorage.removeItem(PUBLIC_KEY_STORAGE);
  cachedPublicKey = null;
}

async function ensurePublicKey() {
  if (cachedPublicKey !== null) return cachedPublicKey;
  const key = await getStoredPublicKey();
  if (!key) {
    try {
      const res = await fetch('/v1/public/verify', { method: 'GET' });
      if (res.ok) { cachedPublicKey = ''; return cachedPublicKey; }
    } catch (e) { /* ignore */ }
    return null;
  }
  try {
    const res = await fetch('/v1/public/verify', {
      method: 'GET', headers: { 'Authorization': `Bearer ${key}` }
    });
    if (!res.ok) throw new Error('Unauthorized');
    cachedPublicKey = `Bearer ${key}`;
    return cachedPublicKey;
  } catch (e) {
    clearPublicKey();
    return null;
  }
}

function buildPublicAuthHeaders(authKey) {
  return authKey ? { 'Authorization': authKey } : {};
}

function publicLogout() {
  clearPublicKey();
  window.location.href = '/login';
}
