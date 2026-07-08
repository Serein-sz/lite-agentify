# admin-visual-design Specification

## Purpose

Defines the visual design language and motion behavior of the web admin console: an achromatic (zero-chroma) palette with color reserved for semantic status, luminance-driven chart legibility across light and dark schemes, stable numeric animation on stat cards, an ambient animated login backdrop, state-change-only motion on authenticated pages, and full respect for the reduced-motion preference.

## Requirements

### Requirement: Interface palette is achromatic with color reserved for semantics
The console SHALL render all interface surfaces (shell, cards, buttons, tabs, inputs, charts) with an achromatic (zero-chroma) palette in both the light and dark color schemes. Chromatic color SHALL appear only for semantic status: destructive/error states and warning states. Decorative elements (login backdrop highlights, border effects) SHALL use foreground or neutral tones at reduced opacity, not hues.

#### Scenario: Interface surfaces stay neutral in both schemes
- **WHEN** the console renders the shell, dashboard, and config pages in light mode and in dark mode
- **THEN** no interface element outside semantic status indicators uses a chromatic hue.

#### Scenario: Semantic status keeps its color
- **WHEN** the request log renders a row with a 5xx status and a row with a 4xx status
- **THEN** the 5xx badge MUST use the destructive (red) treatment and the 4xx badge the warning (amber) treatment, unchanged by this capability.

### Requirement: Chart series are legible through luminance hierarchy in both schemes
Dashboard chart series SHALL remain achromatic and SHALL be clearly distinguishable — from the background and from each other — in both color schemes, with hierarchy carried by luminance and series identity carried by mark shape. The primary metric (cost line) SHALL render as the highest-contrast ink relative to the background; secondary series (token bars) SHALL render at a receding mid-luminance; gridlines SHALL stay near-background.

#### Scenario: Light-mode chart legibility
- **WHEN** the usage trend chart renders in light mode with token and cost data
- **THEN** both series MUST be clearly visible against the background, and the cost line MUST present higher contrast than the token bars.

#### Scenario: Dark-mode chart legibility
- **WHEN** the usage trend chart renders in dark mode with the same data
- **THEN** both series MUST be clearly visible against the dark background, and the cost line MUST present higher contrast than the token bars.

### Requirement: Stat values animate numeric changes without layout shift
Dashboard stat card values SHALL animate numeric transitions on initial load and when the selected time range changes, and SHALL render with tabular (fixed-width) figures so that a value in transition never shifts the layout of its card or neighboring elements.

#### Scenario: Time-range switch animates values stably
- **WHEN** an authenticated admin switches the dashboard time range and new totals arrive
- **THEN** the stat values MUST transition to the new numbers, and card dimensions and text alignment MUST NOT jump during the transition.

#### Scenario: Non-animatable values degrade to static text
- **WHEN** a stat value cannot be represented as a single number (such as cost totals spanning multiple currencies beyond the primary)
- **THEN** the card MUST render the formatted value statically rather than animating an incorrect number.

### Requirement: Login page presents an ambient animated backdrop
The login page SHALL present a full-screen animated grid backdrop whose highlights use neutral foreground tones at low opacity, and an animated border treatment on the login card, in both color schemes. This ambiance SHALL NOT alter the login form's behavior.

#### Scenario: Login ambiance renders in both schemes
- **WHEN** an unauthenticated visitor opens the login page in light mode or dark mode
- **THEN** the animated grid backdrop and the card border treatment MUST be visible, and the login form MUST remain fully usable.

### Requirement: Post-login motion occurs only on state change
Authenticated pages SHALL animate only in response to state changes — page/section entrance and data refresh. Dashboard and config sections SHALL enter with a brief staggered transition on mount. After entrance transitions settle, no decorative animation SHALL run continuously on authenticated pages; ambient animation is confined to the login page.

#### Scenario: Sections enter once, then rest
- **WHEN** an authenticated admin opens the dashboard and waits for entrance transitions to complete
- **THEN** sections MUST have appeared with a staggered transition, and afterwards no decorative animation continues to run until the next state change.

#### Scenario: Config page stays calm
- **WHEN** an authenticated admin opens the config editor
- **THEN** beyond the shared entrance transition, form fields and controls MUST NOT carry decorative animation.

### Requirement: Motion honors the reduced-motion preference
All decorative motion — entrance transitions, stat value animation, the login backdrop, and animated borders — SHALL respect `prefers-reduced-motion: reduce` by rendering final static states.

#### Scenario: Reduced-motion login
- **WHEN** a visitor with `prefers-reduced-motion: reduce` opens the login page
- **THEN** the backdrop MUST render as a static pattern without square animation and the card border MUST NOT animate.

#### Scenario: Reduced-motion dashboard
- **WHEN** an admin with `prefers-reduced-motion: reduce` opens the dashboard or switches time ranges
- **THEN** sections MUST appear without entrance transitions and stat values MUST render their final numbers immediately.
