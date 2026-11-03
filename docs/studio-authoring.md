# Visual authoring in Trigon Studio

Studio uses direct gestures and parameter controls. The command palette remains an optional
advanced tool. The same validated engine operations support the visual tools and MCP clients.
No modelling command is submitted by moving the pointer, changing a draft field or repainting.

## Keep the canvas clear

The ribbon starts minimised. **Ribbon** expands the grouped tools. **Browser** and
**Properties** open one resizable, full-height supporting panel. The panel can be closed without
leaving the active sketch. **Focus** or **F11** hides supporting panels and the ribbon while
retaining view navigation. **View settings** holds level, phase, visibility and display controls.

With nothing selected, Properties describes the active view. Selecting an element exposes its
editable parameters and existing drag handles; **Edit type** opens the referenced shared family.
Type edits and instance overrides have different scopes, shown explicitly in the editor.

## Walls and exterior outlines

Choose **Architecture → Wall**. In an empty project, click the exterior corners on the gridded
construction plane, then close the outline or use **Finish/Enter**. The result is a real building
with the explicitly displayed wall thickness and storeys. The sketch is transient until it is
finished. Middle-drag pans the empty plane and scrolling changes its scale.

In an existing building, choose the building and storey, then click wall start and end points.
The second click creates a wall. Chain mode continues from the accepted endpoint. Ortho axes,
exact length, snapping and thickness are available in Properties; the canvas displays temporary
length feedback. Right-click or Escape ends the current chain; repeat Escape exits an empty tool.

The current interior-wall model uses centre lines and full-storey height. Arbitrary wall heights,
base/top offsets and permanent alignment locks are not implemented by this tool. Exterior
outlines use the engine's exterior-face footprint convention. All coordinates remain site-local
millimetres, including translated or rotated buildings.

## Roofs

Choose **Architecture → Roof**, pick an existing building footprint, then choose a gable,
hip or flat form and its supported parameters in Properties. Gables use explicitly selected
facades. **Create / Update roof** or **Finish** applies the validated parametric roof.

The tool uses the building's existing footprint. It does not claim arbitrary detached roof
boundaries or roof-by-face authoring. Replacing existing roof details requires the explicit
replacement option. A rejected roof leaves the document intact and keeps the draft/error visible.
An intervening API roof edit marks the local form stale. **Reload current roof** explicitly
discards the old local roof values before applying another change. Storey-schedule or phase
changes discard an unfinished wall chain.

## Annotations

Open a saved drawing, or choose **Annotate → Annotate this plan** to create/open a saved plan.
Choose the annotation tool and click in the canvas. The floating editor shows the next step,
fonts, paper text size and applicable graphic options.

- Text: click the insertion point, type, then use Enter or Finish.
- Aligned dimension: pick both witnesses and the signed offset; the completed geometric gesture
  places the dimension. Witness offset is model millimetres; lettering remains paper millimetres.
- Leader: pick the reference, elbow and label position; enter the label and finish.
- Graphic line: pick both endpoints; the completed gesture places it.
- Graphic fill: pick at least three boundary points, then finish the region.
- Level annotations are available in compatible facade/section views, using an explicit datum.

Escape cancels an unfinished placement. Repeat/chain options are explicit. Failed commands keep
the draft for correction; pending commands cannot be submitted twice. Graphic lines/fills do not
establish semantic rooms or apartment boundaries. Generated entrance labels retain their
source-backed door designations; unknown TH/TV information is not invented.
Changing a saved drawing's coordinate frame invalidates an unfinished annotation. Paper styling
and section-depth changes retain the frame. The Commands window owns its keyboard input while
open; its Enter key does not submit a canvas annotation.

## Families and instances

**Components** opens searchable categories and saved types. Choose a type to edit dimensions
and typed family parameters, duplicate it into an independent unsaved type, or open placement.
Apply/Cancel is explicit. Shared edits affect referencing uses; the editor reports source use
counts rather than claiming a freshly built instance count. Stale shared drafts and conflicting
copy IDs are rejected.

**Place component** starts in **Point** mode. Choose a saved family/type, open an explicit
saved plan or choose the host's storey in Properties, then click a real facade in plan,
elevation or 3D. A bare-wall click can capture the host and point before a storey is selected;
it remains unplaceable until that explicit choice. New walls need no bay-grid setup.

