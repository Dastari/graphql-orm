#!/usr/bin/env python3
"""Positive source/manifest conformance only; never dispatch CI or refusal tests."""

import importlib.util
import json
import re
import types

from replay_release import (COMPILERS, EXCLUDED_JOBS, QUALIFIED, ROOT, commands,
                            metadata_commands, require, source)


def jobs(text):
    matches = list(re.finditer(r"(?m)^  ([a-z][a-z0-9-]*):\n", text))
    return {match[1]: text[match.start():matches[i + 1].start() if i + 1 < len(matches) else len(text)]
            for i, match in enumerate(matches) if match.start() > text.index("jobs:\n")}


def without_timeout(body):
    # A job's wall-clock limit is runner capacity, not verification content.
    return re.sub(r"(?m)^    timeout-minutes: [0-9]+\n", "", body)


def main():
    workflow = (ROOT / ".github/workflows/release.yml").read_text()
    old = jobs(source(QUALIFIED, ".github/workflows/release.yml").decode())
    new = jobs(workflow)
    require("default: full\n" in workflow and "- replay-scoped-positive-v1\n" in workflow,
            "full profile must remain the default")
    # Every original matrix command, provider enumeration, failure/skip rule,
    # permission and full publisher step is byte-identical after selection.
    # Only the job-level timeout may differ.
    for name in [*EXCLUDED_JOBS, "validate", "release"]:
        body = new[name].replace("    if: inputs.validation_profile == 'full'\n", "", 1)
        require(without_timeout(body).rstrip() == without_timeout(old[name]).rstrip(),
                f"full profile changed: {name}")
    for name in ["replay-scoped-integrity", "replay-scoped-positive", "replay-scoped-validate", "release-scoped"]:
        require("if: inputs.validation_profile == 'replay-scoped-positive-v1'" in new[name],
                "scoped selection must be explicit")
        require("always()" not in new[name] and "continue-on-error" not in new[name],
                "scoped validation must not bypass failures/skips")
    require("needs: [guard, replay-scoped-integrity, replay-scoped-positive]" in new["replay-scoped-validate"],
            "scoped mandatory dependencies changed")
    require("      - replay-scoped-validate\n" in new["release-scoped"]
            and "      - validate\n" in new["release"], "publisher validation boundary changed")
    for step in ["Create checksums", "Attest release assets", "Create annotated release tags", "Publish immutable workspace release"]:
        require(f"- name: {step}" in new["release-scoped"], "immutable publisher step missing")
    require("--verification-evidence" in new["release-scoped"]
            and "git push --atomic origin" in new["release-scoped"], "scoped evidence/atomic publication missing")
    require("environment: release" in new["guard"] and 'test "${INCLUDE_ROUTER}" = false' in new["guard"]
            and 'test "${GITHUB_SHA}" = "${TARGET_REF}"' in new["guard"], "protected source-only entry guard changed")
    for compiler in COMPILERS:
        specs = commands(compiler)
        require(len(specs) == 38, "literal positive command count changed")
        require(all("--all-features" not in spec["command"] for spec in specs), "unsupported workspace feature union")
        tests = [spec for spec in specs if "test" in spec["command"]]
        require(sum(spec["expectedTests"] or 0 for spec in tests) == 16,
                "positive test counts changed")
    literal = json.loads((ROOT / "docs/operations/release/replay-positive-commands.json").read_text())
    require(literal == {"metadata": metadata_commands(), **{c: commands(c) for c in COMPILERS}},
            "literal command reference differs from implementation")
    module_spec = importlib.util.spec_from_file_location("release_manifest", ROOT / "scripts/generate-release-manifest.py")
    module = importlib.util.module_from_spec(module_spec)
    module_spec.loader.exec_module(module)
    first = module.build_manifest("workspace-2000.01.01.1", "HEAD", True)
    second = module.build_manifest("workspace-2000.01.01.1", "HEAD", True)
    require(first == second and module.render_notes(first) == module.render_notes(second),
            "positive manifest generation is not deterministic")
    require("verification" not in first, "default full manifest shape changed")
    original = types.ModuleType("original_release_manifest")
    original.__file__ = str(ROOT / "scripts/generate-release-manifest.py")
    exec(compile(source(QUALIFIED, "scripts/generate-release-manifest.py"), original.__file__, "exec"), original.__dict__)
    require(first == original.build_manifest("workspace-2000.01.01.1", "HEAD", True)
            and module.render_notes(first) == original.render_notes(first),
            "default full manifest or notes changed from released generator")
    contracts = first["contracts"]
    require(len({row["name"] for row in contracts}) == len(contracts)
            and contracts == sorted(contracts, key=lambda row: row["name"]), "contract order changed")
    replay = [row for row in contracts if row["name"].startswith("graphql-orm-owned-planning-replay-")]
    require(len(replay) == 3, "both explicit recipes and format must remain recorded")
    print("Positive release conformance: 16 original full jobs preserved; 4 explicit scoped jobs; deterministic manifest and three replay contract rows.")


if __name__ == "__main__":
    main()
