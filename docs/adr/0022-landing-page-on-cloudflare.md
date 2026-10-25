# 0022. The landing page lives in `site/`, on Cloudflare Workers

**Status:** Accepted **Date:** 2026-10-25 **Approved by:** @sanamhub **Relates to:** ADR 0002

## Context

The repository is public before 1.0. A README explains the project to someone who already found it;
a landing page has to do that for someone who has not, in one screen, and send them to the spec or
the repository.

The page is static: copy, one terminal, small drawings, no form and no account. It has no server
work to do. There is no domain yet, and no Cloudflare token should be stored in GitHub. The
maintainer already deploys another project the same way, so the operational cost is known.

## Decision

The page is an Astro project in `site/`, static output, one landing page and a 404. The docs stay in
the repository, and every docs link goes to GitHub. There is no docs framework.

Cloudflare Workers static assets serve `dist/` through `site/wrangler.jsonc`, with no Worker code.
There is no domain yet, so `workers_dev` is true and there are no `routes`. The site URL is one
constant in `site/src/site.ts`; a domain change edits it and adds one route. Workers Builds deploys
from the repository, so no Cloudflare token goes into GitHub.

The page sets no cookie, runs no analytics, sends no telemetry, and requests nothing from another
origin. Fonts are self-hosted from pinned packages. A GitHub workflow installs the committed lock
file, checks the types and builds the site, and runs only when `site/` or the workflow changes.

Rejected: GitHub Pages, because it serves no custom headers and ties deployment to repository
settings. A Worker with code, because nothing on the page needs a server. A docs framework, because
the docs already live in the repository and a copy would drift.

## Consequences

- Two hosts: GitHub for the docs, Cloudflare for the page. Every page link to a document leaves the
  site.
- A domain change touches `SITE_URL`, one `routes` entry, and the canonical URLs the build writes.
- There is no analytics to maintain, and nothing to disclose as tracking.
- The deployment belongs to Workers Builds. This repository cannot deploy the page with a token,
  which is the point.
- Revisit if the page needs a redirect or a response header that `_headers` cannot express.
