# Changelog

User-facing release notes live in [docs/release-notes.md](docs/release-notes.md).

## 0.2.21

- Added `AuthSubject`, upgraded `AuthExt`, exact-scope `ScopeEntityPolicy`, and `DbAuthContext`
  constructors.
- Added generated resolver auth modes on entities and schema roots.
- Kept `auth_user()` as a deprecated alias for source compatibility.
- Added a reserved optional `auth-agql` feature; concrete agql-auth converters are deferred until
  the upstream agql-auth 0.7 API is tagged.

See the 0.2.21 section in [docs/release-notes.md](docs/release-notes.md#0221) and the migration
guide in [MIGRATION.md](MIGRATION.md).
