# Trigon: the next level (plan, 2026-10-31)

The owner's asks, in their words: real model files for Vectorworks and Revit; a much higher
fidelity 3D model; much clearer, correct 2D drawings (plans, facades, sections, cut views); walls
that join each other and are built the way Copenhagen apartment buildings are (700, 600, 480,
360 mm through the storeys); possibly the PolyCore kernel; an interface to co-build with the agent
and see it work; CI and compiling on the alpha runner.

## Workstreams

| Workstream | Where | State |
|---|---|---|
| Masonry walls by storey, wall joins, doors, roof build-up, plinth, 3D plan cuts | `crates/model` (`walls`, `joins`, `doors`, `roof`), branch `next/bim-core` | done, tested |
| IFC for Revit and Vectorworks (wall standard cases with layer sets and axes, typed doors/windows, slabs, roof, stairs, spaces, balconies, layers, colours) | `crates/io`, [export contract](../ifc-export.md) | implemented; commercial application import checks remain open |
| Drawings: shared plan/section/elevation scene, DXF, saved views, persistent annotations and snapping | `crates/engine`, `crates/model/src/annotation.rs`, [drawing workflow](../annotations.md) | implemented; associative annotations remain open |
| Trigon Studio: branded native workspace, families, groups, templates, precise edits and drawing authoring | `apps/trigon-studio`, [editing contract](../editing.md) | semantic 2D picking integrated and covered by worker/edit/undo tests; native OS click verification remains open |
| Window/door schedules | `crates/engine`, Studio, [schedule contract](../schedules.md), [#19](https://github.com/WCKDai/trigon/issues/19) | occurrence query, CSV/JSON export and live Studio table locally verified, including selection, filters, copy/Undo and 3D focus; physical OS input and large-model performance remain open |
| CI on the trusted runner (alpha) | `.github/workflows/ci.yml` | workflow configured; Linux execution is blocked by the runner group's workflow allowlist pending owner approval |

## Product acceptance workflows

The benchmark is a complete free BIM workflow alongside Revit, Archicad and Vectorworks. Each
increment must leave a reviewable project and drawings that can be reproduced through commands.
Core authoring and exchange must work with local files without a paid account or mandatory AI
service; agent assistance uses the same undoable tools as manual editing.

1. **Reconstruct an existing building.** Register a plan or facade image, establish measured scale,
   trace and correct walls, floors and openings, and retain the evidence behind uncertain values.
2. **Edit the building precisely.** Pick the same element from plan, elevation, section or 3D;
   move, align, copy and repeat it through typed distances or deliberate snaps; undo exactly.
3. **Build reusable families.** Separate opening, frame, sash, panel and material parameters;
   create type variants and propagate changes to their instances. The Idealcombi inventory is a
   reference for requirements, with manufacturer files kept outside the repository.
4. **Produce coordinated documentation.** Derive plans, sections, elevations, schedules and sheets
   from the model, preserving identity through edits. Saved annotated views exist; composed
   sheets, associative dimensions and general schedules need their own acceptance tests.
5. **Exchange and verify.** Export semantic IFC and dimensionally correct vector drawings. Check
   geometry and properties with independent readers and representative commercial application
   imports, preserving explicit failures and unsupported cases.

These are acceptance targets, not a claim that the full workflow is implemented. The saved
drawing milestone covers annotation authoring and snapping. Direct 2D model selection and IFC
window/door quantities are implemented in separate follow-up increments ([#17](https://github.com/WCKDai/trigon/pull/17),
[#18](https://github.com/WCKDai/trigon/pull/18)), with local verification and an offline workflow
fixture. Those branches remain under review; native interaction and representative commercial
import checks remain open. The next increment connects those quantities to local schedules.

Open-source libraries, ML weights, datasets and component assets each need a source, an exact
version and a checked redistribution licence. Reuse follows the clean-room rules in `AGENTS.md`;
unknown licensing stays outside the dependency and asset closure. Validation programs run as
separate tools when their licences do not fit the application.

## Decisions

**Walls are exact 2.5D, not 3D booleans.** A facade wall is one closed prism per storey between
the outer wall line and the inner line at that storey's thickness. Openings are cut out of each
face with 2D polygon booleans (i_overlay, MIT/Apache), so an opening across a floor cuts both
storeys and nothing depends on a 3D boolean. Interior walls are joined per storey in plan:
butt joints on the near face, the thicker wall running through at corners, door gaps left alone.
This covers ordinary buildings exactly and quickly. It is also the "polyhedral fast path" that
`polycore-requirements.md` §2.3 proposes for PolyCore.

**PolyCore: not for walls now; yes for sections, projection and later components.** Building
geometry is planar and prismatic; Trigon's own 2.5D construction is exact for it and needs no
B-rep boolean. PolyCore's booleans are still in Phase 2 (truck as fallback, robustness and hang
gates open), and taking it as a dependency today would add the whole vendored truck build and
break its "one consumer" contract. The useful path:

1. Upstream to PolyCore (through its RFC process): `section`/`project` in the `Kernel` trait
   (requirements §2.2), and a planar polyhedral backend with exact predicates (§2.3). Trigon's
   `walls`/`joins` code and the drawings agent's plane-cut and hidden-line code are working
   references for both. Track these requirements in PolyCore GitHub issues and link consumer
   fixtures; a proposal in this repository does not count as an upstream report. The shared
   section/projection requirement is filed as [PolyCore #45](https://github.com/WCKDai/polycore/issues/45).
2. Make Trigon a second consumer when PolyCore's Phase 2 gates are green: B-rep components
   (window profiles, curved stair stringers, vaults) and STEP export.
3. Until then Trigon keeps its geometry in `crates/model` and exchanges with other programs
   through IFC4, DXF and glTF.

**Exchange.** IFC4 (ADD2 TC1) is how the model reaches Revit and Vectorworks. Walls are
`IfcWallStandardCase` with axis, body and material layer set usage, so both programs can treat
them as walls. Doors and windows fill their openings. DXF (R2000, real millimetres, named layers
with lineweights) carries the 2D sheets. DWG comes later, clean-room from the public ODA
specification. IfcOpenShell and ezdxf only run as separate programs to check the output.

**Co-building.** Studio and Polykrom share one design: a token-protected JSON-lines control
channel on 127.0.0.1, a discovery file, and `trigon mcp` attaching to the running app. Every
agent command runs in the session the owner is looking at, appears in the activity feed and
is undoable.

## What more compute and agents would buy

- **CI access and capacity.** `alpha` is online in a trusted runner group shared by PolyCore,
  Polykrom and Trigon. Its restricted workflow allowlist currently omits Trigon, so adding
  runner capacity alone will not unblock these runs. A proposal to allow the exact reviewed
  workflow revisions is prepared, pending owner approval. After access is resolved, a second
  runner can reduce contention. macOS and Windows runners are needed for platform verification.
- **Shell access to alpha** (an SSH host for the agent) would let heavy release builds and
  Blender renders run there instead of on the laptop.
- **A GPU render node** (Blender, Eevee/Cycles) for presentation renders; renders take minutes per
  view on the laptop.
- **Parallel agents per independent workstream** in isolated worktrees, with explicit file ownership
  and exclusive build-cache leases. Semantic picking and IFC quantities have been integrated
  sequentially; schedules are the current follow-up. Add reviewers when a concrete diff is ready.
- **Representative import checks** of `examples/sample-building.ifc` in Revit, Archicad and
  Vectorworks, with the application version, element categories, quantities and geometry recorded.
