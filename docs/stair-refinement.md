# Refining a named stair source

`stair.refine_source` edits an entire existing stair definition or partitions its
departure storeys into explicitly named native `Stair` definitions. CLI, MCP and
precision preview/apply and Studio's staircase controls use the same engine
implementation.

## In Studio

Select a named staircase in the canvas or Project Browser. Properties offers
**This storey**, **All storeys of this staircase**, or **Selected storeys**.
The scope refers to departure flights; their receiving floor openings are
validated together. Width/run handles and the dimension, landing, well and
turning-step controls stage a draft without changing the saved model.

Review the draft, then use **Apply change** or **Cancel preview**. The review
panel and canvas outlines show the exact captured action, including an AI
preview received while a different human draft is open. Cancel returns to that
local draft; Apply consumes the captured parameters. Invalid input retains the
previous draft and its error. External source or storey changes require a reload.

Plans show changed departure/arrival outlines only on their affected levels.
3D outlines use physical floor elevations, including a legacy implicit attic.
Plan outlines are hidden in sections and elevations. A handle edit opens
Properties even from Focus mode. Undo restores the whole source partition and
its references.

These controls refine existing native stair kinds. They do not introduce
independent intermediate landing elevations or establish surveyed stair
continuity where the source evidence is incomplete.

## Request

```json
{
  "building": "public",
  "expected_source": {
    "id": "public.core",
    "levels": [0, 1, 2],
    "outline": [[2000,2000],[4500,2000],[4500,4200],[2000,4200]],
    "tread": 276,
    "kind": "u_turn",
    "landing": 1000,
    "well": 150,
    "rounded": false,
    "winders": null,
    "turn_depths": null,
    "turn_chamfer": null,
    "boundary": null
  },
  "partitions": [
    {
      "id": "public.core",
      "levels": [0],
      "outline": [[2000,2000],[4500,2000],[4500,4200],[2000,4200]],
      "tread": 276, "kind": "u_turn", "landing": 1000, "well": 150,
      "rounded": false, "winders": null, "turn_depths": null,
      "turn_chamfer": null, "boundary": null
    },
    {
      "id": "public.core.upper",
      "levels": [1, 2],
      "outline": [[2000,2000],[4500,2000],[4500,4200],[2000,4200]],
      "tread": 276, "kind": "u_turn", "landing": 900.125, "well": 150,
      "rounded": false, "winders": null, "turn_depths": null,
      "turn_chamfer": null, "boundary": null
    }
  ]
}
```

This is a synthetic API example, not a surveyed or approved building solution.
Coordinates and dimensions are site-local millimetres; levels are departure
storey indices. Source values come from the current document's
`buildings[...].stairs[...]`, not rounded displayed measurements. Callers with a
picked occurrence must resolve its exact building/source first.

`expected_source` and every partition use the existing `trigon_model::Stair`
type, including all current winding, turn and fitted-boundary controls. This is
a complete replacement, not a sparse patch: omitted fields use native serde
defaults, never values inherited from `expected_source`. Explicit `null` clears
an optional control. Native frame, fitted-boundary and numeric validation apply.
Unknown fields are refused.

The building and original source must each be unique. The expected source is
compared by native value equality, with no numeric tolerance; source level order
also matters. Anonymous sources cannot be refined. Every partition needs
nonempty, duplicate-free departure levels. Their sets must be disjoint and their
union must equal the original levels exactly. No departures are added or deleted.

Exactly one partition retains the original ID and array index. All other IDs are
explicit. New definitions append in request order, irrespective of the retained
partition's position in the request. Unrelated definitions and their array order
remain exact. Equal geometry never identifies or replaces another source.

The command reserves source IDs, typed selectors and displayed occurrences,
including room aliases, anonymous fallback IDs, numeric suffix aliases,
historical facade placements and derived non-stair displayed IDs. It refuses
collisions within the request or with existing namespaces and refuses new IDs
already present in stored entrance/group references.

## Consumer and identity mapping

Stored entrance defaults remain exact. For each original departure, an effective
assignment to the original source follows the partition owning that departure.
The command inserts or replaces only the required `stair_by_level` slots.
Unrelated overrides, explicit redundant overrides on retained departures,
addresses, points and unit records remain exact. If the original ID no longer
departs at ground level, an explicit ground override preserves the effective
binding; the unchanged stored default can independently be reported as invalid
by `entrance.get`. Cross-building references and explicit original-source
overrides outside its original departures refuse with entrance IDs.

A group's whole-source selector (`stair:<id>` or an existing bare source ID)
expands in place to all partition source selectors in request order. An
occurrence selector maps to the single new occurrence at that departure.
Unrelated targets keep their order and values. Overlapping source/occurrence
selections, ambiguous group IDs, unsupported related references and excessive
expanded target counts refuse rather than being sorted, deduplicated or dropped.
Nested group references are unsupported and refuse with the group and target IDs.
Annotations store drawing coordinates, not stair references, and remain exact.

