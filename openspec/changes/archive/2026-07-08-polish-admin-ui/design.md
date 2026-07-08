## Context

The admin console (`ui/`, Vite 6 + React 19 + Tailwind v4 + shadcn base-lyra on Base UI, ECharts, embedded into the gateway binary via rust-embed) shipped with the default zero-chroma shadcn theme and no motion. Concretely:

- Every token in `src/index.css` is `oklch(x 0 0)` — pure grayscale, including `--chart-1..5`.
- The chart ladder is broken: `--chart-1 = oklch(0.87)` (assigned to the Tokens bar series) is nearly invisible on white; the `.dark` block reuses the same five values, so the darker steps vanish on dark backgrounds.
- The shell (`App.tsx`) uses square tab blocks and a plain text wordmark; nothing animates anywhere.

During exploration the user made two directional decisions: **stay monochrome** (Vercel/Linear school — no brand hue, charts included) and **AnimatedGridPattern** as the login backdrop. Scope is "all three layers": texture polish, login ambiance, and a site-wide motion system.

## Goals / Non-Goals

**Goals:**

- A deliberate monochrome identity: hierarchy carried by luminance, series identity by mark shape, chromatic color reserved for semantic status (destructive red, warning amber).
- Legible charts in both color schemes via a corrected dual-mode luminance ladder.
- A restrained motion layer: animation only on state change inside the app; the login page is the single sanctioned ambient surface.
- Honor `prefers-reduced-motion` everywhere.
- Keep the existing build/embed pipeline untouched.

**Non-Goals:**

- No chromatic theme, no brand color exploration (explicitly decided against).
- No layout restructuring (no sidebar, no new pages, no information-architecture changes).
- No chart type changes and no new dashboard data.
- No Rust/gateway/API changes.
- Solving multi-category (4+) chart coloring — deferred; see the guardrail in D2.

## Decisions

### D1: Achromatic discipline — color is semantics, never decoration

All interface tokens stay zero-chroma in both schemes. The only chromatic pixels are semantic: `--destructive` red and the amber 4xx badge treatment that already exist. Login backdrop highlights use **foreground at low opacity** (white glints in dark mode, black in light), not a hue.

- *Alternative considered*: hued theme (violet/indigo/teal) — offered, user chose monochrome.
- *Alternative considered*: multi-color chart palette — rejected by user; viable here because the only chart has two series distinguished by mark shape (bar vs line).

### D2: Chart legibility via a dual-mode luminance ladder

ECharts assigns palette colors by series order through the existing `EChart` wrapper (`--chart-1` → Tokens bar, `--chart-2` → cost line). Redefine the ladder per scheme:

| Token | Role | Light | Dark |
|---|---|---|---|
| `--chart-1` | Tokens bars (recedes) | `oklch(0.55 0 0)` | `oklch(0.70 0 0)` |
| `--chart-2` | Cost line (strongest ink) | `oklch(0.25 0 0)` | `oklch(0.92 0 0)` |
| `--chart-3..5` | Future series, descending steps | 0.40 / 0.70 / 0.87 | 0.55 / 0.45 / 0.35 |

The cost line additionally gets a slightly heavier `lineWidth` in the chart option so the hierarchy reads even in thumbnails. Axis/grid lines stay near-background (`0.93` light / `0.30` dark) via ECharts' own theme defaults or explicit option values.

**Guardrail (documented, deferred)**: grayscale implies ordering. If a chart with 4+ *unordered* categories is ever added (e.g., per-provider comparison), luminance alone will not distinguish them — at that point use direct labeling (series-end labels) or open a chart-only color exception. Do not pre-pay for this now.

### D3: Magic UI components vendored via shadcn CLI

Add four components as source into `ui/src/components/magicui/` using the existing registry workflow (`pnpm dlx shadcn add "https://magicui.design/r/<name>"`):

| Component | Used on | Needs `motion`? |
|---|---|---|
| `animated-grid-pattern` | Login backdrop | Yes |
| `shine-border` | Login card border | No (CSS keyframes) |
| `number-ticker` | Dashboard stat cards | Yes |
| `blur-fade` | Dashboard/Config section entrances | Yes |

Rationale: identical to the existing copy-paste component model (`src/components/ui/`), we own and can edit the source, and there is no runtime registry dependency. Magic UI's defaults assume the Radix "new-york" look, but these four are plain div/SVG + Tailwind + motion (no interactive primitives), so they coexist with base-lyra; radius/spacing tuned at integration time.

