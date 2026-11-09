"""Cargo-owned package file selection shared by release and PR checks.

Compare Cargo archives, omitting only generated resolution/provenance data.
Authored manifests are compared verbatim; generated manifests are parsed as TOML.
Only inherited workspace-sibling requirement drift is exempt pending #435.
"""
from contextlib import ExitStack
import json
from pathlib import Path
import subprocess
import tempfile
import tarfile
import tomllib


# Option B pending the maintainer's decision in https://github.com/Dastari/graphql-orm/issues/435.
# Set to "strict" for option A; all other manifest fields remain strict either way.
WORKSPACE_SIBLING_REQUIREMENT_DRIFT = "exempt"
WORKSPACE_SIBLINGS = frozenset({
    'graphql-orm', 'graphql-orm-macros', 'graphql-orm-operation-catalog',
    'graphql-orm-storage', 'graphql-orm-backup', 'graphql-orm-ai-tool-profiles',
    'graphql-orm-ai', 'graphql-orm-router-protocol', 'graphql-orm-router',
})


def command(root, *args):
    result = subprocess.run(args, cwd=root, capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError(f"{' '.join(args)}: {result.stderr.strip()}")
    return result.stdout


class PublishedSources:
    def __init__(self, root):
        self.root = Path(root)
        self.stack = ExitStack()
        self.checkouts = {}
        self.snapshots = {}
        self.members = {}

    def __enter__(self):
        return self

    def __exit__(self, *args):
        return self.stack.__exit__(*args)

    def checkout(self, ref):
        commit = command(self.root, 'git', 'rev-parse', f'{ref}^{{commit}}').strip()
        if commit not in self.checkouts:
            directory = Path(self.stack.enter_context(tempfile.TemporaryDirectory())) / 'source'
            command(self.root, 'git', 'worktree', 'add', '--quiet', '--detach', str(directory), commit)
            self.stack.callback(command, self.root, 'git', 'worktree', 'remove', '--force', str(directory))
            self.checkouts[commit] = directory
        return self.checkouts[commit]

    def normalize_sibling_requirements(self, root, package, manifest):
        if WORKSPACE_SIBLING_REQUIREMENT_DRIFT == "strict":
            return
        if WORKSPACE_SIBLING_REQUIREMENT_DRIFT != "exempt":
            raise ValueError("WORKSPACE_SIBLING_REQUIREMENT_DRIFT must be exempt or strict")
        if root not in self.members:
            metadata = json.loads(command(root, 'cargo', 'metadata', '--no-deps', '--format-version', '1'))
            ids = set(metadata['workspace_members'])
            self.members[root] = {
                (member['name'], str(Path(member['manifest_path']).parent))
                for member in metadata['packages'] if member['id'] in ids
            }
        for dependency in package['dependencies']:
            # A matching name alone must not exempt an external registry/Git
            # dependency. Match the actual workspace member's path as well.
            if (dependency['name'] in WORKSPACE_SIBLINGS
                    and (dependency['name'], dependency.get('path')) in self.members[root]):
                section = {
                    None: 'dependencies', 'dev': 'dev-dependencies',
                    'build': 'build-dependencies',
                }[dependency['kind']]
                table = manifest
                if dependency['target'] is not None:
                    table = table.get('target', {}).get(dependency['target'], {})
                entry = table.get(section, {}).get(dependency['rename'] or dependency['name'])
                if entry is not None and 'version' in entry:
                    entry['version'] = '@workspace-sibling-requirement'

    def snapshot(self, ref, manifest):
        root = self.checkout(ref)
        key = (root, manifest)
        if key in self.snapshots:
            return self.snapshots[key]
        path = root / manifest
        if not path.exists():
            return None
        package = json.loads(command(root, 'cargo', 'read-manifest', '--manifest-path', str(path)))
        # Excluding Cargo's synthesized lockfile avoids dependency resolution,
        # including unpublished siblings. No verification build or network is needed.
        output = Path(self.stack.enter_context(tempfile.TemporaryDirectory()))
        command(root, 'cargo', 'package', '--manifest-path', str(path),
                '-p', package['name'], '--allow-dirty', '--no-verify',
                '--exclude-lockfile', '--offline', '--target-dir', str(output))
        archive = output / 'package' / f"{package['name']}-{package['version']}.crate"
        files = {}
        with tarfile.open(archive, 'r:gz') as packaged:
            for member in packaged:
                name = member.name.split('/', 1)[1]
                # These are independently bound by the workspace release manifest.
                if name in {'.cargo_vcs_info.json', 'Cargo.lock'}:
                    continue
                # Cargo follows source symlinks and writes regular files. Refuse
                # unexpected archive types rather than silently losing metadata.
                if not member.isfile():
                    raise RuntimeError(f"{archive}: unexpected archive entry type: {name}")
                content = packaged.extractfile(member).read()
                if name == 'Cargo.toml':
                    content = tomllib.loads(content.decode('utf-8'))
                    self.normalize_sibling_requirements(root, package, content)
                files[name] = (member.mode, content)
        self.snapshots[key] = files
        return files

    def changed(self, base, head, manifest):
        return self.snapshot(base, manifest) != self.snapshot(head, manifest)


def main():
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('base')
    parser.add_argument('head')
    args = parser.parse_args()
    root = Path(command(Path.cwd(), 'git', 'rev-parse', '--show-toplevel').strip())
    with PublishedSources(root) as sources:
        checkout = sources.checkout(args.head)
        metadata = json.loads(command(checkout, 'cargo', 'metadata', '--no-deps', '--format-version', '1'))
        members = set(metadata['workspace_members'])
        for package in metadata['packages']:
            if package['id'] not in members:
                continue
            manifest = Path(package['manifest_path']).relative_to(checkout).as_posix()
            if sources.changed(args.base, args.head, manifest):
                print(package['name'])


if __name__ == '__main__':
    main()
