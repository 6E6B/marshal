"""Release guards must fail before building or publishing mismatched versions."""

import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("release", Path(__file__).resolve().parents[1] / "scripts/release.py")
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleaseVersions(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "data").mkdir()
        self.fixture("0.1.0")

    def fixture(self, version):
        (self.root / "Cargo.toml").write_text(f'[package]\nversion = "{version}"\n')
        (self.root / "Cargo.lock").write_text(f'[[package]]\nname = "marshal"\nversion = "{version}"\n')
        (self.root / "meson.build").write_text(f"project('marshal', version: '{version}')\n")
        (self.root / "data/io.github._6E6B.marshal.metainfo.xml").write_text(
            f'<component><releases><release version="{version}"/></releases></component>'
        )

    def test_stable_and_prerelease(self):
        for version in ("0.1.0", "1.2.3-rc.1"):
            with self.subTest(version=version):
                self.fixture(version)
                self.assertEqual(release.validate(self.root, f"v{version}"), version)

    def test_wrong_tag(self):
        with self.assertRaisesRegex(ValueError, "Tag"):
            release.validate(self.root, "v0.2.0")

    def test_version_mismatches(self):
        for path in ("meson.build", "Cargo.lock", "data/io.github._6E6B.marshal.metainfo.xml"):
            with self.subTest(path=path):
                self.fixture("0.1.0")
                file = self.root / path
                file.write_text(file.read_text().replace("0.1.0", "0.2.0"))
                with self.assertRaises(ValueError):
                    release.validate(self.root)

    def test_invalid_versions(self):
        for version in ("01.2.3", "1.2", "1.2.3-01", "1.2.3+build", "1.2.3-rc/1"):
            with self.subTest(version=version):
                self.fixture(version)
                with self.assertRaisesRegex(ValueError, "SemVer"):
                    release.validate(self.root)


if __name__ == "__main__":
    unittest.main()
