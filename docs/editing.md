# Precise editing

Studio, the CLI and MCP run the same five undoable commands. Every distance is in millimetres,
in the site's local frame. A failing command restores the complete document and its journal.

Targets are the ids shown in Studio. `partition.L1` selects that wall only on storey 1;
`wall:partition` selects its complete source definition on every storey. `stair:<id>` and
`room:<id>` work the same way. A facade instance uses `b1.f1.L1.B2`; a separate balcony uses
`b1.f1.L1.B2.balcony`. `placement:<facade>:<index>` addresses a particular placement in the
saved document, including one from a previous phase. Targets must be unambiguous. Select each
source definition once in a batch.

- `edit.move`: `{"targets":["partition.L1"],"delta":[0,125.5,0]}`. A selected storey splits off
  from a shared definition; its other storeys remain in place. A facade instance moves along
  its host and vertically, without moving its bay axis or the other instances on that axis.
- `edit.copy`: `{"targets":["wall:partition"],"delta":[0,1200,0],"count":3}`. Creates three
  copies, at one, two and three times the offset. Sources remain in place. Each copy has a
  distinct id. New facade instances receive distinct bay indices.
- `edit.copy_to_levels`: `{"targets":["wall:partition"],"levels":[1,2,3]}`. Keeps the source's
  plan geometry or facade position. Source storeys are skipped; repeated destination indices
  count once. Matching geometry or occupied facade slots cause an error. `"replace":true`
  explicitly copies over those matches. Replacing an older facade instance preserves it in
  previous phases and demolishes it in the active phase.
- `edit.align`: give `axis` (`x`, `y`, `u` along a facade, or `z` for absolute opening height),
  `anchor` (`start`, `center`, `end`) and either `coordinate` or `reference` (an element id).
  Alternatively give `line:[[x0,y0],[x1,y1]]` to align an edge or centre to any plan line.
  Wall edges include their thickness. U references must be on the same facade.
- `edit.delete`: `{"targets":["partition.L1"]}` removes that storey instance and preserves the
  definition on its other storeys. Deletion removes source data in every phase; use
  `facade.demolish` for demolition in a particular phase.

Successful edits return `targets`: the handles of the moved or created definitions/instances.
Use those handles for subsequent edits because splitting a definition creates a new id.
`edit.undo` restores a whole batch; `edit.redo` reapplies it.

A batch is limited to 256 targets. Arrays have at most 100 copies and 5,000 new definitions;
facades have at most 200 bay axes. An offset component is limited to 100,000 mm. Plan elements
stay on their storeys; vertical offsets apply to facade instances only. Storey copying is
within the source building. Structural facade walls, slabs, roofs, individual interior door
leaves and cellar stairs are not independent editable targets in this version.

Exact facade positions are stored as an optional `u` override on a placement. Existing
`trigon/1` files without that field retain their original bay-based geometry. Updated files
round-trip with the override; older Trigon versions do not understand its position.
