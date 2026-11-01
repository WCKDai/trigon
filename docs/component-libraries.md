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

The original starter pack in `libraries/balconies.trigonlib` includes two steel-balcony guard/deck
variants and a French-balcony type. Sizes are editable family defaults, not measurements of the
private integration building. Its railing/deck options follow the existing parametric vocabulary,
checked against [MinAltan's published specifications](https://minaltan.dk/altaner/stalaltaner).
The pack contains original parameters; reference photos, commercial models and client artwork
are not distributed. Current steel finishes are galvanised, white and anthracite; current deck
choices are Cumaru, fibre concrete, composite and bamboo.

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

Regression tests cover exact type round trips, read-only exports, shared dimension/finish changes,
preserved instance overrides, batch/host failures, file limits and exact Undo. Revert the library
feature commit to roll back the commands; imported ordinary type definitions remain compatible
with earlier project readers.
