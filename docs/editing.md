# Precise editing

Studio, the CLI and MCP run the same undoable editing commands. Every distance is in millimetres,
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
- `edit.align_walls`: give interior-wall `targets` and one `reference` wall. It makes the
  selected walls' directions and centrelines collinear, rather than just translating their
  bounding boxes. Optional `reference_face` and `target_face` are `center` (default), `left`
  or `right`; left/right follow each source wall's A-to-B direction, including reversed runs.
  Walls must be within 15° of parallel. Run lengths, thicknesses and door distances from the
  start stay unchanged. Displayed occurrence IDs affect one storey; `wall:<id>` affects the
  shared source. The reference cannot also be a target on an overlapping storey. Existing
  model joins still resolve generated junctions; neighbouring wall and room sources stay
  independent. Studio's **Align → Wall line / face** captures a picked reference without
  typing coordinates, then aligns the next picked wall through this same command.
  Active Align, Copy, To levels and Delete controls appear above model parameters in the
  Properties panel, so long parameter forms cannot bury the command's fields or Apply button.
  Wall line / face mode hides model drag handles while picking, so a reference handle cannot
  intercept a nearby target. Other editing modes retain their existing handles.
- `edit.delete`: `{"targets":["partition.L1"]}` removes that storey instance and preserves the
  definition on its other storeys. Deletion removes source data in every phase; use
  `facade.demolish` for demolition in a particular phase.

Successful edits return `targets`: the handles of the moved or created definitions/instances.
Use those handles for subsequent edits because splitting a definition creates a new id.
`edit.undo` restores a whole batch; `edit.redo` reapplies it.

`interior.regularize` cleans traced walls against explicit building-grid angles. Walls with
explicit doors retain their source identities and are never merged into another host. When
a wall axis snaps or its start extends to a junction, each door centre is projected onto that
axis and its distance from the new start is updated; clear widths and heights stay unchanged.
This preserves the opening's position instead of shifting it with a changed start point.
Derived room boundaries still need an explicit refresh after cleanup.

For a native picking regression, open a drawing, capture the reference in Wall line / face mode,
and run `python3 tools/check_studio_drawing_pick.py --help`. The check needs an explicit review
session's control directory, drawing ID, drawing-plane point and expected element ID. It compares
the read-only exact drawing query with the native canvas selection and saves its evidence to the
specified output folder. It never discovers the owner's default Studio session.

A batch is limited to 256 targets. Arrays have at most 100 copies and 5,000 new definitions;
facades have at most 200 bay axes. An offset component is limited to 100,000 mm. Plan elements
stay on their storeys; vertical offsets apply to facade instances only. Storey copying is
within the source building. Structural facade walls, slabs, roofs, individual interior door
leaves and cellar stairs are not independent editable targets in this version.

Exact facade positions are stored as an optional `u` override on a placement. Existing
`trigon/1` files without that field retain their original bay-based geometry. Updated files
round-trip with the override; older Trigon versions do not understand its position.

## Groups, component types and project templates

`group.define` saves a named selection, for example
`{"id":"core","name":"Core walls","targets":["wall:partition","stair:main"]}`.
Use `group:core` as a target of any editing command. A batch expands to its source members and
is still one undo step. Defining a group from other groups flattens their members; stored
nested references are refused. Moves, split storey instances and explicit copy-over update
member handles. Deletion drops removed members and removes an empty group.

`group.list` reports each group's targets and whether its references still resolve. Other
commands, such as replacing all traced interiors, can leave stale references; redefine the
group to repair those. `group.remove` removes just the saved selection. Groups are named
collections, with independent copies; they do not create linked parametric assemblies.

Studio's Families tab browses the component types and edits their sizes and family parameters.
Duplicate a type to create a variant; update a type to change all its instances. Saving and
placing both run the existing `types.define` and `facade.place` commands, including their
geometry validation. New geometric family kinds still require parametric code in the model.

`document.save_template {"path":"seed.trigon"}` writes a complete project template, including
its types, groups, model, evidence and site, without changing the working document's path,
dirty state or undo history. `document.from_template {"path":"seed.trigon"}` starts a detached,
unsaved copy with no save path. Saving that copy requires choosing a new file. A template is a
normal `.trigon` document, so no separate format or migration is needed. These actions are in
Studio's Templates menu.
