# Idealcombi family reference

The owner's local `Downloads/Idealcombi Revit` package was inspected on 2026-10-31. It contains
98 `.rfa` families, three PDF manuals/contact sheets and one Revit shared-parameter text file
(about 209 MB of content). No IFC, glTF, OBJ or STEP files are present. The original download
and its assets stay outside the repository.

The families cover Futura+, Futura+i, Frame IC/Nation IC, Frame I/Nation I, entrance doors,
sliding doors, corner/shaped windows, terrace doors, tags, glazing bars, panels, shutters and
2D detail items. The English user manual says the families were created in Revit 2020.

Useful native modelling requirements from the illustrated English manual:

- Keep rough opening width/height separate from the manufactured frame width/height. The
  example on printed page 9 uses a 1,812 mm rough opening, a 1,788 mm frame and a 12 mm sealant
  gap on each side. These are an illustrated configuration, not universal product limits.
- Keep frame depth, sill setback and sealant width as distinct parameters. The Futura+ example
  on printed page 8 has 149/175 mm frame-depth options; other ranges have their own options.
- Separate double/triple glazing, frame/sash profiles and interior/exterior materials from the
  overall opening dimensions.
- Represent fixed, top-guided, left/right casement and reversible opening symbols explicitly.
  The manual has detailed and generic floor-plan visibility as well as elevation symbols.
- Store mullion position and pane topology independently of overall size. A sash count alone
  cannot represent the catalog's shaped, corner and asymmetrically divided windows.
- Add paper-scale 2D detail representations for the same parametric type, rather than treating
  the Revit detail families as substitute 3D geometry.

The supplied shared-parameter file defines one multiline description field, `Idealcombi
Beskrivelse`; it is not a dimensional type catalog. The names of the 98 files also do not
supply exact frame/sash dimensions. No native `.rfa` geometry or formulas have been decoded.

Trigon currently models its own deterministic window/door families, with an opening width,
height, sill and limited family-specific parameters. The manual is useful for defining the
next version of that contract and checking its plan, elevation and section output. A direct
Revit-family import is not implemented. A future conversion can load the assets in Revit,
place representative instances and export IFC for comparison with Trigon's IFC reader; the
parametric constraints and formulas need a separate mapping. Any distribution of original
vendor assets requires checking the package's redistribution terms first.
