from __future__ import annotations

import shutil
import tempfile
import unittest
from pathlib import Path

import check_docs


PROJECT_ROOT = Path(__file__).resolve().parents[4]


class CheckDocsModesTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.root = Path(self.temp_dir.name) / "repository"
        shutil.copytree(
            PROJECT_ROOT,
            self.root,
            symlinks=True,
            ignore=shutil.ignore_patterns(".git", "__pycache__"),
        )
        self.original_root = check_docs.ROOT
        check_docs.ROOT = self.root.resolve()
        current_result = check_docs.check()
        migration_result = check_docs.check(check_migration=True)
        self.assertTrue(current_result["ok"], current_result["errors"])
        self.assertTrue(migration_result["ok"], migration_result["errors"])

    def tearDown(self) -> None:
        check_docs.ROOT = self.original_root
        self.temp_dir.cleanup()

    def append_broken_link(self, relative_path: str) -> None:
        path = self.root / relative_path
        path.write_text(
            path.read_text(encoding="utf-8") + "\n[broken fixture link](missing.md)\n",
            encoding="utf-8",
        )

    def test_current_document_breakage_fails_both_modes(self) -> None:
        self.append_broken_link("README.md")

        current_result = check_docs.check()
        migration_result = check_docs.check(check_migration=True)
        self.assertFalse(current_result["ok"])
        self.assertFalse(migration_result["ok"])
        self.assertTrue(
            any("Broken relative link" in error for error in current_result["errors"])
        )
        self.assertTrue(
            any("Broken relative link" in error for error in migration_result["errors"])
        )

    def test_archived_document_breakage_only_fails_migration_check(self) -> None:
        self.append_broken_link("docs/meta/source-map.md")

        current_result = check_docs.check()
        self.assertTrue(current_result["ok"], current_result["errors"])
        migration_result = check_docs.check(check_migration=True)
        self.assertFalse(migration_result["ok"])
        self.assertTrue(
            any("Broken relative link" in error for error in migration_result["errors"])
        )

    def test_source_hash_breakage_only_fails_migration_check(self) -> None:
        source = self.root / "archive/design-0.1.md"
        source.write_bytes(source.read_bytes() + b"\nfixture change\n")

        current_result = check_docs.check()
        self.assertTrue(current_result["ok"], current_result["errors"])
        migration_result = check_docs.check(check_migration=True)
        self.assertFalse(migration_result["ok"])
        self.assertTrue(
            any("Source snapshot modified" in error for error in migration_result["errors"])
        )


if __name__ == "__main__":
    unittest.main()
