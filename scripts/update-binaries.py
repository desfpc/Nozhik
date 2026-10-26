#!/usr/bin/env python3
"""Build repository downloads, or verify their source and payload SHA-256 hashes."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import sys
import tempfile

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
        "crates", ".cargo", "scripts/update-binaries.py", stdout=subprocess.PIPE,
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
        raise ValueError("Sources changed during the build; rerun before committing")
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
        if target.endswith("apple-darwin"):
            payload.chmod(0o755)
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
            if previous.name in {"build.json", "nozhik", "nozhik.exe"} or previous.suffix == ".dll":
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
    expected = set(metadata["files"]) | {"build.json"}
    if {p.name for p in directory.iterdir()} != expected:
        raise ValueError(f"{platform}: unexpected or missing payload files")
    for name, checksum in metadata["files"].items():
        if Path(name).name != name or sha256(directory / name) != checksum:
            raise ValueError(f"{platform}: checksum mismatch for {name}")
    validate_binary(directory / TARGETS[metadata["target"]][1], metadata["target"])
    print(f"Current: binaries/{platform} (sources and binary checksums verified)")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", choices=sorted(TARGETS), help="Rust target to build")
    parser.add_argument("--check", action="store_true", help="Verify freshness without compiling")
    parser.add_argument("--platform", choices=sorted(PLATFORMS), help="Check just one platform")
    args = parser.parse_args()
    if args.check:
        for platform in sorted([args.platform] if args.platform else PLATFORMS):
            check(platform)
    else:
        if args.platform:
            parser.error("--platform requires --check")
        target = args.target
        if not target:
            rustc = run("rustc", "-vV", stdout=subprocess.PIPE, text=True).stdout
            target = next(line.removeprefix("host: ") for line in rustc.splitlines() if line.startswith("host: "))
        if target not in TARGETS:
            parser.error("This host needs an explicit supported --target")
        build(target)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print(f"Binary update failed: {error}", file=sys.stderr)
        sys.exit(1)
