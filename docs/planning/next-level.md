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
| IFC for Revit and Vectorworks (wall standard cases with layer sets and axes, typed doors/windows, slabs, roof, stairs, spaces, balconies, layers, colours) | `crates/io`, branch `next/ifc` | in progress |
| Drawings v2: sections, elevations as line drawings, one drawing scene, DXF | `crates/engine`, branch `next/drawings` | in progress |
| Trigon Studio: desktop app, control channel, `trigon mcp` attaching to it, activity feed | `apps/trigon-studio`, branch `next/studio` | in progress |
| CI on the trusted runner (alpha) | `.github/workflows/ci.yml` | done |

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
   references for both.
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

- **More CI capacity.** `alpha` is one trusted runner shared by PolyCore, Polykrom and Trigon;
  Trigon's runs queue behind theirs (observed 2026-10-31: three Trigon runs superseded while
  waiting). A second trusted Linux runner, or two more runner instances on alpha with their own
  labels, removes that wait. A macOS and a Windows runner are needed to build and test Studio on
  the platforms it ships on.
- **Shell access to alpha** (an SSH host for the agent) would let heavy release builds and
  Blender renders run there instead of on the laptop.
- **A GPU render node** (Blender, Eevee/Cycles) for presentation renders; renders take minutes per
  view on the laptop.
- **Parallel agents per workstream** in their own worktrees, as now: one each for IFC, drawings,
  Studio, model fidelity (facade details, chimneys, stair railings), and a reviewer. Agents run on
  Opus, and they burst tokens.
- **The owner's import check** of `examples/sample-building.ifc` in Revit and Vectorworks; agents
  may not drive commercial CAD programs.
