// Generates every AriadUsage brand asset from one geometry definition:
// Liquid Glass app icons (light, dark, macOS template), Icon Composer layers, a single-color mark,
// the outlined wordmark, lockups and a preview board.
// Run with `pnpm build`; outputs land in ./svg, ./png, ./icon-composer and ./preview.png.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { Resvg } from '@resvg/resvg-js';
import opentype from 'opentype.js';
import { optimize } from 'svgo';
import {
  BEAD_R, COLOR, CX, CY, END, NOTCH, R_ARC, R_DIAL, R_INNER, RING, START, THREAD, THREAD_WIDTH,
  notchPath, svgDoc,
} from './src/geometry.mjs';
import { glassIcon } from './src/glass-icon.mjs';
import { generateMonogram, normalizeProviderLogos } from './scripts/normalize-provider-logos.mjs';

const ROOT = path.dirname(fileURLToPath(import.meta.url));
const OUT = {
  svg: path.join(ROOT, 'svg'),
  png: path.join(ROOT, 'png'),
  layers: path.join(ROOT, 'icon-composer'),
};

/** Single-color mono mark: high-contrast gauge arc with datum ring, quota bead & reset notch. */
export function markMono(fill = 'currentColor') {
  return svgDoc(256, 256, `
  <g fill="${fill}">
    <path d="${THREAD}" fill="none" stroke="${fill}" stroke-width="24" stroke-linecap="round"/>
    <circle cx="${START.cx}" cy="${START.cy}" r="18" fill="none" stroke="${fill}" stroke-width="12"/>
    <circle cx="${END.cx}" cy="${END.cy}" r="20" fill="${fill}"/>
    <rect x="${CX - 5}" y="24" width="10" height="22" rx="3" fill="${fill}"/>
  </g>`);
}

/** Flat layers for Apple Icon Composer, which applies native Liquid Glass material. */
function iconComposerLayers() {
  const k = (1024 * 0.74) / 256;
  const off = 512 - 128 * k;
  const layer = (body) =>
    svgDoc(1024, 1024, `<g transform="translate(${off.toFixed(2)} ${off.toFixed(2)}) scale(${k.toFixed(4)})">${body}</g>`);
  const ringHole = RING.r - RING.stroke / 2 + 0.4;

  return {
    '0-background.svg': svgDoc(1024, 1024, `
      <defs><linearGradient id="bg" x1=".1" y1="0" x2=".9" y2="1">
        <stop offset="0" stop-color="${COLOR.bg[0]}"/><stop offset=".55" stop-color="${COLOR.bg[1]}"/><stop offset="1" stop-color="${COLOR.bg[2]}"/>
      </linearGradient></defs>
      <rect width="1024" height="1024" fill="url(#bg)"/>`),
    '1-dial-plate.svg': layer(`
      <circle cx="${CX}" cy="${CY}" r="${R_DIAL}" fill="#FFFFFF"/>
      <path d="${notchPath()}" fill="#FFFFFF"/>`),
    '2-inner-dial.svg': layer(`<circle cx="${CX}" cy="${CY}" r="${R_INNER}" fill="#F0F4FA"/>`),
    '3-thread.svg': layer(`
      <mask id="hole" maskUnits="userSpaceOnUse" x="0" y="0" width="256" height="256">
        <rect width="256" height="256" fill="#fff"/>
        <circle cx="${START.cx}" cy="${START.cy}" r="${ringHole}" fill="#000"/>
      </mask>
      <path d="${THREAD}" stroke="${COLOR.gold[1]}" stroke-width="${THREAD_WIDTH}" stroke-linecap="round" mask="url(#hole)"/>
      <circle cx="${START.cx}" cy="${START.cy}" r="${RING.r}" stroke="${COLOR.gold[1]}" stroke-width="${RING.stroke}"/>
      <circle cx="${END.cx}" cy="${END.cy}" r="${BEAD_R}" fill="${COLOR.gold[1]}"/>`),
  };
}

// Wordmark: outlined glyph paths so no font install is needed wherever the logo is used.
const fontBuffer = fs.readFileSync(
  path.join(ROOT, 'node_modules/@fontsource/plus-jakarta-sans/files/plus-jakarta-sans-latin-700-normal.woff'),
);
const FONT = opentype.parse(
  fontBuffer.buffer.slice(fontBuffer.byteOffset, fontBuffer.byteOffset + fontBuffer.byteLength),
);
const TRACKING = -0.02; // em

function wordmark(text, size) {
  const glyphs = [...text].map((ch) => FONT.charToGlyph(ch));
  const scale = size / FONT.unitsPerEm;
  let x = 0;
  let d = '';
  glyphs.forEach((g, i) => {
    d += g.getPath(x, 0, size).toPathData(2);
    const kern = i < glyphs.length - 1 ? FONT.getKerningValue(g, glyphs[i + 1]) : 0;
    x += (g.advanceWidth + kern) * scale + (i < glyphs.length - 1 ? TRACKING * size : 0);
  });
  return { d, width: x, capHeight: FONT.tables.os2.sCapHeight * scale };
}

const WORD_SIZE = 150;

function wordmarkOnly(textColor) {
  const wm = wordmark('AriadUsage', WORD_SIZE);
  const h = Math.ceil(wm.capHeight + 48);
  return svgDoc(Math.ceil(wm.width + 8), h, `<path transform="translate(2 ${(h - (h - wm.capHeight) / 2).toFixed(2)})" d="${wm.d}" fill="${textColor}"/>`);
}

