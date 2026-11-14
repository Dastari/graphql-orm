#!/usr/bin/env python3
"""Check the CI and release supply-chain rules for workflows and test images.

The rules are:

- Every remote action is pinned to a full 40-character commit SHA and carries a
  version comment. Local actions (`./...`) are part of the checked-out source.
- Every `dtolnay/rust-toolchain` step selects a reviewed numeric toolchain:
  either the reviewed release-branch commit for the workspace baseline, or an
  explicit numeric `toolchain:` input. Floating `stable`, `beta` and `nightly`
  selections are rejected.
- A job that holds any write permission (contents, OIDC tokens, attestations,
  ...) runs no third-party build. It uses only the first-party checkout,
  artifact download and attestation actions, runs no compiler, interpreter,
  package manager or repository script, and consumes its assets from a
  read-only job whose output digest it verifies before use.
- Workflow-level permissions are read-only.
- Full release validation runs the macros crate tests and the schema-export
  equivalence check, as CI does.
- Container images used by tests and local compose files are pinned by digest,
  and compose files publish ports on IPv4 loopback only, without fixed
  default passwords.

The checks read text with indentation-based parsing rather than a YAML library,
so they run on a bare runner Python and on any checkout passed with `--root`.
"""

import argparse
import re
import sys
from pathlib import Path

# The release-branch commit of dtolnay/rust-toolchain whose default toolchain is
# the reviewed workspace compiler baseline (docs/operations/release/process.md).
REVIEWED_TOOLCHAIN_REFS = {
    "cf201f30d2291af293e25b32e435b13c42d51aaf": "1.97.1",
}
NUMERIC_TOOLCHAIN = re.compile(r"\d+\.\d+\.\d+")
MATRIX_TOOLCHAIN = "${{ matrix.toolchain }}"
PINNED_ACTION = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_./-]+@[0-9a-f]{40}")
IMAGE_DIGEST = re.compile(r"@sha256:[0-9a-f]{64}$")

# The only actions a job holding write permissions may run.
PUBLISHER_ACTIONS = {
    "actions/checkout",
    "actions/download-artifact",
    "actions/attest-build-provenance",
}
# Build tools, interpreters and package managers, any of which can run code
# from a dependency, a cache or the checked-out tree.
BUILD_TOOLS = re.compile(
    r"(?<![\w./-])(cargo|rustc|rustup|python3?|pip3?|pipx|npm|npx|node|yarn|pnpm"
    r"|make|cmake|docker|bash|sh|source)(?![\w-])"
)
REPOSITORY_SCRIPT = re.compile(r"(?<![\w-])(\./)?(scripts|\.github)/")

# Commands that full release validation must run because CI runs them.
RELEASE_MIRRORED_COMMANDS = ["scripts/check-schema-export.sh"]
SCHEMA_EXPORT_MACROS_TESTS = [
    "cargo test -p graphql-orm-macros --locked\n",
    "cargo test -p graphql-orm-macros --locked --features schema-export-only\n",
]

# Test fixtures that start containers from a default image reference.
IMAGE_DEFAULTS = {
    "crates/graphql-orm-storage/tests/samba/run.sh": re.compile(
        r"^IMAGE=\$\{SMB_TEST_IMAGE:-([^}]+)\}$", re.M
    ),
}


class Failures:
    def __init__(self):
        self.messages = []

    def __call__(self, path, message):
        self.messages.append(f"{path}: {message}")


def strip_comments(text):
    return "\n".join(line for line in text.splitlines() if not line.lstrip().startswith("#"))


def indent(line):
    return len(line) - len(line.lstrip(" "))


def block(lines, start):
    """Lines of the YAML node that starts at `start`, including that line."""
    base = indent(lines[start])
    end = start + 1
    while end < len(lines):
        line = lines[end]
        if line.strip() and indent(line) <= base:
            break
        end += 1
    return lines[start:end]


def jobs(text):
    """Map each job name to its body text."""
    if "\njobs:\n" not in text:
        return {}
    body = text[text.index("\njobs:\n") + len("\njobs:\n"):]
    matches = list(re.finditer(r"(?m)^  ([A-Za-z0-9_-]+):\n", body))
    return {
        match[1]: body[match.start():matches[i + 1].start() if i + 1 < len(matches) else len(body)]
        for i, match in enumerate(matches)
    }


def steps(job_text):
    """Each step of a job (or composite action) as text."""
    lines = job_text.splitlines()
    found = []
    for i, line in enumerate(lines):
        if line.strip() == "steps:":
            step_indent = None
            for j in range(i + 1, len(lines)):
                current = lines[j]
                if not current.strip():
                    continue
                if indent(current) <= indent(line):
                    break
                if current.lstrip().startswith("- ") and (step_indent is None or indent(current) == step_indent):
                    step_indent = indent(current)
                    found.append("\n".join(block(lines, j)))
    return found


def step_field(step, name):
    match = re.search(rf"(?m)^\s*(?:- )?{re.escape(name)}:[ \t]*(.*)$", step)
    return match[1].strip() if match else None


