---
title: GraphQL ORM agent skills
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-11-01
review_by: 2027-02-01
supersedes: []
---

# GraphQL ORM agent skills

Skills tell a coding agent working in a consuming project which approach to choose. The
[reference documentation](../docs/reference/graphql-orm/README.md) remains the authority on what each
feature does.

| Skill | Load it when |
| --- | --- |
| [`graphql-orm-usage`](graphql-orm-usage/SKILL.md), with [complete examples](graphql-orm-usage/examples.md) | Adding or changing an entity, resolver, relation, subscription, policy, hook or write path, or reviewing a graphql-orm service |

## Install

Packages here are distributed by Git tag, not from a registry, so there's no package to run an
installer from. A short script fetches the skill at the tag you pin:

```sh
TAG=workspace-YYYY.MM.DD.N   # the release you pin: its workspace tag or full commit SHA
curl -fsSL "https://raw.githubusercontent.com/Dastari/graphql-orm/$TAG/skills/install.sh" \
  | sh -s -- --target claude --tag "$TAG"
```

| `--target` | Installs into |
| --- | --- |
| `claude` | `.claude/skills/graphql-orm-usage/` |
| `agents`, `codex` | `.agents/skills/graphql-orm-usage/` |

These match graphql-orm-client's `skills install --target` locations, so both sets of skills sit
together.

Run it from your project root, and commit the result. To upgrade, change `TAG` to your new pin and add
`--update`; without it, the script refuses to replace an existing copy. It changes nothing else in your
project.

Relative links in the skill are rewritten to GitHub links at the same tag, so the installed copy points
at the documentation for your version.

The skill must exist at the release you name. It first ships in the release after
`workspace-2026.11.01.1`.

To read the script before running it, download it from the same URL. Release tags are immutable, so it
can't change afterwards.

## Maintain

- A skill is governed Markdown. `SKILL.md` and `examples.md` carry the [documentation metadata](../docs/decisions/ADR-0001-documentation-authority-and-lifecycle.md),
  and `SKILL.md` also has the `name` and `description` that agents read.
- Keep mechanics in the reference documents, and link to them with relative paths. The installer depends on
  relative links starting `../../`.
- [`verify/`](verify/) compiles and runs every `rust` block in both files as a doctest, and
  `verify/tests/contract.rs` pins each library behaviour the skill states. Label every other code block
  with its language, because rustdoc treats an unlabelled block as Rust. When the library changes a pinned
  behaviour, the contract fails: update the skill in the same branch, and remove the matching issue link
  when a gap closes.
- `verify/` is a standalone crate with its own `Cargo.lock`, kept outside `crates/` so that it adds nothing
  to a package's released source. Update its lockfile with the other fixture lockfiles when a package
  version changes.

```sh
cargo test --manifest-path skills/verify/Cargo.toml --target-dir target/fixtures --locked
scripts/test-skill-install.sh
```
