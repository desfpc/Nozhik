#!/usr/bin/env python3
"""Build/check the host platform by default; cross-builds require --target."""

import argparse
import hashlib
import json
import os
import plistlib
from pathlib import Path
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import zipfile

from third_party_licenses import LEGAL_FILES, collect

ROOT = Path(__file__).resolve().parent.parent
TARGETS = {
    "aarch64-apple-darwin": ("macos-arm64", "nozhik"),
    "x86_64-pc-windows-gnu": ("windows-x64", "nozhik.exe"),
    "x86_64-pc-windows-msvc": ("windows-x64", "nozhik.exe"),
}
PLATFORMS = {platform for platform, _ in TARGETS.values()}


def run(*args, **kwargs):
    return subprocess.run(args, cwd=ROOT, check=True, **kwargs)


def source_hash():
    """Hash build inputs, including new files; exclude binaries to avoid recursion."""
    tracked = run(
        "git", "ls-files", "-z", "--cached", "--others", "--exclude-standard", "--",
        ".gitattributes", "Cargo.toml", "Cargo.lock", "rust-toolchain.toml",
        "crates", "examples", ".cargo", "scripts/update-binaries.py",
        "scripts/third_party_licenses.py", "LICENSE", "THIRD_PARTY_NOTICES.md",
        "licenses", stdout=subprocess.PIPE,
    ).stdout
    digest = hashlib.sha256()
    for raw in sorted(set(tracked.split(b"\0")) - {b""}):
        path = ROOT / os.fsdecode(raw)
        if not path.is_file():
            continue  # Deleted input disappears from the fingerprint.
        body = path.read_bytes()
        digest.update(raw + b"\0" + len(body).to_bytes(8, "big") + body)
    return digest.hexdigest()


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def validate_binary(path, target):
    with path.open("rb") as binary:
        header = binary.read(64)
        if target.endswith("apple-darwin"):
            if header[:4] != b"\xcf\xfa\xed\xfe" or struct.unpack_from("<I", header, 4)[0] != 0x0100000C:
                raise ValueError(f"{path}: expected an ARM64 Mach-O executable")
        else:
            if header[:2] != b"MZ":
                raise ValueError(f"{path}: expected a Windows PE executable")
            binary.seek(struct.unpack_from("<I", header, 60)[0])
            pe = binary.read(6)
            if pe[:4] != b"PE\0\0" or struct.unpack_from("<H", pe, 4)[0] != 0x8664:
                raise ValueError(f"{path}: expected an x86-64 PE executable")


def gnu_runtime(binary, directory):
    """Include any non-system MinGW runtime DLLs, including their dependencies."""
    names = {"libgcc_s_seh-1.dll", "libwinpthread-1.dll", "libstdc++-6.dll"}
    pending = [binary]
    packaged = set()
    while pending:
        current = pending.pop()
        info = run("x86_64-w64-mingw32-objdump", "-p", str(current),
                   stdout=subprocess.PIPE, text=True).stdout
        for name in re.findall(r"DLL Name:\s*(\S+)", info):
            if name.lower() not in names or name.lower() in packaged:
                continue
            runtime = Path(run("x86_64-w64-mingw32-gcc", f"-print-file-name={name}",
                               stdout=subprocess.PIPE, text=True).stdout.strip())
            if not runtime.is_file():
                raise ValueError(f"Missing MinGW runtime: {name}")
            copied = directory / name
            shutil.copy2(runtime, copied)
            packaged.add(name.lower())
            pending.append(copied)