def uses_lines(text):
    return re.findall(r"(?m)^\s*(?:- )?uses:[ \t]*(\S+)[ \t]*(.*)$", text)


def run_scripts(job_text):
    """The shell text of every `run:` step, in single-line or block form."""
    scripts = []
    for step in steps(job_text):
        lines = step.splitlines()
        for i, line in enumerate(lines):
            match = re.match(r"^(\s*)(?:- )?run:[ \t]*(.*)$", line)
            if not match:
                continue
            value = match[2].strip()
            if value[:1] in {"|", ">"}:
                body = []
                for following in lines[i + 1:]:
                    if following.strip() and indent(following) <= indent(line):
                        break
                    body.append(following)
                scripts.append("\n".join(body))
            else:
                scripts.append(value)
    return scripts


def permissions(job_text):
    """The job's explicit permission map, or None when it inherits."""
    lines = job_text.splitlines()
    for i, line in enumerate(lines):
        if indent(line) == 4 and line.strip().startswith("permissions:"):
            value = line.split(":", 1)[1].strip()
            if value:
                return {"*": value}
            result = {}
            for entry in block(lines, i)[1:]:
                if entry.strip() and not entry.strip().startswith("#"):
                    key, _, scope = entry.strip().partition(":")
                    result[key.strip()] = scope.strip()
            return result
    return None


def workflow_permissions(text):
    match = re.search(r"(?m)^permissions:[ \t]*(.*)\n((?:  .*\n)*)", text)
    if not match:
        return None
    if match[1].strip():
        return {"*": match[1].strip()}
    return {
        key.strip(): value.strip()
        for key, _, value in (line.strip().partition(":") for line in match[2].splitlines())
        if key.strip() and not key.strip().startswith("#")
    }


def writes(scopes):
    return bool(scopes) and any(value in {"write", "write-all"} for value in scopes.values())


def needs(job_text):
    inline = re.search(r"(?m)^    needs:[ \t]*(\S.*)$", job_text)
    if inline:
        value = inline[1].strip()
        if value.startswith("["):
            return [item.strip() for item in value.strip("[]").split(",") if item.strip()]
        return [value]
    listed = re.search(r"(?m)^    needs:\n((?:      - .*\n)+)", job_text)
    return [line.strip()[2:].strip() for line in listed[1].splitlines()] if listed else []


def check_action_pins(path, text, fail):
    for target, comment in uses_lines(text):
        if target.startswith("./"):
            continue
        if target.startswith("docker://"):
            if not IMAGE_DIGEST.search(target):
                fail(path, f"container action is not pinned by digest: {target}")
            continue
        if not PINNED_ACTION.fullmatch(target):
            fail(path, f"action is not pinned to a full commit SHA: {target}")
        elif not re.fullmatch(r"# \S.*", comment.strip()):
            fail(path, f"pinned action has no version comment: {target}")


def check_toolchains(path, text, fail):
    for job_name, job_text in (jobs(text) or {"(action)": text}).items():
        matrix = re.search(r"(?m)^        toolchain:[ \t]*\[(.*)\]", job_text)
        matrix_values = [value.strip().strip("'\"") for value in matrix[1].split(",")] if matrix else []
        for step in steps(job_text):
            target = (step_field(step, "uses") or "").split("#", 1)[0].strip()
            if not target.startswith("dtolnay/rust-toolchain@"):
                continue
            ref = target.split("@", 1)[1]
            toolchain = step_field(step, "toolchain")
            if toolchain is None:
                if ref not in REVIEWED_TOOLCHAIN_REFS:
                    fail(path, f"{job_name}: rust-toolchain ref {ref} selects no reviewed toolchain")
            elif toolchain == MATRIX_TOOLCHAIN:
                if not matrix_values or not all(NUMERIC_TOOLCHAIN.fullmatch(v) for v in matrix_values):
                    fail(path, f"{job_name}: matrix toolchain values are not numeric releases")
            elif not NUMERIC_TOOLCHAIN.fullmatch(toolchain.strip("'\"")):
                fail(path, f"{job_name}: rust-toolchain selects floating toolchain {toolchain}")


def check_publishers(path, text, fail):
    top = workflow_permissions(text)
    if top is None or writes(top):
        fail(path, "workflow-level permissions must be explicit and read-only")
    for step in steps(text):
        if "uses: actions/checkout@" in step and "persist-credentials: false" not in step:
            fail(path, "checkout must disable persisted credentials")
    all_jobs = jobs(text)
    for name, job_text in all_jobs.items():
        if not re.search(r"(?m)^    timeout-minutes: [1-9][0-9]*$", job_text):
            fail(path, f"{name}: job must have a positive timeout")
        if not writes(permissions(job_text)):
            continue
        code = strip_comments(job_text)
        if 'test "${package_tags}" = "${expected_tags}"' not in code:
            fail(path, "publisher must bind tags to workspace manifests")
        for target, _ in uses_lines(code):
            action = target.split("@", 1)[0]
            if action not in PUBLISHER_ACTIONS:
                fail(path, f"{name}: job with write permissions uses {target}")
        for script in run_scripts(code):
            tool = BUILD_TOOLS.search(script)
            if tool:
                fail(path, f"{name}: job with write permissions runs {tool[1]}")
            if REPOSITORY_SCRIPT.search(script):
                fail(path, f"{name}: job with write permissions runs a repository script")
        # A build job uploads the bundle and reports its identity as outputs.
        builders = [job for job in needs(job_text)
                    if job in all_jobs and f"needs.{job}.outputs." in code
                    and re.search(r"(?m)^    outputs:\n", all_jobs[job])
                    and "actions/upload-artifact@" in all_jobs[job]]
        if not builders:
            fail(path, f"{name}: job with write permissions consumes no digest from a build job")
        for builder in builders:
            if writes(permissions(all_jobs[builder])):
                fail(path, f"{name}: build job {builder} holds write permissions")
        if "sha256sum --check --strict" not in code:
            fail(path, f"{name}: job with write permissions does not verify asset checksums")


