"""Cargo-owned package file selection shared by release and PR checks.

Only generated resolution/provenance files are omitted. Authored manifests are
compared verbatim, with Cargo's effective manifest also covering inheritance.
"""
from contextlib import ExitStack
import json
from pathlib import Path
import subprocess
import tempfile


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

    def snapshot(self, ref, manifest):
        root = self.checkout(ref)
        key = (root, manifest)
        if key in self.snapshots:
            return self.snapshots[key]
        path = root / manifest
        if not path.exists():
            return None
        package = json.loads(command(root, 'cargo', 'read-manifest', '--manifest-path', str(path)))
        listing = command(root, 'cargo', 'package', '--list', '--manifest-path', str(path),
                          '-p', package['name'], '--allow-dirty', '--no-verify').splitlines()
        files = {}
        for name in listing:
            # Cargo synthesizes these; workspace dependency resolution and Git
            # provenance are recorded separately in the release manifest.
            if name in {'.cargo_vcs_info.json', 'Cargo.lock', 'Cargo.toml.orig'}:
                continue
            source = path if name == 'Cargo.toml' else path.parent / name
            for field in ('readme', 'license_file'):
                configured = package.get(field)
                if configured and Path(configured).name == name:
                    source = path.parent / configured
            files[name] = source.read_bytes()
        package['targets'] = [
            target for target in package['targets']
            if Path(target['src_path']).relative_to(path.parent).as_posix() in files
        ]
        # read-manifest resolves workspace inheritance without dependency fetching.
        # Paths/IDs vary with the temporary checkout; normalize only that prefix.
        effective = json.dumps(package, sort_keys=True).replace(str(root), '$WORKSPACE')
        files['@cargo-effective-manifest'] = effective.encode()
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
