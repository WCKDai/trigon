# Requested outer roof-cover height

`building.set_roof` accepts exactly one of `pitch_deg` and `ridge` for pitched and
half-hipped roofs. `ridge` is the maximum height of the **whole outer roof cover**,
in site millimetres above the terrain. It includes normal build-up, gabled and
extended verge geometry, and any explicit half-hip caps. It is not the height of
a named principal ridge, or a constraint that keeps several roofs aligned after
subsequent edits. Gutters, dormer assemblies and roof-window fittings are separate
elements; this height request concerns the generated roof cover itself.

For example, on an 8000 × 6000 mm footprint with 3000 mm eaves:

```json
{
  "building": "b",
  "kind": "pitched",
  "ridge": 6000,
  "gables": ["b.f2", "b.f4"],
  "straighten_mm": 0,
  "overhang_mm": 400,
  "thickness_mm": 200
}
```

The normal 200 mm build-up raises the top by `200 / cos(pitch)` vertically.
Consequently this request needs a pitch below 45°. At an explicit 45° the top is
6282.842712 mm. With zero thickness, the same requested height needs 45°.

The solver uses the existing gabled heightfield and extended outline, with fresh
half-hip clipping at each trial pitch. It constructs at most two straight skeletons
and measures two endpoints followed by at most 64 bisections. It does not rebuild
walls, openings or the whole building during the solve. The command then builds
the final model once with all existing guards, including half-hip retained-gable
and ridge limits, hosted openings and Two Cut–Plumb wall/cornice bearing. A trial
pitch is only a surface measurement; it does not authorize an invalid final roof.
Two Cut–Plumb retains the top surface and shapes the underside at the solved pitch.

The actual built cover must match the request within **0.000001 mm** in f64.
The command's rounded `ridge_mm` response is not the acceptance measurement.
GLB positions use f32 metres and therefore have export quantization: regression
checks allow `max(0.001 mm, abs(height_mm) × f32::EPSILON)` (one relative f32 ulp).
This is numerical agreement with the parametric model, not survey accuracy.

The existing limits remain: a requested rise of 500–30000 mm above the eaves,
5–75° pitch, and all roof dimension and topology guards. Unreachable height
requests report the height range at the pitch endpoints; a geometrically invalid
final roof reports its existing guard. Both fail atomically, preserving document,
journal and Undo/Redo history. No wall, footprint, source profile, cap drop,
thickness or overhang is adjusted to make a request fit.

The height solver requires finite site-local outline coordinates and eaves within
100 km of the origin, so extreme values cannot hide the requested rise through
floating-point rounding.

Native source still stores only the resulting pitch and the existing roof
parameters. The journal records the original command. No field, default, schema
shape or format migration is added, and the explicit-pitch path is unchanged.
Undo/Redo and native reopen retain the solved geometry. Roll back the code by
reverting the correction commit; existing readers can read the saved roof source.
