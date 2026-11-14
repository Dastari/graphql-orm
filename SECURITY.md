---
title: Security reporting
kind: reference
status: active
owner: workspace-maintainers
last_reviewed: 2026-11-14
review_by: 2027-02-01
supersedes: []
---

# Security reporting

Report suspected vulnerabilities through [GitHub private vulnerability reporting](https://github.com/Dastari/graphql-orm/security/advisories/new).
Include the affected revision, enabled features, reproduction, and impact.
Do not put credentials, application data, or exploit details in public issues.

Security fixes target current main. Consumers should review the release manifest,
package migration notes, and [dependency dispositions](docs/reference/dependency-security-disposition.md)
when upgrading. Old releases have no guaranteed backport support.
