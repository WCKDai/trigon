#!/usr/bin/env python3
"""Package reviewed Trigon binaries without building, signing, or publishing them."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import stat
import struct
import subprocess
import sys
import tempfile
import tomllib
import zipfile


TARGETS = {
    "aarch64-apple-darwin": ("trigon-studio", "trigon"),
    "x86_64-pc-windows-msvc": ("trigon-studio.exe", "trigon.exe"),
}
LICENSES = {
    "LICENSE-MIT": "LICENSE-MIT",
    "LICENSE-APACHE": "LICENSE-APACHE",
    "ATTRIBUTION.md": "ASSET-ATTRIBUTION.md",
    "apps/trigon-studio/assets/figma/NOTICE.md": "Trigon-Artwork-NOTICE.md",
    "apps/trigon-studio/assets/figma/Lucide-Feather-LICENSE.txt": "Lucide-Feather-LICENSE.txt",
    "apps/trigon-studio/assets/fonts/inter/Inter-OFL.txt": "Inter-OFL.txt",
    "apps/trigon-studio/assets/fonts/plex-mono/IBM-Plex-Mono-OFL.txt": "IBM-Plex-Mono-OFL.txt",
    "crates/engine/assets/liberation-sans/OFL.txt": "Liberation-Sans-OFL.txt",
    "crates/engine/assets/liberation/OFL.txt": "Liberation-OFL.txt",
}
RECORD = "verified-inputs.json"
NOTICES = "THIRD-PARTY-NOTICES.txt"
ICON = "app.icns"
CHECKS = {"format": "passed", "clippy": "passed", "tests": "passed"}
SHA256 = re.compile(r"[0-9a-f]{64}\Z")
# These are rejection heuristics, not a general secret or debug-information scanner.
PRIVATE_PREFIXES = (b"/Users/", b"/home/")


class Refusal(Exception):
    """A candidate failed a packaging prerequisite."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise Refusal(message)


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def safe_path(path: Path, label: str) -> Path:
    """Reject symlinks in the entire existing path, including parent directories."""
    path = Path(os.path.abspath(path))
    for component in (*reversed(path.parents), path):
        require(not component.is_symlink(), f"{label}: symlinks are not allowed")
    return path


def regular_bytes(path: Path, label: str, limit: int) -> bytes:
    path = safe_path(path, label)
    require(stat.S_ISREG(path.lstat().st_mode), f"{label}: expected a regular file")
    descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0))
    with os.fdopen(descriptor, "rb") as stream:
        metadata = os.fstat(stream.fileno())
        require(stat.S_ISREG(metadata.st_mode), f"{label}: expected a regular file")
        require(0 < metadata.st_size <= limit, f"{label}: empty or oversized file")
        data = stream.read(limit + 1)
    require(len(data) == metadata.st_size, f"{label}: file changed while reading")
    return data


def public_bytes(data: bytes, label: str) -> None:
    lowered = data.lower()
    for prefix in PRIVATE_PREFIXES:
        utf16 = prefix.decode("ascii").encode("utf-16-le")
        require(prefix.lower() not in lowered and utf16.lower() not in lowered,
                f"{label}: contains a private home-directory prefix; remap build paths")
    require(re.search(rb"[a-z]:[\\/]users[\\/]", lowered) is None and
            re.search(rb"[a-z]\x00:\x00[\\/]\x00u\x00s\x00e\x00r\x00s\x00[\\/]\x00", lowered) is None,
            f"{label}: contains a private home-directory prefix; remap build paths")


def git(source: Path, *args: str) -> bytes:
    environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    result = subprocess.run(
        ["git", "-c", "core.fsmonitor=false", "-C", str(source), *args], env=environment,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False,
    )
    require(result.returncode == 0, "source: Git inspection failed")
    return result.stdout


def clean_revision(source: Path) -> str:
    require(not git(source, "status", "--porcelain=v1", "--untracked-files=no"),
            "source: tracked files are dirty; freeze a clean candidate first")
    revision = git(source, "rev-parse", "HEAD").decode("ascii").strip()
    require(re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", revision) is not None,
            "source: invalid revision")
    return revision


