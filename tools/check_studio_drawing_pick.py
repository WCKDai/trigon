"""Check exact native drawing picking through an explicitly selected local Studio session.

Open the drawing and arrange the editing mode first. For the wall-alignment regression, capture
a reference wall, then check a nearby target within the reference's normal drag-handle radius.
The check runs read-only view.pick with the canvas's ten-window-point tolerance, clicks the
corresponding point, and compares native selection. It never discovers or connects to the
owner's default Studio session.
"""

import argparse
import json
import math
from pathlib import Path
import socket


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--control-dir', type=Path, required=True)
    parser.add_argument('--drawing', required=True)
    parser.add_argument('--point', type=float, nargs=2, required=True, metavar=('X_MM', 'Y_MM'))
    parser.add_argument('--expected-id', required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    if not all(math.isfinite(value) and abs(value) <= 1e8 for value in args.point):
        parser.error('point must contain finite site/drawing coordinates')
    port = int((args.control_dir / 'current.port').read_text())
    if not 0 < port < 65536:
        parser.error('invalid Studio control port')
    token = (args.control_dir / f'{port}.token').read_text().strip()

    def ask(method, params):
        with socket.create_connection(('127.0.0.1', port), timeout=5) as connection:
            connection.settimeout(55)
            connection.sendall((json.dumps({'id': 1, 'method': method, 'params': params, 'token': token}) + '\n').encode())
            reply = json.loads(connection.makefile('rb').readline())
        if not reply.get('ok'):
            raise RuntimeError(reply.get('error', 'Studio request failed'))
        return reply['result']

    before = ask('ui.view', {})
    if before['drawing'] != args.drawing or before['drawing_mapping'] is None:
        parser.error('open the requested drawing and let it finish rendering first')
    mapping = before['drawing_mapping']
    scale = mapping['points_per_mm']
    if not math.isfinite(scale) or scale <= 0:
        parser.error('the native drawing mapping must have a finite positive scale')
    # Match PlanView::tolerance_mm. A fixed model-space tolerance can select a region
    # while the native cursor correctly chooses a nearby boundary at the current zoom.
    tolerance_mm = min(100_000.0, 10.0 / scale)
    hit = ask('command.run', {'command': 'view.pick', 'params': {
        'id': args.drawing, 'point': args.point, 'tolerance_mm': tolerance_mm,
        'phase': before['phase'], 'filter': before['filter']}})['hit']
    if hit is None or hit['id'] != args.expected_id:
        raise RuntimeError('the exact drawing query does not identify the expected test element')
    screen = [mapping['origin'][0] + args.point[0] * mapping['points_per_mm'],
              mapping['origin'][1] - args.point[1] * mapping['points_per_mm']]
    x, y, width, height = mapping['viewport']
    if not (x <= screen[0] <= x + width and y <= screen[1] <= y + height):
        parser.error('test point is outside the native viewport; fit or pan the drawing first')
    args.out.mkdir(parents=True, exist_ok=True)
    ask('ui.click', {'x': screen[0], 'y': screen[1]})
    # Native clicks queue move/down/up on separate frames. Two window captures consume that
    # finite sequence and its screenshot frame; a single capture may still show pointer-down.
    for name in ['queued.png', 'settled.png']:
        ask('ui.screenshot', {'target': 'window', 'path': str((args.out / name).resolve())})
    after = ask('ui.view', {})
    report = {'expected_id': args.expected_id, 'query_id': hit['id'],
              'selection_before': before['selection'], 'selection_after': after['selection'],
              'drawing_point_mm': args.point, 'window_point': screen, 'handles_before': before['handles'],
              'native_tolerance_mm': tolerance_mm,
              'passed': after['selection'] == args.expected_id,
              'scope': 'Native selection consistency; frame pacing and GPU upload cost are not measured.'}
    (args.out / 'result.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({key: report[key] for key in ['expected_id', 'selection_after', 'passed']}))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
