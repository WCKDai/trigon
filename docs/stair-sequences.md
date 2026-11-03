# Standalone stair sequence geometry

`trigon_model::stair::sequence` is an additive geometry foundation for Trigon #27.
It is **not an activated native stair source**. `Document`, `Building`, `Stair`,
`FORMAT`, commands, Studio, drawings and exporters retain their existing behavior.
Do not insert these parameters into a saved native document: the current stair
reader deliberately refuses `kind: "explicit_sequence_v1"`.

`SequenceV1::from_json(bytes)` reads the strict proposed sequence shape, and
`resolve_geometry(&sequence, &physical_floors)` returns its authored route geometry.
The public Rust structs can also be constructed directly; the resolver repeats
all cheap scalar, size and index checks before geometry. No dependency is added.

```rust
use trigon_model::stair::sequence::{PhysicalFloor, FloorRef, SequenceV1, resolve_geometry};
# fn example(bytes: &[u8]) -> trigon_model::Result<()> {
let sequence = SequenceV1::from_json(bytes)?;
let floors = [PhysicalFloor {
    floor: FloorRef::Storey { level: 0 },
    top_z_mm: -1000.0,
    thickness_mm: 220.0,
}];
let geometry = resolve_geometry(&sequence, &floors)?;
// Inspect geometry.flights, geometry.landings and geometry.proposed_openings.
// Success establishes component geometry only.
# Ok(())
# }
```

## Datum and material semantics

All XY and Z values are site-local millimetres, Z up. Callers must supply the actual
physical slab top and thickness for each referenced storey or attic. The API takes
no storey height. It resolves each landing independently as
`physical_floor.top_z_mm + landing.elevation.offset_mm`. A floor datum is not proof
that the route reaches or clears that floor. An attic reference requires a real
attic slab supplied by the caller; the resolver does not infer one from a roof.

The ordered path has one more landing than flights. Flight `i` connects landing
`i` to landing `i+1`, using every declared port once. Every elevation strictly
increases. Access levels are unique and must match the landing's storey datum.
Results retain the room-floor top, offset and `requires_source_review` flag for
nonzero offsets; they do not invent a bridge or claim flush room connectivity.

For `m` explicit tread IDs/cells, there are `m+1` risers. With landing difference
`delta`, the riser height is `h = delta / (m+1)`. Tread `j` (one-based) has top
`from_z + j*h` and bottom `top - h - soffit_mm`. The last riser reaches the arrival
landing; there is no extra arrival tread. Arrival slab/datum thickness must be at
least `h+1 mm`, providing a nonzero *nominal component interface*. This does not
certify structural support or retained floor material.

An owned landing slab is an exact polygon prism from `z-thickness` to `z`.
A floor-owned landing requires the same physical floor reference and exactly zero
offset. It has a stable landing ID and `FloorOwnedSupportUnverified` role, **no mesh
and no owned volume**. Its authored outline is retained in the result for later support checks.
A first tread can extend below its departure datum by its soffit; floor-owned
support-interface classification, allowed embedding and actual overlap quantities
remain unverified. No general clash waiver follows from geometry resolution.

Each `SolidPart` carries its role, stable ID, exact source outline, top and bottom,
indexed outward closed mesh and gross volume. Flights hold tread solids and an
aggregate quantity, not a second aggregate mesh. Sum either the component volumes
or each flight aggregate plus owned landing slabs, never both. These are gross
component quantities, not a union or net whole-building material volume.

## Geometry contract

Rings are simple CCW polygons without repeated closing vertices, crossings,
backtracking or edges shorter than 0.001 mm; area is at least 1 mm². Concave rings
and collinear boundary corners are supported. The triangulator retains every
corner, including collinear vertices, without boundary T-junctions.

Ports are 600–4000 mm segments of one landing boundary edge and use ascending
left/right endpoint order. Departure ports run opposite the CCW landing boundary;
arrival ports follow it. Port subsegments are allowed; overlapping ports are not.
Derived gate widths use the same 1e-6 mm geometric boundary tolerance, including
after rigid placement; this does not move endpoints or relax shared scalar limits.
Landing walks connect port midpoints through `walk_via`, checking every boundary
interval so a concave notch cannot be skipped.

Straight profiles split the exact rectangular run between equally directed,
parallel gates into equal cells; the run must be perpendicular and positive.
Tread-chain profiles retain authored polygons, including corners outside any
hypothetical straight frame. Entry indices must follow a CCW polygon edge, and
exit indices must reverse one. Full-width gates match in source order. Each
midpoint walking segment stays in its cell. Same-flight cells have disjoint
interiors; adjacent cells touch only at their declared gate. Nonadjacent cells
may share only a genuine inner fan pivot: the same zero-length side throughout
all intervening cells. Other shared points or edges refuse.

Owned solids are checked for positive-volume overlap. Different flights may
occupy the same XY area at disjoint heights. This check considers sequence-owned
parts only, not building slabs, walls, rails, headroom or unrelated sources.
The geometric boundary tolerance is **1e-6 mm**; source coordinates are never
snapped, reordered, convexified or clipped to pass a check.