def tracked_bytes(source: Path, name: str) -> bytes:
    data = regular_bytes(source / name, f"source {name}", 2 * 1024 * 1024)
    require(data == git(source, "show", f"HEAD:{name}"),
            f"source {name}: differs from the candidate revision")
    public_bytes(data, f"source {name}")
    return data


def macho(data: bytes) -> dict:
    require(len(data) >= 32 and data[:4] == b"\xcf\xfa\xed\xfe",
            "binary: expected a thin little-endian 64-bit Mach-O executable")
    _, cpu, subtype, kind, count, size, _, _ = struct.unpack_from("<8I", data)
    require(cpu == 0x0100000C and subtype & 0x00FFFFFF == 0 and kind == 2,
            "binary: expected an arm64 macOS executable")
    require(0 < count <= 65535 and 32 + size <= len(data), "binary: invalid Mach-O commands")
    position, entry, minimum = 32, None, None
    for _ in range(count):
        require(position + 8 <= 32 + size, "binary: truncated Mach-O command")
        command, length = struct.unpack_from("<2I", data, position)
        require(length >= 8 and length % 8 == 0 and position + length <= 32 + size,
                "binary: invalid Mach-O command size")
        if command == 0x80000028:  # LC_MAIN
            require(length >= 24, "binary: truncated Mach-O entry point")
            entry = struct.unpack_from("<Q", data, position + 8)[0]
        if command in (0x32, 0x24):  # LC_BUILD_VERSION / LC_VERSION_MIN_MACOSX
            require(length >= (24 if command == 0x32 else 16),
                    "binary: truncated Mach-O platform version")
            if command == 0x32:
                platform, version = struct.unpack_from("<2I", data, position + 8)
                require(platform == 1, "binary: Mach-O platform is not macOS")
            else:
                version = struct.unpack_from("<I", data, position + 8)[0]
            require(minimum in (None, version), "binary: conflicting macOS deployment versions")
            minimum = version
        position += length
    require(position == 32 + size and entry is not None and 32 + size <= entry < len(data),
            "binary: invalid Mach-O entry point or command table")
    require(minimum is not None and minimum >= 0x000B0000,
            "binary: missing or unsupported macOS arm64 deployment version")
    return {"format": "Mach-O", "architecture": "arm64", "minimum_macos":
            [minimum >> 16, (minimum >> 8) & 255, minimum & 255]}


def pe(data: bytes, gui: bool) -> dict:
    require(len(data) >= 64 and data[:2] == b"MZ", "binary: expected a PE executable")
    offset = struct.unpack_from("<I", data, 60)[0]
    require(64 <= offset and offset + 24 <= len(data) and data[offset:offset + 4] == b"PE\0\0",
            "binary: invalid PE header")
    machine, sections, _, _, _, optional_size, flags = struct.unpack_from("<HHIIIHH", data, offset + 4)
    optional = offset + 24
    section_table = optional + optional_size
    require(machine == 0x8664 and flags & 2 and not flags & 0x2000,
            "binary: expected an AMD64 PE executable, not a DLL")
    require(0 < sections <= 96 and optional_size >= 112 and section_table + sections * 40 <= len(data),
            "binary: invalid PE optional header or sections")
    require(struct.unpack_from("<H", data, optional)[0] == 0x20B,
            "binary: expected PE32+ (64-bit)")
    subsystem = struct.unpack_from("<H", data, optional + 68)[0]
    require(subsystem == (2 if gui else 3),
            "binary: Studio needs Windows GUI subsystem 2; CLI needs console subsystem 3")
    entry = struct.unpack_from("<I", data, optional + 16)[0]
    executable_entry = False
    for index in range(sections):
        section = section_table + index * 40
        virtual_size, address, raw_size, raw_offset = struct.unpack_from("<4I", data, section + 8)
        characteristics = struct.unpack_from("<I", data, section + 36)[0]
        require(not raw_size or (raw_offset >= section_table + sections * 40 and
                                  raw_offset + raw_size <= len(data)), "binary: invalid PE section data")
        if address <= entry < address + min(virtual_size, raw_size) and characteristics & 0x20000000:
            executable_entry = True
    require(entry != 0 and executable_entry, "binary: PE entry point is outside executable file data")
    return {"format": "PE32+", "architecture": "x86_64", "subsystem": subsystem}


