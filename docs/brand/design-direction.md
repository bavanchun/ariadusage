# Design direction: AriadUsage

Outcome: A premium, Liquid Glass–era app icon and brand system for AriadUsage that conveys "metering and guiding AI token usage along a safe path", never as a generic utility battery, speed gauge, or duplicate mark.
Sub-skills: logo, icon

| Axis | Decision | Why |
| --- | --- | --- |
| Logo style | Combination Mark: the Liquid Glass app icon is the primary mark (as Raycast, Arc, and Linear do), paired with the outlined wordmark | A rich, dimensional icon carries the brand across the desktop dock, status bar, Omarchy panel, README, and lockup |
| Concept | Ariadne's **golden thread** drawn as a **270-degree gauge arc**, running clockwise from a hollow datum ring (0% quota) to a luminous quota bead (current usage level) on a circular frosted glass dial plate with a top reset notch | Combines the Ariadne labyrinth mythos with token-budget metering in a single, uncluttered silhouette |
| Material | Liquid Glass: frosted glass dial plate that refracts the deep substrate, specular top-left rims, diffuse drop shadows, an illuminated glass-tube golden thread, and continuous-corner squircle bounds | Integrates naturally with modern desktop environments (macOS 26, Omarchy shell) while preserving high legibility at 16 px and 32 px |
| Palette | Cobalt Twilight background `#7AA5EB → #2D5DA8 → #0E224A` (dark: `#294375 → #142546 → #060E1E`), glass tint `#F0F6FF`, gold thread `#FFF2C8 / #F5C252 / #D68822`, ink `#162033`, paper `#F4F6FA`, night `#12161F`, mono `#2455A8` | Deep cobalt twilight (hue 220°) clearly differentiates from AriadShift's sage green (hue 145°); ink/paper contrast is 15.34:1 |
| Typography | Plus Jakarta Sans 700, tracking -0.02em, converted to outlined SVG glyph paths | Clean modern geometric sans with warm humanist details; OFL license; no font installation required on target systems |
| Voice | Precise, calm, observant, dependable | The product tracks sensitive AI costs, quotas, and account usage with cryptographic and protocol integrity |

Constraints: Vector masters (SVG), byte-for-byte reproducibility via SVGO `prefixIds`, legible at 32 px and recognizable at 16 px, light and dark appearances, single-color mono mark with ≥3:1 stroke-pixel contrast across all 22 Omarchy themes, native Liquid Glass layers for Apple Icon Composer.
Non-goals: Animated spinner/SVG, full brand manual/merchandise mockups, runtime rasterization pipelines.
Acceptance criteria:
- The golden thread arc and dial silhouette remain clearly readable under normal vision, a Deuteranopia simulation, and grayscale; luminance contrast and geometric silhouette supply the edge.
- The mono mark is recognizable at 16 px and legible at 32 px (verified in `brand/preview.png`).
- The mono mark achieves ≥3:1 stroke-pixel contrast on all 22 Omarchy themes at 16 px and 32 px.
- Provider icons (Claude, Codex, Antigravity) and dynamic fallback monograms share the `0 0 24 24` viewBox and optical weight.
- Every asset is fully reproducible with `just brand` (`git diff --exit-code brand/svg` returns 0).

## Concept Exploration and Decision Record

Three distinct concepts were drafted and rendered on an experimental concept board:

1. **Concept 1: The Gauge Arc (Selected)**: A 270° clockwise golden thread sweep from an open datum ring (0%) to a glowing quota bead, seated on a circular frosted glass dial plate with a top reset notch.
   - *Strengths*: Direct and intuitive representation of token consumption and quota resets; strong circular silhouette that scales down to 16 px cleanly.
   - *Trade-offs*: Requires careful calibration of stroke weight so the reset notch and datum ring do not merge at small scales.
   - *Decision*: **Selected by the project owner.** Thread stroke weight and datum ring contrast were boosted to ensure instant recognition at 16 px and 32 px.
2. **Concept 2: The Labyrinth Clew (Rejected)**: A golden thread coiled into a bold U for "Usage" around a central spool core.
   - *Rejection rationale*: Strong mythological link, but the inner coil crowds at 16 px and the bar glyph reads as a monogram rather than a usage instrument.
3. **Concept 3: The Horizon Strata (Rejected)**: A stepped golden thread crossing two tiered glass slabs for the burst window and the quota ceiling.
   - *Rejection rationale*: The two glass slabs echo AriadShift's two-sheet metaphor and add visual weight in small tiles, so it separates the family members less clearly.

## Assets

All assets are generated deterministically by `brand/build.mjs` from `brand/src/geometry.mjs`, `brand/src/glass-icon.mjs`, and `brand/scripts/normalize-provider-logos.mjs` (design-as-code: SVG → SVGO 4 with `prefixIds` → resvg 2.6). Provenance: manual mathematical composition, no image-generation models.

| File | Use |
| --- | --- |
| `brand/svg/ariadusage-app-icon.svg` | Primary mark: full-bleed Liquid Glass app icon (Linux desktop, web, app launchers) |
| `brand/svg/ariadusage-app-icon-dark.svg` | Dark appearance for nocturnal desktop themes |
| `brand/svg/ariadusage-app-icon-macos.svg` | Pre-macOS 26 template: 100 px inset margin and soft drop shadow |
| `brand/icon-composer/0-background.svg` … `3-thread.svg` | Flat vector layers for Apple Icon Composer to apply native Liquid Glass material |
| `brand/svg/ariadusage-mark-mono.svg` | Single-color mark (`currentColor`) for panel bars, notifications, embossing, and print |
| `brand/svg/ariadusage-wordmark.svg` | Outlined vector wordmark without runtime font dependencies |
| `brand/svg/ariadusage-lockup.svg`, `ariadusage-lockup-dark.svg` | App icon + wordmark for light and dark backgrounds |
| `brand/svg/favicon.svg` | Web favicon |
| `brand/svg/providers/*.svg` | Vendored and normalized provider logos (Claude, Codex, Antigravity) plus dynamic fallback monograms (`0 0 24 24`) |
| `brand/png/*` | Raster PNG renders from 16 px to 1200 px |
| `brand/png/cvd-simulation.png` | Deuteranopia and Grayscale perception simulation board |
| `brand/preview.png` | Complete visual verification board (1600x1240 px) |

## Self-critique

| Dimension | Score | Note |
| --- | --- | --- |
| Concept | 9 | Ariadne's golden thread metering token consumption; the metaphor is inseparable from the brand name and purpose |
| Philosophy alignment | 9 | Frosted glass refraction, specular rims, continuous-corner squircle, and layered depth faithfully embody Liquid Glass |
| Visual hierarchy | 9 | Radiant gold thread arc commands primary focus, frosted dial plate second, deep cobalt twilight background third |
| Craft quality | 9 | Point-precise SVG geometry, stroke-pixel contrast ≥4.9:1 across all 22 Omarchy themes, Deuteranopia and Grayscale verified |
| Functionality | 9 | One-command reproducible build, byte-for-byte identical SVG outputs, zero font dependency, normalized provider icons |
| Originality | 8.5 | Distinct from standard speedometers or battery bars; the Ariadne thread arc gives a mythic yet functional identity |
| **Total** | **8.9** | Exceeds the target threshold of 8.0/10 |
