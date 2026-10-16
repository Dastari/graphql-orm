---
title: Owner-local SMB dependency review addendum
kind: investigation
status: archived
owner: workspace-maintainers
last_reviewed: 2026-10-15
review_by: none
supersedes: []
---

# Owner-local SMB dependency review addendum

This later collection is complete **with findings**, not clean. It supplements
rather than replaces the original eight raw reports and their immutable 84e7
source identities. The [machine-readable index](smb-addendum.json) records exact
selectors, seven workspace/fixture locks plus auth's unchanged lock, hashes,
source mappings and advisory identities.

The actual candidate is `5d155bab70fc3c2369507b9c6b402a8c065a8e65`, tree
`7ddd83ba45a6d70d1f0b86326305b55a3f8b65c5`, carrying storage 0.7.4. Root and
backend-coexistence locks change only the reviewed storage version and SMB
registry-to-owner-local identity mapping; six unchanged inputs reuse their
actual earlier raw audits. All 67 selected normal/build/dev/feature profiles
match after the explicitly reviewed local identity/path mappings. Default root
metadata and explicit storage-SMB metadata each establish nine first-party
members; only the latter selects optional SMB.

## Raw findings and scanner boundary

The new unsuppressed [root report](raw-audits/smb-addendum/root.json) and
[backend-coexistence report](raw-audits/smb-addendum/backend-coexistence.json)
each exit 1. Their retained advisory/warning identities do not change from the
earlier reports. The [original-registry SMB identity lookup](raw-audits/smb-addendum/upstream-smb-identity-only.json)
exits 0 against the same frozen database; this is not a clean scan of patched
code. cargo-audit 0.22.2 skips source-less path packages and non-default-registry
sources, including downstream Git dependencies. Registry companions still
receive ordinary graph-audit coverage. The two directory-stream hunks do not
repair or waive the retained RSA, XML, LRU or maintenance findings.

The frozen advisory revision is
`edb58f5974b83eb8302332821cb6d389777aa351`; no advisory refresh occurred. The
collection and resource wrapper both exit 1 because findings remain, with matching
terminal and no cleanup-failure diagnostic. This is an expected findings status,
not wrapper failure or incomplete collection. The original eight outputs,
settings, counts and times are unchanged.

Full metadata, selected graphs and kernel/native logs remain in maintainer packet
`capture-5d155bab-20261015T231620Z`; request that ID for independent review.
The JSON index binds its summary, terminal reconciliation and artifact manifest.

## Acceptance limits

Dependency applicability is separate from runtime acceptance. The owned Samba
nine-case pass at the same source is separate evidence; it does not prove the
historical wake race deterministically. Remaining local AI, native package API
and final assembled metadata/release gates retain their own actual status.
Actions, remote merges and publication remain held. Reassess this source and
selected feature graph on dependency, configuration or credential changes.
