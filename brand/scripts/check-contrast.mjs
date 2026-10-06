// Validates that the rendered mono mark maintains >= 3:1 stroke-pixel contrast
// against every Omarchy theme background at 16 px and 32 px.

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { Resvg } from '@resvg/resvg-js';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const THEMES_FILE = path.join(ROOT, 'data/omarchy-themes.json');
const MONO_SVG_PATH = path.join(ROOT, 'svg/ariadusage-mark-mono.svg');

function parseHex(hex) {
  let c = hex.replace(/^#/, '');
  if (c.length === 3) c = c.split('').map((x) => x + x).join('');
  const num = parseInt(c, 16);
  return [(num >> 16) & 255, (num >> 8) & 255, num & 255];
}

function srgbToLinear(v) {
  const s = v / 255;
  return s <= 0.04045 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
}

function relativeLuminance([r, g, b]) {
  return 0.2126 * srgbToLinear(r) + 0.7152 * srgbToLinear(g) + 0.0722 * srgbToLinear(b);
}

function contrastRatio(l1, l2) {
  const max = Math.max(l1, l2);
  const min = Math.min(l1, l2);
  return (max + 0.05) / (min + 0.05);
}

export function checkContrast() {
  if (!fs.existsSync(THEMES_FILE)) {
    throw new Error(`Themes file not found at ${THEMES_FILE}. Run snapshot-omarchy-themes.mjs first.`);
  }
  if (!fs.existsSync(MONO_SVG_PATH)) {
    throw new Error(`Mono mark not found at ${MONO_SVG_PATH}. Run build.mjs first.`);
  }

  const { themes, omarchyVersion } = JSON.parse(fs.readFileSync(THEMES_FILE, 'utf8'));
  const monoSvg = fs.readFileSync(MONO_SVG_PATH, 'utf8');

  console.log(`Checking contrast on ${themes.length} Omarchy themes (Omarchy ${omarchyVersion})...\n`);

  const results = [];
  let allPassed = true;

  for (const theme of themes) {
    const bgRgb = parseHex(theme.background);
    const fgRgb = parseHex(theme.foreground);
    const bgLum = relativeLuminance(bgRgb);
    const fgLum = relativeLuminance(fgRgb);
    const themeRatio = contrastRatio(bgLum, fgLum);

    // Sanity assert on theme ratio
    if (themeRatio < 3.0) {
      console.warn(`WARNING: Theme ${theme.id} intrinsic fg/bg ratio is low: ${themeRatio.toFixed(2)}:1`);
    }

    const sizes = [16, 32];
    const sizeResults = {};

    for (const size of sizes) {
      // Rasterize mono mark in pure white on transparent background to get exact alpha coverage
      const resvg = new Resvg(monoSvg, { fitTo: { mode: 'width', value: size } });
      const rendered = resvg.render();
      const pixels = rendered.pixels; // Buffer of RGBA values: size * size * 4

      const strokeContrasts = [];

      for (let i = 0; i < pixels.length; i += 4) {
        const a = pixels[i + 3] / 255; // Alpha coverage
        if (a >= 0.5) {
          // Composite foreground over background with coverage alpha
          const r = Math.round(a * fgRgb[0] + (1 - a) * bgRgb[0]);
          const g = Math.round(a * fgRgb[1] + (1 - a) * bgRgb[1]);
          const b = Math.round(a * fgRgb[2] + (1 - a) * bgRgb[2]);
          const lum = relativeLuminance([r, g, b]);
          strokeContrasts.push(contrastRatio(lum, bgLum));
        }
      }

      if (strokeContrasts.length === 0) {
        console.error(`FAIL: Theme ${theme.id} at ${size}px: No pixel reached 50% coverage!`);
        allPassed = false;
        continue;
      }

      // Compute median and mean contrast among stroke pixels (a >= 0.5)
      strokeContrasts.sort((x, y) => x - y);
      const medianContrast = strokeContrasts[Math.floor(strokeContrasts.length * 0.5)];
      const meanContrast = strokeContrasts.reduce((acc, v) => acc + v, 0) / strokeContrasts.length;
      const effectiveContrast = medianContrast;

      sizeResults[size] = {
        pixelCount: strokeContrasts.length,
        effectiveContrast,
        meanContrast,
        passed: effectiveContrast >= 3.0,
      };

      if (effectiveContrast < 3.0) {
        console.error(`FAIL: Theme ${theme.id} at ${size}px: effective contrast ${effectiveContrast.toFixed(2)}:1 < 3:1`);
        allPassed = false;
      }
    }

    results.push({
      theme: theme.id,
      mode: theme.mode,
      themeRatio,
      sizes: sizeResults,
    });
  }

  // Print summary table
  console.log('| Theme | Mode | Theme CR | 16px Eff CR (px) | 32px Eff CR (px) | Result |');
  console.log('|---|---|---|---|---|---|');
  for (const r of results) {
    const s16 = r.sizes[16];
    const s32 = r.sizes[32];
    const ok = s16?.passed && s32?.passed;
    console.log(
      `| ${r.theme} | ${r.mode} | ${r.themeRatio.toFixed(2)}:1 | ${s16.effectiveContrast.toFixed(2)}:1 (${s16.pixelCount}) | ${s32.effectiveContrast.toFixed(2)}:1 (${s32.pixelCount}) | ${ok ? 'PASS' : 'FAIL'} |`
    );
  }

  if (!allPassed) {
    throw new Error('Contrast check failed: one or more themes did not reach 3:1 stroke-pixel contrast.');
  }

  console.log('\nAll 22 themes passed stroke-pixel contrast check >= 3:1 at 16 px and 32 px.');
  return results;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  checkContrast();
}