All non-null flight guards and nonempty landing guard collections explicitly
refuse as unsupported. Required `left` and `right` nulls are explicit absence,
not safety approval. Results carry `Assessment::NotAssessed` for guard safety,
retained-floor support and building clearance; there is no all-clear flag.

## Parsing, limits and identity

Every required field is explicit, including nullable `access_level`, `left` and
`right`. Unknown fields and duplicate keys refuse at every nesting level. Tagged
alternatives deserialize directly into typed fields, without a JSON `Value` or
internally tagged content buffer. Array limits are checked while deserializing,
before reading the first excess element. `from_json` additionally caps the entire
byte slice at 2 MiB. Generic serde callers must bound their own input bytes,
including long strings/whitespace; this is not a native file/transport memory bound.

Limits follow the proposed v1 contract: 32 flights, 33 landings, 80 treads per
flight, 1024 per sequence, 16 vertices per tread, 64 per landing/opening, 20,000
vertices total, 64 opening rings and four per floor. Ports, walk-via points and
landing guards are bounded to 2, 8 and 16 respectively. XY/resolved Z are within
±1,000,000 mm, offsets within ±20,000 mm; flight rise is 40–20,000 mm, each riser
20–400 mm, straight run 10–20,000 mm and total rise at most 100,000 mm. Landing
thickness is 50–1000 mm; soffit is 20–500 mm. Caller physical datums are limited
to 61 distinct references, storey indices 0–59, and thickness 1–2000 mm.

Source IDs are 1–256 ASCII bytes (`A-Z a-z 0-9 _ - .`); local IDs are 1–64 bytes
without dots. Names are 1–200 UTF-8 bytes without control characters. Computation
has a 2,000,000-unit polygon/segment work budget and a 250,000-triangle output cap.
Budget exhaustion is an error with an explicitly unverified result; no partial
geometry is returned. These limits are computational, not regulatory rules.

IDs are `<source>.flight.<flight>`, `<source>.flight.<flight>.tread.<tread>`,
`<source>.landing.<landing>` and `<source>.opening.<opening>`. IDs are unique within
their declared category, with tread IDs scoped to a flight and ports to a landing.
Array order remains authored order. `ResolvedSequence::identity_delta(previous)`
requires the same source ID and reports added/removed IDs plus explicit old/new
tread and riser counts. It does not migrate references. A zero delta means identities
and counts stayed fixed, not that geometry stayed fixed.

## Public verification and provenance

The two public synthetic fixture JSON files under `crates/model/src/stair/sequence/`
are copied from the reviewed #27 design packet's `public-oracles.json`; they contain
no private fitting or survey values. The packet is a proposal, not proof of runtime
implementation. Their arithmetic is independently tested against actual generated
mesh bodies:

- Landing Z `[-2100,-120,600,1800,3000]`, riser heights `[180,180,200,200]`, counts
  `[11,4,6,6]`, gross owned material **3.012 m³**.
- Raising only the rear landing from 600 to 660 gives adjacent heights 195/190 mm
  and **3.0105 m³**; other flights and other landing solids stay exactly equal.
- The rectangular fan retains `[1000,1000]` and its 500,000 mm² centre cell,
  totaling **0.730 m³**.

Tests independently integrate oriented triangle signed volumes, audit paired
half-edges, nondegenerate faces, outward side/cap normals and actual top/bottom Z.
They cover rigid transforms, concavity/collinearity, malformed references, hostile
numbers/indices, input/work caps, strict nested keys and the actual unchanged
legacy reader's refusal. Volume comparisons allow 0.02 mm³ numerical integration
error; this does not relax the 1e-6 mm geometric boundary tolerance.
Optional `TRIGON_SEQUENCE_ARTIFACT_DIR` on the public fixture tests writes genuine
resolved mesh evidence for separate review; it does not open Studio or activate
native source support.

## Mandatory later integration

The following remain separate stages before public native authoring can be enabled:

- Real floor subtraction, concave material containment, slab crossings and
  retained floor-owned landing support; proposed openings here are only validated
  simple 2D rings with existing floor datums. Same-floor overlaps refuse, and
  touching rings explicitly refuse pending a verified floor-union path.
- Building namespace reservation, physical part adoption and complete reference
  migration across groups, entrances and legacy occurrences.
- Native source dispatch, atomic/journaled commands, Undo/Redo and saved-file
  compatibility, including all read/write consumers before activation.
- Studio route editing, picking, plans, sections, drawings, audits and IFC/GLB
  exchange from these same resolved physical parts.
- Guard generation, clearance/headroom, safety/structure and actual source QA.

No security, release, Windows, Revit, whole-building or private C5/32B acceptance
is established. The #27 engineering-kit/Plane exception remains UNVERIFIED and
six baseline OSV findings remain unaccepted. Rollback is one additive commit
revert; no saved source migration or private model change is involved.
