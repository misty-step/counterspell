# Counterspell DESIGN.md

This file is the product's public-site brand contract. Keep it short and exact:
agents and humans should be able to update `site/` from this file without
inventing a second design system.

## Brand Voice

- Plain-spoken, concrete, and operator-facing.
- Lead with the user outcome, then the proof.
- Avoid marketing fog, mascot language, and decorative claims.

## Pitch One-Liner

`Keep your Fable sessions on Fable.`

## Locked Homepage

- Lock: operator lock-in 2026-07-07, `misty-step-936`.
- Layout: Split.
- Homepage H1: `Keep your Fable sessions on Fable.`
- Hero image: `site/assets/hero.jpg`, copied from the production
  `counterspell-hero.jpg` asset generated with `gpt-image-1` in the Misty
  Step fresco language.
- Hero image opacity: `0.85`.
- Homepage structure: one viewport only — header, left-aligned hero H1,
  `Get started` CTA, and footer. Feature detail and release notes live on
  `features.html` and `changelog.html`.

## Lucide Mark

- Icon: `wand-sparkles`
- Reason: Counterspell's whole job is spellcasting on a drifted session back
  onto Fable — the wand reads as the product action, not a generic mark.
- Rule: the mark is an inline Lucide SVG inside `.ae-app-mark`. No bespoke
  marks, logo images, emoji marks, or colored wordmarks.

## Palette Hooks

Counterspell reuses its own live dashboard's accent (an indigo/blue) so the
marketing site reads as the same product as `counterspell ui`:

```css
:root {
  --ae-accent: #2643d0;
  --ae-accent-dark: #8c9eff;
}
```

## Screenshot Inventory

Counterspell's live surface is a Herdr pane and a menu-bar dot, not a
screenshot-worthy app UI. `features.html` carries text feature cards (job/proof
rows) instead of a screenshot gallery. Add a real gallery here once the local
dashboard (`counterspell ui`) has screenshots worth shipping.

## Footer Links

- Misty Step: `https://mistystep.io`
- GitHub: `https://github.com/misty-step/counterspell`

Footer contract: the mode toggle sits on the left. The right side reads
`a Misty Step project`, with `Misty Step` linked to `https://mistystep.io`,
followed by the GitHub glyph linked to the public repo. No bare URL text, no
email, no copyright line, and no Weave links.

## Release Notes Rule

`site/changelog.html` is user-facing. Write entries as product outcomes, not
commit logs. Each entry needs a date, a version or release label, and one or
two plain-language bullets.

Counterspell has one tagged release, `v0.1.0` (2026-07-02), with real GitHub
release artifacts. `site/changelog.html` ships a hand-written history of the
shipped milestones since (fast-path remediation, auto-Fable precedence, the
dashboard, pane rebind) because there is no Landmark user-facing export yet.
Switch to the Landmark export once one exists.
