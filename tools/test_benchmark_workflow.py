"""CLI regressions for benchmark platform selection and honest binary provenance."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("benchmark-workflow.py")


class BenchmarkContract(unittest.TestCase):
    def test_windows_default_matches_cargo_executable_name(self):
        spec = importlib.util.spec_from_file_location("workflow_benchmark", SCRIPT)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        self.assertEqual(module.default_binary("Windows").name, "drawing-workflow.exe")
        self.assertEqual(module.default_binary("Darwin").name, "drawing-workflow")
        self.assertEqual(module.default_binary("Linux").name, "drawing-workflow")

    def test_an_arbitrary_selected_executable_is_not_attributed_to_the_checkout(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "result"
            # Python is deliberately not Trigon. Its failed warmup must still leave
            # truthful provenance instead of a current-revision/release assertion.
            result = subprocess.run([sys.executable, str(SCRIPT), "--binary", sys.executable, "--out", str(output)],
                                    capture_output=True, text=True, check=False)
            self.assertNotEqual(result.returncode, 0)
            report = json.loads((output / "results.json").read_text())
            self.assertEqual(report["status"], "FAIL")
            self.assertIsNone(report.get("profile"), "a selected Python executable must not be described as a Trigon release build")
            self.assertEqual(report["binary_provenance"]["status"], "unverified")
            self.assertIsNone(report["revision"])
            self.assertIsNone(report["profile"])
            self.assertIsNone(report["tracked_dirty"])
            self.assertTrue(report["checkout_context"]["revision"])
            self.assertEqual(report["binary_path"], str(Path(sys.executable).resolve()))


if __name__ == "__main__":
    unittest.main()
