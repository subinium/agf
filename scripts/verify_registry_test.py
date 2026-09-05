import hashlib
import io
import json
import tarfile
import unittest
from unittest.mock import patch

import verify_registry as registry


class RegistryTests(unittest.TestCase):
    def archive(self, commit="a" * 40, dirty=False):
        contents = json.dumps({"git": {"sha1": commit, "dirty": dirty}}).encode()
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
            member = tarfile.TarInfo("agf-0.15.0/.cargo_vcs_info.json")
            member.size = len(contents)
            archive.addfile(member, io.BytesIO(contents))
        return buffer.getvalue()

    def test_record_requires_exact_unyanked_version(self):
        records = b'{"vers":"0.14.1","yanked":false}\n{"vers":"0.15.0","yanked":false,"cksum":"abc"}\n'
        with patch.object(registry, "fetch", return_value=records):
            self.assertEqual(registry.record("0.15.0")["cksum"], "abc")
            self.assertIsNone(registry.record("0.15.1"))
        with patch.object(registry, "fetch", return_value=b'{"vers":"0.15.0","yanked":true}'):
            with self.assertRaisesRegex(RuntimeError, "yanked"):
                registry.record("0.15.0")

    def test_archive_matches_checksum_and_clean_commit(self):
        archive = self.archive()
        with patch.object(registry, "fetch", return_value=archive):
            registry.verify_archive("0.15.0", hashlib.sha256(archive).hexdigest(), "a" * 40)

    def test_bad_checksum_is_rejected(self):
        with patch.object(registry, "fetch", return_value=self.archive()):
            with self.assertRaisesRegex(RuntimeError, "checksum"):
                registry.verify_archive("0.15.0", "0" * 64, "a" * 40)

    def test_wrong_or_dirty_commit_is_rejected(self):
        for archive in (self.archive(commit="b" * 40), self.archive(dirty=True)):
            with patch.object(registry, "fetch", return_value=archive):
                with self.assertRaisesRegex(RuntimeError, "clean release commit"):
                    registry.verify_archive("0.15.0", hashlib.sha256(archive).hexdigest(), "a" * 40)


if __name__ == "__main__":
    unittest.main()
