## 1. Dependencies & vendored components

- [x] 1.1 Add `motion` to `ui/package.json` via `pnpm add motion` (run in `ui/`)
- [x] 1.2 Vendor the four Magic UI components into `ui/src/components/magicui/` via `pnpm dlx shadcn add "https://magicui.design/r/animated-grid-pattern"` (and `shine-border`, `number-ticker`, `blur-fade`)
- [x] 1.3 Review vendored source: imports resolve (`motion/react`, `@/lib/utils`), styling is token/`currentColor`-driven, `pnpm check` passes; add a header comment to each file noting it is vendored Magic UI source with local modifications

## 2. Theme tokens & typography

- [x] 2.1 Replace the light-mode `--chart-1..5` ladder in `ui/src/index.css` per design D2 (bars `0.55`, cost line `0.25`, descending future steps)
- [x] 2.2 Replace the dark-mode `--chart-1..5` ladder per design D2 (bars `0.70`, cost line `0.92`, descending future steps)
- [x] 2.3 Apply `tabular-nums` to stat card values and numeric table columns (Tokens/成本/延迟 cells)
- [x] 2.4 Unify hover/focus transitions on interactive elements (`transition-colors` timing; keep existing `outline-ring/50` focus base)

## 3. Motion foundations

- [x] 3.1 Wrap the app root in `<MotionConfig reducedMotion="user">`
- [x] 3.2 Short-circuit `blur-fade`, `number-ticker`, and `animated-grid-pattern` on `useReducedMotion()` so each renders its final static state
- [x] 3.3 Pause the `shine-border` CSS animation under `@media (prefers-reduced-motion: reduce)`
- [x] 3.4 Adapt `number-ticker`: add `format?: (n: number) => string` prop (default stays Intl), spring animates the raw number, rendering goes through `format`

## 4. Shell polish

- [x] 4.1 Add a logo mark to the header in `App.tsx` (lucide `Waypoints` in a small rounded tile + wordmark)
- [x] 4.2 Restyle `TabLink` from square blocks to rounded pills (active `bg-primary text-primary-foreground`, inactive `hover:bg-muted`)
- [x] 4.3 Make the header sticky with `backdrop-blur` over `bg-card/80`

## 5. Login page

- [x] 5.1 Add the `AnimatedGridPattern` full-screen backdrop with foreground low-opacity highlights (both schemes, no hue)
- [x] 5.2 Wrap the login card with `ShineBorder` using a neutral gradient; verify the form (autofocus, submit, error states) is unaffected
- [ ] 5.3 Visual pass on the login page in light and dark modes

## 6. Dashboard

- [x] 6.1 Rework `StatCard` to accept `{ value: number, format }` and render values through the adapted `NumberTicker`; wire the five cards (requests, tokens, cost, latency, error rate)
- [x] 6.2 Cost card: animate the primary-currency amount via `costAmount(...)`; degrade to static formatted text when totals are not representable as one number
- [x] 6.3 Verify tickers re-animate on time-range switch without layout shift (tabular figures in place)
- [x] 6.4 Add staggered `BlurFade` entrance for dashboard sections (stat row, trend chart, breakdown, request log) — runs once per mount
- [x] 6.5 Increase the cost line `lineWidth` in the chart option so the line reads as the strongest ink over the bars

## 7. Config page

- [x] 7.1 Add the shared `BlurFade` entrance to config page sections; confirm form controls carry no other animation

## 8. Verification

- [ ] 8.1 Light/dark visual pass across login, dashboard, config: achromatic surfaces, semantic badge colors intact, chart series distinguishable in both schemes (spec scenarios for palette and chart legibility)
- [ ] 8.2 Reduced-motion pass (DevTools emulation): static login backdrop, no entrance transitions, instant final stat values (spec reduced-motion scenarios)
- [ ] 8.3 Confirm no perpetual decorative animation on authenticated pages after entrance settles (spec post-login motion scenarios)
- [ ] 8.4 `pnpm build` in `ui/` passes (tsc + vite); note `dist/` gzip size before/after to record the motion bundle cost
- [ ] 8.5 Rebuild the gateway and verify `/admin` serves the new UI from embedded assets
