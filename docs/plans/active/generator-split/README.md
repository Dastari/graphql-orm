---
title: Generator and authorization pipeline split
kind: plan
status: active
owner: workspace-maintainers
last_reviewed: 2026-11-15
review_by: 2027-02-01
supersedes: []
---

# Generator and authorization pipeline split

## Outcome

Implement #504 through two scoped follow-ups:
[operation-kind extraction (#723)](https://github.com/Dastari/graphql-orm/issues/723)
and [runtime authorization pipeline (#724)](https://github.com/Dastari/graphql-orm/issues/724).
Queries and mutations acquire separate emission functions; a few runtime
pipeline functions own the repeated write authorization sequence.

## Non-goals

No API cleanup, feature renaming, AI module move, fingerprint change or broad
rewrite. Do not combine mechanical extraction with moving authorization.

## Dependencies

Finish concurrent generator correctness changes first. Extract read/query
emission, then each mutation family, preserving token output. Runtime pipeline
work follows extraction, one family per reviewed PR. Entity/field/row policy,
principal refresh, transforms, transaction boundaries, journals, notifications
and dependent effects remain explicit acceptance contracts.

## Acceptance gates

Compare generated APIs, SDL, schema export and catalogue fingerprints before
and after extraction. Exercise success, refusal, cancellation, rollback and
post-commit effects on SQLite and owned PostgreSQL/MSSQL lanes. Run warnings-denied
Clippy/Rustdoc and measure compile cost. Review authorization order for every
moved family; no security change may hide in the extraction diff.

## Current checkpoint

Maintainer recommendation accepted on 2026-11-15. Scoped issues #723 and #724
own implementation; no generator or authorization code moves in this PR.
