"""Exercise exports using real temporary Git repositories, including refusal paths."""
import contextlib
import io
import json
from pathlib import Path
import tempfile
import unittest

import handoff


class HandoffTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.source, self.destination = self.root / "app", self.root / "firmware"
        for repo in (self.source, self.destination):
            repo.mkdir()
            handoff.git(repo, "init", "-q")
            handoff.git(repo, "config", "user.email", "test@example.invalid")
            handoff.git(repo, "config", "user.name", "Handoff test")
            self.put(repo, "README.md", "existing repository\n")
            self.commit(repo)
        for prefix in handoff.OWNED:
            self.put(self.source, prefix + "/source.txt", prefix)
        self.put(self.source, "private/operator-keypair.json", "not for export")
        self.revision = self.commit(self.source)

    def put(self, repo, name, text):
        path = repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        return path

    def commit(self, repo):
        handoff.git(repo, "add", ".")
        handoff.git(repo, "commit", "-qm", "fixture")
        return handoff.git(repo, "rev-parse", "HEAD").decode().strip()

    def export(self, apply=True, revision=None):
        with contextlib.redirect_stdout(io.StringIO()):
            handoff.export(self.source, self.destination, revision or self.revision, apply)

    def test_preview_writes_nothing(self):
        self.export(apply=False)
        self.assertFalse((self.destination / handoff.RECORD).exists())
        self.assertEqual(handoff.git(self.destination, "status", "--porcelain"), b"")

    def test_round_trip_omits_private_and_unrelated_dirty_app_work(self):
        self.put(self.source, "README.md", "local app work")
        self.export()
        handoff.verify(self.destination)
        self.assertFalse((self.destination / "private").exists())
        self.assertEqual((self.destination / "README.md").read_text(), "existing repository\n")
        self.assertEqual(json.loads((self.destination / handoff.RECORD).read_text())["source_commit"], self.revision)

    def test_dirty_source_export_path_is_refused(self):
        self.put(self.source, handoff.OWNED[0] + "/source.txt", "uncommitted")
        with self.assertRaisesRegex(ValueError, "uncommitted work"):
            self.export()
        self.assertFalse((self.destination / handoff.RECORD).exists())

    def test_dirty_destination_is_refused(self):
        self.put(self.destination, "README.md", "local changes")
        with self.assertRaisesRegex(ValueError, "uncommitted work"):
            self.export()

    def test_ignored_file_collision_is_refused_before_any_writes(self):
        self.put(self.destination, ".gitignore", "source.txt\n")
        self.commit(self.destination)
        self.put(self.destination, handoff.OWNED[0] + "/source.txt", "local ignored data")
        with self.assertRaisesRegex(ValueError, "untracked destination"):
            self.export()
        self.assertFalse((self.destination / handoff.RECORD).exists())

    def test_source_symlink_is_refused(self):
        (self.source / handoff.OWNED[0] / "link").symlink_to("source.txt")
        self.revision = self.commit(self.source)
        with self.assertRaisesRegex(ValueError, "symlink|regular tracked"):
            self.export()

    def test_ignored_parent_file_is_refused_before_any_writes(self):
        self.put(self.destination, ".gitignore", "/tools\n")
        self.commit(self.destination)
        self.put(self.destination, "tools", "local ignored file")
        with self.assertRaisesRegex(ValueError, "parent is not a directory"):
            self.export()
        self.assertFalse((self.destination / "firmware").exists())

    def test_destination_symlink_parent_is_refused(self):
        outside = self.root / "outside"
        outside.mkdir()
        (self.destination / "firmware").symlink_to(outside, target_is_directory=True)
        self.commit(self.destination)
        with self.assertRaisesRegex(ValueError, "symlink"):
            self.export()
        self.assertEqual(list(outside.iterdir()), [])

    def test_tracked_removals_and_executable_modes_follow_commit(self):
        old = self.put(self.source, handoff.OWNED[0] + "/old.txt", "old")
        self.revision = self.commit(self.source)
        self.export()
        self.commit(self.destination)
        old.unlink()
        script = self.put(self.source, handoff.OWNED[0] + "/new.sh", "#!/bin/sh\nexit 0\n")
        script.chmod(0o755)
        self.revision = self.commit(self.source)
        self.export()
        self.assertFalse((self.destination / handoff.OWNED[0] / "old.txt").exists())
        self.assertTrue((self.destination / handoff.OWNED[0] / "new.sh").stat().st_mode & 0o111)
        self.commit(self.destination)
        handoff.verify(self.destination)

    def test_source_tampering_is_detected(self):
        self.export()
        self.put(self.destination, handoff.OWNED[0] + "/source.txt", "drift")
        with self.assertRaisesRegex(ValueError, "source changed"):
            handoff.verify(self.destination)

    def test_extra_source_file_is_detected(self):
        self.export()
        self.put(self.destination, handoff.OWNED[0] + "/extra.txt", "drift")
        with self.assertRaisesRegex(ValueError, "file list changed"):
            handoff.verify(self.destination)

    def test_missing_required_tree_is_refused(self):
        (self.source / handoff.OWNED[-1] / "source.txt").unlink()
        self.revision = self.commit(self.source)
        with self.assertRaisesRegex(ValueError, "missing required tree"):
            self.export()

    def test_full_commit_is_required(self):
        with self.assertRaisesRegex(ValueError, "full 40-character"):
            self.export(revision="HEAD")


if __name__ == "__main__":
    unittest.main()
