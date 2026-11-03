#!/usr/bin/env python3
"""Exercise the installer against disposable app bundles and mocked system tools."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

INSTALLER = Path(__file__).resolve().parents[1] / "script" / "install.sh"
APP = "闪电全能下载.app"


class InstallerTests(unittest.TestCase):
    def run_install(self, failure=""):
        with tempfile.TemporaryDirectory(prefix="shandian-install-test-") as root:
            root = Path(root)
            bin_dir, dest, trash = root / "bin", root / "dest", root / "trash"
            for directory in (bin_dir, dest, trash):
                directory.mkdir()
            (dest / APP).mkdir()
            (dest / APP / "old").write_text("old")
            mocks = {
                "uname": 'echo Darwin',
                "sysctl": 'echo 1',
                "sw_vers": 'echo 14.0',
                "curl": '''if [[ "$*" == *api.github.com* ]]; then
  if [[ "$TEST_FAIL" == network ]]; then exit 22
  elif [[ "$TEST_FAIL" == no_assets ]]; then echo '{"assets": []}'; exit 0
  elif [[ "$TEST_FAIL" == windows_only ]]; then
    url='https://github.com/chenmozhe008/shandian-download/releases/download/v1/Shandian-1-Windows-x64.zip'
  elif [[ "$TEST_FAIL" == legacy ]]; then
    url='https://github.com/chenmozhe008/shandian-download/releases/download/v0.16.0/Shandian-0.16.0.zip'
  elif [[ "$TEST_FAIL" == wrong_host ]]; then
    url='https://example.com/Shandian-1-AppleSilicon.zip'
  else
    url='https://github.com/chenmozhe008/shandian-download/releases/download/v1/Shandian-1-AppleSilicon.zip'
  fi
  digest='sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855'
  if [[ "$TEST_FAIL" == bad_digest ]]; then digest='sha256:0000000000000000000000000000000000000000000000000000000000000000'; fi
  if [[ "$TEST_FAIL" == invalid_digest ]]; then digest='sha256:invalid'; fi
  if [[ "$TEST_FAIL" == no_digest ]]; then digest=''; fi
  if [[ "$TEST_FAIL" == multiple ]]; then
    printf '{"assets":[{"browser_download_url": "https://github.com/chenmozhe008/shandian-download/releases/download/v1/Shandian-1-Intel.zip", "digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000"},{"browser_download_url": "%s", "digest": "%s"}]}' "$url" "$digest"
  else
    printf '{"assets":[{"browser_download_url": "%s", "digest": "%s"}]}' "$url" "$digest"
  fi
else
  while [[ $# -gt 0 ]]; do if [[ "$1" == -o ]]; then : > "$2"; exit 0; fi; shift; done
  exit 1
fi''',
                "ditto": '''if [[ "$1" == -x ]]; then
  app="$4/闪电全能下载.app"
  mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
  echo new > "$app/new"; touch "$app/Contents/Info.plist" "$app/Contents/MacOS/Shandian"
  chmod +x "$app/Contents/MacOS/Shandian"
  if [[ "$TEST_FAIL" != missing_engine ]]; then touch "$app/Contents/Resources/gopeed"; chmod +x "$app/Contents/Resources/gopeed"; fi
elif [[ "$TEST_FAIL" == stage ]]; then exit 1
else /bin/cp -R "$1" "$2"; fi''',
                "plutil": '''if [[ "$*" == *Info.plist* ]]; then
  if [[ "$TEST_FAIL" == bad_id ]]; then echo wrong.id; else echo io.github.chenmozhe008.shandian; fi
else exec /usr/bin/plutil "$@"; fi''',
                "codesign": '[[ "$TEST_FAIL" != bad_signature ]]',
                "mv": '''src="$1"; dst="$2"
case "$src" in "$HOME/.Trash/"*) src="$TEST_TRASH/${src##*/}" ;; esac
case "$dst" in "$HOME/.Trash/"*) dst="$TEST_TRASH/${dst##*/}" ;; esac
if [[ ( "$TEST_FAIL" == replace || "$TEST_FAIL" == restore ) && "$src" == *"/.shandian-install."* ]]; then exit 1; fi
if [[ "$TEST_FAIL" == restore && "$src" == "$TEST_TRASH/"* ]]; then exit 1; fi
/bin/mv "$src" "$dst"''',
                "xattr": 'exit 0',
            }
            for name, body in mocks.items():
                path = bin_dir / name
                path.write_text("#!/bin/bash\n" + body + "\n")
                path.chmod(0o755)
            env = os.environ.copy()
            env.update(PATH=f"{bin_dir}:/usr/bin:/bin:/usr/sbin:/sbin", SHANDIAN_DEST=str(dest),
                       SHANDIAN_TEST="1", TEST_TRASH=str(trash), TEST_FAIL=failure)
            installer = root / "install.sh"
            installer.write_text(INSTALLER.read_text().replace("/usr/bin/plutil", str(bin_dir / "plutil"))
                                 .replace("/usr/bin/codesign", str(bin_dir / "codesign")))
            result = subprocess.run(["/bin/bash", str(installer), "--no-ffmpeg"], env=env,
                                    capture_output=True, text=True)
            old = (dest / APP / "old").exists()
            new = (dest / APP / "new").exists()
            backups = list(trash.glob("*.app/old"))
            stages = list(dest.glob(".shandian-install.*"))
            return result.returncode, old, new, len(backups), stages, result.stderr

    def test_success_preserves_backup(self):
        status, old, new, backups, stages, stderr = self.run_install()
        self.assertEqual((status, old, new, backups, stages), (0, False, True, 1, []), stderr)

    def test_prepare_failure_preserves_original(self):
        status, old, new, backups, stages, _ = self.run_install("stage")
        self.assertNotEqual(status, 0)
        self.assertEqual((old, new, backups, stages), (True, False, 0, []))

    def test_replacement_failure_restores_original(self):
        status, old, new, backups, stages, stderr = self.run_install("replace")
        self.assertNotEqual(status, 0)
        self.assertEqual((old, new, backups, stages), (True, False, 0, []))
        self.assertIn("已恢复原来的应用", stderr)

    def test_windows_package_is_not_a_mac_fallback(self):
        status, old, new, backups, stages, _ = self.run_install("windows_only")
        self.assertNotEqual(status, 0)
        self.assertEqual((old, new, backups, stages), (True, False, 0, []))

    def test_restore_failure_keeps_backup_and_reports_location(self):
        status, old, new, backups, stages, stderr = self.run_install("restore")
        self.assertNotEqual(status, 0)
        self.assertEqual((old, new, backups, stages), (False, False, 1, []))
        self.assertIn("请手动恢复", stderr)

    def test_legacy_mac_package_is_accepted(self):
        status, old, new, backups, stages, stderr = self.run_install("legacy")
        self.assertEqual((status, old, new, backups, stages), (0, False, True, 1, []), stderr)

    def test_invalid_packages_preserve_original(self):
        for failure in ("bad_id", "bad_signature", "missing_engine", "bad_digest", "invalid_digest"):
            with self.subTest(failure=failure):
                status, old, new, backups, stages, _ = self.run_install(failure)
                self.assertNotEqual(status, 0)
                self.assertEqual((old, new, backups, stages), (True, False, 0, []))

    def test_release_without_digest_remains_supported(self):
        status, old, new, backups, stages, stderr = self.run_install("no_digest")
        self.assertEqual((status, old, new, backups, stages), (0, False, True, 1, []), stderr)

    def test_digest_matches_selected_architecture(self):
        status, old, new, backups, stages, stderr = self.run_install("multiple")
        self.assertEqual((status, old, new, backups, stages), (0, False, True, 1, []), stderr)

    def test_unavailable_release_preserves_original(self):
        for failure in ("network", "no_assets", "wrong_host"):
            with self.subTest(failure=failure):
                status, old, new, backups, stages, _ = self.run_install(failure)
                self.assertNotEqual(status, 0)
                self.assertEqual((old, new, backups, stages), (True, False, 0, []))


if __name__ == "__main__":
    unittest.main()
