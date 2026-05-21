---
title: Releases and npm Packages
description: Release tags, GitHub releases, and trusted npm publishing.
order: 110
---

# Releases and npm packages

qpayd releases are tag-driven. GitHub Actions builds the Rust binary release,
and the npm workflow publishes browser packages through trusted publishing.

Published browser packages:

- `@qpayd/checkout`
- `@qpayd/admin`

The npm packages use GitHub Actions OIDC trusted publishing. Keep the workflow
file name stable so npm trust remains valid:

```text
.github/workflows/npm-publish.yml
```

Use PRs for release changes so version bumps, release notes, and package
changes have an audit trail.
