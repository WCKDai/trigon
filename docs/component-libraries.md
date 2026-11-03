# Parametric component libraries

Library files store the same type definitions used by Studio, CLI and MCP. Placements refer to
a type ID, so editing that type with `types.define` updates all its linked instances, including
its dimensions, railing style, finish and deck choices. An explicit placement width/height/sill
override keeps that value; creating a type variant creates a separate type.

`types.export` saves a local `.trigonlib` file. Omit `ids` for all types, or select a subset:

```json
{"path":"my-balconies.trigonlib","ids":["balcony.minaltan","balcony.french"]}
```

`types.import` adds its definitions to a project:

```json
{"path":"libraries/balconies.trigonlib"}
```

The filename extension is lowercase `.trigonlib`. Studio's Import/Export forms use the same
case-sensitive extension rule as the engine; uppercase or mixed-case extensions must be renamed.

Existing IDs are rejected by default. To refresh a shared library definition and all its linked
uses, choose replacement explicitly:

```json
{"path":"my-balconies.trigonlib","replace":true}
```

The entire batch is validated against real hosted geometry before assignment. If one definition
is invalid, conflicts, or causes invalid building geometry, no types change. A successful import
is one journal entry and one Undo step. Export does not change the project or history.

The project stores imported parametric definitions locally. To refresh an externally edited
library, import it again with replacement. Reopening the project does not need the library file,
an online account, or an external model asset.

`types.define` also validates complete hosted geometry before committing a shared type edit,
including roof dormer consumers. A type that would turn a dormer window into a door, for
example, is rejected without changing the project or its Undo history.

## File contract

```json
{
  "format":"trigon/library/1",
  "types":[
    {"id":"my.guard-door","name":"French balcony","width":1200,"height":2250,"sill":0,
     "kind":{"family":"french_balcony","leaves":2,"rail_height":1000}}
  ]
}
```

Version 1 accepts 1–1024 unique definitions and at most 4 MiB. It uses existing component JSON
schemas and ranges, with no external meshes, textures, URLs or executable content. Unsupported
formats, unknown file fields and duplicate IDs are rejected. File diagnostics do not echo
arbitrary file contents. Existing project/type JSON and default types remain unchanged.

