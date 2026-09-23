#!/usr/bin/env python3
"""Exercise release guards in a disposable repository with local command stubs."""

import hashlib
import os
import plistlib
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


VERSION = "0.3.27"
IDENTITY = "Developer ID Application: Vibra (ABCDEFGHIJ)"


class ReleaseScriptTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="vibra-release-test-")
        self.addCleanup(self.temporary.cleanup)
        base = Path(self.temporary.name)
        self.root = base / "repo"
        self.app = base / "fixture/Vibra.app"
        for directory in (
            "Scripts",
            "Resources",
            "third_party/sparkle-2.9.4/bin",
            "third_party/sparkle-2.9.4/Sparkle.framework",
            "mock-bin",
            "dist",
        ):
            (self.root / directory).mkdir(parents=True, exist_ok=True)
        (self.app / "Contents").mkdir(parents=True)
        shutil.copy2(Path(__file__).with_name("release.sh"), self.root / "Scripts/release.sh")
        self.install_command_stubs()
        (self.root / ".gitignore").write_text("dist/\n")
        (self.root / "Cargo.toml").write_text(
            f'[package]\nname = "vibra"\nversion = "{VERSION}"\n'
        )
        (self.root / "CHANGELOG.md").write_text(
            "## 0x3x27 — wrong\n- Wrong release\n\n"
            "## 0.3.27 — valid\n- Correct release\n\n"
            "## 0.3.26 — older\n- Older release\n"
        )
        (self.root / "Resources/Info.plist").write_bytes(
            plistlib.dumps({"CFBundleIdentifier": "app.vibra.Vibra"})
        )
        for command in (
            ["git", "init", "-q", "-b", "main"],
            ["git", "config", "user.email", "test@example.com"],
            ["git", "config", "user.name", "Test"],
            ["git", "add", "."],
            ["git", "commit", "-qm", "fixture"],
        ):
            subprocess.run(command, cwd=self.root, check=True)
        self.commit = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=self.root, text=True
        ).strip()
        self.environment = os.environ.copy()
        self.environment.update(
            PATH=str(self.root / "mock-bin") + os.pathsep + self.environment["PATH"],
            VIBRA_TEST_APP=str(self.app),
            VIBRA_SIGNING_IDENTITY=IDENTITY,
        )

    def install_command_stubs(self):
        self.write_executable(
            "Scripts/package_app.sh",
            '#!/bin/zsh\nset -e\nprint -r -- "$*" > "${0:A:h:h}/dist/package-args.txt"\n'
            'print -r -- fake > "${0:A:h:h}/dist/Vibra.dmg"\n',
        )
        self.write_executable(
            "Scripts/fetch_sparkle.sh",
            '#!/bin/zsh\nprint -r -- "$*" > "${0:A:h:h}/dist/fetch-args.txt"\n'
            'print -r -- "${0:A:h:h}/third_party/sparkle-2.9.4/Sparkle.framework"\n',
        )
        self.write_executable(
            "third_party/sparkle-2.9.4/bin/generate_appcast",
            '#!/bin/zsh\nfor argument in "$@"; do output=$argument; done\n'
            'print -r -- "<rss/>" > "$output/appcast.xml"\n',
        )
        self.write_executable(
            "mock-bin/hdiutil",
            '#!/bin/zsh\nif [[ $1 == attach ]]; then\n'
            '  while (( $# )); do\n'
            '    if [[ $1 == -mountpoint ]]; then shift; point=$1; fi\n'
            '    shift\n'
            '  done\n'
            '  cp -R "$VIBRA_TEST_APP" "$point/Vibra.app"\n'
            'fi\n',
        )
        self.write_executable(
            "mock-bin/codesign",
            '#!/bin/zsh\nif [[ $1 == -dv ]]; then\n'
            '  print -u2 -- "Authority=Developer ID Application: Vibra (ABCDEFGHIJ)"\n'
            '  print -u2 -- "TeamIdentifier=${VIBRA_TEST_TEAM:-ABCDEFGHIJ}"\n'
            'fi\n',
        )
        self.write_executable(
            "mock-bin/xcrun",
            '#!/bin/zsh\n'
            'if [[ $1 == stapler && $2 == validate && -n ${VIBRA_TEST_STAPLE_MARKER:-} ]]; then\n'
            '  [[ -f $VIBRA_TEST_STAPLE_MARKER ]] || exit 1\n'
            'elif [[ $1 == stapler && $2 == staple ]]; then\n'
            '  touch "$VIBRA_TEST_STAPLE_MARKER"\n'
            'elif [[ $1 == notarytool && $2 == info ]]; then\n'
            '  print -r -- \'{"status":"Accepted"}\'\n'
            'fi\n',
        )
        self.write_executable("mock-bin/spctl", "#!/bin/zsh\nexit 0\n")

    def write_executable(self, relative_path, contents):
        path = self.root / relative_path
        path.write_text(contents)
        path.chmod(0o755)

    def run_release(self, *arguments, environment=None):
        return subprocess.run(
            [str(self.root / "Scripts/release.sh"), VERSION, *arguments],
            cwd=self.root,
            env=environment or self.environment,
            capture_output=True,
            text=True,
        )

    def prepare_resumed_dmg(self, source_commit):
        (self.root / "dist/Vibra.dmg").write_text("fake signed dmg")
        (self.app / "Contents/Info.plist").write_bytes(
            plistlib.dumps(
                {
                    "CFBundleIdentifier": "app.vibra.Vibra",
                    "CFBundleShortVersionString": VERSION,
                    "CFBundleVersion": "1",
                    "VibraSourceCommit": source_commit,
                }
            )
        )

    def test_local_dry_run_uses_literal_changelog_heading(self):
        result = self.run_release("--dry-run", "--no-notarize")
        self.assertEqual(result.returncode, 0, result.stderr)
        notes = (self.root / f"dist/appcast/Vibra-{VERSION}.html").read_text()
        self.assertIn("Correct release", notes)
        self.assertNotIn("Wrong release", notes)
        package_args = (self.root / "dist/package-args.txt").read_text()
        self.assertIn("--sign -", package_args)
        rejected = self.run_release("--no-notarize")
        self.assertEqual(rejected.returncode, 64)

    def test_release_rejects_symlinked_dist_and_shallow_history(self):
        external = self.app.parent / "outside-dist"
        external.mkdir()
        sentinel = external / "keep.txt"
        sentinel.write_text("untouched")
        (self.root / "dist").rmdir()
        (self.root / "dist").symlink_to(external, target_is_directory=True)
        linked = self.run_release("--dry-run", "--no-notarize")
        self.assertEqual(linked.returncode, 65, linked.stderr)
        self.assertEqual(sentinel.read_text(), "untouched")

        (self.root / "dist").unlink()
        (self.root / "dist").mkdir()
        (self.root / ".git/shallow").write_text(self.commit + "\n")
        shallow = self.run_release("--dry-run", "--no-notarize")
        self.assertEqual(shallow.returncode, 65, shallow.stderr)
        self.assertIn("shallow clone", shallow.stderr)

    def test_resume_rejects_other_commit_and_signing_team(self):
        self.prepare_resumed_dmg("0" * 40)
        stale = self.run_release("--resume-dmg", "--dry-run")
        self.assertEqual(stale.returncode, 65, stale.stderr)
        self.assertIn("expected", stale.stderr)

        self.prepare_resumed_dmg(self.commit)
        wrong_environment = self.environment | {"VIBRA_TEST_TEAM": "ZZZZZZZZZZ"}
        wrong_team = self.run_release(
            "--resume-dmg", "--dry-run", environment=wrong_environment
        )
        self.assertEqual(wrong_team.returncode, 65, wrong_team.stderr)
        self.assertIn("was not signed", wrong_team.stderr)

        accepted = self.run_release("--resume-dmg", "--dry-run")
        self.assertEqual(accepted.returncode, 0, accepted.stderr)

    def test_resume_staples_an_accepted_submission(self):
        self.prepare_resumed_dmg(self.commit)
        record = self.root / "dist/notarization/Vibra.dmg.submission-id"
        record.parent.mkdir(parents=True)
        record.write_text("11111111-2222-3333-4444-555555555555\n")
        marker = self.root / "dist/stapled"
        environment = self.environment | {"VIBRA_TEST_STAPLE_MARKER": str(marker)}

        result = self.run_release("--resume-dmg", "--dry-run", environment=environment)

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(marker.exists())
        self.assertEqual((self.root / "dist/fetch-args.txt").read_text().strip(), "--refresh")

    def test_notarized_package_requires_source_matching_clean_commit(self):
        shutil.copy2(Path(__file__).with_name("package_app.sh"), self.root / "Scripts/package_app.sh")
        subprocess.run(["git", "add", "Scripts/package_app.sh"], cwd=self.root, check=True)
        subprocess.run(["git", "commit", "-qm", "package guard"], cwd=self.root, check=True)

        cargo = self.root / "Cargo.toml"
        cargo.write_text(cargo.read_text() + "# uncommitted source\n")
        dirty = subprocess.run(
            [str(self.root / "Scripts/package_app.sh"), "release", "--notarize"],
            cwd=self.root,
            env=self.environment,
            capture_output=True,
            text=True,
        )
        self.assertEqual(dirty.returncode, 65, dirty.stderr)
        self.assertIn("clean checkout", dirty.stderr)

        subprocess.run(["git", "checkout", "--", "Cargo.toml"], cwd=self.root, check=True)
        mismatched = subprocess.run(
            [str(self.root / "Scripts/package_app.sh"), "release", "--notarize"],
            cwd=self.root,
            env=self.environment | {"VIBRA_SOURCE_COMMIT": "0" * 40},
            capture_output=True,
            text=True,
        )
        self.assertEqual(mismatched.returncode, 65, mismatched.stderr)
        self.assertIn("does not match", mismatched.stderr)

    def test_sparkle_refresh_replaces_cache_only_after_checksum_verification(self):
        shutil.copy2(Path(__file__).with_name("fetch_sparkle.sh"), self.root / "Scripts/fetch_sparkle.sh")
        source = Path(self.temporary.name) / "sparkle-source"
        (source / "Sparkle.framework").mkdir(parents=True)
        (source / "bin").mkdir()
        (source / "Sparkle.framework/verified.txt").write_text("verified")
        tool = source / "bin/generate_appcast"
        tool.write_text("#!/bin/sh\n")
        tool.chmod(0o755)
        archive = Path(self.temporary.name) / "sparkle.tar.xz"
        subprocess.run(["tar", "-cJf", str(archive), "-C", str(source), "."], check=True)
        checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
        self.write_executable(
            "mock-bin/curl",
            '#!/bin/zsh\nwhile (( $# )); do\n'
            '  if [[ $1 == -o ]]; then shift; output=$1; fi\n'
            '  shift\ndone\ncp "$VIBRA_TEST_ARCHIVE" "$output"\n',
        )
        cache = self.root / "third_party/sparkle-9.9.9"
        (cache / "Sparkle.framework").mkdir(parents=True)
        (cache / "Sparkle.framework/stale.txt").write_text("stale")
        environment = self.environment | {
            "VIBRA_SPARKLE_VERSION": "9.9.9",
            "VIBRA_SPARKLE_SHA256": checksum,
            "VIBRA_TEST_ARCHIVE": str(archive),
        }
        refresh = subprocess.run(
            [str(self.root / "Scripts/fetch_sparkle.sh"), "--refresh"],
            cwd=self.root,
            env=environment,
            capture_output=True,
            text=True,
        )
        self.assertEqual(refresh.returncode, 0, refresh.stderr)
        self.assertFalse((cache / "Sparkle.framework/stale.txt").exists())
        self.assertEqual((cache / "Sparkle.framework/verified.txt").read_text(), "verified")

        rejected = subprocess.run(
            [str(self.root / "Scripts/fetch_sparkle.sh"), "--refresh"],
            cwd=self.root,
            env=environment | {"VIBRA_SPARKLE_SHA256": "0" * 64},
            capture_output=True,
            text=True,
        )
        self.assertEqual(rejected.returncode, 65, rejected.stderr)
        self.assertEqual((cache / "Sparkle.framework/verified.txt").read_text(), "verified")

        pinned_override = subprocess.run(
            [str(self.root / "Scripts/fetch_sparkle.sh"), "--refresh"],
            cwd=self.root,
            env=environment | {
                "VIBRA_SPARKLE_VERSION": "2.9.4",
                "VIBRA_SPARKLE_SHA256": "0" * 64,
            },
            capture_output=True,
            text=True,
        )
        self.assertEqual(pinned_override.returncode, 64, pinned_override.stderr)
        self.assertIn("pinned checksum", pinned_override.stderr)


if __name__ == "__main__":
    unittest.main()
