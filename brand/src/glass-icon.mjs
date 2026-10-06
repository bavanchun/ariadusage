// Liquid Glass rendering of the AriadUsage app icon: frosted circular dial lens that refracts
// what lies behind it, specular rims, layered shadows and a golden gauge arc thread.
import {
  COLOR, CX, CY, END, NOTCH, R_ARC, R_DIAL, R_INNER, RING, SPINE_PATH, START, TAPERED_THREAD_GLASS,
  notchPath, squircle, svgDoc,
} from './geometry.mjs';

const APPEARANCE = {
  light: {
    bg: COLOR.bg,
    bloom: 0.4,
    glassA: [0.38, 0.12],
    glassB: [0.46, 0.14],
    shadow: 0.42,
    shadowColor: COLOR.shadow,
  },
  dark: {
    bg: COLOR.bgDark,
    bloom: 0.22,
    glassA: [0.24, 0.08],
    glassB: [0.32, 0.1],
    shadow: 0.58,
    shadowColor: '#000000',
  },
};

const SIZE = 1024;
const MARK_SCALE = 0.74; // mark width relative to squircle

/**
 * @param {{ appearance?: 'light' | 'dark', inset?: number, outerShadow?: boolean }} options
 */
export function glassIcon({ appearance = 'light', inset = 0, outerShadow = false } = {}) {
  const a = APPEARANCE[appearance];
  const half = SIZE / 2 - inset;
  const k = (half * 2 * MARK_SCALE) / 256;
  const off = SIZE / 2 - 128 * k;
  const mark = `translate(${off.toFixed(2)} ${off.toFixed(2)}) scale(${k.toFixed(4)})`;
  const box = `x="${SIZE / 2 - half}" y="${SIZE / 2 - half}" width="${half * 2}" height="${half * 2}"`;
  const ringHole = RING.holeR || (RING.r - RING.stroke / 2 + 0.4);

  const defs = `
  <defs>
    <linearGradient id="bg" x1=".1" y1="0" x2=".9" y2="1">
      <stop offset="0" stop-color="${a.bg[0]}"/><stop offset=".55" stop-color="${a.bg[1]}"/><stop offset="1" stop-color="${a.bg[2]}"/>
    </linearGradient>
    <radialGradient id="bloom" cx=".3" cy=".2" r=".75">
      <stop offset="0" stop-color="#fff" stop-opacity="${a.bloom}"/><stop offset="1" stop-color="#fff" stop-opacity="0"/>
    </radialGradient>
    <linearGradient id="iconRim" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#fff" stop-opacity=".55"/><stop offset=".5" stop-color="#fff" stop-opacity=".08"/><stop offset="1" stop-color="#fff" stop-opacity=".2"/>
    </linearGradient>
    <linearGradient id="glassA" x1="0" y1="0" x2=".7" y2="1">
      <stop offset="0" stop-color="${COLOR.glass}" stop-opacity="${a.glassA[0]}"/><stop offset="1" stop-color="${COLOR.glass}" stop-opacity="${a.glassA[1]}"/>
    </linearGradient>
    <linearGradient id="glassB" x1="0" y1="0" x2=".7" y2="1">
      <stop offset="0" stop-color="${COLOR.glass}" stop-opacity="${a.glassB[0]}"/><stop offset="1" stop-color="${COLOR.glass}" stop-opacity="${a.glassB[1]}"/>
    </linearGradient>
    <linearGradient id="rim" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="#fff" stop-opacity=".95"/><stop offset=".45" stop-color="#fff" stop-opacity=".15"/>
      <stop offset=".8" stop-color="#fff" stop-opacity=".06"/><stop offset="1" stop-color="#fff" stop-opacity=".45"/>
    </linearGradient>
    <linearGradient id="edge" x1="0" y1="0" x2="1" y2="1">
      <stop offset=".55" stop-color="#000" stop-opacity="0"/><stop offset="1" stop-color="#000" stop-opacity=".25"/>
    </linearGradient>
    <linearGradient id="gold" gradientUnits="userSpaceOnUse" x1="50" y1="50" x2="210" y2="210">
      <stop offset="0" stop-color="${COLOR.gold[0]}"/><stop offset=".5" stop-color="${COLOR.gold[1]}"/><stop offset="1" stop-color="${COLOR.gold[2]}"/>
    </linearGradient>
    <radialGradient id="tipGleam" cx=".4" cy=".4" r=".6">
      <stop offset="0" stop-color="#FFFFFF"/><stop offset=".6" stop-color="${COLOR.gold[0]}"/><stop offset="1" stop-color="${COLOR.gold[1]}" stop-opacity="0"/>
    </radialGradient>
    <clipPath id="icon"><path d="${squircle(SIZE / 2, SIZE / 2, half)}"/></clipPath>
    <clipPath id="dialClip"><circle cx="${CX}" cy="${CY}" r="${R_DIAL}"/></clipPath>
    <clipPath id="dialTrans"><circle transform="${mark}" cx="${CX}" cy="${CY}" r="${R_DIAL}"/></clipPath>
    <mask id="ringHole" maskUnits="userSpaceOnUse" x="0" y="0" width="256" height="256">
      <rect width="256" height="256" fill="#fff"/>
      <circle cx="${START.cx}" cy="${START.cy}" r="${ringHole}" fill="#000"/>
    </mask>
    <mask id="tube" maskUnits="userSpaceOnUse" x="0" y="0" width="256" height="256">
      <path d="${TAPERED_THREAD_GLASS}" fill="#fff"/>
      <circle cx="${START.cx}" cy="${START.cy}" r="${RING.r}" stroke="#fff" stroke-width="${RING.stroke}"/>
      <circle cx="${START.cx}" cy="${START.cy}" r="${ringHole}" fill="#000"/>
    </mask>
    <filter id="drop" x="-30%" y="-30%" width="160%" height="170%">
      <feGaussianBlur stdDeviation="8"/><feOffset dy="10"/>
      <feComponentTransfer><feFuncA type="linear" slope="${a.shadow}"/></feComponentTransfer>
    </filter>
    <filter id="frost" x="0" y="0" width="100%" height="100%"><feGaussianBlur stdDeviation="${(7 * k).toFixed(1)}"/></filter>
    <filter id="glow" x="-20%" y="-20%" width="140%" height="140%"><feGaussianBlur stdDeviation="5"/></filter>
    <filter id="contour" x="-10%" y="-10%" width="120%" height="120%"><feGaussianBlur stdDeviation="1.2"/></filter>
    <filter id="lift" x="-20%" y="-20%" width="140%" height="150%"><feGaussianBlur stdDeviation="3"/><feOffset dy="4"/></filter>
    <filter id="outer" x="-15%" y="-15%" width="130%" height="135%">
      <feDropShadow dx="0" dy="12" stdDeviation="16" flood-color="#000" flood-opacity=".3"/>
    </filter>
  </defs>`;

  const background = `
  <g clip-path="url(#icon)">
    <rect ${box} fill="url(#bg)"/>
    <rect ${box} fill="url(#bloom)"/>
  </g>`;

  const thread = `
  <g mask="url(#ringHole)">
    <path d="${TAPERED_THREAD_GLASS}" fill="${a.shadowColor}" fill-opacity=".45" filter="url(#lift)"/>
    <path d="${TAPERED_THREAD_GLASS}" fill="${COLOR.goldGlow}" fill-opacity=".6" filter="url(#glow)"/>
    <path d="${TAPERED_THREAD_GLASS}" stroke="${a.shadowColor}" stroke-opacity=".42" stroke-width="3" filter="url(#contour)"/>
    <path d="${TAPERED_THREAD_GLASS}" fill="url(#gold)"/>
  </g>
  <circle cx="${START.cx}" cy="${START.cy}" r="${RING.r}" stroke="${a.shadowColor}" stroke-opacity=".42" stroke-width="${RING.stroke + 4}" filter="url(#contour)"/>
  <circle cx="${START.cx}" cy="${START.cy}" r="${RING.r}" stroke="url(#gold)" stroke-width="${RING.stroke}"/>
  <g mask="url(#tube)">
    <path d="${SPINE_PATH}" fill="none" stroke="#fff" stroke-opacity=".8" stroke-width="4.2" stroke-linecap="round" transform="translate(-1.8 -2.2)"/>
    <path d="${SPINE_PATH}" fill="none" stroke="#6E3E04" stroke-opacity=".2" stroke-width="4.2" stroke-linecap="round" transform="translate(1.8 2.5)"/>
    <circle cx="${START.cx}" cy="${START.cy}" r="${RING.r}" stroke="#fff" stroke-opacity=".75" stroke-width="2.8" transform="translate(-1.4 -1.8)"/>
  </g>
  <circle cx="${END.cx}" cy="${END.cy}" r="5.5" fill="url(#tipGleam)"/>
  <circle cx="${END.cx - 0.7}" cy="${END.cy - 0.7}" r="2" fill="#FFFFFF" opacity=".95"/>`;

  const body = `${defs}
  <g${outerShadow ? ' filter="url(#outer)"' : ''}>
    ${background}
    <path d="${squircle(SIZE / 2, SIZE / 2, half - 1.5)}" stroke="url(#iconRim)" stroke-width="3"/>
  </g>
  <g transform="${mark}">
    <circle cx="${CX}" cy="${CY}" r="${R_DIAL}" fill="${a.shadowColor}" filter="url(#drop)"/>
  </g>
  <g clip-path="url(#dialTrans)">
    <g filter="url(#frost)">
      <rect ${box} fill="url(#bg)"/>
      <rect ${box} fill="url(#bloom)"/>
    </g>
  </g>
  <g transform="${mark}">
    <circle cx="${CX}" cy="${CY}" r="${R_DIAL}" fill="url(#glassA)"/>
    <circle cx="${CX}" cy="${CY}" r="${R_DIAL}" stroke="url(#rim)" stroke-width="1.8"/>
    <circle cx="${CX}" cy="${CY}" r="${R_DIAL}" stroke="url(#edge)" stroke-width="3.2" transform="translate(-.8 -.8)" clip-path="url(#dialClip)"/>
    <!-- Recessed inner dial plateau -->
    <circle cx="${CX}" cy="${CY}" r="${R_INNER}" fill="url(#glassB)"/>
    <circle cx="${CX}" cy="${CY}" r="${R_INNER}" stroke="url(#rim)" stroke-width="1.2" stroke-opacity=".7"/>
    <!-- Top reset calibration notch -->
    <path d="${notchPath()}" fill="${a.shadowColor}" opacity=".3" transform="translate(0 1)"/>
    <path d="${notchPath()}" fill="#FFFFFF" opacity=".75"/>
    <path d="${notchPath()}" stroke="url(#rim)" stroke-width="0.8"/>
    ${thread}
  </g>`;
  return svgDoc(SIZE, SIZE, body);
}