/** Horizontal lockup: the glass app icon (256px) beside the wordmark, optically centred. */
function lockup(textColor, appearance = 'light') {
  const wm = wordmark('AriadUsage', WORD_SIZE);
  const gap = 56;
  const baseline = 128 + wm.capHeight / 2;
  const width = Math.ceil(256 + gap + wm.width + 8);
  const icon = glassIcon({ appearance }).replace(/^<svg[^>]*>/, '<svg x="0" y="0" width="256" height="256" viewBox="0 0 1024 1024" fill="none">');
  return svgDoc(width, 256, `${icon}<path transform="translate(${256 + gap} ${baseline.toFixed(2)})" d="${wm.d}" fill="${textColor}"/>`);
}

// prefixIds keeps ids unique when several logos are inlined into one HTML page.
const clean = (svg, name) =>
  optimize(svg, { multipass: true, plugins: ['preset-default', { name: 'prefixIds', params: { prefix: name.replace('.svg', '') } }] }).data;
const render = (svg, width) => new Resvg(svg, { fitTo: { mode: 'width', value: width } }).render().asPng();

for (const dir of Object.values(OUT)) {
  fs.rmSync(dir, { recursive: true, force: true });
  fs.mkdirSync(dir, { recursive: true });
}

const assets = {
  'ariadusage-app-icon.svg': glassIcon(),
  'ariadusage-app-icon-dark.svg': glassIcon({ appearance: 'dark' }),
  'ariadusage-app-icon-macos.svg': glassIcon({ inset: 100, outerShadow: true }),
  'ariadusage-mark-mono.svg': markMono(),
  'ariadusage-wordmark.svg': wordmarkOnly(COLOR.ink),
  'ariadusage-lockup.svg': lockup(COLOR.ink, 'light'),
  'ariadusage-lockup-dark.svg': lockup(COLOR.paper, 'dark'),
  'favicon.svg': glassIcon(),
};

for (const [name, svg] of Object.entries(assets)) {
  fs.writeFileSync(path.join(OUT.svg, name), clean(svg, name));
}

// Normalize provider logos and write fallbacks
normalizeProviderLogos();
const fallbackMono = generateMonogram('Generic AI', { mono: true });
const fallbackColor = generateMonogram('Generic AI', { mono: false });
fs.writeFileSync(path.join(OUT.svg, 'providers/fallback-mono.svg'), clean(fallbackMono, 'provider-fallback-mono'));
fs.writeFileSync(path.join(OUT.svg, 'providers/fallback-color.svg'), clean(fallbackColor, 'provider-fallback-color'));

for (const [name, svg] of Object.entries(iconComposerLayers())) {
  fs.writeFileSync(path.join(OUT.layers, name), clean(svg, name));
}

const pngs = [
  ['ariadusage-app-icon.svg', [1024, 512, 256, 128, 64, 32, 16]],
  ['ariadusage-app-icon-dark.svg', [1024, 512]],
  ['ariadusage-app-icon-macos.svg', [1024]],
  ['ariadusage-lockup.svg', [1200]],
  ['ariadusage-lockup-dark.svg', [1200]],
];

for (const [name, sizes] of pngs) {
  for (const w of sizes) {
    fs.writeFileSync(path.join(OUT.png, `${name.replace('.svg', '')}-${w}.png`), render(assets[name], w));
  }
}

// Preview board: true-pixel small sizes are upscaled with nearest-neighbour so blur is honest.
const b64 = (buf) => `data:image/png;base64,${buf.toString('base64')}`;
const tile = (x, y, w, h, fill) => `<rect x="${x}" y="${y}" width="${w}" height="${h}" rx="24" fill="${fill}"/>`;
const img = (x, y, w, h, buf, pixelated = false) =>
  `<image x="${x}" y="${y}" width="${w}" height="${h}" href="${b64(buf)}"${pixelated ? ' style="image-rendering:pixelated"' : ''}/>`;

const lockW = Number(assets['ariadusage-lockup.svg'].match(/width="(\d+)"/)[1]);
const lockH = Math.round((256 / lockW) * 1000);

const board = svgDoc(1600, 1240, `
  <rect width="1600" height="1240" fill="#FFFFFF"/>
  ${tile(40, 40, 560, 560, COLOR.paper)}${img(60, 60, 520, 520, render(assets['ariadusage-app-icon-macos.svg'], 1040))}
  ${tile(640, 40, 560, 560, COLOR.night)}${img(700, 100, 440, 440, render(assets['ariadusage-app-icon-dark.svg'], 880))}
  ${tile(1240, 40, 320, 270, COLOR.paper)}
  ${img(1265, 70, 128, 128, render(assets['ariadusage-app-icon.svg'], 64))}
  ${img(1410, 70, 64, 64, render(assets['ariadusage-app-icon.svg'], 32), true)}
  ${img(1490, 70, 32, 32, render(assets['ariadusage-app-icon.svg'], 16), true)}
  ${tile(1240, 330, 320, 270, '#FFFFFF')}
  <g transform="translate(1300 345) scale(0.78)" color="${COLOR.mono}">${markMono(COLOR.mono).replace(/^<svg[^>]*>|<\/svg>$/g, '')}</g>
  ${tile(40, 640, 1520, 280, COLOR.paper)}${img(300, 780 - lockH / 2, 1000, lockH, render(assets['ariadusage-lockup.svg'], 1000))}
  ${tile(40, 940, 1520, 280, COLOR.night)}${img(300, 1080 - lockH / 2, 1000, lockH, render(assets['ariadusage-lockup-dark.svg'], 1000))}`);

fs.writeFileSync(path.join(ROOT, 'preview.png'), new Resvg(board).render().asPng());

console.log(`Wrote ${Object.keys(assets).length} SVGs, 4 Icon Composer layers, PNG renders and preview.png`);
