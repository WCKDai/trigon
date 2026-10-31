# What Trigon needs from PolyCore (proposal)

Status: proposal, 2026-10-31. This is input to PolyCore's own RFC process. PolyCore is a separate
repository with its own owner process, so nothing here changes it directly.

Read alongside: `~/wckdboy/polycore/docs/rfcs/0001-polycore.md` (phases, gates),
`0002-ai-native-engineering-platform.md` (kernel contract, backends),
`docs/research/reuse-survey.md`, and `bench/scorecard.md` (2026-10-27).

## 1. Where PolyCore stands (observed)

- Score on its own 509-case corpus: truck **67.7** against **50.7** for "OCCT behind Polykrom's bridge" (133.6 %).
  The scorecard itself warns that this does **not** mean it beats OCCT: the bridge loses operations, and mass
  properties are measured on tessellations.
- The weak metrics are **correctness 31.4** (a tessellation-based integrator, fixed by exact mass properties in Phase 1),
  **robustness 30**, **11 hangs**, and **11 nondeterministic cases**.
- The root cause, per RFC 0001, is truck's numeric design: mesh-seeded SSI, no local tolerances, no p-curves,
  and about ten special-case modules plus a retry ladder.
- Plan: Phases 0–6, about 25–50 engineering-months to reach OCCT's score on the corpus; blends and offsets
  rivalling commercial kernels is multi-year. That estimate is honest and should stand.

## 2. Gaps that Trigon exposes

### 2.1 Building-scale coordinates (correctness, design now)

Danish coordinates in EPSG:25832 have northings around 6.1 × 10⁶ m = **6.1 × 10⁹ mm**. One f64 step
at that magnitude is about **1.4 × 10⁻⁶ mm**, so the planned model resolution of 1e-6 mm **cannot hold
in world coordinates**.

- Kernel: define a **size box** (the largest model extent for which the resolution holds) and refuse
  operations outside it (`RefusalKind::InvalidInput`). Bodies are always modelled in a local frame.
- Trigon: each site has a local engineering origin, and map coordinates appear only in georeferencing
  metadata (IFC `IfcMapConversion`, CityJSON transform). This never touches the kernel.
- Add corpus cases: a 200 m building, a 2 km site, and a 5 mm window gasket in the same model.

### 2.2 Sections and hidden-line projection are not in the `Kernel` trait

RFC 0002's trait has primitive, extrude, revolve, sweep, loft, boolean, blend, offset, split, check, mass,
tessellate, analysis_mesh, read and write, but **no `section` or `project`**. Polykrom's W01 drawing work
stores section *results* in `solidcraft_geom::section`, but computing them belongs in the kernel:

```rust
fn section(&self, cx: &Cx, bodies: &[Body], plane: &Plane) -> Outcome<Section>;          // exact cut curves + regions, with history
fn project(&self, cx: &Cx, bodies: &[Body], view: &View) -> Outcome<Projection>;         // HLR: visible / hidden / silhouette edges, with history
```

Both Polykrom (part drawings) and Trigon (plans, sections, elevations) need these. With history attached, every
drawn line knows which face or edge it came from, so annotations and dimensions survive model edits.
Exact HLR on free-form surfaces is hard, but the planar/analytic case (most of architecture) is tractable early.

### 2.3 An exact polyhedral fast path

Most building geometry is planar: walls, slabs, openings, roof planes. A boolean and section path for
**planar-faced bodies using exact predicates** (Shewchuk-style adaptive predicates, or exact rationals as in
manifold-rust's second engine) could be **both the fastest and the most robust in open source** for that class.
It would serve as the first backend in `CompositeKernel` under `Prefer(polyhedral)`, with the general NURBS path as fallback.
It also helps Polykrom (prismatic parts, sheet metal).

### 2.4 Scale: instancing and many small bodies

A city block holds about 1,000 windows in maybe 10 types. Content-addressed, immutable bodies (RFC 0002) already fit;
what is missing are gates for it. Add bench cases for: 10k instances tessellated and shown, 1k opening cuts in one wall
set, and incremental re-evaluation after one type parameter changes.

### 2.5 An AEC corpus and flagship next to TinyRex

The gates are mechanical (TinyRex, printer, robot dog). Add a **building flagship**: one real Danish building
rebuilt by an agent through Trigon's commands, scored like TinyRex (median of three runs). This keeps the kernel
honest for both domains.

## 3. What makes PolyCore *the best open-source kernel*, not just Polykrom's

These follow from RFC 0001's plan. Points marked **new** go beyond it.

1. **Do the planned phases with the gates as written.** Phase 1 (own topology, local tolerances, p-curves,
   exact mass properties) moves correctness, the weakest metric. Phase 2 (general fuse, deleting the retry ladder)
   moves robustness and removes the hangs. Don't skip ahead to features.
2. **Determinism and no-hang as hard gates now**, not later: deadlines and budgets in `Cx` turn hangs into
   `OutOfBudget` refusals. A kernel that never hangs and always gives the same answer is already better than
   most open kernels in practice.
3. **Make the corpus public as a benchmark (new).** Publish the cases, answers and the scoring harness as an
   open kernel benchmark that anyone can run against OCCT, truck, solidcore or Remus. Whoever owns the benchmark
   defines "best", and that also draws contributors.
4. **Bindings beyond Rust (new; revisits RFC 0001's "no C ABI").** Most open-source CAD users today script OCCT
   through CadQuery or build123d in Python. A **Python binding (PyO3)** and a **JS/wasm package**, both over a
   stable, semver'd API, would make PolyCore usable outside WCKD. That is the difference between a good kernel
   and a widely used one. This should wait until the Phase 1 API stops moving.
5. **Clean provenance as a feature.** The clean-room rules, the sources table and the attribution trail are what let
   companies trust and adopt an Apache-2.0 kernel. Keep them strict, including the provenance review of Remus.
6. **Exchange fidelity.** STEP AP242 read/write with analytic surfaces kept (step-io candidate), IFC geometry for
   Trigon, and USD `omniSolid`. A kernel is judged by what it can read.
7. **Documentation of the math.** Every algorithm has a doc page that cites its paper. That is how a contributor
   community forms around a kernel, and it is rare in open CAD.

## 4. Keeping it free and open source

- **Apache-2.0 keeps PolyCore free forever:** nobody can revoke a released version. It does **not** force others'
  modifications to stay open, so someone can build a closed product on it.
- If you want derivatives to stay open too, **MPL-2.0** is the mildest option: modified PolyCore files must stay
  open, but it can still be used inside larger works. LGPL/GPL would conflict with the clean-room, permissive
  stack and with Polykrom's own policy.
- **Recommendation: stay Apache-2.0.** It is PolyCore's main advantage over OCCT (LGPL) for adoption, and
  the vendored truck, SMLib and solidcore ports are Apache-2.0 too. Protect the project in other ways:
  - **Own the name:** "PolyCore", "Polykrom" and "Trigon" trademarks/policy, so forks can't pass themselves off as the project.
  - **DCO sign-off** on contributions, so the provenance of every line is recorded.
  - **Keep the corpus, benchmark and roadmap public.** A closed fork can't take the community with it.
- Changing a license later needs every contributor's consent for their code, so decide before outside contributions arrive.