def icon(data: bytes) -> None:
    require(len(data) >= 16 and data[:4] == b"icns" and struct.unpack_from(">I", data, 4)[0] == len(data),
            "icon: expected an ICNS container")
    offset = 8
    while offset < len(data):
        require(offset + 8 <= len(data), "icon: truncated ICNS element")
        size = struct.unpack_from(">I", data, offset + 4)[0]
        require(size > 8 and offset + size <= len(data), "icon: invalid ICNS element length")
        offset += size
    require(offset == len(data), "icon: invalid ICNS container length")


def unique_object(pairs: list) -> dict:
    result = {}
    for key, value in pairs:
        require(key not in result, "record: duplicate JSON key")
        result[key] = value
    return result


def security_record(value: object) -> None:
    keys = {"scanner", "status", "finding_count", "risk_acceptance", "evidence_sha256"}
    require(isinstance(value, dict) and set(value) == keys, "record: unexpected security fields")
    require(isinstance(value["scanner"], str) and re.fullmatch(r"[A-Za-z0-9_.+-]{1,80}", value["scanner"]) is not None,
            "record: invalid security scanner identifier")
    status, count, evidence = value["status"], value["finding_count"], value["evidence_sha256"]
    require(status in ("passed", "findings", "not-run", "unavailable"), "record: invalid security status")
    require(value["risk_acceptance"] in ("recorded", "not-recorded"), "record: invalid risk acceptance status")
    if status in ("passed", "findings"):
        require(type(count) is int and 0 <= count <= 1000000 and (count == 0) == (status == "passed"),
                "record: security finding count contradicts status")
        require(isinstance(evidence, str) and SHA256.fullmatch(evidence) is not None,
                "record: completed security scan needs its evidence SHA-256")
    else:
        require(count is None and evidence is None and value["risk_acceptance"] == "not-recorded",
                "record: unrun security scan must remain explicitly unverified")


def record(data: bytes, revision: str, version: str, target: str, names: set[str]) -> dict:
    try:
        value = json.loads(data, object_pairs_hook=unique_object)
    except (ValueError, UnicodeError):
        raise Refusal("record: expected UTF-8 JSON") from None
    keys = {"schema", "source_revision", "version", "target", "sha256", "checks", "native_smoke_test", "security"}
    require(isinstance(value, dict) and set(value) == keys, "record: unexpected or missing fields")
    require(type(value["schema"]) is int and value["schema"] == 1, "record: unsupported schema")
    require((value["source_revision"], value["version"], value["target"]) == (revision, version, target),
            "record: revision, version, or target differs from the candidate")
    hashes = value["sha256"]
    require(isinstance(hashes, dict) and set(hashes) == names and
            all(isinstance(h, str) and SHA256.fullmatch(h) for h in hashes.values()),
            "record: expected exact allowlisted filenames and lowercase SHA-256 hashes")
    require(value["checks"] == CHECKS, "record: format, clippy, and tests must all report passed")
    require(value["native_smoke_test"] in ("passed", "not-run"), "record: invalid native smoke status")
    security_record(value["security"])
    return value


def json_bytes(value: dict) -> bytes:
    return (json.dumps(value, sort_keys=True, indent=2) + "\n").encode("utf-8")


