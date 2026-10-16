import hashlib
from pathlib import Path
import tarfile
import tempfile
import unittest

import release


class ReleaseTests(unittest.TestCase):
    def test_tag_must_match_stable_package_version(self):
        with tempfile.TemporaryDirectory() as directory:
            manifest = Path(directory) / "Cargo.toml"
            manifest.write_text('[package]\nversion = "1.2.3"\n')
            self.assertEqual(release.release_version("v1.2.3", manifest), "1.2.3")
            for tag in ["1.2.3", "v1.2.4", "v1.2.3-rc.1", "v01.2.3", "v1.2.3\n"]:
                with self.assertRaises(ValueError):
                    release.release_version(tag, manifest)
            manifest.write_text('[package]\nversion = "1.2.3-rc.1"\n')
            with self.assertRaises(ValueError):
                release.release_version("v1.2.3-rc.1", manifest)

    def test_package_contains_one_executable_and_a_relative_ep_symlink(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "editor-pro"
            binary.write_text("#!/bin/sh\nprintf 'editor-pro 1.2.3\\n'\n")
            binary.chmod(0o755)
            archive = release.package("1.2.3", release.TARGETS[0], binary, root / "dist")
            with tarfile.open(archive) as tar:
                members = {Path(member.name).name: member for member in tar.getmembers()}
                self.assertTrue(members["editor-pro"].isfile())
                self.assertEqual(members["editor-pro"].mode, 0o755)
                self.assertTrue(members["ep"].issym())
                self.assertEqual(members["ep"].linkname, "editor-pro")
                self.assertEqual(set(members), {archive.name.removesuffix(".tar.gz"), "editor-pro", "ep", "README.md"})
            with self.assertRaises(ValueError):
                release.package("1.2.4", release.TARGETS[0], binary, root / "dist")

    def test_formula_requires_arm64_archive_and_uses_its_real_hash(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaises(FileNotFoundError):
                release.formula("1.2.3", "perrness/editor-pro", root)
            target = "aarch64-apple-darwin"
            (root / release.archive_name("1.2.3", target)).write_bytes(target.encode())
            formula = release.formula("1.2.3", "perrness/editor-pro", root)
            self.assertIn(hashlib.sha256(target.encode()).hexdigest(), formula)
            self.assertIn(f"https://github.com/perrness/editor-pro/releases/download/v1.2.3/{release.archive_name('1.2.3', target)}", formula)
            self.assertIn("depends_on arch: :arm64", formula)
            self.assertIn('bin.install_symlink bin/"editor-pro" => "ep"', formula)
            self.assertNotIn("file://", formula)
            self.assertIn(root.as_uri(), release.formula("1.2.3", "perrness/editor-pro", root, local=True))
            with self.assertRaises(ValueError):
                release.formula("1.2.3", 'owner/repo"; system("bad")', root)


if __name__ == "__main__":
    unittest.main()
