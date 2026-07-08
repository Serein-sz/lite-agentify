## Why

The admin console is functional but reads as unfinished: every theme token is zero-chroma with a broken grayscale ladder (the first chart color `oklch(0.87)` is nearly invisible on light backgrounds, and dark mode reuses the light-mode ladder so low-luminance steps vanish), the shell is unstyled rectangles, and nothing moves. We are adopting a deliberate monochrome identity (Vercel/Linear school: hierarchy by luminance, identity by shape, color only for semantics) plus a purposeful motion layer, so the console looks designed rather than default.

## What Changes

- **Chart palette fix**: replace the broken 5-step gray ramp with a dual-mode luminance ladder — the cost line becomes the strongest ink (near-foreground), token bars recede to mid-gray, grid lines go faint; series identity is carried by mark shape (bar vs line), never by hue.
- **Data typography**: stat card values and numeric table cells render with tabular (fixed-width) figures so changing/animated values do not shift layout.
- **Shell polish**: logo mark (icon + wordmark), tab links restyled from square blocks to rounded pills, unified hover/focus transitions, card border/shadow hierarchy.
- **Login page ambiance**: full-screen animated grid backdrop with neutral (foreground low-opacity) highlights and a shine-border treatment on the login card — the only surface where perpetual motion is allowed.
- **Dashboard motion**: stat values roll in via number ticker on load and when switching time ranges; page sections enter with a staggered blur-fade.
- **Config page**: shared entrance animation only; forms stay static.
- **New dependency**: `motion` (framer-motion successor). Magic UI components (`animated-grid-pattern`, `shine-border`, `number-ticker`, `blur-fade`) are vendored as source into `ui/src/components/magicui/` via the shadcn CLI, consistent with the existing copy-paste component model.
- **Accessibility**: all motion honors `prefers-reduced-motion` — reduced-motion users get the final static state.

## Capabilities

### New Capabilities

- `admin-visual-design`: presentation-layer requirements for the admin console — achromatic palette discipline in both color schemes, chart legibility via luminance hierarchy, layout-stable animated numerics, login-page ambient backdrop, and reduced-motion accessibility.

### Modified Capabilities

<!-- none — existing admin-ui / admin-usage requirements (serving, auth flow, what data is displayed, config editor behavior) are unchanged; this change adds presentation-layer requirements on top. -->

## Impact

- **Affected code**: `ui/` only — `src/index.css` (theme tokens), `src/App.tsx` (shell), `src/pages/LoginPage.tsx`, `src/pages/DashboardPage.tsx`, `src/pages/ConfigPage.tsx`, `src/components/EChart.tsx` (series weight/emphasis), new `src/components/magicui/*`.
- **Dependencies**: `motion` added to `ui/package.json` (~45–50 KB gzip); four Magic UI components vendored as source (no runtime registry dependency).
- **No Rust changes**: gateway code, admin API, and the rust-embed pipeline are untouched; embedded asset size grows by roughly the motion bundle.
- **No behavior changes** to auth, usage queries, or config editing.