- *Alternative*: hand-write the effects — more control but reinvents tested code for no benefit.
- *Alternative*: an npm animation kit — opaque, heavier, styles not token-driven.

### D4: `motion` is the single new dependency

`animated-grid-pattern`, `number-ticker`, and `blur-fade` require `motion` (~45–50 KB gzip). Accepted: the UI already embeds ECharts (~300 KB gzip), and the alternative (pure-CSS number roll / in-view staggering) is materially worse in quality. `shine-border` stays CSS-only. No other animation runtime enters the project.

### D5: NumberTicker gains a `format` prop

Upstream NumberTicker formats via `Intl.NumberFormat`; our stat values are domain-formatted (`formatTokens` → "1.2M", `formatLatency` → "123 ms", `formatPercent`, multi-currency cost). Adapt the vendored source: the spring animates a raw `number`, and rendering passes through `format?: (n: number) => string` (default remains Intl). `StatCard` accepts `{ value: number, format }` for tickable stats. The cost card animates the **primary-currency amount** (`costAmount(cost, primaryCurrency)`); if totals are not representable as one number (mixed currencies beyond the primary), it degrades to static text — correctness over spectacle.

### D6: Motion conventions — animate state changes, never idle

- **Entrance**: sections enter with `BlurFade` — stagger 60–80 ms between siblings, ~400 ms duration, small translate + blur; runs once per mount.
- **Data refresh**: tickers re-spring when the time range changes; nothing else moves.
- **Login exception**: the grid backdrop and shine border may run continuously — the page is glanced at for seconds and unmounts after auth.
- **Hover/focus**: unified `transition-colors` timing on interactive elements; focus-visible rings via the existing `outline-ring/50` base.
- No perpetual decorative animation inside the authenticated app, ever.

### D7: Reduced motion handled inside the vendored components

Each vendored component short-circuits on `useReducedMotion()` from `motion/react`: `blur-fade` renders visible, `number-ticker` renders the final formatted value, `animated-grid-pattern` renders the static grid (no square animation). `shine-border` (pure CSS) gets an `@media (prefers-reduced-motion: reduce)` pause. A root `<MotionConfig reducedMotion="user">` is added as belt-and-braces for declarative animations.

- *Alternative*: rely solely on root `MotionConfig` — insufficient, it does not cover the imperative spring inside `number-ticker`.

### D8: Data typography — tabular figures

Apply `font-variant-numeric: tabular-nums` (Tailwind `tabular-nums`) to stat card values and numeric table columns. Inter Variable ships the `tnum` feature, so no new font. This is also what keeps ticking numbers from reflowing card layout (see spec).

### D9: Shell polish

- Logo: a lucide mark (e.g. `Waypoints`) in a small rounded tile + the existing wordmark.
- `TabLink`: square blocks → rounded pills (`rounded-full`); active keeps `bg-primary text-primary-foreground` (a near-black pill reads very Linear), inactive gets `hover:bg-muted`.
- Header: sticky with `backdrop-blur` over `bg-card/80`.
- Cards/tables: keep base-lyra defaults, tighten only shadow/border consistency. Exact values tuned visually at implementation.

## Risks / Trade-offs

- **[Registry drift]** Magic UI source fetched at add-time may differ from docs → components are vendored and reviewed at add time, then pinned in git; no runtime dependency.
- **[Bundle/binary growth]** `motion` adds ~50 KB gzip to the embedded assets → measure `dist/` size before/after in the verification task; abort threshold not needed at this scale.
- **[Style mismatch]** Magic UI defaults vs base-lyra aesthetics → all four components are token/`currentColor`-driven; adjust radius/opacity during integration, verified in both schemes.
- **[Login CPU cost]** continuous canvas/SVG animation → confined to login (unmounts after auth); static under reduced motion.
- **[Gray-on-gray regressions]** a luminance-only system has less margin for error than hue → the ladder values in D2 are explicit, and light/dark visual passes are a dedicated task; chart contrast is spec-tested.
- **[NumberTicker divergence from upstream]** the `format` prop is a local fork → acceptable, the file is vendored source by design; note the change in a header comment for future re-syncs.

## Migration Plan

Pure frontend, additive change: `pnpm add motion`, vendor components, edit tokens/pages, `pnpm build`, rebuild the gateway binary (embed picks up `dist/` automatically). Rollback is `git revert` — no data, config, or API migration exists.

## Open Questions

None blocking. Exact easing/duration values and shell shadow depths are tuned visually during implementation within the conventions of D6/D9.