def check_release_validation(root, fail):
    path = ".github/workflows/release.yml"
    workflow = jobs((root / path).read_text())
    if "validate" not in workflow:
        fail(path, "full release validation job is missing")
        return
    lanes = needs(workflow["validate"])
    for command in RELEASE_MIRRORED_COMMANDS:
        if not any(command in strip_comments(workflow.get(lane, "")) for lane in lanes):
            fail(path, f"full release validation does not run {command}")
    script_path = "scripts/check-schema-export.sh"
    script = (root / script_path).read_text()
    for command in SCHEMA_EXPORT_MACROS_TESTS:
        if command not in script:
            fail(script_path, f"missing locked macros test: {command.strip()}")


def compose_files(root):
    for pattern in ["compose.y*ml", "compose.*.y*ml", "docker-compose*.y*ml"]:
        yield from sorted(root.glob(pattern))


def check_compose(root, fail):
    for file in compose_files(root):
        path = file.relative_to(root).as_posix()
        text = strip_comments(file.read_text())
        for image in re.findall(r"(?m)^\s+image:[ \t]*(\S+)", text):
            if not IMAGE_DIGEST.search(image.strip("'\"")):
                fail(path, f"image is not pinned by digest: {image}")
        lines = text.splitlines()
        for i, line in enumerate(lines):
            if line.strip() != "ports:":
                continue
            for entry in block(lines, i)[1:]:
                entry = entry.strip()
                if not entry.startswith("- "):
                    continue
                mapping = entry[2:].strip().strip("'\"")
                if not mapping.startswith("127.0.0.1:"):
                    fail(path, f"port is not bound to IPv4 loopback: {mapping}")
        for key, value in re.findall(r"(?m)^\s+([A-Z_]*PASSWORD[A-Z_]*):[ \t]*(.+)$", text):
            value = value.strip().strip("'\"")
            if not re.fullmatch(r"\$\{[A-Z0-9_]+:\?[^}]+\}", value):
                fail(path, f"{key} must come from a required environment variable")


def check_image_defaults(root, fail):
    for path, pattern in IMAGE_DEFAULTS.items():
        match = pattern.search((root / path).read_text())
        if not match:
            fail(path, "default test image declaration is missing")
        elif not IMAGE_DIGEST.search(match[1]):
            fail(path, f"default test image is not pinned by digest: {match[1]}")


def check(root):
    fail = Failures()
    files = sorted((root / ".github/workflows").glob("*.y*ml"))
    files += sorted((root / ".github/actions").glob("**/action.y*ml"))
    for file in files:
        path = file.relative_to(root).as_posix()
        text = file.read_text()
        check_action_pins(path, text, fail)
        check_toolchains(path, text, fail)
        if "/workflows/" in f"/{path}":
            check_publishers(path, text, fail)
            for image in re.findall(r"(?m)^\s+(?:image|container):[ \t]*(\S+)$", strip_comments(text)):
                if not IMAGE_DIGEST.search(image.strip("'\"")):
                    fail(path, f"job container image is not pinned by digest: {image}")
    for file in (root / "crates").rglob("*.rs"):
        for image in re.findall(r'"(postgres:(?!//)[^"\s]+)"', file.read_text()):
            if not IMAGE_DIGEST.search(image):
                fail(file.relative_to(root), f"Rust test image is not pinned by digest: {image}")
    check_release_validation(root, fail)
    for dockerfile in (root / "crates").rglob("Dockerfile"):
        for image in re.findall(r"(?m)^FROM ([^\s]+)", dockerfile.read_text()):
            if image != "scratch" and not IMAGE_DIGEST.search(image):
                fail(dockerfile.relative_to(root), f"Dockerfile image is not pinned by digest: {image}")
    check_compose(root, fail)
    check_image_defaults(root, fail)
    return fail.messages, len(files)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1],
                        help="checkout to inspect (default: this repository)")
    args = parser.parse_args()
    messages, count = check(args.root.resolve())
    if messages:
        for message in messages:
            print(f"workflow-supply-chain: {message}", file=sys.stderr)
        sys.exit(1)
    print(f"Workflow supply chain: {count} workflow/action files pinned; publishers run no builds; "
          "release validation mirrors CI schema export; test images digest-pinned.")


if __name__ == "__main__":
    main()