The starter pack in `libraries/balconies.trigonlib` includes two steel-balcony guard/deck
variants, a French-balcony type and an independent flat-bar guard. Sizes are editable family
defaults, not measurements of the private integration building. Its railing/deck options follow
the existing parametric vocabulary,
checked against [MinAltan's published specifications](https://minaltan.dk/altaner/stalaltaner).
The pack contains original parameters; reference photos, commercial models and client artwork
are not distributed. Current steel finishes are galvanised, white and anthracite; current deck
choices are Cumaru, fibre concrete, composite and bamboo.

The optional `guard_rail` family creates one railing attachment, with no wall cut, door or deck.
It can share a bay with a separately placed window or door. `width` is the guard's overall run,
`height` includes the optional 40 mm hardwood handrail, and `sill` is its base above the storey.
`projection` is the distance from the wall to the guard centreline, not a balcony depth. Width
is 300–6000 mm, height 900–1500 mm and projection 20–3000 mm (at least 35 mm with the handrail).
These are modelling limits; bracket geometry does not establish structural capacity or approval.
Properties and pull handles change the selected instance; replacing its shared type changes every
linked instance while preserving placement overrides. Plans and IFC classify it as a railing.
The stable `.balcony.rail` suffix is the existing attachment namespace; it does not add a slab.

Default project types and existing family forms are unchanged. `ComponentKind::GuardRail` is a
new public Rust enum variant, and `family: "guard_rail"` is an optional file-format extension.
Earlier readers can still open files without this family; feature-bearing projects/libraries need
an updated reader. Before downgrading, export a separate library subset or save a separate project
copy with these definitions and placements removed. Reverting the feature commit rolls back code.

Painted white steel and dark glazing seals have distinct `steel_white` and `gasket` export
materials, with steel and rubber IFC classifications. Their colors are unchanged. The public
Rust `Material` enum appends `SteelWhite` and `Gasket` after the existing 24 values; exhaustive
Rust matches must handle these variants. Existing material indices and source finish strings
remain unchanged. GLB includes every referenced appended material, including in projects without
site objects, while retaining the legacy material prefixes.

Window/door IFC exports associate a material constituent set with the product and give each
material group a matching named shape aspect. Framing, glazing, panels, seals and hardware remain
separate; material fractions are unset rather than invented. This follows the public IFC4
[material constituent](https://standards.buildingsmart.org/IFC/RELEASE/IFC4/ADD2_TC1/HTML/schema/ifcmaterialresource/lexical/ifcmaterialconstituent.htm),
[constituent set](https://standards.buildingsmart.org/IFC/RELEASE/IFC4/ADD2_TC1/HTML/schema/ifcmaterialresource/lexical/ifcmaterialconstituentset.htm)
and [shape aspect](https://standards.buildingsmart.org/IFC/RELEASE/IFC4/ADD2/HTML/schema/ifcrepresentationresource/lexical/ifcshapeaspect.htm) contracts.

Window sills seat on the nominal rough-opening bottom and meet the front of the frame. The
illustrative family section rises 32 mm at the back, slopes down 12 mm to its 40 mm projecting
nose, and keeps a 5 mm underside drip groove. The 30 mm ears extend only outside the wall face;
the bed stays between the nominal jambs. Composite door/window units omit ears at their jambs.
These are original presentation defaults, not surveyed construction dimensions. This seating
correction changes generated sill geometry without changing stored family parameters, opening
width/height/sill, IDs, phases, or scheduled opening quantities. Reverting the correction restores
the earlier geometry without a document migration.

Regression tests cover exact type round trips, read-only exports, shared dimension/finish changes,
preserved instance overrides, batch/host failures, file limits and exact Undo. Revert the library
feature commit to roll back the commands; imported ordinary type definitions remain compatible
with earlier project readers.

## Direct placement on a facade

`facade.place_at` places one component at an exact facade-local coordinate in mm. It works
on a newly authored wall with an empty bay list; there is no shared-grid setup step. Studio
and AI callers use this same typed command, directly or as a `precision.preview` action.

```json
{"facade":"b.f1","level":0,"u_mm":3000.125,"type":"window.fixed",
 "width":1234.5,"height":1450.25,"sill":800.125}
```

`facade`, `level`, `u_mm` and `type` are required. `level` is an existing, zero-based storey
index, including basement indices when present. No storey is inferred or created. `u_mm`
is measured from the facade's source start in its local direction, with no rounding or
snapping. Optional `width`, `height` and `sill` use mm and the existing `facade.place`
conventions; omission uses the type. `spans_storeys` defaults to false and allows an opening
to cross the floor above only within existing host/eaves limits. `phase` defaults to the
active phase. Unknown parameters are rejected.

The command reuses a bay only when exactly one centre is within an absolute `0.000001 mm`
of `u_mm`, the existing tight local-coordinate numerical convention. The bay centre stays
exactly unchanged; the requested coordinate is stored in the placement's existing `u`
override. Multiple matching centres are ambiguous and refused. A distinct coordinate is
appended to the end of `bays`, even when smaller than existing centres. Bays are never sorted,
rebuilt or remapped by this command. At most 200 bays are supported; an existing bay may still
be reused at that limit. This equality tolerance is not a snap radius or a claim about source
measurement accuracy.

A slot is facade × storey × bay × class (opening or attachment). The default `replace:false`
refuses a same-class occupant alive in the chosen phase, including an instance moved away
from its bay centre. The other class may share the slot if the existing geometry rules allow
it. The command does not select another bay to evade an occupied slot.

`replace:true` explicitly replaces one same-class occupant alive in the chosen phase. An
occupant created in that phase is replaced at its existing placement-array index. An older
occupant is marked demolished in that phase and the replacement is appended. This uses the
existing `facade.place` lifetime semantics. All other placement values and indices are
preserved. Multiple live same-class occupants, any future same-class reservation in that
slot, or replacement of an occupant with a scheduled demolition are refused, even with
replacement enabled. A new placement lasts from its creation
phase onwards; future reservations are not truncated, deleted or silently rewritten.

Every successful call is one journal entry and one Undo step, including bay creation. Every
failed call leaves the complete document, journal, dirty flag, Undo and retained Redo intact,
including removal of any provisional bay. Existing host and geometry validation rejects
party walls, absent storeys, invalid dimensions, openings outside the wall, insufficient
headroom and overlapping same-class geometry whose lifetimes intersect. Invalid source
coordinates and ambiguous host/type/phase IDs are also refused. The command builds the final
candidate and returns actual generated product IDs; no mesh or geometry algorithm is added.

For the example above on an empty host, the result is:

```json
{"facade":"b.f1","placed":1,"type":"window.fixed","phase":"existing","demolished":0,
 "level":0,"bay":0,"u_mm":3000.125,"bay_u_mm":3000.125,"bay_appended":true,
 "placement_index":0,"placement_target":"placement:b.f1:0","targets":["b.f1.L0.B0"]}
```

`targets` contains the actual built product IDs, suitable for `element.get` / `element.set`
and Properties handles in the returned `phase`. Assemblies may return several product IDs
for their fill, slab and rail, all belonging to the one placed component. An independent
attachment returns its real `.balcony.slab` / `.balcony.rail` IDs, not a nonexistent parent
product. `placement_target` selects the exact source for `edit.*` operations;
`placement_index` is its current index in `facade.placements`. These locators remain subject
to the existing identity rules of later commands, such as explicit deletion or legacy grid
editing. `bay_u_mm` reports the retained bay centre, while `u_mm` reports the exact instance
coordinate. Neither is rounded.

For read-only feedback, preview the same action:

```json
{"action":{"command":"facade.place_at","params":{
 "facade":"b.f1","level":0,"u_mm":3000.125,"type":"window.fixed"}},
 "sanity":{"levels":[0],"max_results":100}}
```

Call `precision.apply` with the returned `action` and `preview_token`. The candidate is
validated on a separate document; previews preserve history and retained Redo. Changed source
fields and optional sanity findings retain the existing bounded preview contracts. Stale
document/action tokens are refused. Apply journals the underlying `facade.place_at` action,
so replay requires no ephemeral preview token. Model validation does not establish survey or
source accuracy; unavailable drawing/audit checks remain explicitly unverified.

This is an additive command and precision-action variant. Existing `facade.place` defaults,
replacement behavior, sorting and output, and `facade.set_bays` replacement/pruning behavior
remain unchanged. No document schema, format version or migration is introduced: only existing
bay, placement and `u` fields are persisted. Files without this command remain unchanged;
older engines that already understand occurrence `u` can read its saved geometry, but cannot
replay a journal containing `facade.place_at`. Keep the newer engine for journal replay or
save a separate geometry snapshot before downgrading. Reverting the command implementation
rolls back authoring support; Undo rolls back an individual placement and its appended bay.
