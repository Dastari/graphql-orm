#!/usr/bin/env python3
"""Inspect exact recipe source preservation; no database or failure scenarios."""
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]
BASE = "8e1e5698437d9ea5c0d6ff2c0f29cf7af58d45a0"
PREFIX = "crates/graphql-orm/src/graphql/orm/"


def original(name):
    return subprocess.check_output(
        ["git", "show", f"{BASE}:{PREFIX}{name}"], cwd=ROOT, text=True
    )


def current(name):
    return (ROOT / PREFIX / name).read_text()


def compact(text):
    # Ignore formatting only outside Rust literals. SQL/default/reason strings,
    # including whitespace and raw strings, remain byte-sensitive.
    tokens = re.compile(r"r(?P<hashes>\#{0,255})\".*?\"(?P=hashes)|b?\"(?:\\.|[^\"\\])*\"|b?'(?:\\.|[^'\\])'|\s+", re.DOTALL)
    return tokens.sub(lambda match: "" if match.group().isspace() else match.group(), text)


def same(a, b, label):
    if compact(a) != compact(b):
        raise SystemExit(f"replay recipe source changed: {label}; review required")
    print(f"source preserved: {label}")


for name in ["core.rs", "owned_schema.rs", "owned_foreign_keys.rs", "dialect.rs",
             "search.rs", "schema_manager.rs"]:
    # Exact bytes for unchanged files include Debug implementations and reasons.
    if original(name) != current(name):
        raise SystemExit(f"replay recipe source changed: {name}; review required")
    print(f"source byte-identical: {name}")

old, new = original("migrations.rs"), current("migrations.rs")
start = "pub(super) fn build_migration_plan_views("
end = "\npub fn migration_filename"
oa, ob = old.index(start), old.index(end, old.index(start))
na, nb = new.index(start), new.index(end, new.index(start))
old_block, new_block = old[oa:ob], new[na:nb]
old_phase = old_block.index("    let statements = match backend {")
new_phase = new_block.index("    let statements = render_owned_plan_steps(")
same(old_block[:old_phase], new_block[:new_phase], "diff wrapper and backend projection")
old_render = old_block[old_phase:old_block.index("\n    OwnedMigrationPlan {")]
old_render = old_render.replace("let statements = match backend {", "match backend {", 1)
old_render = old_render.replace("for step in &steps {", "for step in steps {")
old_render = old_render.rstrip().removesuffix(";")
new_render = new_block[new_block.index("    match backend {", new_phase):].rstrip()
new_render = new_render.removesuffix("}").rstrip()
same(old_render, new_render, "render-from-steps body")
# Replacing only the reviewed private phase factor must recover the old engine.
new = new[:na] + old_block + new[nb:]
new = re.sub(
    r'\s*#\[cfg\(all\(test, feature = "owned-planning-replay"\)\)\]\s*'
    r'super::runtime_migration::REPLAY_OBSERVATIONS.fetch_add\(1, std::sync::atomic::Ordering::SeqCst\);',
    "", new,
)
same(old, new, "all other migration helpers and live introspection")

old, new = original("runtime_migration.rs"), current("runtime_migration.rs")
# The old public coordinator, effects, scope and live checks must remain exact.
cut = '\n#[cfg(feature = "owned-planning-replay")]'
production = new[:new.index(cut)]
for first, last in [
    ("    fn check_limits(\n", "\n}\n\n/// Read-only inspection"),
    ("    fn integrity_hash(&self)", "    pub(super) fn validate_apply"),
]:
    oa, ob = old.index(first), old.index(last, old.index(first))
    na, nb = production.index(first), production.index(last, production.index(first))
    production = production[:na] + old[oa:ob] + production[nb:]
same(old, production, "public owned coordinator, effects, scope and live checks")
# Count-only bounds factoring retains the complete original structural/Debug body.
old_check = old[old.index("        if self.tables.len()", old.index("    fn check_limits(\n")):
                old.index("\n}\n\n/// Read-only inspection")]
new_check = new[new.index("        if self.tables.len()", new.index("    fn check_limits_size(\n")):
                new.index("\n}\n\n/// Read-only inspection")]
new_check = new_check.replace("?;\n        Ok(writer.size)", "")
same(old_check, new_check, "definition limits and Debug traversal")
old_hash = old[old.index("        let semantic = stable_plan_hash_views(", old.index("    fn integrity_hash")):
               old.index("    pub(super) fn validate_apply")].rstrip().removesuffix("}")
for name in ["backend", "version", "description", "source_schema_hash", "target_schema_hash", "ownership", "steps"]:
    old_hash = old_hash.replace("self." + name, name)
old_hash = old_hash.replace("&self.statements", "statements")
new_hash = new[new.index("    let semantic = stable_plan_hash_views(", new.index("fn owned_integrity_hash(")):
               new.index("\n#[cfg(all(test, feature = \"owned-planning-replay\"))]")].rstrip().removesuffix("}")
same(old_hash, new_hash, "owned integrity wrapper bytes and semantic helper")
version = subprocess.check_output(["rustc", "-Vv"], text=True)
if "commit-hash: 8bab26f4f68e0e26f0bb7960be334d5b520ea452\n" not in version:
    raise SystemExit("replay recipe toolchain needs accounting/conformance review")
print("qualified recipe source/toolchain inspection passed")
