// Snapshots the 22 Omarchy theme palettes from /usr/share/omarchy/themes/*/colors.toml
// into brand/data/omarchy-themes.json, recording the Omarchy version from pacman -Q omarchy.

import fs from 'node:fs';
import path from 'node:path';
import { execSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const THEMES_DIR = '/usr/share/omarchy/themes';
const OUT_FILE = path.join(ROOT, 'data/omarchy-themes.json');

fs.mkdirSync(path.dirname(OUT_FILE), { recursive: true });

function getOmarchyVersion() {
  try {
    const stdout = execSync('pacman -Q omarchy 2>/dev/null', { encoding: 'utf8' }).trim();
    const match = stdout.match(/omarchy\s+([^\s]+)/);
    if (match) return match[1];
  } catch {}
  return '4.0.4-1'; // Pinned fallback when run in CI or non-Arch
}

export function snapshotThemes() {
  const version = getOmarchyVersion();
  const themes = [];

  if (fs.existsSync(THEMES_DIR)) {
    const entries = fs.readdirSync(THEMES_DIR, { withFileTypes: true });
    for (const ent of entries) {
      if (!ent.isDirectory()) continue;
      const colorsPath = path.join(THEMES_DIR, ent.name, 'colors.toml');
      if (!fs.existsSync(colorsPath)) continue;
      const content = fs.readFileSync(colorsPath, 'utf8');

      const getVal = (key) => {
        const m = content.match(new RegExp(`^${key}\\s*=\\s*"([^"]+)"`, 'm'));
        return m ? m[1] : null;
      };

      themes.push({
        id: ent.name,
        mode: getVal('mode') || 'dark',
        background: getVal('background'),
        foreground: getVal('foreground'),
        accent: getVal('accent'),
      });
    }
  }

  themes.sort((a, b) => a.id.localeCompare(b.id));

  const data = {
    omarchyVersion: version,
    themeCount: themes.length,
    themes,
  };

  fs.writeFileSync(OUT_FILE, JSON.stringify(data, null, 2) + '\n');
  return data;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const data = snapshotThemes();
  console.log(`Wrote ${data.themeCount} themes to ${OUT_FILE} (Omarchy ${data.omarchyVersion})`);
}