def write_archive(output: Path, entries: dict[str, tuple[bytes, int]], overwrite: str | None) -> str:
    require(output.suffix.lower() == ".zip", "output: expected a .zip filename")
    require(output.parent.is_dir(), "output: parent directory must already exist")
    if overwrite is not None:
        require(SHA256.fullmatch(overwrite) is not None, "output: invalid overwrite SHA-256")
    def check_existing() -> None:
        safe_path(output, "output")
        if output.exists():
            require(overwrite is not None, "output: already exists; use its exact --overwrite-sha256")
            require(digest(regular_bytes(output, "existing output", 2 * 1024 ** 3)) == overwrite,
                    "output: existing archive does not match --overwrite-sha256")
        else:
            require(overwrite is None, "output: cannot overwrite a missing archive")
    check_existing()
    descriptor, name = tempfile.mkstemp(prefix=".trigon-package-", suffix=".zip", dir=output.parent)
    temporary = Path(name)
    try:
        with os.fdopen(descriptor, "wb") as stream:
            with zipfile.ZipFile(stream, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
                for name, (data, mode) in sorted(entries.items()):
                    require(not name.startswith("/") and ".." not in Path(name).parts and "\\" not in name,
                            "archive: invalid entry name")
                    info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
                    info.create_system = 3
                    info.external_attr = (stat.S_IFREG | mode) << 16
                    info.compress_type = zipfile.ZIP_DEFLATED
                    archive.writestr(info, data, compresslevel=9)
            stream.flush()
            os.fsync(stream.fileno())
        check_existing()
        if overwrite is None:
            # A hard link refuses a destination that appeared after check_existing().
            os.link(temporary, output)
        else:
            os.replace(temporary, output)
        return digest(regular_bytes(output, "output", 2 * 1024 ** 3))
    finally:
        temporary.unlink(missing_ok=True)


def package(args: argparse.Namespace) -> str:
    source = safe_path(args.source_root, "source")
    inputs = safe_path(args.input_dir, "inputs")
    output = safe_path(args.output, "output")
    require(source.is_dir() and inputs.is_dir(), "source and inputs must be directories")
    require(not output.is_relative_to(inputs), "output: must be outside the input directory")
    revision = clean_revision(source)
    manifest = tracked_bytes(source, "Cargo.toml")
    try:
        version = tomllib.loads(manifest.decode("utf-8"))["workspace"]["package"]["version"]
    except (KeyError, UnicodeError, tomllib.TOMLDecodeError):
        raise Refusal("source: cannot read the workspace version") from None
    require(isinstance(version, str) and re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version) is not None,
            "source: expected a numeric three-part workspace version")
    binaries = TARGETS[args.target]
    names = set(binaries) | {NOTICES}
    actual = {item.name for item in inputs.iterdir()}
    if args.target == "aarch64-apple-darwin" and ICON in actual:
        names.add(ICON)
    require(actual == names | {RECORD}, "inputs: missing, private, or unexpected entries; use a dedicated allowlist directory")
    data = {name: regular_bytes(inputs / name, f"input {name}",
                               512 * 1024 ** 2 if name in binaries else 16 * 1024 ** 2)
            for name in names | {RECORD}}
    require(len(data[RECORD]) <= 65536, "record: oversized verification record")
    verified = record(data[RECORD], revision, version, args.target, names)
    for name in names:
        require(digest(data[name]) == verified["sha256"][name], f"input {name}: SHA-256 mismatch")
        public_bytes(data[name], f"input {name}")
    try:
        notices = data[NOTICES].decode("utf-8")
    except UnicodeError:
        raise Refusal("notices: expected UTF-8 text") from None
    require("\0" not in notices and len(notices.strip()) >= 80, "notices: expected substantive license text")
    metadata = {name: (macho(data[name]) if args.target == "aarch64-apple-darwin" else pe(data[name], index == 0))
                for index, name in enumerate(binaries)}
    licenses = {destination: tracked_bytes(source, path) for path, destination in LICENSES.items()}
    licenses[NOTICES] = data[NOTICES]
    provenance = {
        "schema": 1, "product": "Trigon Studio", "version": version, "target": args.target,
        "source_revision": revision, "candidate": "preview-unsigned",
        "signature_verification": "not-performed", "native_smoke_test_reported": verified["native_smoke_test"],
        "security_reported": verified["security"], "release_approval": "not-provided-by-packager",
        "required_checks_reported": verified["checks"], "verification_record_sha256": digest(data[RECORD]),
        "cargo_lock_sha256": digest(tracked_bytes(source, "Cargo.lock")),
        "packager_sha256": digest(regular_bytes(Path(__file__), "packager", 2 * 1024 ** 2)),
        "inputs_sha256": {name: digest(data[name]) for name in sorted(names)},
        "binary_headers": metadata,
        "licenses_sha256": {name: digest(content) for name, content in sorted(licenses.items())},
    }
    security_count = verified["security"]["finding_count"]
    security_count = "unverified" if security_count is None else str(security_count)
    readme = (f"Trigon Studio {version} — PREVIEW / UNSIGNED CANDIDATE\n\n"
              "Signing and notarization status are unverified. This is a local preview candidate.\n"
              "The packager checks binary headers and hashes; reported build and smoke checks\n"
              "come from the operator's reviewed verification record. Read PROVENANCE.json.\n"
              f"Native smoke test reported: {verified['native_smoke_test']}.\n\n"
              f"Security scan reported: {verified['security']['status']}; finding count: {security_count}.\n"
              f"Risk acceptance: {verified['security']['risk_acceptance']}. Release approval is not provided.\n\n"
              "The package allowlist contains executables, notices, and generated metadata only.\n"
              "No project files are copied.\n"
              "Studio starts a new document with no arguments. Its local control channel is\n"
              "enabled by default; pass --no-control to disable it. See trigon-studio --help.\n"
              "Use trigon commands or trigon --help from a terminal for CLI/MCP instructions.\n"
              "Review LICENSES before redistribution. Keep the previous version for rollback.\n")
    entries: dict[str, tuple[bytes, int]] = {}
    if args.target == "aarch64-apple-darwin":
        contents = "Trigon Studio.app/Contents/"
        resources = contents + "Resources/"
        minimum = max(meta["minimum_macos"] for meta in metadata.values())
        info = {
            "CFBundleDevelopmentRegion": "en", "CFBundleDisplayName": "Trigon Studio",
            "CFBundleExecutable": "trigon-studio", "CFBundleIdentifier": "io.github.wckdai.trigon-studio",
            "CFBundleName": "Trigon Studio", "CFBundlePackageType": "APPL",
            "CFBundleShortVersionString": version, "CFBundleVersion": version,
            "LSMinimumSystemVersion": ".".join(map(str, minimum)),
            "LSApplicationCategoryType": "public.app-category.graphics-design", "NSHighResolutionCapable": True,
        }
        if ICON in names:
            icon(data[ICON])
            info["CFBundleIconFile"] = "Trigon.icns"
            entries[resources + "Trigon.icns"] = (data[ICON], 0o644)
        entries[contents + "Info.plist"] = (plistlib.dumps(info, sort_keys=True), 0o644)
        for name in binaries:
            entries[contents + "MacOS/" + name] = (data[name], 0o755)
        readme += ("\nmacOS: extract the ZIP and move Trigon Studio.app to Applications. Open the app\n"
                   "to start Studio. CLI: \"/Applications/Trigon Studio.app/Contents/MacOS/trigon\" commands\n"
                   "Do not disable Gatekeeper globally. An unsigned candidate may be blocked by macOS.\n")
        entries["README.txt"] = (readme.encode("utf-8"), 0o644)
    else:
        resources = f"Trigon-{version}-preview-unsigned-windows-x64/"
        for name in binaries:
            entries[resources + name] = (data[name], 0o755)
        readme += ("\nWindows: extract the whole ZIP to a writable local folder. Run trigon-studio.exe\n"
                   "for Studio, or .\\trigon.exe commands from PowerShell for the CLI.\n"
                   "A cross-build and valid PE headers alone do not establish native Windows QA.\n")
    entries[resources + "README.txt"] = (readme.encode("utf-8"), 0o644)
    entries[resources + "PROVENANCE.json"] = (json_bytes(provenance), 0o644)
    for name, content in licenses.items():
        entries[resources + "LICENSES/" + name] = (content, 0o644)
    require(clean_revision(source) == revision, "source: revision changed during packaging")
    return write_archive(output, entries, args.overwrite_sha256)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-root", type=Path, default=Path(__file__).resolve().parent.parent)
    parser.add_argument("--target", choices=TARGETS, required=True)
    parser.add_argument("--input-dir", type=Path, required=True,
                        help="dedicated directory containing only named binaries, notices, and verification record")
    parser.add_argument("--output", type=Path, required=True, help="ZIP outside input-dir; parent must exist")
    parser.add_argument("--overwrite-sha256", help="replace only an existing archive with this exact SHA-256")
    args = parser.parse_args()
    try:
        sha256 = package(args)
    except Refusal as error:
        print(f"package-release: {error}", file=sys.stderr)
        return 1
    except (OSError, ValueError, struct.error) as error:
        # OSError text can contain private input paths; expose only the exception type.
        print(f"package-release: file or binary inspection failed ({type(error).__name__})", file=sys.stderr)
        return 1
    print(f"{args.output.name}  sha256:{sha256}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