The blue hover envelope becomes a captured orange preview on click. It shows the effective
width, height, sill, along-facade coordinate and depth. Plan openings show the actual host wall
thickness on that storey; elevations and 3D show the dimension rectangle at the storey elevation.
Attachments show their size/projection envelope, not a wall cut. These are cheap local dimension
previews, not detailed family meshes or clash certificates. **Place component** submits
`facade.place_at` and runs the engine's complete validation once. A rejected command retains
its actual error and changes no model data.

The captured target stays fixed while moving over Properties. Edit **Along facade from source
start** or enable **Override new instance sizes** to enter exact millimetres. Numeric edits
are never snapped. By default, cursor picks align within 100 mm of existing bay or live occurrence
axes; otherwise they use a 100 mm increment. **Snap / align** changes those settings (0 disables
the increment), and **Alt** bypasses both. The guide and coordinate are visible on the canvas.
This UI snap distance is separate from the engine's 0.000001 mm bay-identity tolerance.

Placement refuses occupied same-class slots by default. **Replace existing in these slots**
is explicit consent for `replace:true`; changing the type, host, point or storey clears it.
Successful placement selects an exact product returned by the engine in its returned phase,
including attachment leaf IDs. **Inspect selected instance** opens its instance Properties.
The point tool remains available for repeat placement; its own appended bay is acknowledged.
Undo/Redo includes both the instance and any appended bay. **Escape** or **Exit placement**
leaves the tool; open Commands, close dialogs, dropdowns and text editors retain keyboard ownership.

**Bay pattern** is an explicit optional mode for the existing `facade.place` workflow. Choose
one bay or opt into **All bays on this storey**. Its separate **Facade bay grid** control changes
the shared grid across storeys and retains the existing grid pruning/replacement rules.
Neither mode sorts or remaps source bays during a pick. Shared type editing and local instance
overrides remain separate; portable library import/export uses the local `.trigonlib` format.

Plan picks use actual site-local coordinates. Elevation picks project their horizontal coordinate
through that facade's source frame; section host picks are refused because depth is ambiguous.
In 3D the real triangle hit supplies the source point. A bare facade does not imply a storey.
An initial unambiguous saved plan may supply its explicit level; the global numeric level and
first building/storey are never used as substitutes. Missing/ambiguous hosts are refused.

Changing the host edge/frame, wall depth, storey schedule, bay grid or active phase invalidates
a captured point. This check runs with Properties hidden and before accepting another pick.
After invalidation, choose a current storey or leave and reopen the saved plan, then capture a
fresh point. A basement insertion's automatic saved-plan remap is not treated as renewed intent.
A pending click never moves or rotates with an edited host. Picking/placing waits for the current
model/drawing and the active phase's complete view.

## AI control and feedback

MCP clients can use the existing schemas for `building.add`, `interior.add_walls`,
`building.set_roof`, `annotation.define`, `types.define`, the placement commands and precision
preview/audit. These operations validate the same parametric model and share its journal/Undo.

Studio's read-only view state adds an `authoring` object: active model tool, exact draft points,
building/storey scope, pending command, error, annotation next step and explicit units. Empty
construction-plane feedback includes its window origin and millimetres per point. Hidden browser
hit rectangles are null. Draft state is UI feedback and is not silently saved as building geometry.
Family-placement feedback reports the mode, exact captured/hover coordinates, dimension preview,
snap settings, selected scope, readiness, stale source and failure state without placing an instance. `command_error` retains an actual placement rejection separately
from current local readiness; a successful retry or an explicit new type/scope clears it. Storey
scope is checked with Properties hidden and before accepting a bare-wall pick, so adding a
basement cannot renew an old floor index through a viewport click.

Open dropdowns own Enter and Escape before canvas tools. Handles retain the original pointer
press when the native integration batches a press and movement into one frame; one completed
drag still produces one undoable parameter edit.

Compatibility: these controls reuse existing model commands and file fields. No native file
format migration or new modelling dependency is introduced by this UI change. Native appearance,
platform builds and full gates must be checked on the integrated revision before release.
