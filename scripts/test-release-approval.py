#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import Mock

spec = importlib.util.spec_from_file_location('approval', Path(__file__).with_name('check-release-approval.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class ApprovalTests(unittest.TestCase):
    def test_merged_pr(self):
        api = Mock(return_value={'merged': True, 'base': {'ref': 'main', 'repo': {'full_name': 'Dastari/graphql-orm'}}})
        module.check('https://github.com/Dastari/graphql-orm/pull/42', 'Dastari/graphql-orm', api)
        api.assert_called_once_with('repos/Dastari/graphql-orm/pulls/42')

    def test_approved_review(self):
        api = Mock(side_effect=[{'base': {'ref': 'main', 'repo': {'full_name': 'Dastari/graphql-orm'}}}, {'id': 123, 'state': 'APPROVED', 'author_association': 'OWNER', 'user': {'type': 'User'}, 'pull_request_url': 'https://api.github.com/repos/Dastari/graphql-orm/pulls/42'}])
        module.check('https://github.com/Dastari/graphql-orm/pull/42#pullrequestreview-123', 'Dastari/graphql-orm', api)

    def test_review_requires_repository_association(self):
        pr = {'base': {'ref': 'main', 'repo': {'full_name': 'Dastari/graphql-orm'}}}
        valid = {'id': 123, 'state': 'APPROVED', 'user': {'type': 'User'}, 'pull_request_url': 'https://api.github.com/repos/Dastari/graphql-orm/pulls/42'}
        for association in ['OWNER', 'MEMBER', 'COLLABORATOR']:
            with self.subTest(association=association):
                module.check('https://github.com/Dastari/graphql-orm/pull/42#pullrequestreview-123', 'Dastari/graphql-orm', Mock(side_effect=[pr, {**valid, 'author_association': association}]))
        for association in ['CONTRIBUTOR', 'NONE', 'FIRST_TIME_CONTRIBUTOR', None]:
            with self.subTest(association=association), self.assertRaises(ValueError):
                module.check('https://github.com/Dastari/graphql-orm/pull/42#pullrequestreview-123', 'Dastari/graphql-orm', Mock(side_effect=[pr, {**valid, 'author_association': association}]))

    def test_both_paths_require_main_base(self):
        for branch in ['next/breaking', 'release/other', None]:
            for suffix in ['', '#pullrequestreview-123']:
                pr = {'merged': True, 'base': {'ref': branch, 'repo': {'full_name': 'Dastari/graphql-orm'}}}
                api = Mock(side_effect=[pr, {'id': 123, 'state': 'APPROVED', 'author_association': 'OWNER', 'user': {'type': 'User'}, 'pull_request_url': 'https://api.github.com/repos/Dastari/graphql-orm/pulls/42'}])
                with self.subTest(branch=branch, suffix=suffix), self.assertRaises(ValueError):
                    module.check('https://github.com/Dastari/graphql-orm/pull/42' + suffix, 'Dastari/graphql-orm', api)
                api.assert_called_once_with('repos/Dastari/graphql-orm/pulls/42')

    def test_rejects_invalid_urls_without_api(self):
        for value in ['', 'approved', 'https://github.com/other/repo/pull/42', 'http://github.com/Dastari/graphql-orm/pull/42', 'https://github.com/Dastari/graphql-orm/pull/42?x=1', 'https://github.com/Dastari/graphql-orm/pull/42#issuecomment-123']:
            api = Mock()
            with self.subTest(value=value), self.assertRaises(ValueError):
                module.check(value, 'Dastari/graphql-orm', api)
            api.assert_not_called()

    def test_rejects_unmerged_or_wrong_repository(self):
        for result in [{'merged': False, 'base': {'ref': 'main', 'repo': {'full_name': 'Dastari/graphql-orm'}}}, {'merged': True, 'base': {'ref': 'main', 'repo': {'full_name': 'other/repo'}}}]:
            with self.assertRaises(ValueError):
                module.check('https://github.com/Dastari/graphql-orm/pull/42', 'Dastari/graphql-orm', Mock(return_value=result))

    def test_rejects_nonapproval_and_api_failures(self):
        for state in ['DISMISSED', 'CHANGES_REQUESTED', 'COMMENTED', 'PENDING']:
            api = Mock(side_effect=[{'base': {'ref': 'main', 'repo': {'full_name': 'Dastari/graphql-orm'}}}, {'id': 123, 'state': state, 'author_association': 'OWNER', 'user': {'type': 'User'}, 'pull_request_url': 'https://api.github.com/repos/Dastari/graphql-orm/pulls/42'}])
            with self.assertRaises(ValueError):
                module.check('https://github.com/Dastari/graphql-orm/pull/42#pullrequestreview-123', 'Dastari/graphql-orm', api)
        with self.assertRaises(RuntimeError):
            module.check('https://github.com/Dastari/graphql-orm/pull/42', 'Dastari/graphql-orm', Mock(side_effect=RuntimeError('API unavailable')))

    def test_rejects_bot_or_mismatched_review(self):
        valid = {'id': 123, 'state': 'APPROVED', 'author_association': 'OWNER', 'user': {'type': 'User'}, 'pull_request_url': 'https://api.github.com/repos/Dastari/graphql-orm/pulls/42'}
        for change in [{'id': 456}, {'user': {'type': 'Bot'}}, {'pull_request_url': 'https://api.github.com/repos/Dastari/graphql-orm/pulls/43'}]:
            api = Mock(side_effect=[{'base': {'ref': 'main', 'repo': {'full_name': 'Dastari/graphql-orm'}}}, {**valid, **change}])
            with self.subTest(change=change), self.assertRaises(ValueError):
                module.check('https://github.com/Dastari/graphql-orm/pull/42#pullrequestreview-123', 'Dastari/graphql-orm', api)

    def test_both_workflow_inputs_are_checked(self):
        workflow = Path('.github/workflows/release.yml').read_text().split('  workspace-integrity:')[0]
        self.assertIn('pull-requests: read', workflow)
        for value in ['RELEASE_APPROVAL', 'ROUTER_APPROVAL']:
            self.assertIn('python3 scripts/check-release-approval.py "${' + value + '}"', workflow)

if __name__ == '__main__':
    unittest.main()
