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

// High-contrast thread dimensions (asymmetric gauge needle & datum ring)
export const THREAD_WIDTH = 22;
export const GLASS_THREAD_WIDTH = 19;
export const NEEDLE_TIP_W = 3.5;
export const RING = { cx: 82.75, cy: 173.25, r: 16, stroke: 10, holeR: 11 };

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

/** Generates a continuous tapered gauge needle path from ring center to quota point. */
export function generateTaperedArc({
  baseW = THREAD_WIDTH,
  tipW = NEEDLE_TIP_W,
  taperStartT = 0.68,
  steps = 64,
  startDeg = 135,
  endDeg = 405,
} = {}) {
  const ptsOuter = [];
  const ptsInner = [];
  const startA = startDeg * (Math.PI / 180);
  const endA = endDeg * (Math.PI / 180);
  const totalSweep = endA - startA;

  for (let i = 0; i <= steps; i++) {
    const t = i / steps;
    const a = startA + t * totalSweep;
    let w = baseW;
    if (t > taperStartT) {
      const taperT = (t - taperStartT) / (1 - taperStartT);
      w = baseW * (1 - taperT) + tipW * taperT;
    }
    const rOut = R_ARC + w / 2;
    const rIn = R_ARC - w / 2;
    ptsOuter.push(`${(CX + rOut * Math.cos(a)).toFixed(2)} ${(CY + rOut * Math.sin(a)).toFixed(2)}`);
    ptsInner.unshift(`${(CX + rIn * Math.cos(a)).toFixed(2)} ${(CY + rIn * Math.sin(a)).toFixed(2)}`);
  }
  const tipPt = `${(CX + R_ARC * Math.cos(endA)).toFixed(2)} ${(CY + R_ARC * Math.sin(endA)).toFixed(2)}`;
  return `M ${ptsOuter[0]} L ${ptsOuter.slice(1).join(' L ')} L ${tipPt} L ${ptsInner.join(' L ')} Z`;
}

export const TAPERED_THREAD_MONO = generateTaperedArc({ baseW: 22, tipW: 3.5, taperStartT: 0.68 });
export const TAPERED_THREAD_GLASS = generateTaperedArc({ baseW: 19, tipW: 3, taperStartT: 0.65 });

/** Centerline arc for Liquid Glass specular tube reflections. */
export function generateCenterline({ startDeg = 145, endDeg = 400 } = {}) {
  const startA = startDeg * (Math.PI / 180);
  const endA = endDeg * (Math.PI / 180);
  const startPt = {
    x: Number((CX + R_ARC * Math.cos(startA)).toFixed(2)),
    y: Number((CY + R_ARC * Math.sin(startA)).toFixed(2)),
  };
  const endPt = {
    x: Number((CX + R_ARC * Math.cos(endA)).toFixed(2)),
    y: Number((CY + R_ARC * Math.sin(endA)).toFixed(2)),
  };
  return `M ${startPt.x} ${startPt.y} A ${R_ARC} ${R_ARC} 0 1 1 ${endPt.x} ${endPt.y}`;
}

export const SPINE_PATH = generateCenterline();

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
