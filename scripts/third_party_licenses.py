"""Collect target-specific third-party legal materials without changing upstream files."""

import json
from pathlib import Path
import subprocess
import zipfile

ROOT = Path(__file__).resolve().parent.parent
LEGAL_FILES = ("LICENSE", "THIRD_PARTY_NOTICES.md", "THIRD_PARTY_LICENSES.txt",
               "THIRD_PARTY_SOURCES.zip")


def notice_file(path):
    """Include upstream license, attribution and bundled font notices."""
    name = path.name.lower()
    return (name.startswith(("license", "licence", "copying", "copyright", "notice", "authors"))
            or name in {"ofl.txt", "ufl.txt"}
            or (path.parent.name == "fonts" and path.suffix == ".txt"))


def zip_source(archive, root, prefix):
    """Keep complete published package sources, including embedded notices."""
    for path in sorted(root.rglob("*")):
        if path.is_file():
            entry = zipfile.ZipInfo(f"{prefix}/{path.relative_to(root).as_posix()}",
                                    date_time=(2026, 1, 1, 0, 0, 0))
            entry.compress_type = zipfile.ZIP_DEFLATED
            entry.external_attr = 0o100644 << 16
            archive.writestr(entry, path.read_bytes())


def collect(destination, target):
    """Write notices and sources for the locked target graph, including build deps."""
    metadata = json.loads(subprocess.run(
        ["cargo", "metadata", "--locked", "--offline", "--format-version=1",
         "--filter-platform", target], cwd=ROOT, check=True,
        stdout=subprocess.PIPE, text=True,
    ).stdout)
    resolved = {node["id"] for node in metadata["resolve"]["nodes"]}
    packages = sorted((package for package in metadata["packages"]
                       if package["source"] and package["id"] in resolved),
                      key=lambda p: (p["name"], p["version"]))
    text = ["Third-party licenses for Nozhik", f"Target: {target}",
            "Upstream terms apply independently of the Nozhik license.",
            "Original sources where noted: THIRD_PARTY_SOURCES.zip", ""]
    with zipfile.ZipFile(destination / "THIRD_PARTY_SOURCES.zip", "w") as archive:
        for package in packages:
            name = f"{package['name']}-{package['version']}"
            root = Path(package["manifest_path"]).parent
            license_id = package["license"]
            if not license_id:
                raise ValueError(f"Review missing license declaration: {name}")
            files = {path for path in root.rglob("*") if path.is_file() and notice_file(path)}
            if package["license_file"]:
                files.add(root / package["license_file"])
            text.extend(["=" * 72, name, f"License declaration: {license_id}",
                         f"Authors: {'; '.join(package['authors'])}",
                         f"Repository: {package['repository'] or ''}",
                         f"Source: https://crates.io/crates/{package['name']}/{package['version']}"])
            if "MPL-2.0" in license_id or not files:
                zip_source(archive, root, name)
                text.append(f"Original source supplied in THIRD_PARTY_SOURCES.zip/{name}/")
            if not files:
                # Some published crates omit workspace-level license files.
                # Supply a declared permissive option and the complete original
                # source, rather than inventing upstream copyright statements.
                fallback = next((item for item in ("Apache-2.0", "MIT", "CC0-1.0")
                                 if license_id == item or item in license_id.split(" OR ")), None)
                if fallback is None:
                    raise ValueError(f"Supply upstream license text for {name}: {license_id}")
                text.extend([f"Standard {fallback} text (upstream archive omits standalone license):",
                             (ROOT / "licenses" / f"{fallback}.txt").read_text(encoding="utf-8")])
            for path in sorted(files):
                text.extend([f"--- {name}/{path.relative_to(root).as_posix()} ---",
                             path.read_text(encoding="utf-8")])
            text.append("")
    text.extend(["=" * 72, "JetBrains Mono Regular",
                 (ROOT / "crates/ui/assets/fonts/OFL.txt").read_text(encoding="utf-8")])
    (destination / "THIRD_PARTY_LICENSES.txt").write_text("\n".join(text), encoding="utf-8")
    for name in ("LICENSE", "THIRD_PARTY_NOTICES.md"):
        (destination / name).write_bytes((ROOT / name).read_bytes())
