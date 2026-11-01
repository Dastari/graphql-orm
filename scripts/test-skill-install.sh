#!/usr/bin/env bash
# Exercises skills/install.sh against the checked-in skill, with no network.
set -euo pipefail

repository_root=$(git rev-parse --show-toplevel)
installer="${repository_root}/skills/install.sh"
skill="${repository_root}/skills/graphql-orm-usage"
workspace=$(mktemp -d)
trap 'rm -rf "${workspace}"' EXIT
cd "${workspace}"

tag=workspace-2030.01.01.1
install() {
  sh "${installer}" --tag "${tag}" --from "${skill}" "$@"
}

expect_failure() {
  if "$@" >/dev/null 2>&1; then
    echo "skill-install: expected failure: $*" >&2
    exit 1
  fi
}

install --target claude >/dev/null
installed=.claude/skills/graphql-orm-usage/SKILL.md
test -f "${installed}"
test -f .claude/skills/graphql-orm-usage/examples.md

if grep -q '](\.\./\.\./' "${installed}"; then
  echo "skill-install: relative links survived installation" >&2
  exit 1
fi
expected=$(grep -c '](\.\./\.\./' "${skill}/SKILL.md")
actual=$(grep -c "](https://github.com/Dastari/graphql-orm/blob/${tag}/" "${installed}")
if [[ "${expected}" -eq 0 || "${expected}" -ne "${actual}" ]]; then
  echo "skill-install: rewrote ${actual} of ${expected} linked lines" >&2
  exit 1
fi
grep -q '^name: graphql-orm-usage$' "${installed}"

expect_failure install --target claude
echo stale >"${installed}"
install --target claude --update >/dev/null
grep -q '^name: graphql-orm-usage$' "${installed}"

install --target agents >/dev/null
test -f .agents/skills/graphql-orm-usage/SKILL.md
install --target codex --update >/dev/null

expect_failure install --target unknown
expect_failure sh "${installer}" --target claude
expect_failure sh "${installer}" --target claude --tag 'bad#tag' --from "${skill}"
mkdir other
echo 'name: something-else' >other/SKILL.md
echo '# Examples' >other/examples.md
expect_failure sh "${installer}" --target claude --update --tag "${tag}" --from other

echo "skill-install: every target, update path, refusal and link rewrite passed"
