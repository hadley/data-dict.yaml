#!/usr/bin/env python3
"""Repack dist's release archives into PyPI wheels.

Each wheel carries the prebuilt `data-dict` binary as a wheel script, so
installers put it straight on the PATH and there is no Python code involved.

Usage:
    make_wheels.py --version 0.1.0 --artifacts <dir> --out <dir>
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import io
import posixpath
import re
import struct
import sys
import tarfile
import zipfile
from pathlib import Path

NAME = "data-dict"
SUMMARY = "Command-line tool for data-dict.yaml"
HOMEPAGE = "https://data-dict.tidyverse.org"
REPOSITORY = "https://github.com/tidyverse/data-dict"
ASSET_PREFIX = "data-dict-cli"

CLASSIFIERS = [
    "Development Status :: 3 - Alpha",
    "Environment :: Console",
    "Intended Audience :: Developers",
    "Intended Audience :: Science/Research",
    "License :: OSI Approved :: MIT License",
    "Operating System :: MacOS",
    "Operating System :: Microsoft :: Windows",
    "Operating System :: POSIX :: Linux",
    "Programming Language :: Rust",
    "Topic :: Database",
    "Topic :: Software Development :: Quality Assurance",
]

# The Linux binaries are statically linked against musl, so one wheel serves
# both glibc and musl distros. `check_static` enforces that before we claim it.
TARGETS = {
    "aarch64-apple-darwin": ["macosx_11_0_arm64"],
    "x86_64-apple-darwin": ["macosx_10_12_x86_64"],
    "aarch64-unknown-linux-musl": [
        "manylinux_2_17_aarch64",
        "manylinux2014_aarch64",
        "musllinux_1_1_aarch64",
        "musllinux_1_2_aarch64",
    ],
    "x86_64-unknown-linux-musl": [
        "manylinux_2_17_x86_64",
        "manylinux2014_x86_64",
        "musllinux_1_1_x86_64",
        "musllinux_1_2_x86_64",
    ],
    "x86_64-pc-windows-msvc": ["win_amd64"],
}

ZIP_TIMESTAMP = (1980, 1, 1, 0, 0, 0)


class Error(Exception):
    pass


def is_windows(target: str) -> bool:
    return "windows" in target


def binary_name(target: str) -> str:
    return "data-dict.exe" if is_windows(target) else "data-dict"


def asset_name(target: str) -> str:
    extension = "zip" if is_windows(target) else "tar.xz"
    return f"{ASSET_PREFIX}-{target}.{extension}"


def pep440_version(tag: str) -> str:
    """Convert a release tag to a PEP 440 version, or fail.

    Accepts `v`-prefixed tags and the SemVer prerelease spellings dist can
    produce (`1.2.3-rc.1`, `1.2.3-alpha1`), which PyPI would reject verbatim.
    """
    version = tag[1:] if tag.startswith("v") else tag
    match = re.fullmatch(
        r"(?P<release>[0-9]+(?:\.[0-9]+)*)"
        r"(?:-?(?P<pre_kind>a|b|rc|alpha|beta)\.?(?P<pre_num>[0-9]+)?)?"
        r"(?:\.post\.?(?P<post>[0-9]+))?",
        version,
    )
    if match is None:
        raise Error(f"cannot map release tag {tag!r} to a PEP 440 version")

    out = match["release"]
    if match["pre_kind"]:
        kind = {"alpha": "a", "beta": "b"}.get(match["pre_kind"], match["pre_kind"])
        out += kind + (match["pre_num"] or "0")
    if match["post"]:
        out += ".post" + match["post"]
    return out


def read_binary(archive: Path, target: str) -> bytes:
    """Pull the single `data-dict` executable out of a release archive.

    The tarballs nest it under a target-named directory while the zip keeps it
    at the root, so search recursively and insist on exactly one match.
    """
    wanted = binary_name(target)
    found: list[tuple[str, bytes]] = []

    if archive.suffix == ".zip":
        with zipfile.ZipFile(archive) as zf:
            for info in zf.infolist():
                if not info.is_dir() and posixpath.basename(info.filename) == wanted:
                    found.append((info.filename, zf.read(info)))
    else:
        with tarfile.open(archive, "r:xz") as tf:
            for member in tf.getmembers():
                if member.isfile() and posixpath.basename(member.name) == wanted:
                    stream = tf.extractfile(member)
                    assert stream is not None
                    found.append((member.name, stream.read()))

    if len(found) != 1:
        names = ", ".join(sorted(name for name, _ in found)) or "none"
        raise Error(
            f"expected exactly one {wanted} in {archive.name}, found {len(found)}: {names}"
        )
    return found[0][1]


def check_static(data: bytes, target: str) -> None:
    """Fail unless an ELF binary has no interpreter and no shared libraries.

    The Linux wheels claim manylinux *and* musllinux tags, which is only sound
    for a fully static binary. Parsed here rather than shelled out to readelf
    so the check also runs on the macOS machines that build wheels by hand.
    """
    if data[:4] != b"\x7fELF":
        raise Error(f"{target}: not an ELF binary")
    if data[4] != 2 or data[5] != 1:
        raise Error(f"{target}: expected a little-endian 64-bit ELF binary")

    (e_phoff,) = struct.unpack_from("<Q", data, 0x20)
    e_phentsize, e_phnum = struct.unpack_from("<HH", data, 0x36)

    for index in range(e_phnum):
        offset = e_phoff + index * e_phentsize
        p_type, _ = struct.unpack_from("<II", data, offset)
        p_offset, _, _, p_filesz = struct.unpack_from("<QQQQ", data, offset + 8)

        if p_type == 3:  # PT_INTERP
            raise Error(f"{target}: binary is dynamically linked (has PT_INTERP)")
        if p_type == 2:  # PT_DYNAMIC
            for entry in range(p_filesz // 16):
                d_tag, _ = struct.unpack_from("<qQ", data, p_offset + entry * 16)
                if d_tag == 0:  # DT_NULL
                    break
                if d_tag == 1:  # DT_NEEDED
                    raise Error(
                        f"{target}: binary needs shared libraries (has DT_NEEDED)"
                    )


def metadata(version: str, description: str) -> str:
    lines = [
        "Metadata-Version: 2.1",
        f"Name: {NAME}",
        f"Version: {version}",
        f"Summary: {SUMMARY}",
        "License: MIT",
        f"Project-URL: Homepage, {HOMEPAGE}",
        f"Project-URL: Repository, {REPOSITORY}",
        f"Project-URL: Specification, {HOMEPAGE}/spec.html",
        *(f"Classifier: {classifier}" for classifier in CLASSIFIERS),
        "Requires-Python: >=3.8",
        "Description-Content-Type: text/markdown",
        "",
        description,
    ]
    return "\n".join(lines)


def record_hash(data: bytes) -> str:
    digest = base64.urlsafe_b64encode(hashlib.sha256(data).digest())
    return "sha256=" + digest.rstrip(b"=").decode("ascii")


def wheel_filename(version: str, tags: list[str]) -> str:
    escaped = re.sub(r"[-_.]+", "_", NAME)
    return f"{escaped}-{version}-py3-none-{'.'.join(tags)}.whl"


def build_wheel(
    out_dir: Path,
    version: str,
    target: str,
    binary: bytes,
    description: str,
) -> Path:
    tags = TARGETS[target]
    escaped = re.sub(r"[-_.]+", "_", NAME)
    dist_info = f"{escaped}-{version}.dist-info"
    scripts = f"{escaped}-{version}.data/scripts"

    wheel = "\n".join(
        [
            "Wheel-Version: 1.0",
            "Generator: data-dict make_wheels.py",
            "Root-Is-Purelib: false",
            *(f"Tag: py3-none-{tag}" for tag in tags),
            "",
        ]
    )

    payload = [
        (f"{scripts}/{binary_name(target)}", binary, 0o755),
        (f"{dist_info}/METADATA", metadata(version, description).encode(), 0o644),
        (f"{dist_info}/WHEEL", wheel.encode(), 0o644),
    ]

    record = io.StringIO()
    for path, data, _ in payload:
        record.write(f"{path},{record_hash(data)},{len(data)}\n")
    record.write(f"{dist_info}/RECORD,,\n")
    payload.append((f"{dist_info}/RECORD", record.getvalue().encode(), 0o644))

    out_dir.mkdir(parents=True, exist_ok=True)
    path = out_dir / wheel_filename(version, tags)
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as zf:
        for name, data, mode in payload:
            info = zipfile.ZipInfo(name, date_time=ZIP_TIMESTAMP)
            info.external_attr = mode << 16
            info.create_system = 3  # Unix, so the mode above is honoured
            info.compress_type = zipfile.ZIP_DEFLATED
            zf.writestr(info, data)
    return path


def make_wheels(version: str, artifacts: Path, out_dir: Path, description: str):
    built = []
    for target in TARGETS:
        archive = artifacts / asset_name(target)
        if not archive.is_file():
            raise Error(f"missing release archive {archive}")
        binary = read_binary(archive, target)
        if "linux" in target:
            check_static(binary, target)
        built.append(build_wheel(out_dir, version, target, binary, description))
    return built


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True, help="release tag or version")
    parser.add_argument(
        "--artifacts",
        required=True,
        type=Path,
        help="directory holding the release archives",
    )
    parser.add_argument("--out", required=True, type=Path, help="where to write wheels")
    parser.add_argument(
        "--description",
        type=Path,
        default=Path(__file__).parent / "DESCRIPTION.md",
        help="markdown file used as the PyPI long description",
    )
    args = parser.parse_args(argv)

    try:
        version = pep440_version(args.version)
        description = args.description.read_text(encoding="utf-8")
        for wheel in make_wheels(version, args.artifacts, args.out, description):
            print(wheel)
    except Error as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
