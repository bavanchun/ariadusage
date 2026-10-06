// Normalizes vendored lobe-icons provider logos into brand/svg/providers/
// with a shared viewBox (0 0 24 24), optical margins, and valid currentColor mono marks.
// Also provides a monogram fallback generator for providers with no official icon.

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { optimize } from 'svgo';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const VENDOR_DIR = path.join(ROOT, 'vendor/lobe-icons/v5.23.0');
const OUT_DIR = path.join(ROOT, 'svg/providers');

fs.mkdirSync(OUT_DIR, { recursive: true });

const clean = (svg, prefix) =>
  optimize(svg, {
    multipass: true,
    plugins: [
      'preset-default',
      { name: 'prefixIds', params: { prefix } },
    ],
  }).data;

const PROVIDERS = [
  { id: 'claude', vendorMono: 'claude.svg', vendorColor: 'claude-color.svg' },
  { id: 'codex', vendorMono: 'codex.svg', vendorColor: 'codex-color.svg' },
  { id: 'antigravity', vendorMono: 'antigravity.svg', vendorColor: 'antigravity-color.svg' },
];

export function normalizeProviderLogos() {
  fs.mkdirSync(OUT_DIR, { recursive: true });
  for (const p of PROVIDERS) {
    // 1. Mono mark: ensure viewBox="0 0 24 24", fill="currentColor"
    const monoRaw = fs.readFileSync(path.join(VENDOR_DIR, p.vendorMono), 'utf8');
    let monoNormalized = monoRaw
      .replace(/width="[^"]*"/, 'width="24"')
      .replace(/height="[^"]*"/, 'height="24"')
      .replace(/style="[^"]*"/, '')
      .replace(/fill="[^"]*"/, 'fill="currentColor"');
    if (!monoNormalized.includes('fill="currentColor"')) {
      monoNormalized = monoNormalized.replace('<svg ', '<svg fill="currentColor" ');
    }
    const monoCleaned = clean(monoNormalized, `provider-${p.id}-mono`);
    fs.writeFileSync(path.join(OUT_DIR, `${p.id}-mono.svg`), monoCleaned);

    // 2. Color mark: ensure viewBox="0 0 24 24", preserve internal colors
    const colorRaw = fs.readFileSync(path.join(VENDOR_DIR, p.vendorColor), 'utf8');
    const colorNormalized = colorRaw
      .replace(/width="[^"]*"/, 'width="24"')
      .replace(/height="[^"]*"/, 'height="24"')
      .replace(/style="[^"]*"/, '');
    const colorCleaned = clean(colorNormalized, `provider-${p.id}-color`);
    fs.writeFileSync(path.join(OUT_DIR, `${p.id}-color.svg`), colorCleaned);
  }
}

/**
 * Generates a clean 24x24 SVG monogram for providers without a dedicated logo.
 * @param {string} name - e.g. "DeepSeek", "Minimax", "OpenRouter"
 * @param {{ fill?: string, bg?: string, mono?: boolean }} options
 */
export function generateMonogram(name, { fill = 'currentColor', bg = '#2D5DA8', mono = false } = {}) {
  const letters = name
    .trim()
    .split(/\s+/)
    .map((w) => w[0].toUpperCase())
    .slice(0, 2)
    .join('');
  const fontSize = letters.length === 1 ? 14 : 11;
  const dy = letters.length === 1 ? 17 : 16;

  if (mono) {
    return `<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none">
  <rect x="1" y="1" width="22" height="22" rx="6" stroke="currentColor" stroke-width="2"/>
  <text x="12" y="${dy}" text-anchor="middle" font-family="system-ui, -apple-system, sans-serif" font-size="${fontSize}" font-weight="700" fill="currentColor">${letters}</text>
</svg>`;
  }

  return `<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none">
  <rect x="1" y="1" width="22" height="22" rx="6" fill="${bg}"/>
  <text x="12" y="${dy}" text-anchor="middle" font-family="system-ui, -apple-system, sans-serif" font-size="${fontSize}" font-weight="700" fill="#FFFFFF">${letters}</text>
</svg>`;
}

// When run directly as a script
if (process.argv[1] === fileURLToPath(import.meta.url)) {
  normalizeProviderLogos();
  // Example fallback generation
  const fallbackMono = generateMonogram('Generic AI', { mono: true });
  const fallbackColor = generateMonogram('Generic AI', { mono: false });
  fs.writeFileSync(path.join(OUT_DIR, 'fallback-mono.svg'), clean(fallbackMono, 'provider-fallback-mono'));
  fs.writeFileSync(path.join(OUT_DIR, 'fallback-color.svg'), clean(fallbackColor, 'provider-fallback-color'));
  console.log(`Normalized ${PROVIDERS.length * 2} provider logos + 2 monogram fallbacks in brand/svg/providers/`);
}
