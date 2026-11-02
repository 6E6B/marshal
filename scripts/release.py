#!/usr/bin/env python3
"""Validate release versions before spending time on a build."""

import argparse
from pathlib import Path
import re
import tomllib
import xml.etree.ElementTree as ET


def validate(root: Path, tag: str | None = None) -> str:
    with (root / "Cargo.toml").open("rb") as f:
        version = tomllib.load(f)["package"]["version"]
    identifier = r"(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)"
    semver = rf"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-{identifier}(?:\.{identifier})*)?"
    if not re.fullmatch(semver, version):
        raise ValueError("Release version must be SemVer without build metadata")
    if tag is not None and tag != f"v{version}":
        raise ValueError(f"Tag {tag!r} does not match Cargo.toml (v{version})")
    meson = re.search(r"version:\s*'([^']+)'", (root / "meson.build").read_text())
    if meson is None or meson[1] != version:
        raise ValueError("meson.build version must match Cargo.toml")
    with (root / "Cargo.lock").open("rb") as f:
        packages = tomllib.load(f)["package"]
    if not any(p["name"] == "marshal" and p["version"] == version for p in packages):
        raise ValueError("Cargo.lock marshal version must match Cargo.toml")
    release = ET.parse(root / "data/io.github._6E6B.marshal.metainfo.xml").find("releases/release")
    if release is None or release.get("version") != version:
        raise ValueError("The first AppStream release must match Cargo.toml")
    return version


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", help="Existing tag to validate, including the v prefix")
    args = parser.parse_args()
    try:
        print(validate(Path(__file__).resolve().parent.parent, args.tag))
    except (ValueError, KeyError, OSError, ET.ParseError) as error:
        parser.exit(1, f"Release validation failed: {error}\n")
