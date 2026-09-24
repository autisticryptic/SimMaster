import unittest
from unittest.mock import Mock

from release_target import check_target


class ReleaseTargetTests(unittest.TestCase):
    def test_only_two_confirmed_absences_allow_a_new_release(self):
        get = Mock(side_effect=[404, 404])
        check_target('example/project', 'v1.2.3-beta1', get)
        self.assertEqual(get.call_count, 2)
        self.assertTrue(get.call_args_list[0].args[0].endswith('/git/ref/tags/v1.2.3-beta1'))
        self.assertTrue(get.call_args_list[1].args[0].endswith('/releases/tags/v1.2.3-beta1'))

    def test_existing_tag_or_release_is_never_reused(self):
        for statuses in ([200], [404, 200]):
            with self.subTest(statuses=statuses), self.assertRaisesRegex(ValueError, 'already_exists'):
                check_target('example/project', 'v1.2.3', Mock(side_effect=statuses))

    def test_rate_limit_auth_server_or_redirect_errors_fail_closed(self):
        for code in (301, 302, 401, 403, 429, 500, 503):
            for statuses in ([code], [404, code]):
                with self.subTest(statuses=statuses), self.assertRaisesRegex(ValueError, 'check_failed'):
                    check_target('example/project', 'v1.2.3', Mock(side_effect=statuses))

    def test_transport_failure_is_not_absence(self):
        with self.assertRaises(TimeoutError):
            check_target('example/project', 'v1.2.3', Mock(side_effect=TimeoutError))

    def test_bad_repository_or_version_never_makes_a_request(self):
        for repo, tag in [('example/project/extra', 'v1.2.3'), ('example/project', 'v../1.2.3'),
                          ('example/project', '1.2.3'), ('example/project', 'v1.2.3\nunsafe')]:
            get = Mock()
            with self.subTest(repo=repo, tag=tag), self.assertRaises(ValueError):
                check_target(repo, tag, get)
            get.assert_not_called()


if __name__ == '__main__':
    unittest.main()
