"""Bound one saved-view CLI export and retain reproducible local evidence, including timeouts.

This checks complete SVG generation, not raster/GPU frame pacing. It reads an explicitly
selected local document and never connects to Studio or changes the source document.
"""

import argparse
import hashlib
import json
import math
from pathlib import Path
import subprocess
import time


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cli', type=Path, required=True)
    parser.add_argument('--doc', type=Path, required=True)
    parser.add_argument('--view', required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--limit-s', type=float, default=30.0)
    args = parser.parse_args()
    if not math.isfinite(args.limit_s) or not 0.1 <= args.limit_s <= 600:
        parser.error('limit-s must be between 0.1 and 600 seconds')
    binary, source = args.cli.resolve(), args.doc.resolve()
    if args.out.exists() and any(args.out.iterdir()):
        parser.error('out must be a new or empty directory, so stale output cannot pass')
    args.out.mkdir(parents=True, exist_ok=True)
    output = (args.out / 'drawing.svg').resolve()
    before = {'cli': digest(binary), 'document': digest(source)}
    commands = [{'command': 'document.open', 'params': {'path': str(source)}},
                {'command': 'view.export', 'params': {'id': args.view, 'path': str(output)}}]
    script = (args.out / 'commands.json').resolve()
    script.write_text(json.dumps(commands, indent=2) + '\n')
    command = [str(binary), 'script', str(script)]
    start = time.perf_counter()
    with (args.out / 'stdout.log').open('w') as stdout, (args.out / 'stderr.log').open('w') as stderr:
        try:
            completed = subprocess.run(command, stdout=stdout, stderr=stderr, timeout=args.limit_s)
            status, code = 'completed', completed.returncode
        except subprocess.TimeoutExpired:
            status, code = 'timeout', None
    elapsed = time.perf_counter() - start
    unchanged = before == {'cli': digest(binary), 'document': digest(source)}
    valid = status == 'completed' and code == 0 and output.is_file() and b'<svg' in output.read_bytes()[:1000]
    result = {'command': command, 'cli_sha256': before['cli'], 'document_sha256': before['document'],
              'status': status, 'exit_code': code, 'elapsed_s': elapsed, 'limit_s': args.limit_s,
              'inputs_unchanged': unchanged, 'passed': valid and unchanged,
              'output_sha256': digest(output) if valid else None,
              'scope': 'Fresh-process startup, model load/build, exact saved-view generation and SVG write; excludes native FPS and GPU.'}
    (args.out / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))
    return 0 if result['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