def macos_bundle(binary, directory):
    """Ship a Finder/Dock application alongside the command-line executable."""
    packages = json.loads(run("cargo", "metadata", "--no-deps", "--format-version=1",
                              stdout=subprocess.PIPE, text=True).stdout)["packages"]
    version = next(package["version"] for package in packages if package["name"] == "app")
    info = plistlib.dumps({
        "CFBundleName": "Nozhik",
        "CFBundleDisplayName": "Nozhik",
        "CFBundleIdentifier": "dev.Nozhik.Nozhik",
        "CFBundleExecutable": "nozhik",
        "CFBundleIconFile": "nozhik.icns",
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": version,
        "CFBundleVersion": version,
        "LSMinimumSystemVersion": "11.0",
        "NSHighResolutionCapable": True,
    })
    contents = {
        "Info.plist": (info, 0o644),
        "MacOS/nozhik": (binary.read_bytes(), 0o755),
        "Resources/nozhik.icns": ((ROOT / "crates/app/assets/nozhik.icns").read_bytes(), 0o644),
    }
    for name in LEGAL_FILES:
        contents[f"Resources/Legal/{name}"] = ((directory / name).read_bytes(), 0o644)
    with tempfile.TemporaryDirectory(prefix="nozhik-app-") as temporary:
        root = Path(temporary)
        bundle = root / "Nozhik.app"
        for name, (body, mode) in contents.items():
            path = bundle / "Contents" / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(body)
            path.chmod(mode)
        # ARM64 executables carry a linker ad-hoc signature. Seal the complete
        # bundle so verification also covers its plist and icon resources.
        run("codesign", "--force", "--sign", "-", str(bundle))
        run("codesign", "--verify", "--deep", "--strict", str(bundle))
        with zipfile.ZipFile(directory / "Nozhik.app.zip", "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for path in sorted(bundle.rglob("*")):
                if path.is_file():
                    entry = zipfile.ZipInfo(path.relative_to(root).as_posix(), date_time=(2026, 1, 1, 0, 0, 0))
                    entry.create_system = 3
                    entry.external_attr = path.stat().st_mode << 16
                    entry.compress_type = zipfile.ZIP_DEFLATED
                    archive.writestr(entry, path.read_bytes())


def build(target):
    platform, name = TARGETS[target]
    before = source_hash()
    rustc = run("rustc", "-vV", stdout=subprocess.PIPE, text=True).stdout
    host = next(line.removeprefix("host: ") for line in rustc.splitlines() if line.startswith("host: "))
    env = os.environ.copy()
    if target.endswith("windows-gnu"):
        env.setdefault("CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER", "x86_64-w64-mingw32-gcc")
    if target.endswith("windows-msvc"):
        # Downloaded .exe must not require separately installing the VC runtime.
        key = "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS"
        env[key] = env.get(key, "") + " -C target-feature=+crt-static"
    command = ["cargo", "build", "--locked", "--release", "-p", "app"]
    explicit_target = target != host or target.endswith("windows-msvc")
    if explicit_target:
        command += ["--target", target]
    run(*command, env=env)
    after = source_hash()
    if before != after:
        raise ValueError("Sources changed during the build; rerun to refresh local binaries")
    target_dir = Path(env.get("CARGO_TARGET_DIR", "target"))
    if not target_dir.is_absolute():
        target_dir = ROOT / target_dir
    executable = target_dir / target / "release" / "app.exe" if explicit_target else target_dir / "release" / "app"
    if explicit_target and target.endswith("apple-darwin"):
        executable = target_dir / target / "release" / "app"
    destination = ROOT / "binaries" / platform
    destination.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="nozhik-binaries-") as staging:
        staging = Path(staging)
        payload = staging / name
        shutil.copy2(executable, payload)
        validate_binary(payload, target)
        collect(staging, target)
        if target.endswith("apple-darwin"):
            payload.chmod(0o755)
            macos_bundle(payload, staging)
        if target.endswith("windows-gnu"):
            gnu_runtime(payload, staging)
        metadata = {
            "platform": platform,
            "target": target,
            "source_sha256": after,
            "rustc": rustc.splitlines()[0],
            "build_host": host,
            "runtime_tested": False,
            "files": {p.name: sha256(p) for p in sorted(staging.iterdir())},
        }
        (staging / "build.json").write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
        # Remove only previously generated payloads, after a successful build.
        for previous in destination.iterdir():
            if previous.name in {"build.json", "nozhik", "nozhik.exe", "Nozhik.app.zip", *LEGAL_FILES} or previous.suffix == ".dll":
                previous.unlink()
        for artifact in staging.iterdir():
            shutil.copy2(artifact, destination / artifact.name)
    print(f"Updated binaries/{platform}: {after}")


def check(platform):
    directory = ROOT / "binaries" / platform
    metadata = json.loads((directory / "build.json").read_text(encoding="utf-8"))
    if metadata["source_sha256"] != source_hash():
        raise ValueError(f"{platform}: binaries are stale; run update-binaries.py on a build host")
    if TARGETS.get(metadata["target"], (None, None))[0] != platform or metadata["platform"] != platform:
        raise ValueError(f"{platform}: wrong build target")
    if not set(LEGAL_FILES).issubset(metadata["files"]):
        raise ValueError(f"{platform}: missing legal materials; rebuild the distribution")
    expected = set(metadata["files"]) | {"build.json"}
    if {p.name for p in directory.iterdir()} != expected:
        raise ValueError(f"{platform}: unexpected or missing payload files")
    for name, checksum in metadata["files"].items():
        if Path(name).name != name or sha256(directory / name) != checksum:
            raise ValueError(f"{platform}: checksum mismatch for {name}")
    validate_binary(directory / TARGETS[metadata["target"]][1], metadata["target"])
    print(f"Current: binaries/{platform} (sources and binary checksums verified)")


def host_target():
    """Read the native Rust target without building any platform."""
    rustc = run("rustc", "-vV", stdout=subprocess.PIPE, text=True).stdout
    return next(line.removeprefix("host: ") for line in rustc.splitlines() if line.startswith("host: "))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", choices=sorted(TARGETS), help="Rust target to build")
    parser.add_argument("--check", action="store_true", help="Verify freshness without compiling")
    platforms = parser.add_mutually_exclusive_group()
    platforms.add_argument("--platform", choices=sorted(PLATFORMS), help="Check an explicit platform")
    platforms.add_argument("--all-platforms", action="store_true", help="Check all platforms (requires --check)")
    args = parser.parse_args()
    if args.check:
        if args.target:
            parser.error("--target cannot be combined with --check; use --platform")
        if args.all_platforms:
            selected = PLATFORMS
        elif args.platform:
            selected = [args.platform]
        else:
            target = host_target()
            if target not in TARGETS:
                parser.error("This host needs --check --platform or --check --all-platforms")
            selected = [TARGETS[target][0]]
        for platform in sorted(selected):
            check(platform)
    else:
        if args.platform or args.all_platforms:
            parser.error("--platform and --all-platforms require --check")
        target = args.target or host_target()
        if target not in TARGETS:
            parser.error("This host needs an explicit supported --target")
        build(target)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print(f"Binary update failed: {error}", file=sys.stderr)
        sys.exit(1)