Splitting changes displayed and exchange identities on moved departures. For the
example, `public.core.L1` becomes `public.core.upper.L1`. It does **not** persist
under its old occurrence ID. Undo restores the exact previous sources and IDs.

## Validation and feedback

Before assigning a document, the command maps consumers, validates annotations,
builds the full candidate and validates each replacement shaft on every existing
departure and arrival floor. It uses native `stair::opening` and
`Building::storey_usable_outline`, including storey-specific facade thicknesses
and the attic boundary. It never tests containment against the slab polygon
after cutting stair holes: that would conceal wall intrusions or reject valid
flush contact. Concave edges are checked between boundary crossings, not merely
at the shaft corners. The floor boundary is also tested for intrusion into the
shaft, including intervals between its intersections; a near-contact endpoint
cannot excuse a deeper concave crossing.

Boundary contact has a `1e-6 mm` geometric tolerance. The new command normalizes
intersection contact by edge length so rotation/translation does not reject exact
wall contact. The existing footprint command retains its original predicate;
its tests and behavior are unchanged. Neither path changes or snaps source
coordinates. This tolerance is numerical, not a survey allowance.

The response includes:

| Field | Meaning |
|---|---|
| `original` | Exact original source ID, index and level sequence |
| `partitions` | Request-order IDs, actual indices and exact level sequences |
| `displayed_id_mapping` | Old/new occurrence IDs for each original departure |
| `departure_levels`, `arrival_levels`, `affected_levels` | Sorted affected existing storey indices |
| `affected_floors` | Checked source/floor IDs, level, departure/arrival roles; includes attic floors |
| `references` | Exact before/after group targets and changed entrance slots |
| `validation` | Full model, annotations and compulsory changed-shaft checks |
| `adjacent_feedback.before`, `.after` | Existing `precision.audit` on affected storeys, with its unchanged thresholds |

A legacy attic slab can share the top departure storey index; use its floor ID
and arrival flag to distinguish it. No nonexistent arrival storey is invented.

The audit includes other sources on those storeys: pre-existing overlap remains
visible, as does an exposed arrival beside a changed departure. It reports areas,
IDs, levels, counts, truncation and scope. Its bounded failure returns
`status: "unverified"` plus the error. It is advisory and cannot bypass compulsory
shaft containment or candidate validity. A broader optional `precision.preview`
`sanity` audit is independent. Geometric fit is not survey, accessibility,
structural or all-source clearance approval.

## Preview, apply, refusal and bounds

Wrap the request in the existing action envelope:

```json
{"action":{"command":"stair.refine_source","params":{"building":"public","expected_source":"<complete Stair>","partitions":["<complete Stair>"]}}}
```

The placeholders above denote objects, not literal strings. Send this envelope
to `precision.preview`, then send the same `action` and returned `preview_token`
to `precision.apply`. MCP names are `stair_refine_source`, `precision_preview`
and `precision_apply`. Direct command execution performs the same validation.
Preview changes no document/history. Apply/direct journal exactly one
`stair.refine_source` entry. A no-op refuses, preserving Redo.

Representative fail messages identify the relevant source, entrance, group or
floor: `stale expected_source`, `duplicate departure level`, `overlapping
departure level`, `partition departure union must equal original levels`,
`ambiguous source/instance ID`, `unsupported stair occurrence reference`, and
`outside b.slab.L1`. Treat messages as diagnostics, not a machine error-code
enum. Any refusal preserves the document, Undo, Redo, dirty flag and path.

Bounds follow current authoring/edit/audit conventions: 1–60 partitions,
1–60 levels per source, at most 60 storeys in the edited building, 1000 resulting
stair sources, 100 buildings, 200 groups, 256 targets per group, 512 bytes per
target, 5000 entrances, and 60 overrides per entrance. Source/building IDs are
1–256 ASCII letters, digits, dots, underscores or dashes. The namespace walk is
capped at 100000 identities. Existing native geometry and audit work bounds
remain in force. Exceeding a bound refuses; it does not truncate edits.

## Persistence and rollback

There is no format, native-field, writer, old-command schema or default change.
Native `Stair` values and existing entrance/group fields carry the result.
An older compatible reader can open the generated native file. Replaying its
new journal entry on an older binary legitimately refuses the unknown additive
command; native-file compatibility is distinct from command-replay support.

`edit.undo` restores the complete prior document and geometry; `edit.redo`
restores the result. A saved result reopens without migration. Roll back the
implementation by reverting the relevant engine and Studio commits. Studio
submits this same request, retaining exact source values instead of inferring
shared scope from geometric similarity.
