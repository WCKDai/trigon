"""External check of a Trigon IFC export with IfcOpenShell, run as a separate program.

    uv run -q --with ifcopenshell --with pytest python -I tools/ifc_oracle.py FILE.ifc [--no-rules]

IfcOpenShell (LGPL) is only run here, never linked into or read by Trigon. The script reports, as
JSON on stdout:

- schema and where-rule errors (`ifcopenshell.validate` with EXPRESS rules unless --no-rules);
- products by class;
- whether IfcOpenShell builds a shape for every product that has a representation;
- per wall, IfcOpenShell's volume (openings cut) against the wall's Qto_WallBaseQuantities
  NetVolume, which Trigon computes from the piece it exports (plan outline times height, less
  the openings; Trigon's tests check this equals its own closed wall piece).

Exit status 0 when there are no errors, every product has a shape and every wall volume is within
0.5 %; 1 otherwise.
"""

import collections
import json
import multiprocessing
import sys

import ifcopenshell
import ifcopenshell.geom
import ifcopenshell.util.element
import ifcopenshell.util.shape
import ifcopenshell.validate

TOLERANCE = 0.005


def validate(f, rules):
    logger = ifcopenshell.validate.json_logger()
    ifcopenshell.validate.validate(f, logger, express_rules=rules)
    by_kind = collections.Counter()
    first = []
    for s in logger.statements:
        if s.get("level", "error") != "error":
            continue
        msg = str(s.get("message", ""))
        key = msg.split("\n")[0][:160]
        by_kind[key] += 1
        if len(first) < 15:
            inst = s.get("instance")
            first.append({"message": msg[:400], "instance": str(inst)[:200] if inst is not None else None})
    return {"errors": sum(by_kind.values()), "by_message": dict(by_kind.most_common(20)), "first": first}


def shapes(f):
    products = [p for p in f.by_type("IfcProduct") if p.Representation is not None]
    settings = ifcopenshell.geom.settings()
    it = ifcopenshell.geom.iterator(settings, f, multiprocessing.cpu_count(), include=products)
    built = {}
    if it.initialize():
        while True:
            s = it.get()
            built[s.id] = s
            if not it.next():
                break
    missing = [p for p in products if p.id() not in built]
    return products, built, missing


def net_volume(wall):
    q = ifcopenshell.util.element.get_pset(wall, "Qto_WallBaseQuantities")
    return q.get("NetVolume") if q else None


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    if len(args) != 1:
        print(__doc__, file=sys.stderr)
        return 2
    f = ifcopenshell.open(args[0])
    out = {"file": args[0], "schema": f.schema}
    out["validation"] = validate(f, "--no-rules" not in sys.argv)
    out["by_class"] = dict(sorted(collections.Counter(p.is_a() for p in f.by_type("IfcProduct")).items()))
    products, built, missing = shapes(f)
    out["shapes"] = {
        "products_with_representation": len(products),
        "built": len(built),
        "missing": len(missing),
        "first_missing": [f"#{p.id()} {p.is_a()} {p.Name}" for p in missing[:10]],
    }
    rows = []
    for wall in f.by_type("IfcWall"):
        expected = net_volume(wall)
        s = built.get(wall.id())
        if expected is None or s is None:
            rows.append({"id": wall.GlobalId, "name": wall.Name, "expected": expected, "ifcopenshell": None})
            continue
        got = ifcopenshell.util.shape.get_volume(s.geometry)
        rows.append({"id": wall.GlobalId, "name": wall.Name, "class": wall.is_a(), "expected": expected, "ifcopenshell": got})
    devs = [abs(r["ifcopenshell"] - r["expected"]) / r["expected"] for r in rows if r.get("ifcopenshell") is not None and r["expected"]]
    worst = sorted(
        (r for r in rows if r.get("ifcopenshell") is not None and r["expected"]),
        key=lambda r: -abs(r["ifcopenshell"] - r["expected"]) / r["expected"],
    )[:5]
    out["wall_volumes"] = {
        "walls": len(rows),
        "compared": len(devs),
        "without_quantity_or_shape": len(rows) - len(devs),
        "max_relative_deviation": max(devs) if devs else None,
        "over_tolerance": sum(1 for d in devs if d > TOLERANCE),
        "worst": worst,
    }
    print(json.dumps(out, indent=1, default=str))
    ok = (
        out["validation"]["errors"] == 0
        and not missing
        and out["wall_volumes"]["over_tolerance"] == 0
        and out["wall_volumes"]["without_quantity_or_shape"] == 0
    )
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
