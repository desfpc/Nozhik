#!/usr/bin/env python3
"""Generate PHP 7.0–7.4 / 8.0–8.5 catalogs sequentially in disposable Docker containers.

Only images pulled by this run are removed after each version. Existing images,
containers, volumes and build caches are preserved. No Docker prune is used.
"""

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import uuid

ROOT = Path(__file__).resolve().parent.parent
VERSIONS = ["7.0", "7.1", "7.2", "7.3", "7.4", "8.0", "8.1", "8.2", "8.3", "8.4", "8.5"]


def command(*args, capture=False, check=True, timeout=600):
    return subprocess.run(args, check=check, text=True, stdout=subprocess.PIPE if capture else None,
                          stderr=subprocess.PIPE if capture else None, timeout=timeout)


def validate(document, version):
    """Reject incomplete or incorrectly versioned generator output before replacing a catalog."""
    if document.get("schema_version") != 1 or document.get("version") != version:
        raise ValueError(f"{version}: generator returned the wrong schema/version")
    entities = document["entities"]
    kinds = {entity["kind"] for entity in entities}
    if len(entities) < 1000 or not {"function", "class", "interface", "constant", "keyword"} <= kinds:
        raise ValueError(f"{version}: incomplete runtime inventory")
    functions = {entity["name"] for entity in entities if entity["kind"] == "function"}
    keywords = {entity["name"] for entity in entities if entity["kind"] == "keyword"}
    if ("str_contains" in functions) != (version.startswith("8.")):
        raise ValueError(f"{version}: str_contains version boundary is incorrect")
    for keyword, since in [("fn", "7.4"), ("match", "8.0"), ("readonly", "8.1")]:
        if (keyword in keywords) != (tuple(map(int, version.split('.'))) >= tuple(map(int, since.split('.')))):
            raise ValueError(f"{version}: {keyword} version boundary is incorrect")
    return entities


def generate(docker, output, version, keep_images):
    image = f"php:{version}-cli-alpine"
    container = f"nozhik-php-catalog-{version.replace('.', '-')}-{uuid.uuid4().hex[:8]}"
    existed = command(docker, "image", "inspect", image, capture=True, check=False).returncode == 0
    print(f"PHP {version}: {image}, one disposable container", flush=True)
    try:
        if not existed:
            command(docker, "pull", "--platform", "linux/amd64", image)
        info = json.loads(command(docker, "image", "inspect", image, capture=True).stdout)[0]
        if info.get("Os") != "linux" or info.get("Architecture") != "amd64":
            raise ValueError(f"{image}: expected linux/amd64; existing image is preserved")
        with tempfile.TemporaryDirectory(prefix="nozhik-php-catalog-") as temporary:
            directory = Path(temporary)
            # A snapshot prevents edits to the generator during a run from affecting its provenance.
            generator = (ROOT / "scripts/generate_php_stubs.php").read_bytes()
            (directory / "generate.php").write_bytes(generator)
            command(docker, "run", "--rm", "--pull=never", "--name", container, "--platform", "linux/amd64",
                    "--network", "none", "-v", f"{directory}:/output", image,
                    "php", "/output/generate.php", f"--output=/output/php_{version}.json", timeout=120)
            source = directory / f"php_{version}.json"
            payload = source.read_bytes()
            document = json.loads(payload)
            entities = validate(document, version)
            constants = {entity['name']: entity.get('value') for entity in entities if entity['kind'] == 'constant'}
            metadata = {
                "image": image,
                "image_id": info["Id"],
                "repo_digests": info.get("RepoDigests", []),
                "platform": "linux/amd64",
                "php_version": constants["PHP_VERSION"].strip("'"),
                "extensions": document["extensions"],
                "entities": len(entities),
                "methods": sum(len(entity.get("methods", [])) for entity in entities),
                "class_constants": sum(len(entity.get("constants", [])) for entity in entities),
                "sha256": hashlib.sha256(payload).hexdigest(),
                "generator_sha256": hashlib.sha256(generator).hexdigest(),
            }
            output.mkdir(parents=True, exist_ok=True)
            destination = output / source.name
            staged = destination.with_suffix(".json.tmp")
            staged.write_bytes(payload)
            staged.replace(destination)
            print(f"PHP {version}: validated {len(entities)} entities -> {destination}", flush=True)
            return metadata
    finally:
        # --rm removes successful containers; this also handles generator errors/timeouts.
        remaining = command(docker, "container", "inspect", container, capture=True, check=False)
        if remaining.returncode == 0:
            command(docker, "rm", "-f", container, timeout=60)
        if not existed and not keep_images:
            # Remove only this run's newly pulled tag and its unused layers, never global caches.
            remaining = command(docker, "image", "inspect", image, capture=True, check=False)
            if remaining.returncode == 0:
                command(docker, "image", "rm", image, timeout=120)
        image_status = "pre-existing image preserved" if existed else ("new image retained" if keep_images else "new image removed")
        print(f"PHP {version}: container cleaned; {image_status}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", action="append", choices=VERSIONS, help="Generate selected versions (default: all)")
    parser.add_argument("--docker", default=shutil.which("docker") or str(Path.home() / ".orbstack/bin/docker"))
    parser.add_argument("--output", type=Path, default=ROOT / "crates/language_support/data")
    parser.add_argument("--keep-images", action="store_true", help="Keep images pulled by this run")
    args = parser.parse_args()
    versions = sorted(set(args.version or VERSIONS), key=lambda v: tuple(map(int, v.split('.'))))
    manifest_path = args.output / "catalog-manifest.json"
    manifest = json.loads(manifest_path.read_text()) if manifest_path.exists() else {"schema_version": 1, "versions": {}}
    for version in versions:
        manifest["versions"][version] = generate(args.docker, args.output, version, args.keep_images)
        staged = manifest_path.with_suffix(".json.tmp")
        staged.write_text(json.dumps(manifest, indent=2) + "\n")
        staged.replace(manifest_path)
    print(f"Generated {len(versions)} catalog(s); provenance: {manifest_path}", flush=True)


if __name__ == "__main__":
    main()
