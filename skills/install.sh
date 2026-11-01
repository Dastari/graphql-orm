#!/bin/sh
# Installs the graphql-orm usage skill from one release into the current project.
#
#   curl -fsSL "https://raw.githubusercontent.com/Dastari/graphql-orm/$TAG/skills/install.sh" \
#     | sh -s -- --target claude --tag "$TAG"
#
# Relative links in the skill become GitHub links at the same tag, so an installed
# copy always points at the documentation for the version it came from.
set -eu

repository=Dastari/graphql-orm
skill=graphql-orm-usage

usage() {
  echo "usage: install.sh --target claude|agents|codex --tag <release-tag> [--update] [--from <skill-directory>]" >&2
  exit 2
}

fail() {
  echo "install.sh: $1" >&2
  exit 1
}

target=
tag=
update=false
from=
while [ "$#" -gt 0 ]; do
  case "$1" in
    --target) [ "$#" -ge 2 ] || usage; target=$2; shift 2 ;;
    --tag) [ "$#" -ge 2 ] || usage; tag=$2; shift 2 ;;
    --from) [ "$#" -ge 2 ] || usage; from=$2; shift 2 ;;
    --update) update=true; shift ;;
    *) usage ;;
  esac
done

case "$target" in
  claude) destination=.claude/skills ;;
  agents | codex) destination=.agents/skills ;;
  *) usage ;;
esac
[ -n "$tag" ] || usage
case "$tag" in
  *[!A-Za-z0-9._-]*) fail "invalid release tag: $tag" ;;
esac

installed="$destination/$skill/SKILL.md"
if [ -e "$installed" ] && [ "$update" = false ]; then
  fail "$installed already exists. Use --update to replace it."
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

for file in SKILL.md examples.md; do
  if [ -n "$from" ]; then
    cp "$from/$file" "$work/$file"
  else
    curl -fsSL "https://raw.githubusercontent.com/$repository/$tag/skills/$skill/$file" \
      -o "$work/$file" || fail "could not download the skill at $tag. Does that tag contain it?"
  fi
done
grep -q "^name: $skill\$" "$work/SKILL.md" || fail "the downloaded file is not the $skill skill"

mkdir -p "$destination/$skill"
for file in SKILL.md examples.md; do
  sed "s#](\.\./\.\./#](https://github.com/$repository/blob/$tag/#g" "$work/$file" \
    >"$destination/$skill/$file"
done
echo "Installed $skill at $tag into $destination/$skill. Existing project instructions were not changed."
