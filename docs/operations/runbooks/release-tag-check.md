---
title: Release tag check runbook
kind: runbook
status: active
owner: workspace-maintainers
last_reviewed: 2026-11-09
review_by: 2027-02-01
supersedes: []
---

# Release tag check

Before release, fetch package tags and check out the intended clean source commit.
Use the repository's reviewed Rust toolchain. Cargo package listing requires no
build or dependency resolution. Temporary detached worktrees require a writable
Git common directory and are removed automatically.

```bash
git fetch origin --tags
python3 scripts/generate-release-manifest.py \
  --release-id workspace-2099.01.01.1 --ref HEAD \
  --check-clean --verify-tags --output /tmp/release-tag-check.json
scripts/check-package-release-policy.sh origin/main
```

A failure naming `<package>-v<version>` means its Cargo-selected source differs
from that immutable tag. Inspect both file lists using
`cargo package --list -p <package> --allow-dirty --no-verify` at each ref, then
compare contents, including effective inherited manifest values. The
[published-source reference](../../reference/published-source.md) defines generated
file handling and the additional storage vendor gate.

Excluded tests and fixture lockfiles need no bump. Included README, license,
build script, source, and tests do. A workspace dependency requirement change can
alter an inheriting package's effective manifest even if its local manifest did
not change. Under option B, inherited workspace-sibling version requirement
changes alone are exempt while all other sibling fields and external requirements
remain strict. See [#435](https://github.com/Dastari/graphql-orm/issues/435) for the
pending maintainer decision. `WORKSPACE_SIBLING_REQUIREMENT_DRIFT` can be changed
from `"exempt"` to `"strict"` in `scripts/published_source.py` to select option A;
then reconcile existing drift before attempting release.

Resolve a real published-source difference with an owner-reviewed
version and release documentation update. Escalate unexpected Cargo selection
or inherited dependency changes to the package owner; do not move existing tags.

The preview only writes the requested local output file. Remove it to roll back
that output; no tags or release assets are created. The full Git tree identities
remain in the release manifest for provenance even when excluded files differ.
