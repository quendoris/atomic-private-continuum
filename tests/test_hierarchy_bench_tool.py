from __future__ import annotations

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


class HierarchyBenchToolTests(unittest.TestCase):
    def test_smoke_preset_writes_machine_readable_result(self) -> None:
        repo_root = Path(__file__).resolve().parents[1]
        tool = repo_root / "tools" / "run_hierarchy_bench.py"

        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "result.json"
            completed = subprocess.run(
                [
                    sys.executable,
                    str(tool),
                    "--preset",
                    "smoke",
                    "--full-history",
                    "--output",
                    str(output),
                ],
                cwd=repo_root,
                check=True,
                capture_output=True,
                text=True,
            )

            payload = json.loads(output.read_text(encoding="utf-8"))
            stdout_payload = json.loads(completed.stdout)

        self.assertEqual(payload, stdout_payload)
        self.assertEqual(payload["schema"], "apc-hierarchy-bench-v1")
        self.assertEqual(payload["workload"]["atoms"], 100)
        self.assertIn("root", payload["policies"])
        self.assertIn("one_witness", payload["policies"])
        self.assertIn("full_history", payload["policies"])
        self.assertGreaterEqual(
            payload["policies"]["root"]["metrics"]["initial_cycle_count"],
            4,
        )


if __name__ == "__main__":
    unittest.main()
