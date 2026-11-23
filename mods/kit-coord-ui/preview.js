import { appHome, checking } from 'herdr';
import * as fs from 'fs/promises';
import { Buffer } from 'buffer';
import { MOD_NAME } from './runtime.js';

const MAX_CACHED = 4;
const MAX_PREVIEW_BYTES = 4 * 1024 * 1024;
const SAFE_KEY = /^[A-Za-z0-9._-]{1,96}$/;
const SAFE_EXT = new Set(['png', 'jpg', 'webp', 'gif']);

let nonce = 0;
const cache = new Map(); // cacheKey -> { relPath, absPath }

function previewDir() {
  const root = appHome();
  if (!root || typeof root !== 'string') throw new Error('App home unavailable');
  return `${root}/data/${MOD_NAME}/preview`;
}

function relDir() {
  return `data/${MOD_NAME}/preview`;
}

async function safeUnlink(absPath) {
  try { await fs.unlink(absPath); } catch { /* ignore already-removed */ }
}

export async function cleanupStalePreviews() {
  if (checking()) return;
  try {
    const dir = previewDir();
    const names = await fs.readdir(dir);
    for (const name of names) {
      if (typeof name === 'string' && name.startsWith('p-')) {
        await safeUnlink(`${dir}/${name}`);
      }
    }
  } catch { /* preview directory may not exist yet */ }
}

export async function stagePreview(info, fetchBase64) {
  const key = String(info?.cache_key || '');
  const ext = String(info?.ext || '').toLowerCase();
  if (!SAFE_KEY.test(key) || !SAFE_EXT.has(ext)) {
    throw new Error('Invalid image preview descriptor');
  }
  const existing = cache.get(key);
  if (existing) {
    // Refresh LRU order without re-writing or re-decoding.
    cache.delete(key);
    cache.set(key, existing);
    return existing.relPath;
  }
  let bytes = null;
  if (info.resolved_path) {
    try { bytes = await fs.readFile(info.resolved_path); } catch { bytes = null; }
  }
  if (!bytes && typeof fetchBase64 === 'function') {
    const b64 = await fetchBase64();
    if (typeof b64 === 'string' && b64.length) bytes = Buffer.from(b64, 'base64');
  }
  if (!bytes || !bytes.length) throw new Error('Image bytes could not be read');
  if (bytes.length > MAX_PREVIEW_BYTES) throw new Error('Image exceeds 4 MiB inline preview limit');

  const dir = previewDir();
  await fs.mkdir(dir, { recursive: true });
  nonce += 1;
  const fileName = `p-${nonce}-${key}.${ext}`;
  const absPath = `${dir}/${fileName}`;
  const relPath = `${relDir()}/${fileName}`;
  await fs.writeFile(absPath, bytes);

  while (cache.size >= MAX_CACHED) {
    const [oldestKey, oldest] = cache.entries().next().value;
    cache.delete(oldestKey);
    await safeUnlink(oldest.absPath);
  }
  cache.set(key, { relPath, absPath });
  return relPath;
}

export async function evictPreview(cacheKey) {
  const entry = cache.get(cacheKey);
  if (!entry) return;
  cache.delete(cacheKey);
  await safeUnlink(entry.absPath);
}

export async function cleanupAllPreviews() {
  const entries = [...cache.values()];
  cache.clear();
  for (const entry of entries) await safeUnlink(entry.absPath);
}
