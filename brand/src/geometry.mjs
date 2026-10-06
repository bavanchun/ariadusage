// Shared AriadUsage geometry and color tokens on a 256 grid.
// Concept 1: The Gauge Arc — 270-degree golden thread gauge arc on a circular frosted glass dial.

export const COLOR = {
  // App icon background (light / default appearance), top-left to bottom-right.
  bg: ['#7AA5EB', '#2D5DA8', '#0E224A'],
  // Dark appearance background: deep indigo sapphire to nocturnal obsidian.
  bgDark: ['#294375', '#142546', '#060E1E'],
  // Frosted glass tint (ice-crystal clear with faint cool undertone)
  glass: '#F0F6FF',
  // Ariadne's golden thread: specular highlight, rich radiant gold, deep amber body
  gold: ['#FFF2C8', '#F5C252', '#D68822'],
  goldGlow: '#FFD478',
  shadow: '#051020',
  ink: '#162033', // wordmark on light
  paper: '#F4F6FA', // light surface background
  night: '#12161F', // dark surface background
  mono: '#2455A8', // single-color mark default
};

// Geometry on 256x256 grid
export const CX = 128;
export const CY = 128;
export const R_DIAL = 94;
export const R_INNER = 46;
export const R_ARC = 64;

// High-contrast thread dimensions (tuned for 16px/32px legibility)
export const THREAD_WIDTH = 18;
export const RING = { r: 14.5, stroke: 8.5 };
export const BEAD_R = 16.5;

// Arc coordinates: 135 deg to 45 deg (270 deg clockwise sweep)
const SQRT1_2 = Math.SQRT1_2;
export const START = {
  cx: Number((CX - R_ARC * SQRT1_2).toFixed(2)),
  cy: Number((CY + R_ARC * SQRT1_2).toFixed(2)),
};
export const END = {
  cx: Number((CX + R_ARC * SQRT1_2).toFixed(2)),
  cy: Number((CY + R_ARC * SQRT1_2).toFixed(2)),
};

export const THREAD = `M ${START.cx} ${START.cy} A ${R_ARC} ${R_ARC} 0 1 1 ${END.cx} ${END.cy}`;

// Top reset notch on dial plate
export const NOTCH = {
  x: CX - 4,
  y: CY - R_DIAL + 4,
  w: 8,
  h: 18,
  rx: 2.5,
};

export const notchPath = (n = NOTCH) =>
  `M ${n.x + n.rx} ${n.y} H ${n.x + n.w - n.rx} A ${n.rx} ${n.rx} 0 0 1 ${n.x + n.w} ${n.y + n.rx} V ${n.y + n.h - n.rx} A ${n.rx} ${n.rx} 0 0 1 ${n.x + n.w - n.rx} ${n.y + n.h} H ${n.x + n.rx} A ${n.rx} ${n.rx} 0 0 1 ${n.x} ${n.y + n.h - n.rx} V ${n.y + n.rx} A ${n.rx} ${n.rx} 0 0 1 ${n.x + n.rx} ${n.y} Z`;

/** Superellipse (n=5) approximates Apple's continuous-corner squircle better than a rounded rect. */
export function squircle(cx, cy, half, n = 5, steps = 256) {
  const pts = [];
  for (let i = 0; i < steps; i++) {
    const t = (i / steps) * Math.PI * 2;
    const c = Math.cos(t);
    const s = Math.sin(t);
    pts.push(`${(cx + half * Math.sign(c) * Math.abs(c) ** (2 / n)).toFixed(2)} ${(cy + half * Math.sign(s) * Math.abs(s) ** (2 / n)).toFixed(2)}`);
  }
  return `M${pts.join('L')}Z`;
}

export const svgDoc = (w, h, body) =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}" fill="none">${body}</svg>`;
