"""CLI regressions for benchmark platform selection and honest binary provenance."""
import importlib.util
from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock


SCRIPT = Path(__file__).with_name("benchmark-workflow.py")


class BenchmarkContract(unittest.TestCase):
    def unicode_workflow(self, capture_encoding, log_encoding):
        """Run tiny real child processes while emulating legacy Windows text defaults."""
        spec = importlib.util.spec_from_file_location("unicode_workflow_benchmark", SCRIPT)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        run = subprocess.run
        write_text = Path.write_text
        read_text = Path.read_text
        binary = str(Path(sys.executable).resolve())
        child = """
import json, sys
from pathlib import Path
directory = Path(sys.argv[1])
directory.mkdir()
for filename in ['ground.svg', 'street.svg', 'section.svg', 'sample.glb', 'opening-schedule.json']:
    (directory / filename).write_bytes(b'synthetic runner fixture')
(directory / 'verification.json').write_text(json.dumps({
    'move_undo_exact': True, 'copy_undo_schedule_exact': True,
    'ifc': {'readers': [{'failures': 0}, {'failures': 0}]}, 'note': '李'
}, ensure_ascii=False), encoding='utf-8')
sys.stdout.buffer.write(('færdig 李 ' + str(directory) + '\\n').encode('utf-8'))
"""

        def legacy_run(args, **kwargs):
            if str(args[0]) == binary:
                kwargs.setdefault("encoding", capture_encoding)
                return run([binary, "-c", child, str(args[1])], **kwargs)
            return run(args, **kwargs)

        def legacy_write(path, data, *args, **kwargs):
            if path.suffix == ".log":
                kwargs.setdefault("encoding", log_encoding)
            return write_text(path, data, *args, **kwargs)

        def legacy_read(path, *args, **kwargs):
            if path.name == "verification.json":
                kwargs.setdefault("encoding", log_encoding)
            return read_text(path, *args, **kwargs)

        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "李"
            with mock.patch.object(sys, "argv", [str(SCRIPT), "--binary", binary, "--out", str(output)]), \
                    mock.patch.object(module.subprocess, "run", side_effect=legacy_run), \
                    mock.patch.object(Path, "write_text", legacy_write), \
                    mock.patch.object(Path, "read_text", legacy_read), redirect_stdout(io.StringIO()):
                code = module.main()
            self.assertEqual(code, 0, (output / "results.json").read_text(encoding="utf-8"))
            report = json.loads((output / "results.json").read_text(encoding="utf-8"))
            self.assertEqual(report["status"], "PASS")
            self.assertEqual(len(report["trials"]), 5)
            for name in ["warmup", *[f"trial-{i:02}" for i in range(1, 6)]]:
                self.assertEqual((output / f"{name}.log").read_bytes(), f"færdig 李 {(output / name).resolve()}\n".encode("utf-8"))

    def test_utf8_workflow_output_decodes_under_a_legacy_windows_default(self):
        self.unicode_workflow("cp1252", "utf-8")

    def test_utf8_workflow_logs_survive_a_legacy_windows_default(self):
        self.unicode_workflow("utf-8", "cp1252")

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
