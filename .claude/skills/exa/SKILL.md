---
name: exa
description: >-
  Use the Exa MCP (mcp__exa__* — `web_search_exa`, `web_search_advanced_exa`, `web_fetch_exa`)
  for ALL web access in this harness. Native `WebSearch` and `WebFetch` are denied, so Exa is the only
  path to the open web. Reach for it whenever the user wants to search the web for something, find
  recent articles or docs on a topic, fetch the content of a known URL, or check the current
  online state of some library, tool, or company. Trigger on indirect phrasing too — "what's the
  latest on X", "find me sources about Y", "pull up that page", "is there a newer way to do Z",
  "what are people saying about W". Boundary: versioned LIBRARY/framework/API docs go to Context7
  (`/find-docs`), not Exa — Exa is for everything else web (articles, blogs, company/product
  state, arbitrary URLs).
---

# Exa — the only web path

Native `WebSearch`/`WebFetch` are denied and `curl`/`wget` to external hosts is steered off, so
**Exa is how Claude reaches the open web** on this box.

## Why Exa, not native WebSearch/WebFetch

Three concrete reasons, not just "a different vendor":

- **Cleaner, smaller results.** Exa returns semantic search hits with extracted
  content/highlights, not raw HTML to pick through — less noise per result means less
  context spent reading past what the query actually needed.
- **One consistent contract.** A single search backend across every session, with
  explicit filter parameters (`web_search_advanced_exa`'s recency/domain include-exclude)
  instead of an opaque native tool whose ranking/formatting you can't inspect or steer.
- **Traceable citations.** Results carry clean source URLs, which is what makes
  "cite the URL you acted on" (see Guardrails below) actually enforceable — a
  grounded-progress-claim discipline that's hard to keep with a black-box search tool.

Native `WebSearch`/`WebFetch` aren't broken — this is a deliberate one-tool-for-one-job
choice so there's never a "which web tool did this session use" ambiguity to reason
about.

## The three tools

- **`web_search_exa`** — general semantic web search. The default for "find sources / what's out
  there on X".
- **`web_search_advanced_exa`** — search with filters (recency, domain include/exclude). Use when
  freshness or a specific source matters ("only the last 6 months", "from the official site").
- **`web_fetch_exa`** — fetch + extract the content of a **known URL**. Use when you already have
  the link and want its text.

## WHEN web vs Context7

- **Versioned library / framework / API docs** (React, Next.js, Drizzle, NestJS, a CLI's flags) →
  **Context7** (`/find-docs`), which is version-correct. Do NOT use Exa for these.
- **Everything else web** — articles, blog posts, product/company state, standards pages,
  arbitrary URLs → **Exa**.

## WHEN to reach for it

- Research fan-out: Exa is the primary engine behind `.claude/agents/plan/researcher.md` and the
  `/research` command. Reach for it directly for any in-loop web lookup too — don't try native
  WebSearch (it's denied) or `curl` (it's steered off).

## Guardrails

- Keep result sets tight — request fewer results and project the fields you need; don't dump full
  page bodies into context when a snippet answers the question.
- Cite URLs you act on so claims stay traceable.
