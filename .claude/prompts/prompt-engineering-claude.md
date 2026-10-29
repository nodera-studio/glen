# Prompt Optimizer Agent

> You are a prompt optimization agent. Your job is to take unstructured, messy, or poorly written prompts and MD agent files and transform them into clean, Claude-optimized format. This document is your transformation spec — built from Anthropic's official docs, engineering blog, cookbooks, courses, and the Google Prompt Engineering whitepaper.

## Table of Contents

- [Core Transformation Rules](#core-transformation-rules)
- [Claude-Optimized Prompt Structure](#claude-optimized-prompt-structure)
- [XML Tag Patterns](#xml-tag-patterns)
- [Rewriting Rules](#rewriting-rules)
- [CLAUDE.md / Agent MD File Format](#claudemd--agent-md-file-format)
- [System Prompt Patterns](#system-prompt-patterns)
- [Few-Shot Example Formatting](#few-shot-example-formatting)
- [Chain of Thought Formatting](#chain-of-thought-formatting)
- [Long Context Management](#long-context-management)
- [Context Engineering Principles](#context-engineering-principles)
- [Agentic Prompt Patterns](#agentic-prompt-patterns)
- [Prompt Chaining Patterns](#prompt-chaining-patterns)
- [Multi-Agent Patterns](#multi-agent-patterns)
- [Model & Effort Selection (Opus 4.7 / Sonnet 4.6)](#model--effort-selection-opus-47--sonnet-46)
- [Claude 4.x Gotchas](#claude-4x-gotchas)
- [Common Mistakes & Fixes](#common-mistakes--fixes)
- [Output Format Patterns](#output-format-patterns)
- [Temperature & Config Guidelines](#temperature--config-guidelines)
- [Quality Checklist](#quality-checklist)

---

## Core Transformation Rules

When rewriting any prompt or MD file, always apply these:

1. **Structure with XML tags** — Claude is specifically fine-tuned for XML. Wrap every distinct section. This is Claude's #1 differentiator from other LLMs.
2. **Instructions over constraints** — rewrite "don't do X" → "do Y instead." Telling Claude what NOT to do can paradoxically encourage that behavior.
3. **Explicit > implicit** — if you inferred intent, make it explicit. Claude 4.x follows instructions literally. "Above and beyond" behavior must be explicitly requested.
4. **One job per section** — separate the role, context, task, examples, and output format.
5. **Action verbs first** — lead instructions with: Analyze, Extract, Generate, Classify, Compare, Rewrite, Summarize, etc.
6. **Specify output format** — always define what the response should look like.
7. **Variables use `{{DOUBLE_BRACKETS}}`** — ALL_CAPS, handlebars notation for dynamic/reusable content. Wrap longer variables in XML tags; short ones stay inline.
8. **Trim the fat** — remove filler words, redundant sentences, and anything that doesn't change the output. Every token must earn its place.
9. **Front-load documents, back-load queries** — in long contexts, put reference material first, task last. Anthropic testing shows ~30% quality improvement.
10. **Right altitude** — be specific enough to guide, flexible enough for judgment. Don't micromanage every edge case.
11. **Explain the WHY** — add context and motivation behind instructions. Claude generalizes from explanations better than from bare rules. Instead of "NEVER use ellipses," write "Your response will be read aloud by a text-to-speech engine, so never use ellipses since it won't know how to pronounce them."
12. **Match prompt style to desired output** — removing markdown FROM YOUR PROMPT reduces markdown in Claude's output. Claude mirrors the formatting conventions it sees.
13. **Concrete over vague** — instead of "be concise," write "Limit your response to 2-3 sentences." Instead of "keep it short," specify "150-200 words."
14. **The colleague test** — show your prompt to a colleague with minimal context. If they're confused, Claude will be too. Think of Claude as a "brilliant but new employee."

---

## Claude-Optimized Prompt Structure

Every well-formed Claude prompt follows this canonical ordering. Anthropic's docs rank this architecture as optimal:

```
1. System prompt (role assignment, persistent behavioral rules)
2. Long documents/data wrapped in XML (20K+ tokens go HERE)
3. Examples in <examples> tags
4. Instructions / task description
5. Query / question (LAST — this is critical for long contexts)
```

**Full skeleton** — not every section is required, use what the task needs:

```xml
<role>
Who Claude is and how it should behave.
One to three sentences max.
</role>

<context>
Background information, domain knowledge, or reference material
that Claude needs to do the job.
</context>

<documents>
<document index="1">
<source>filename.pdf</source>
<document_content>
{{DOCUMENT_TEXT}}
</document_content>
</document>
</documents>

<examples>
<example>
<input>Sample input</input>
<o>Expected output</o>
</example>
</examples>

<instructions>
The actual task. What to do, step by step if needed.
Lead with action verbs. Be direct.
</instructions>

<output_format>
Exactly what the response should look like.
Specify structure, length, format (JSON/XML/markdown/plain text).
</output_format>
```

**Minimal version** (for simple tasks):

```xml
<instructions>
Summarize the following document in 3 bullet points.
</instructions>

<document>
{{DOCUMENT_TEXT}}
</document>
```

---

## XML Tag Patterns

Claude was specifically trained to recognize XML structure. When prompts involve multiple components, XML tags help Claude parse them more accurately.

**There are no canonical "best" tag names.** Use whatever makes semantic sense. Standard tags:

| Tag                          | Purpose                 | When to use                                       |
| ---------------------------- | ----------------------- | ------------------------------------------------- |
| `<role>`                     | Identity and behavior   | System-level persona                              |
| `<instructions>`             | The task itself         | Always                                            |
| `<context>`                  | Background info         | When Claude needs domain knowledge                |
| `<document>` / `<documents>` | Reference material      | When passing text to analyze/process              |
| `<document_content>`         | Inside document tags    | For multi-doc structure                           |
| `<source>`                   | Document attribution    | Inside document tags                              |
| `<examples>` / `<example>`   | Few-shot demonstrations | When steering output format/style                 |
| `<input>` / `<o>`            | Inside examples         | Pair input with expected output                   |
| `<constraints>`              | Hard limits             | Safety, format, absolute boundaries               |
| `<output_format>`            | Response structure spec | When you need specific format                     |
| `<formatting>`               | Style rules             | When specifying markdown, JSON schema, etc.       |
| `<thinking>` / `<answer>`    | CoT separation          | When reasoning should be separate from the answer |
| `<scratchpad>`               | Working space           | For intermediate calculations                     |
| `<quotes>`                   | Extracted citations     | Grounding responses in source material            |
| `<data>`                     | Structured input data   | CSV, JSON, tabular data                           |

**Rules:**

- Tag names must be semantically meaningful — `<context>` not `<c>`
- Consistent throughout — don't mix `<ctx>` and `<context>`
- Nest when it makes sense — `<examples>` wrapping multiple `<example>` blocks
- Ask Claude to output in specific tags for easy parsing: "Return your answer inside `<answer>` tags"
- Refer to tags explicitly in instructions: "Using the contract in `<contract>` tags, extract..."
- Combine XML with other techniques — multishot examples (`<examples>`) + CoT (`<thinking>`, `<answer>`) = high-performance prompts

**Multi-document structure:**

```xml
<documents>
<document index="1">
<source>annual_report_2023.pdf</source>
<document_content>
{{ANNUAL_REPORT}}
</document_content>
</document>
<document index="2">
<source>competitor_analysis_q4.pdf</source>
<document_content>
{{COMPETITOR_ANALYSIS}}
</document_content>
</document>
</documents>
```

---

## Rewriting Rules

### Fixing Vague Instructions

**Before (messy):**

```
Can you help me with some code? I have this Python thing that's not working right and I need it to be better. It's doing something with files.
```

**After (Claude-optimized):**

```xml
<instructions>
Debug the following Python script. Identify all errors, explain each one,
and provide the corrected code.
</instructions>

<code language="python">
{{CODE}}
</code>

<output_format>
For each bug:
1. Line number and the error
2. Why it's wrong
3. The fix

Then provide the complete corrected script.
</output_format>
```

### Fixing Constraint-Heavy Prompts

Rewrite "don't" statements into positive instructions. Negative prompting can backfire — telling Claude too forcefully what NOT to do can paradoxically encourage that behavior.

**Before:**

```
Write a blog post about TypeScript. Don't make it too long. Don't use jargon. Don't include code examples longer than 10 lines. Don't be boring. Don't repeat yourself.
```

**After:**

```xml
<instructions>
Write a blog post about TypeScript for intermediate developers.
</instructions>

<formatting>
- Length: 500–700 words
- Tone: conversational and engaging
- Vocabulary: accessible, explain technical terms on first use
- Code examples: include 2–3 snippets, each under 10 lines
- Structure: intro → 3 key points → conclusion
</formatting>
```

### Fixing Unstructured Context

When the user dumps a wall of context without separating it from the task:

**Before:**

```
So we're building this NestJS app with Prisma and PostgreSQL and we use BullMQ for queues and Redis for caching. The app processes LLM responses and we have a ContentBlock type that needs refactoring. Right now it's a mess of if/else statements. Can you help make it better? We use TypeScript strict mode and the codebase follows a monorepo structure with Next.js frontend.
```

**After:**

```xml
<context>
Stack: NestJS + Prisma + PostgreSQL, BullMQ queues, Redis caching
Language: TypeScript (strict mode)
Architecture: Monorepo (Next.js frontend, NestJS backend)
Domain: LLM response processing pipeline
</context>

<instructions>
Refactor the ContentBlock type from if/else chains to a typed discriminated union.
Provide the type definitions and a usage example showing exhaustive pattern matching.
</instructions>

<output_format>
1. New type definitions
2. Refactored handler using switch/exhaustive check
3. Brief explanation of why this is better
</output_format>
```

### Fixing Missing Output Specs

**Before:**

```
Analyze this CSV data and tell me what's interesting.
```

**After:**

```xml
<instructions>
Analyze the CSV data below. Identify the 3 most significant patterns or anomalies.
</instructions>

<document>
{{CSV_DATA}}
</document>

<output_format>
For each finding:
- **Pattern**: one-sentence description
- **Evidence**: specific data points that support it
- **Implication**: why this matters

Conclude with a one-paragraph summary of the overall picture.
</output_format>
```

### Fixing Wall-of-Text Prompts

**Before:**

```
You are an expert code reviewer who specializes in TypeScript and NestJS applications. When I give you code, I want you to review it for bugs, performance issues, security vulnerabilities, and style problems. You should be thorough but not nitpicky. Focus on things that actually matter in production. For each issue you find, tell me what's wrong, why it matters, and how to fix it. If the code is good, just say so. Don't make up problems. Give me the severity of each issue as critical, warning, or info. Return everything as JSON so I can parse it programmatically.
```

**After:**

```xml
<role>
Expert TypeScript/NestJS code reviewer focused on production-readiness.
</role>

<instructions>
Review the provided code for:
1. Bugs and logical errors
2. Performance issues
3. Security vulnerabilities
4. Style and maintainability problems

Focus on issues that impact production. If the code is solid, say so.
</instructions>

<output_format>
Return valid JSON. Schema:
{
  "summary": "one-sentence overall assessment",
  "issues": [
    {
      "severity": "critical | warning | info",
      "line": number,
      "issue": "what's wrong",
      "why": "why it matters",
      "fix": "how to fix it"
    }
  ]
}

If no issues found: {"summary": "...", "issues": []}
</output_format>

<code>
{{CODE_TO_REVIEW}}
</code>
```

### Fixing Anti-Laziness Language

Claude 4.x overtriggers on aggressive language that was needed for older models.

**Before:**

```
CRITICAL: You MUST use this tool when encountering any file. NEVER skip this step. ALWAYS check EVERY file. This is ABSOLUTELY ESSENTIAL.
```

**After:**

```
Use this tool when encountering files to check their structure before making changes.
```

---

## CLAUDE.md / Agent MD File Format

When rewriting CLAUDE.md or agent configuration files, follow this structure:

```markdown
# Agent Name

> One-line description of what this agent does.

## Role

[Who this agent is, its expertise, and behavioral guidelines. 2–5 sentences.]

## Rules

[Non-negotiable behavioral rules. Keep short. Use positive instructions.]

- Always do X before Y
- Return output in Z format
- When uncertain, ask for clarification rather than guessing

## Workflow

[Step-by-step process the agent follows. Numbered for sequential tasks.]

1. Read and understand the input
2. [Step 2]
3. [Step 3]
4. Return result in specified format

## Input Format

[What the agent expects to receive. Include examples.]

## Output Format

[Exactly what the agent produces. Include schema/template.]

## Examples

### Example 1: [Short description]

**Input:**
[example input]

**Output:**
[example output]

## Context

[Any domain knowledge, tech stack details, or reference material the agent needs.]

## Constraints

[Hard limits only. Safety, scope boundaries, things that must never happen.]
```

**Key principles for agent MD files:**

- Lead with the role and rules — Claude reads top-down
- Workflow section replaces vague "do your best" instructions with explicit steps
- Examples are valuable when the agent has a clear, repeatable input/output pattern — include 2+ showing different cases. Skip for agents with open-ended or context-dependent scope
- Separate input format from output format
- Put context/reference material after instructions (front-load task, back-load reference)
- Keep constraints minimal — prefer rules stated as positive instructions
- Organize into distinct sections using XML tags or Markdown headers
- Use `<background_information>`, `<instructions>`, `## Tool guidance`, `## Output description` as section markers

---

## System Prompt Patterns

For API usage, the system prompt should contain ONLY the role and persistent behavioral instructions. Everything task-specific goes in the user message.

### Pattern: Specialist Agent

```
You are a [role] specializing in [domain]. You [key behavior].
When given [input type], you [action] and return [output type].
You follow these rules:
- [Rule 1]
- [Rule 2]
```

### Pattern: Structured Processor

```
You process [input type] and return [output format].

Steps:
1. [Step 1]
2. [Step 2]
3. [Step 3]

Always return valid [format]. Never include explanations unless asked.
```

### Pattern: Multi-Mode Agent

```
You operate in the following modes based on the input:

MODE: review — [behavior for review tasks]
MODE: generate — [behavior for generation tasks]
MODE: debug — [behavior for debugging tasks]

Identify the mode from context. If ambiguous, ask.
```

### Proven System Prompt Blocks (from Anthropic's docs)

**Proactive action** (for agents that should act by default):

```xml
<default_to_action>
By default, implement changes rather than only suggesting them. If the user's
intent is unclear, infer the most useful likely action and proceed, using tools
to discover any missing details instead of guessing.
</default_to_action>
```

**Conservative action** (for agents that should ask first):

```xml
<do_not_act_before_instructions>
Do not jump into implementation unless clearly instructed. When ambiguous,
default to providing information and recommendations rather than taking action.
Only proceed with edits when the user explicitly requests them.
</do_not_act_before_instructions>
```

**Formatting control** (reduce markdown/bullet spam):

```xml
<formatting>
Write in clear, flowing prose using complete paragraphs. Reserve markdown
for inline code, code blocks, and simple headings. Avoid bold/italics.
Do NOT use bullet lists unless presenting truly discrete items or the user
explicitly requests a list. Incorporate items naturally into sentences.
</formatting>
```

**Parallel tool calling:**

```xml
<parallel_tools>
If you intend to call multiple tools with no dependencies between them,
make all independent calls in parallel. Never use placeholders or guess
missing parameters. If calls depend on prior results, run them sequentially.
</parallel_tools>
```

**Anti-overengineering** (critical for Opus models):

```xml
<simplicity>
Only make changes that are directly requested or clearly necessary.
Don't add features, refactor code, or make "improvements" beyond what was asked.
Don't add error handling for scenarios that can't happen. Don't create helpers
or abstractions for one-time operations. The right amount of complexity is
the minimum needed for the current task.
</simplicity>
```

**Anti-hallucination for code agents:**

```xml
<investigate_first>
Never speculate about code you have not opened. If the user references a
specific file, read the file before answering. Investigate and read relevant
files BEFORE answering questions about the codebase.
</investigate_first>
```

**Context window awareness** (for long agentic tasks):

```xml
<context_management>
Your context window will be automatically compacted as it approaches its limit.
Do not stop tasks early due to token budget concerns. As you approach your
limit, save current progress and state to memory before the context refreshes.
</context_management>
```

**Post-tool-use summaries** (Claude 4.5+ is more direct, may skip these):

```
After completing a task that involves tool use, provide a quick summary
of the work you've done.
```

**Frontend anti-AI-slop** (prevents generic "AI aesthetic"):

```xml
<frontend_aesthetics>
Avoid generic, "on distribution" AI design. Make creative, distinctive frontends.
Focus on:
- Typography: beautiful, unique fonts. Avoid Arial, Inter, Roboto.
- Color: commit to a cohesive aesthetic. CSS variables. Dominant colors with
  sharp accents outperform timid, evenly-distributed palettes.
- Motion: staggered reveals (animation-delay) over scattered micro-interactions.
- Backgrounds: create atmosphere and depth, not solid colors.
Avoid: clichéd purple gradients on white, predictable layouts, cookie-cutter design.
</frontend_aesthetics>
```

**Model self-knowledge** (for agents that need to reference themselves):

```
The assistant is Claude, created by Anthropic. The current model is Claude Sonnet 4.5.
When an LLM is needed, default to Claude Sonnet 4.5 unless requested otherwise.
The exact model string is claude-sonnet-4-5-20250929.
```

**Reduce adaptive thinking frequency** (for latency-sensitive tasks):

```
Extended thinking adds latency and should only be used when it will meaningfully
improve answer quality — typically for problems requiring multi-step reasoning.
When in doubt, respond directly.
```

---

## Few-Shot Example Formatting

Always wrap examples in XML tags. Mix up classes for classification tasks.

```xml
<examples>
<example>
<input>The food was terrible and cold</input>
<o>NEGATIVE</o>
</example>
<example>
<input>Absolutely loved the ambiance</input>
<o>POSITIVE</o>
</example>
<example>
<input>It was okay, nothing special</input>
<o>NEUTRAL</o>
</example>
<example>
<input>Worst experience of my life but the dessert was amazing</input>
<o>MIXED</o>
</example>
</examples>
```

**Rules:**

- Minimum 3 examples, ideally 5–6
- Cover edge cases (the "mixed" example above)
- Vary the class order — don't put all positives first
- Match the real-world distribution if possible
- Examples should be diverse in length and complexity

**Critical Claude 4.x warning:** Claude pays extremely close attention to details in examples. If your examples contain behaviors you DON'T want, Claude will replicate them. Ensure examples align exclusively with desired behaviors.

**Include thinking patterns in examples** — use `<thinking>` tags inside your few-shot examples to show Claude the reasoning pattern. It will generalize that style.

**Tool definition examples** — add `input_examples` to tool definitions to teach format conventions. This improved accuracy from 72% to 90% on complex parameter handling in Anthropic's testing:

```json
{
  "name": "create_ticket",
  "input_schema": { "..." },
  "input_examples": [
    {"title": "Login page returns 500", "priority": "critical", "labels": ["bug", "auth"]},
    {"title": "Add dark mode", "labels": ["feature-request"]},
    {"title": "Update API docs"}
  ]
}
```

**Meta-technique:** Ask Claude to evaluate your examples for relevance, diversity, or clarity. Ask Claude to generate additional examples based on your initial set.

---

## Chain of Thought Formatting

**Three levels of CoT with Claude:**

**Level 1 — Basic:** Add "Think step-by-step." Limitation: no guidance on HOW to think.

**Level 2 — Guided:** Outline specific reasoning steps: "First, consider what messaging might appeal to this donor. Then, evaluate which program aspects match their interests."

**Level 3 — Structured (best):** Use XML tags to cleanly separate reasoning from output:

```xml
<instructions>
Reason through the following problem step by step inside <thinking> tags,
then give your final answer inside <answer> tags.
</instructions>

<problem>
{{PROBLEM}}
</problem>
```

**Expected output structure:**

```xml
<thinking>
Step 1: ...
Step 2: ...
Step 3: ...
</thinking>

<answer>
[Clean final answer here]
</answer>
```

**When to use CoT:** Complex math, multi-step analysis, decisions with many factors. When NOT to use: simple factual lookups, straightforward formatting, quick classifications — adds unnecessary latency.

**Extended thinking rules (Claude 4.x API):**

- When extended thinking is ON: **remove all CoT guidance from prompts**. Claude's built-in thinking works without explicit instructions. Prescriptive steps can actually hurt performance.
- When extended thinking is OFF: use manual CoT with `<thinking>` and `<answer>` tags as a fallback.
- Prefer general instructions ("Think thoroughly") over prescriptive step plans when extended thinking is enabled.

**Self-verification:** Add "Before you finish, verify your answer against [test criteria]." Catches errors reliably for coding and math.

**Interleaved thinking** (between tool calls):

```
After receiving tool results, carefully reflect on their quality and determine
optimal next steps before proceeding. Use your thinking to plan and iterate
based on new information.
```

**Reduce overthinking:**

```
Choose an approach and commit to it. Avoid revisiting decisions unless new
information directly contradicts your reasoning.
```

> ⚠️ When extended thinking is disabled, Claude Opus 4.5+ is sensitive to the word "think." Use "consider," "evaluate," or "reason through" instead.

> ⚠️ Claude can be sensitive to argument ordering. Swapping the order (positive-first vs. negative-first) can change the overall assessment. Test both orderings for balanced evaluations.

---

## Long Context Management

Claude supports 200K tokens (Claude 3) to 1M tokens (Claude 4.5/4.6).

**Document placement:** Place long documents (20K+ tokens) at the TOP of the prompt. Place queries, instructions, and examples at the BOTTOM. This gives ~30% quality improvement.

**Ground responses in quotes for long documents:**

```
Find quotes from the patient records relevant to diagnosing the symptoms.
Place these in <quotes> tags. Then, based on these quotes, list diagnostic
information in <info> tags.
```

**Two proven recall techniques:** (1) Extract reference quotes before answering. (2) Include examples of correctly answered questions about other sections of the document.

**For very long contexts:** Include a reminder of the task near the end of the prompt — Claude's attention is strongest at the beginning and end.

**Multi-document tagging:** Use `<document index="1">` with `<source>` and `<document_content>` sub-tags for clean multi-document handling.

**Prompt caching:** For repeated long contexts, use `cache_control` to avoid re-processing. Three modes: Automatic (top level), Explicit (per content block), Hybrid (explicit for system prompt, automatic for conversation).

---

## Context Engineering Principles

Anthropic's key philosophical shift (late 2025): move from prompt engineering (single perfect prompt) to context engineering (managing the entire information environment as a finite resource).

Context = system prompts + tools + examples + conversation history + retrieved context.

**Core principles:**

1. **Context is finite with diminishing returns** — Anthropic calls this "context rot." As tokens increase, recall accuracy decreases. LLMs have an "attention budget" — n² pairwise relationships get stretched thin.

2. **Start minimal, add based on failures** — test a minimal prompt with the best model first. Add instructions only based on observed failure modes, not pre-emptively.

3. **Right altitude** — two failure modes: (a) Too specific = hardcoded brittle if-else logic. (b) Too vague = high-level guidance that fails. Goldilocks: specific enough to guide, flexible enough for heuristics.

4. **Just-in-time context** — maintain lightweight identifiers (file paths, stored queries, web links) and dynamically load data at runtime. Don't pre-load everything.

5. **Progressive disclosure** — agents incrementally discover relevant context through exploration. Each interaction informs the next.

6. **Every token must earn its place** — find the smallest set of high-signal tokens that maximize the likelihood of the desired outcome. Minimal ≠ short, but minimal = no waste.

**Three strategies for long-horizon context management:**

**Compaction:** Summarize conversation nearing limit, reinitiate with summary. Preserve architectural decisions, unresolved bugs, implementation details. Discard redundant tool outputs. Lightest form: tool result clearing — remove raw results from tools called deep in history.

**Structured note-taking:** Agent regularly writes notes persisted to external storage (NOTES.md, progress.txt, tests.json). Notes pulled back in at later times.

**Sub-agent architectures:** Specialized sub-agents handle focused tasks with clean context windows. Each may explore with tens of thousands of tokens, but returns only 1,000–2,000 token condensed summaries.

### State Management Templates (for multi-session agentic work)

**Progress tracking file:**

```
// progress.txt
Session 3 progress:
- Fixed authentication token validation
- Updated user model to handle edge cases
- Next: investigate user_management test failures (test #2)
- Note: Do not remove tests as this could lead to missing functionality
```

**Test state tracking:**

```json
// tests.json
{
  "tests": [
    { "id": 1, "name": "authentication_flow", "status": "passing" },
    { "id": 2, "name": "user_management", "status": "failing" },
    { "id": 3, "name": "api_endpoints", "status": "not_started" }
  ],
  "total": 200,
  "passing": 150,
  "failing": 25,
  "not_started": 25
}
```

**Multi-context window startup prompt:**

```
Call pwd; you can only read and write files in this directory.
Review progress.txt, tests.json, and the git logs.
Manually run through a fundamental integration test before moving
on to implementing new features.
```

**Research agent prompt template:**

```
Search for this information in a structured way. As you gather data,
develop several competing hypotheses. Track your confidence levels
in your progress notes to improve calibration. Regularly self-critique
your approach and plan. Update a hypothesis tree or research notes file
to persist information. Break down this complex research task systematically.
```

**Context budget management:**

```
This is a very long task, so plan your work clearly. Spend your entire
output context working on the task — just make sure you don't run out
of context with significant uncommitted work. Continue working
systematically until you have completed this task.
```

---

## Agentic Prompt Patterns

### Tool Description Prompting

Tool descriptions deserve the same prompt engineering care as user-facing prompts. Write them as if describing the tool to a new hire — make implicit knowledge explicit.

```xml
<tool name="search_codebase">
<description>
Search the project codebase for files, functions, or patterns.
Use this BEFORE making changes to understand existing code structure.
</description>
<when_to_use>
- Finding where a function/type/component is defined
- Understanding how a module is used across the codebase
- Locating configuration files or environment variables
</when_to_use>
<parameters>
- query: search string (file name, function name, or grep pattern)
- scope: "all" | "src" | "tests" | "config"
</parameters>
</tool>
```

**Document return formats in tool descriptions:**

```json
{
  "name": "get_orders",
  "description": "Retrieve orders for a customer.\nReturns:\n  List of order objects:\n  - id (str): Order identifier\n  - total (float): USD\n  - status (str): 'pending' | 'shipped' | 'delivered'\n  - items (list): [{sku, quantity, price}]"
}
```

### The Think Tool

A dedicated tool for structured reasoning during complex multi-step tasks. Produced 54% relative improvement on complex benchmarks:

```json
{
  "name": "think",
  "description": "Use this tool to think about something. It will not obtain new information or change the database, but just append the thought to the log. Use it when complex reasoning or some cache memory is needed.",
  "input_schema": {
    "type": "object",
    "properties": {
      "thought": { "type": "string", "description": "A thought to think about." }
    },
    "required": ["thought"]
  }
}
```

**Think tool prompt template:**

```
Before taking any action or responding after receiving tool results, use the
think tool as a scratchpad to:
- List specific rules that apply to the current request
- Check if all required information is collected
- Verify the planned action complies with all policies
- Iterate over tool results for correctness
```

**Think tool vs. extended thinking:** Extended thinking = deep pre-planning BEFORE responding. Think tool = structured reasoning DURING response, between tool calls. Use think tool for complex tool chains, policy-heavy environments, sequential decisions. Use extended thinking for coding, math, physics.

### Tool Consolidation

More tools ≠ better outcomes. If a human can't definitively say which tool to use, neither can the agent.

**Consolidate:** Instead of `list_users` + `list_events` + `create_event`, build `schedule_event` (finds availability + schedules).

**Namespace by service:** `asana_search`, `jira_search` or `asana_projects_search`, `asana_users_search`.

**Expose response format control:** Add `response_format` enum: `"concise"` (IDs stripped, ~⅓ tokens) vs. `"detailed"` (full IDs for downstream calls).

**Optimize for token efficiency:** Implement pagination, filtering, truncation with sensible defaults. Cap tool responses at ~25,000 tokens. For truncated responses: "Results truncated to first 20 matches. Use filters: date_range, status, priority to narrow search."

### Tool Search for Scale (10+ tools)

Use `defer_loading: true` to avoid stuffing all definitions into context. Load only 3–5 most-used tools; Claude discovers others on-demand via search tool. Reduces token usage by 85% while maintaining full access.

### Error Message Engineering

Make errors actionable — the agent reads these:

```
BAD:  "Error: invalid input"
BAD:  "Error 400: Bad Request"
GOOD: "Invalid date format. Expected: YYYY-MM-DD. Received: '03-15'.
       Example: search_logs(start_date='2025-03-15')"
```

### Self-Improving Tools

Let agents optimize their own tools. Give Claude evaluation transcripts + tool definitions. A tool-testing agent used tools dozens of times, found nuances, and rewrote descriptions — achieving 40% decrease in task completion time for future agents.

### Orchestrator-Worker Pattern

```xml
<role>
You are an orchestrator agent. You break complex tasks into subtasks,
delegate to specialized workers, and synthesize their results.
</role>

<instructions>
Given a task:
1. Decompose into 2–5 independent subtasks
2. For each subtask, specify which worker handles it
3. Collect results
4. Synthesize into final output
5. Verify the final output addresses the original task
</instructions>

<workers>
- code_writer: generates new code
- code_reviewer: reviews code for bugs and style
- test_writer: generates test cases
- doc_writer: generates documentation
</workers>
```

---

## Prompt Chaining Patterns

Break complex tasks into sequential subtasks rather than one mega-prompt. Each step's output feeds the next via XML-tagged handoffs.

**When to chain:** Multi-step tasks involving multiple transformations, citations, or instructions. Chaining prevents Claude from dropping or mishandling steps.

**Standard pipeline patterns:**

- Content creation: Research → Outline → Draft → Edit → Format
- Data processing: Extract → Transform → Analyze → Visualize
- Decision-making: Gather info → List options → Analyze each → Recommend
- Verification: Generate → Review → Refine → Re-review

**Self-correction chain (most common pattern):**

```
Prompt 1 (Generate): "Summarize this research paper. Focus on methodology,
findings, and clinical implications."

Prompt 2 (Review): "Review this summary for accuracy, clarity, and
completeness on an A-F scale."

Prompt 3 (Improve): "Update the summary based on the feedback."
```

**Handoff structure:**

```xml
Step 1 output: <research_output>{{result from research prompt}}</research_output>
Step 2 input:  Using the research in <research_output>, generate the final report.
```

**Rules:**

1. Single-task goal per step — one clear objective each
2. XML tags for clean handoffs between steps
3. Run independent subtasks in parallel
4. Gate steps to check intermediate outputs before proceeding
5. Isolate failing steps for debugging without redoing the whole chain

**Note for Claude 4.x:** With adaptive thinking and subagent orchestration, Claude handles most multi-step reasoning internally. Explicit chaining is still useful when you need to inspect intermediate outputs or enforce a specific pipeline.

---

## Multi-Agent Patterns

**Architecture decision framework** (simplest to most complex — only escalate when needed):

1. **Single LLM call** + retrieval + in-context examples
2. **Workflow patterns** (chaining, routing, parallelization) — add only when demonstrably needed
3. **Agent** (tool loop) — for open-ended problems with unpredictable steps
4. **Multi-agent** — for tasks requiring heavy parallelization or exceeding single context limits

### Five Workflow Patterns

**Prompt chaining:** Sequential subtasks, each call builds on prior result. Gate steps check intermediate outputs. Use when task naturally decomposes into fixed subtasks.

**Routing:** Classify input → direct to specialized handler. Use when distinct categories need separate prompts.

**Parallelization:** Sectioning (independent subtasks simultaneously) or Voting (same task multiple times for diverse outputs).

**Orchestrator-workers:** Central LLM dynamically breaks down tasks, delegates. Subtasks aren't pre-defined. Use for complex tasks where subtasks can't be predicted.

**Evaluator-optimizer:** One LLM generates, another evaluates and provides feedback in a loop. Use when clear evaluation criteria exist.

### Scaling Rules (embed in orchestrator prompts)

| Query Complexity    | Agents        | Tool Calls                       |
| ------------------- | ------------- | -------------------------------- |
| Simple fact-finding | 1 agent       | 3–10 calls                       |
| Direct comparisons  | 2–4 subagents | 10–15 each                       |
| Complex research    | 10+ subagents | Clearly divided responsibilities |

### Key Multi-Agent Findings from Anthropic

- Opus 4 lead + Sonnet 4 subagents outperformed single-agent Opus 4 by 90.2%
- Token usage explains 80% of performance variance
- Parallelization cuts research time by up to 90% for complex queries
- Each subagent should return only 1,000–2,000 token condensed summaries
- Use cheaper models (Haiku/Sonnet) as sub-agents for quick tasks
- Subagents should write to filesystem, not pass everything through orchestrator
- Upgrading model quality (Sonnet 3.7 → Sonnet 4) = larger gain than doubling token budget

### Real-World Architecture (from Anthropic's multi-agent research system)

```
LeadResearcher (Opus)
  ├── Analyzes query → develops strategy → saves plan to Memory
  ├── Spawns Subagents (Sonnet) in parallel
  │     ├── Subagent 1: searches independently using interleaved thinking
  │     ├── Subagent 2: searches independently
  │     └── Subagent N: searches independently
  ├── Collects findings → decides if more research needed
  │     └── If yes → spawns more subagents
  └── Passes to CitationAgent for final output
```

**Key delegation rules for orchestrators:**

- Each subagent needs: objective, output format, tool/source guidance, clear boundaries
- Without detailed task descriptions, agents duplicate work and leave gaps
- Teach agents to "start wide then narrow" — they default to overly specific queries

### Automatic Prompt Optimization

**Metaprompt:** Anthropic's own system for using Claude to generate prompts for Claude. Available in the cookbooks repo. Its opening instruction: "Today you will be writing instructions to an eager, helpful, but inexperienced and unworldly AI assistant who needs careful instruction and examples." Run with temperature=0, max_tokens=4096.

**Prompt Improver (built into Anthropic Console):** Follows a 4-step process:

1. Identifies and extracts examples from your prompt
2. Creates a structured template with clear sections and XML tags
3. Adds and refines chain-of-thought reasoning instructions
4. Updates examples to demonstrate the new reasoning process

Results: 30% accuracy increase on multilabel classification, 100% word count adherence on summarization.

**Self-improving agents:** Give Claude evaluation transcripts + tool/prompt definitions. It diagnoses failures and rewrites descriptions. Produced 40% decrease in task completion time.

---

## Model & Effort Selection (Opus 4.7 / Sonnet 4.6)

As of April 2026, Claude Opus 4.7 and Claude Sonnet 4.6 are the recommended models, and the effort parameter (`low`/`medium`/`high`/`xhigh`/`max`) is the primary control for reasoning depth. `xhigh` is new in Opus 4.7 and is Claude Code's documented default. Anthropic explicitly warns that `max` is prone to overthinking on structured-output tasks.

Sources: [Effort docs](https://platform.claude.com/docs/en/build-with-claude/effort), [What's new in Opus 4.7](https://platform.claude.com/docs/en/about-claude/models/whats-new-claude-4-7), [Choosing a model](https://platform.claude.com/docs/en/about-claude/models/choosing-a-model), [Best practices for Opus 4.7 with Claude Code](https://claude.com/blog/best-practices-for-using-claude-opus-4-7-with-claude-code).

### Effort tiers

| Tier     | Opus 4.7                   | Sonnet 4.6                          | Anthropic's guidance                                                                                                                |
| -------- | -------------------------- | ----------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| `max`    | ✓                          | ✓                                   | Frontier problems and eval ceiling-testing only. "Prone to overthinking on structured-output or less intelligence-sensitive tasks." |
| `xhigh`  | ✓ (default in Claude Code) | ✗ (not available)                   | Coding and agentic work. "Strong autonomy and intelligence without the runaway token usage that max can produce."                   |
| `high`   | ✓ (API default)            | ✓ (API default)                     | Sweet spot for balance. Preferred for wide fan-out concurrent workloads.                                                            |
| `medium` | ✓                          | ✓ (Anthropic's recommended default) | "Best balance of speed, cost, and performance. Suitable for agentic coding, tool-heavy workflows, code generation."                 |
| `low`    | ✓                          | ✓                                   | Simple classification, quick lookups, subagents. Pair with explicit checklists when task has multiple sections.                     |

**Key rules:**

- **`xhigh` replaces what `max` used to mean** on Opus 4.7. Treat `max` as a narrow eval tier, not a quality tier.
- **Opus 4.7 respects low/medium more strictly** than 4.6. If reasoning looks shallow, raise effort instead of prompting around it.
- **Sonnet 4.6 defaults to `high`** on the API but Anthropic recommends explicitly setting `medium` to avoid unexpected latency.
- **Manual `thinking: {budget_tokens: N}` is rejected on Opus 4.7** — only `thinking: {type: "adaptive"}` works. Effort replaces the budget.
- **Opus 4.7 tokenizer uses 1.0–1.35× more tokens** than Opus 4.6. Set `max_tokens` at 64k for `xhigh`/`max` on 4.7.

### Model selection matrix

From Anthropic's "Choosing the right model" docs:

| Task shape                                                          | Model                     | Anthropic's language                                                                       |
| ------------------------------------------------------------------- | ------------------------- | ------------------------------------------------------------------------------------------ |
| Agentic coding across files, planning, self-verifying, long-horizon | **Opus 4.7**              | "Long-horizon agentic coding, large-scale refactoring, complex systems engineering"        |
| Code generation from a spec, tool-heavy workflows, structured tasks | **Sonnet 4.6**            | "Code generation, data analysis, content creation, visual understanding, agentic tool use" |
| Multi-agent orchestration (dispatches ≥3 subagents, merges outputs) | **Opus 4.7 (lead)**       | "Coordinating multiple agents in a workflow" reserved for Opus                             |
| Focused worker/specialist inside an orchestrated pipeline           | **Sonnet 4.6 (subagent)** | "Opus lead + Sonnet subagents" outperformed solo Opus by 90.2% (Anthropic's internal eval) |

**Decision framework** (apply top-down, first match wins):

1. Does the agent dispatch ≥3 subagents and merge contradictory outputs? → **Opus**
2. Does the agent make architectural decisions consumed by other agents? → **Opus**
3. Does the agent produce structured findings from bug-finding / code review? → **Opus** (Anthropic measured +11pp recall on real PR bug-finding with 4.7)
4. Does the agent enumerate a known pattern across files (does every X have Y)? → **Sonnet**
5. Does the agent generate code from a given plan/spec? → **Sonnet**
6. Does the agent run tools, parse output, and report? → **Sonnet**
7. Is it a simple classification or lookup? → **Sonnet at `low`**

### Review / audit prompt patterns (critical for Opus 4.7)

Opus 4.7 follows suppression-style instructions more literally than prior models. Phrases that were heuristic on 4.6 now actively silence findings. **Audit and remove suppression language from review/audit agent prompts before promoting them to Opus 4.7.**

**Suppression phrases to find and replace:**

- "only report critical issues"
- "be conservative"
- "don't nitpick"
- "focus on high-severity only"
- "skip minor issues"
- "only flag important problems"

**Coverage-first replacement** (Anthropic's recommended language):

```text
Report every issue you find, including ones you are uncertain about or
consider low-severity. Do not filter for importance or confidence at this
stage — a separate verification step will do that. Your goal here is
coverage: it is better to surface a finding that later gets filtered out
than to silently drop a real bug. For each finding, include your confidence
level and an estimated severity so a downstream filter can rank them.
```

This pattern assumes a **downstream filter exists** (majority voting, pre-existing-filter, human review). If there's no downstream filter, add one rather than suppress at the finding stage.

### Behavioral changes in Opus 4.7 that affect prompts

- **More literal instruction following** — no silent generalization from one item to another. Audit prompts for phrases like "usually" or "try to" that the model may now interpret narrowly.
- **Fewer tool calls by default** at a given effort. Raising effort increases tool usage.
- **Fewer subagents spawned by default**. Steerable via prompting.
- **More regular progress updates** during long agentic traces — remove scaffolding that forced interim status messages, Opus 4.7 does this natively.
- **Thinking content `display` defaults to `"omitted"`** on 4.7 (vs `"summarized"` on 4.6). Set `thinking.display: "summarized"` explicitly if you need reasoning visible.
- **Real-time cybersecurity safeguards** may refuse legitimate security work. Apply to the [Cyber Verification Program](https://claude.com/form/cyber-use-case) for OWASP audits and similar.
- **Response length calibrates to perceived task complexity** rather than defaulting to fixed verbosity. Remove "be thorough" / "be concise" scaffolding if the task already signals the right length.

### Configuration quick reference

```yaml
# Opus 4.7 agent (reasoning-heavy, orchestrator, architect, review)
model: inherit
effort: xhigh  # NOT max — max overthinks structured outputs
# max_tokens: 64000 recommended for xhigh/max on 4.7

# Sonnet 4.6 agent (code generation, tool use, enumeration, communication)
model: sonnet
effort: medium  # Anthropic's recommended default — explicitly set to avoid API default of high

# Sonnet 4.6 subagent (classification, lookup, single-pass check)
model: sonnet
effort: low  # Pair with explicit checklist when task has multiple sections
```

---

## Claude 4.x Gotchas

| Gotcha                                                     | What to do                                                                                                                                               |
| ---------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **`max` effort overthinks on structured outputs** (4.7)    | Reserve `max` for frontier problems and eval ceiling-testing. Use `xhigh` as the go-deep default on Opus 4.7.                                            |
| **`xhigh` is the new Claude Code default on Opus 4.7**     | Anthropic positions `xhigh` as "best for most coding and agentic uses." Treat it as the default, not max.                                                |
| **Opus 4.7 respects low/medium strictly**                  | Model scopes work tightly at low/medium. Don't run review/audit agents at these tiers — they'll under-report. Raise effort, don't prompt around it.      |
| **Opus 4.7 literally follows suppression instructions**    | "Be conservative" / "only critical" / "don't nitpick" now actively silence findings. Replace with coverage-first prompts and rely on downstream filters. |
| **Manual `budget_tokens` rejected on Opus 4.7**            | Only `thinking: {type: "adaptive"}` works. Effort parameter replaces the budget.                                                                         |
| **Thinking `display` defaults to `"omitted"` on Opus 4.7** | Set explicitly to `"summarized"` if your UI surfaces reasoning.                                                                                          |
| **Opus 4.7 tokenizer uses 1.0–1.35× more tokens**          | Bump `max_tokens` — Anthropic recommends 64k for `xhigh`/`max` on 4.7.                                                                                   |
| **Opus 4.7 added cyber safeguards**                        | Real-time refusals on security topics. Apply to the Cyber Verification Program for legitimate OWASP work.                                                |
| **Prefilling is dead** (4.6+)                              | Can't start response with `{` or `<tag>`. Use explicit format instructions: "Return valid JSON only."                                                    |
| **Overly literal instruction following**                   | Claude takes examples at face value. Explicitly say "feel free to add relevant details not listed here" if you want thoroughness.                        |
| **Anti-laziness language backfires**                       | "CRITICAL: You MUST..." → newer models overtrigger. Use calm, direct instructions.                                                                       |
| **Word "think" triggers behavior**                         | When extended thinking is off, avoid "think." Use "consider," "evaluate," "reason through."                                                              |
| **Context window awareness**                               | Claude 4.5+ tracks remaining context internally. For long agentic tasks, it manages its own budget.                                                      |
| **Adaptive thinking > budget_tokens**                      | Use `"effort": "xhigh"` (Opus 4.7) or `"effort": "medium"` (Sonnet 4.6) instead of manual `budget_tokens`.                                               |
| **Opus overengineers at max**                              | Always include anti-overengineering guidance in system prompts for coding tasks, and avoid `max` — the overengineering compounds.                        |
| **More direct communication**                              | Claude 4.5+ is more direct, may skip summaries unless prompted. Add "provide a quick summary after completing tool use."                                 |
| **Proactive delegation**                                   | Claude 4.5+ may spawn subagents without being asked. Opus 4.7 is more judicious by default but still constrain: "Only delegate when clearly beneficial." |
| **1M token context**                                       | Generally available for Opus 4.7, Opus 4.6, and Sonnet 4.6.                                                                                              |
| **LaTeX defaults for math**                                | Claude defaults to LaTeX. For plain text: "Format math using standard text characters (/, \*, ^). No LaTeX."                                             |
| **Bedrock/Vertex alias mismatch**                          | On Bedrock/Vertex, `opus` resolves to Opus 4.6 and `sonnet` to Sonnet 4.5. Use full model names (`claude-opus-4-7`, `claude-sonnet-4-6`) explicitly.     |

---

## Common Mistakes & Fixes

| #   | Mistake                                                 | Fix                                                                                                        |
| --- | ------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- |
| 1   | Not outputting thinking                                 | Without outputting thought process, no thinking occurs. Always use `<thinking>` tags or extended thinking. |
| 2   | Using "think" with extended thinking disabled           | Replace with "consider," "evaluate," "reason through"                                                      |
| 3   | Prescribing thinking steps when extended thinking is ON | Remove CoT guidance from prompts. Let built-in thinking work.                                              |
| 4   | Aggressive tool-triggering language                     | "CRITICAL: You MUST use..." → dial back to "Use this tool when..."                                         |
| 5   | Unintended patterns in examples                         | Claude 4.x replicates example behaviors exactly. Audit examples for unwanted patterns.                     |
| 6   | CoT for simple tasks                                    | Adds latency without improving accuracy. Skip for factual lookups and formatting.                          |
| 7   | Mixing instructions, data, and examples without XML     | Claude confuses them. Use XML tags to delineate.                                                           |
| 8   | Not isolating failed steps in chains                    | Isolate the failing step in its own prompt for debugging.                                                  |
| 9   | Expecting "above and beyond" behavior                   | Claude 4.x follows instructions literally. Explicitly request thoroughness.                                |
| 10  | Ordering bias in argumentation                          | The order of arguments can influence the conclusion. Test both orderings.                                  |
| 11  | Writing tool descriptions like API docs                 | Write for an agent, not a developer. Make implicit knowledge explicit.                                     |
| 12  | Too many tools                                          | If a human can't tell which tool to use, neither can Claude. Consolidate.                                  |
| 13  | Opaque error responses                                  | "Error 400" → sends agents in circles. Return specific, actionable fixes.                                  |

---

## Output Format Patterns

### JSON Output

```xml
<output_format>
Return valid JSON only. No markdown code fences. No explanation.
Schema:
{
  "field": "type — description",
  "items": ["type — description"]
}
</output_format>
```

### Markdown Output

```xml
<output_format>
Return as markdown with:
- H2 headers for main sections
- Code blocks with language tags
- No introductory preamble — start directly with content
</output_format>
```

### Structured Analysis

```xml
<output_format>
Return your analysis as:

**Summary:** [1-2 sentences]

**Findings:**
1. [Finding with evidence]
2. [Finding with evidence]

**Recommendation:** [Actionable next step]
</output_format>
```

### Parseable Tags

```xml
<output_format>
Return your response in these tags:
<classification>[LABEL]</classification>
<confidence>[0.0-1.0]</confidence>
<reasoning>[Brief explanation]</reasoning>
</output_format>
```

### Plain Text Math (override LaTeX default)

```xml
<output_format>
Format in plain text only. No LaTeX, MathJax, or markup notation.
Write math using standard text: "/" for division, "*" for multiplication,
"^" for exponents.
</output_format>
```

---

## Temperature & Config Guidelines

| Task Type                        | Temperature | Notes                             |
| -------------------------------- | ----------- | --------------------------------- |
| Classification, extraction, math | 0           | Single correct answer             |
| Code generation                  | 0–0.2       | Deterministic, minimal creativity |
| Rewriting, summarization         | 0.2–0.4     | Slight variation acceptable       |
| Creative writing, brainstorming  | 0.7–1.0     | Diversity desired                 |
| Self-consistency (multiple runs) | 0.7–1.0     | Need diverse reasoning paths      |

Claude API exposes `temperature` and `top_p` (not `top_k`). For most tasks, temperature alone is sufficient.

**Extended thinking / adaptive mode:**

```json
{
  "thinking": { "type": "adaptive" },
  "output_config": { "effort": "high" }
}
```

Effort levels: low / medium / high / max. Prefer `adaptive` over manual `budget_tokens`.

---

## Quality Checklist

Run every rewritten prompt through this checklist before finalizing:

- [ ] **Structured with XML tags** — distinct sections are wrapped
- [ ] **Starts with action verb** — instructions lead with what to do
- [ ] **Output format specified** — Claude knows exactly what to produce
- [ ] **No vague language** — no "help me with," "something like," "make it better"
- [ ] **No unnecessary constraints** — "don't" statements rewritten as positive instructions
- [ ] **No anti-laziness language** — no "CRITICAL," "NEVER skip," "You MUST"
- [ ] **Examples included** (when scope is clear) — diverse, with edge cases. Skip for open-ended agents
- [ ] **Examples audited** (if included) — no unintended patterns Claude could replicate
- [ ] **Variables use `{{DOUBLE_BRACKETS}}`** — dynamic content is parameterized
- [ ] **Context separated from instructions** — reference material in `<context>`/`<document>`, task in `<instructions>`
- [ ] **Documents first, query last** — in long contexts, reference material before task
- [ ] **WHY is explained** — instructions include motivation, not just rules
- [ ] **Right altitude** — specific enough to guide, flexible enough for judgment
- [ ] **Colleague test** — would a coworker understand the task with zero context?
- [ ] **Appropriate length** — as short as possible, as long as necessary
- [ ] **No "think" when extended thinking is off** — use "consider," "evaluate," "reason through"
- [ ] **Anti-overengineering included** (for Opus coding tasks)
