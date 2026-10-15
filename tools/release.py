#!/usr/bin/env python3
"""Small, standard-library-only release packager and Homebrew formula generator."""

import argparse
import hashlib
from pathlib import Path
import re
import shutil
from string import Template
import subprocess
import tarfile
import tempfile
import tomllib


ROOT = Path(__file__).resolve().parents[1]
TARGETS = ("aarch64-apple-darwin", "x86_64-apple-darwin")
STABLE_VERSION = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")


def release_version(tag, manifest=ROOT / "Cargo.toml"):
    version = tomllib.loads(manifest.read_text())["package"]["version"]
    if not STABLE_VERSION.fullmatch(version) or tag != f"v{version}":
        raise ValueError(f"release tag {tag!r} must equal stable Cargo version v{version}")
    return version


def archive_name(version, target):
    if target not in TARGETS:
        raise ValueError(f"unsupported release target: {target}")
    return f"editor-pro-v{version}-{target}.tar.gz"


def package(version, target, binary, output):
    binary = binary.resolve(strict=True)
    actual = subprocess.check_output([str(binary), "--version"], text=True).strip()
    if actual != f"editor-pro {version}":
        raise ValueError(f"binary version {actual!r} does not match release {version}")
    name = archive_name(version, target)
    output.mkdir(parents=True, exist_ok=True)
    archive = output / name
    with tempfile.TemporaryDirectory(prefix="editor-pro-release-") as directory:
        stage = Path(directory) / name.removesuffix(".tar.gz")
        stage.mkdir()
        shutil.copy2(binary, stage / "editor-pro")
        (stage / "editor-pro").chmod(0o755)
        (stage / "ep").symlink_to("editor-pro")
        shutil.copy2(ROOT / "README.md", stage / "README.md")
        with tarfile.open(archive, "w:gz") as tar:
            tar.add(stage, arcname=stage.name)
    return archive


def checksums(version, archives):
    # Require every supported architecture before updating the public formula.
    hashes = {}
    for target in TARGETS:
        with (archives / archive_name(version, target)).open("rb") as archive:
            hashes[target] = hashlib.file_digest(archive, "sha256").hexdigest()
    return hashes


def formula(version, repository, archives, local=False):
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
        raise ValueError("repository must be OWNER/REPO")
    hashes = checksums(version, archives)
    base_url = (
        archives.resolve().as_uri() if local
        else f"https://github.com/{repository}/releases/download/v{version}"
    )
    template = Template((ROOT / "packaging/homebrew/editor-pro.rb.in").read_text())
    return template.substitute(
        repository=repository, version=version, base_url=base_url,
        arm_sha256=hashes[TARGETS[0]], intel_sha256=hashes[TARGETS[1]],
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("check-version")
    pack = commands.add_parser("package")
    pack.add_argument("--target", choices=TARGETS, required=True)
    pack.add_argument("--binary", type=Path, required=True)
    pack.add_argument("--output", type=Path, default=Path("dist"))
    prepare = commands.add_parser("prepare")
    prepare.add_argument("--archives", type=Path, default=Path("dist"))
    prepare.add_argument("--repository", required=True)
    prepare.add_argument("--formula", type=Path, required=True)
    prepare.add_argument("--local", action="store_true", help="use local archives for brew testing")
    args = parser.parse_args()
    try:
        version = release_version(args.tag)
        if args.command == "check-version":
            print(version)
        elif args.command == "package":
            print(package(version, args.target, args.binary, args.output))
        else:
            rendered = formula(version, args.repository, args.archives, args.local)
            hashes = checksums(version, args.archives)
            args.formula.parent.mkdir(parents=True, exist_ok=True)
            args.formula.write_text(rendered)
            (args.archives / "SHA256SUMS").write_text("".join(
                f"{hashes[target]}  {archive_name(version, target)}\n" for target in TARGETS
            ))
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"release: {error}\n")


if __name__ == "__main__":
    main()
