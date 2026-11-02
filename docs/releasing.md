# Preparing desktop preview candidates

Trigon currently uses workspace version **0.1.0**. Preparing a package does not bump that
version, tag, sign, notarize, publish, or authorize a release. Keep packages labelled
`preview-unsigned` until a separately approved signing and release process verifies them.

`tools/package-release.py` uses Python 3.11+ and its standard library. It packages reviewed
executables into a macOS Apple silicon `.app` ZIP or a Windows x64 portable ZIP. It never
builds or runs its inputs. Its hash and header checks do not prove that a binary was built
from the claimed revision or that the operator's verification record is truthful. Preserve
the build logs, dependency inventory, test logs, and native smoke evidence separately and
review their connection to the exact binary hashes.

## Freeze and verify the source

1. Find or create the Plane work item and record it in the branch or PR, following `AGENTS.md`.
2. Finish review, record the exact commit, and use a clean tracked tree. Do not reuse a private
   review bundle or package an owner's working model. Untracked files are not swept into an
   archive; all package inputs come from the small allowlist below.
3. Run and retain results for the repository gates at that revision:

   ```sh
   cargo fmt --all
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```

   If the engineering kit is installed, also run its configured full verification, security,
   impact, release-check, and evidence verification steps. If it is absent or a scanner is
   unavailable, report that check as unverified. Do not install the kit as an incidental
   packaging change.
4. Build both `trigon-studio` and `trigon` with the lockfile, for the intended target. Retain
   the Rust toolchain, SDK/linker versions, target, build command, dependency inventory, and
   SHA-256 of each resulting binary. Remap private build paths, including paths from Cargo's
   dependency cache; inspect the finished binaries for leaks. For example, on a macOS build
   host, a clean candidate can be built with:

   ```sh
   RUSTFLAGS="--remap-path-prefix=$HOME=/trigon/build" \
     cargo build --locked --release --target aarch64-apple-darwin \
       -p trigon-studio -p trigon
   ```

   This example does not establish minimum supported OS or reproducible compilation. The
   packager derives `LSMinimumSystemVersion` from the maximum deployment minimum encoded
   in the two Mach-O inputs. Verify that OS with the finished app.

   The Windows target is `x86_64-pc-windows-msvc`. A cross-build, including a build using
   `cargo xwin`, is useful build evidence but **is not native Windows QA**. The Studio PE
   must use Windows GUI subsystem 2; the CLI must use console subsystem 3. Missing runtimes,
   GPU/driver behavior, native dialogs, and security prompts require a real Windows run.

## Stage only the allowed inputs

Use a fresh dedicated input directory, separate from the output ZIP. For macOS it contains
exactly `trigon-studio`, `trigon`, `THIRD-PARTY-NOTICES.txt`, and `verified-inputs.json`.
It may additionally contain `app.icns`. For Windows, replace the executable names with
`trigon-studio.exe` and `trigon.exe`; an ICNS file is not accepted. Even `.DS_Store`, subfolders,
symlinks, project files, and unrelated build outputs cause refusal.

`THIRD-PARTY-NOTICES.txt` must contain the reviewed notices and license texts for the
dependencies actually included in these binaries. Resolve the target-specific, locked
dependency inventory and retain the inventory and license review with the build evidence.
The tool requires a nonempty UTF-8 notice file and checks its hash; it cannot establish
notice completeness or license compatibility. Never read or ship GPL/LGPL/AGPL source to
produce this file. Resolve a licensing uncertainty before distributing a candidate.

The tool also copies the committed project MIT and Apache licenses, `ATTRIBUTION.md`, the
Trigon artwork notice, the Lucide/Feather licenses, and Inter, IBM Plex Mono, and Liberation
OFL notices from fixed repository paths. Studio's icons and fonts are embedded, so it does
not copy the asset folders or require a source checkout at runtime. An optional `app.icns`
must be supplied from the approved Trigon artwork with compatible licensing; ICNS length
validation does not check artwork origin or rendering quality.

Create `verified-inputs.json` only after reviewing the actual gate results and input hashes.
It must have exactly this schema; replace the illustrative hash values with lowercase
64-character SHA-256 values and use the full candidate commit ID:

```json
{
  "schema": 1,
  "source_revision": "FULL_CANDIDATE_COMMIT_ID",
  "version": "0.1.0",
  "target": "aarch64-apple-darwin",
  "sha256": {
    "trigon-studio": "STUDIO_SHA256",
    "trigon": "CLI_SHA256",
    "THIRD-PARTY-NOTICES.txt": "NOTICES_SHA256"
  },
  "checks": {"format": "passed", "clippy": "passed", "tests": "passed"},
  "native_smoke_test": "not-run",
  "security": {
    "scanner": "cargo-audit",
    "status": "not-run",
    "finding_count": null,
    "risk_acceptance": "not-recorded",
    "evidence_sha256": null
  }
}
```

For Windows use its target triple and `.exe` names. If `app.icns` is present, include its hash
as another `sha256` entry. Do not add paths, logs, credentials, arbitrary notes, or other JSON
fields. `native_smoke_test` is either `passed` or `not-run`; record `passed` only after testing
the exact binary hashes on that platform. Required gates with failures or missing results
cannot be represented as passed and cannot be packaged with this tool. Platform smoke may
remain `not-run` for a local unsigned candidate, with that limitation explicit in its README
and provenance. Such a candidate is not a verified public release.

