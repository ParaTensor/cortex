---
name: admin-ui-change
description: Change xrouter Admin UI with progressive reading of docs/design.md chapters. Use for admin pages, components, dialogs, wizards, lists, master-detail, selection styling, settings IA, tokens, or any user-visible admin UI work. Never load every design chapter unless editing the whole system.
---

# Admin UI change (progressive design docs)

## Rule
[`docs/design.md`](../../../docs/design.md) is the **entry** (Overview + Hard rules + PR checklist). Detail chapters live under [`docs/design/`](../../../docs/design/). **Do not** Read every chapter by default. Load the entry, then only the files the task needs.

If editing the design system itself, open only the chapters you change (plus Hard rules / PR checklist).

## Always load
1. `docs/design.md` (whole file — kept short on purpose)
2. `AGENTS.md` §始终生效 → UI + Admin i18n + 列表排序与检索；细则 [`docs/ai/agents/ui-entry.md`](../../../docs/ai/agents/ui-entry.md)
3. Global settings IA: settings page is the only global-config home

## Route by task (chapter files)

| Task signal | Also read |
| --- | --- |
| Colors, dark mode, badges, status, brand narrative | `docs/design/colors.md` |
| Token table / hex values | `docs/design/tokens.md` |
| Page title, subtitle, density, i18n wrap | `docs/design/typography.md`, `docs/design/layout.md` |
| Page shell, cards, dashboard heights, filters toolbar | `docs/design/layout.md` |
| Wizard / Select / expand causes jump | `docs/design/layout.md` (Layout stability) |
| Shadows, selected-row depth, radius | `docs/design/surfaces.md` |
| New list page, row actions, create entry | `docs/design/components.md` (Entity list pattern) + `docs/ai/agents/ui-entry.md` (列表排序与检索) |
| Side-by-side list+detail browser | `docs/design/components.md` (Detail modes, Master–detail) |
| Edit/detail dialog | `docs/design/components.md` (Entity detail dialog, Overlay a11y) |
| Create wizard (API Key, Route) | `docs/design/components.md` + `docs/design/layout.md` (stability) |
| Org/project name display | `docs/design/components.md` (naming display) |
| Quick anti-patterns | `docs/design/dos-donts.md` |
| Token / CSS baseline change | `docs/design/tokens.md` + `admin` `index.css` / Tailwind; then change order in PR checklist footer |

## Do not invent
If a chapter and an old page disagree, treat the page as drift unless the user asked to change the spec. Prefer shared primitives under `admin/src/components/ui/*` and canonical references named in the chapters.

## Verify
1. Re-check **PR checklist** in `docs/design.md`.
2. New/changed user-visible strings exist in both `admin/src/lib/i18n/locales/zh.ts` and `en.ts` (do not rely on `t` English fallback alone).
3. `cd admin && npm run lint`
4. In the delivery note, name which chapter files you followed.

## When promoting new UI lessons
Add the lesson to the matching chapter under `docs/design/` (or Hard rules in the entry). Update this routing table if a new file appears. Mirror this skill to `.agents/skills/admin-ui-change/SKILL.md`.
