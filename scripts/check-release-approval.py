#!/usr/bin/env python3
"""Validate a same-repository merged PR or approved review using the job token."""
import json
import os
import re
import subprocess
import sys


def github_api(path):
    return json.loads(subprocess.check_output(['gh', 'api', path], text=True))


def check(reference, repository, api=github_api):
    match = re.fullmatch(r'https://github\.com/' + re.escape(repository) + r'/pull/([1-9][0-9]*)(?:#pullrequestreview-([1-9][0-9]*))?', reference)
    if not match:
        raise ValueError('approval must be a merged PR or approved review URL on this repository')
    number, review = match.groups()
    path = f'repos/{repository}/pulls/{number}'
    pr = api(path)
    if pr.get('base', {}).get('repo', {}).get('full_name') != repository:
        raise ValueError('approval PR belongs to another repository')
    if pr.get('base', {}).get('ref') != 'main':
        raise ValueError('approval PR must target main')
    if review is None:
        if pr.get('merged') is not True:
            raise ValueError('approval PR is not merged')
    else:
        result = api(f'{path}/reviews/{review}')
        if (result.get('author_association') not in {'OWNER', 'MEMBER', 'COLLABORATOR'}
                or result.get('state') != 'APPROVED' or result.get('id') != int(review)
                or result.get('user', {}).get('type') != 'User'
                or result.get('pull_request_url') != f'https://api.github.com/{path}'):
            raise ValueError('review is not an approved human review of this PR')


if __name__ == '__main__':
    try:
        if len(sys.argv) != 2:
            raise ValueError('usage: check-release-approval.py <approval-url>')
        check(sys.argv[1], os.environ['GITHUB_REPOSITORY'])
    except (ValueError, KeyError, subprocess.CalledProcessError) as error:
        sys.exit(f'release approval rejected: {error}')