The security record reports `passed`, `findings`, `not-run`, or `unavailable`. Completed scans
require their report's SHA-256 and a finding count (zero for `passed`, positive for `findings`).
Unrun/unavailable scans use null count and evidence. A scanner identifier may contain letters,
numbers, dots, underscores, plus, or hyphen; include its version there or in the retained
report. Existing findings without accepted risk remain `findings` and `not-recorded`; do not
invent a scanner pass. The tool allows these explicit limits for a local preview candidate
and carries them into the README and provenance. Public release still requires the owner to
resolve or explicitly accept the findings under the repository's policy.

## Package and inspect

From the clean candidate checkout, with existing `out` directories, for example:

```sh
python3 tools/package-release.py \
  --target aarch64-apple-darwin \
  --input-dir out/package-input/macos-arm64 \
  --output out/Trigon-0.1.0-preview-unsigned-macos-arm64.zip

python3 tools/package-release.py \
  --target x86_64-pc-windows-msvc \
  --input-dir out/package-input/windows-x64 \
  --output out/Trigon-0.1.0-preview-unsigned-windows-x64.zip
```

The command prints the ZIP's SHA-256 and exits zero on success; refusal exits one. Invalid
CLI arguments use argparse's exit code two. Existing output is preserved unless the caller
supplies `--overwrite-sha256` with its exact current hash. Use distinct filenames for public
versions; controlled overwrite is for local candidates. Paths through symlinks, outputs
inside the input directory, wrong architectures, malformed executable headers, hash or
revision mismatches, dirty tracked source, and common private home-path prefixes are refused.
The home-path check covers common ASCII and UTF-16 prefixes; it is not a general secret or
debug-information scanner. Use operator-controlled input and output directories; concurrent
hostile replacement of parent directories is outside this tool's guarantees.

macOS output contains `Trigon Studio.app/Contents/MacOS/trigon-studio` and `trigon`, plus a
generated `Info.plist` and resources. The direct Studio binary is `CFBundleExecutable`; no
shell launcher, working-directory change, or model argument is injected. Studio starts a new
document with no arguments. The CLI prints usage with no arguments. The bundle identifier
is `io.github.wckdai.trigon-studio`. No Finder document association is declared because CLI
file arguments do not establish native open-document event support. These plist keys follow
[Apple's bundle key reference](https://developer.apple.com/library/archive/documentation/General/Reference/InfoPlistKeyReference/Articles/CoreFoundationKeys.html).

Windows output contains both executables in `Trigon-0.1.0-preview-unsigned-windows-x64/`.
Header checks follow the architecture and subsystem fields in
[Microsoft's PE format specification](https://learn.microsoft.com/en-us/windows/win32/debug/pe-format).
Mach-O deployment and entry commands follow
[Apple's published format definitions](https://github.com/apple-oss-distributions/xnu/blob/main/EXTERNAL_HEADERS/mach-o/loader.h).
These checks establish packaging compatibility, not executable correctness.

Both archives include generated READMEs, license notices, and `PROVENANCE.json` binding the
source revision, workspace version, target, lockfile hash, packager hash, input and notice
hashes, reported checks, native smoke status, scanner status, finding count, risk acceptance
status, and scan evidence hash. The original verification record is
represented by its hash and validated fields rather than copied as arbitrary text. No
absolute input paths are written into generated metadata. Entries are sorted, have fixed
1980 timestamps and explicit file modes; repeated packaging of identical inputs with the
same Python/zlib implementation produces identical ZIP bytes. This does not make the Rust
build reproducible or provide a cryptographically signed attestation.

Extract each ZIP to a fresh directory and inspect its exact file list, hashes, executable
permissions, plist, notices, and provenance. On macOS run `plutil -lint` on the plist and
inspect linked libraries and deployment commands with `otool`; on Windows inspect imports
and the actual runtime requirements. A package needing a non-system dynamic library is
unsupported by this executable-only allowlist: fix the build or design a separately reviewed
explicit runtime allowlist before packaging it.

## Native acceptance and release handoff

On Apple silicon macOS, launch the extracted `.app` from Finder and verify a usable blank
document, the intended window/icon, a small public modelling workflow, local save/reopen,
undo, export, and native Open/Save dialogs. Exercise the bundled CLI's `commands` and schema
commands. Check the documented MCP/control workflow with a fresh local discovery state and
confirm that `--no-control` works. The default control channel listens locally with its
per-launch token; never place that token or discovery state in an archive or evidence to share.

On Windows x64, run the extracted Studio and CLI on a real Windows installation and repeat
the workflow, including graphics, native dialogs, the expected absence of a console window
for Studio, clean shutdown, and relaunch. Check the minimum supported OS and architecture;
do not infer support from a successful cross-build. Use a new public-safe document, not a
private owner project, and keep smoke outputs outside the distributable input directory.

Retain the archive hash, full candidate revision, build and full-gate evidence, dependency
inventory and notices review, scanner results or explicit gaps, native results per platform,
and signing/notarization status. The package tool neither signs nor verifies signatures.
Signing changes executable hashes and must be a separately approved, documented process
with fresh hashes and provenance; do not silently relabel an unsigned candidate as signed.
Ask the release owner to approve the concrete inspected artifacts before tag, upload,
publication, or any signing credential access. After authorized publication, download the
public artifacts, compare hashes/signatures, and repeat a smoke run.

Rollback for this portable preview is to quit Studio, keep user documents separately, and
replace the extracted app or folder with the previously retained verified archive. Exercise
that replacement in a disposable install location and reopen a public-safe copy of a
document with the earlier version. Packaging changes no document format and promises no
forward compatibility for later modelling changes; test the actual versions involved.
Retain the old artifact and its digest, and name the release owner responsible for rollback.
