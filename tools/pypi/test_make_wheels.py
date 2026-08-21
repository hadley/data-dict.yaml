#!/usr/bin/env python3
"""Tests for make_wheels.py: `python3 -m unittest discover -s tools/pypi`."""

from __future__ import annotations

import io
import struct
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path

import make_wheels
from make_wheels import Error

DESCRIPTION = "# data-dict\n\nA test description.\n"


def elf(interp: bool = False, needed: bool = False) -> bytes:
    """A minimal little-endian 64-bit ELF, optionally dynamically linked."""
    phentsize = 56
    phoff = 64
    segments = []
    dynamic = b""

    if interp:
        segments.append((3, 0, 0))  # PT_INTERP
    if needed:
        dynamic = struct.pack("<qQ", 1, 0) + struct.pack("<qQ", 0, 0)  # DT_NEEDED, NULL

    header = bytearray(b"\x7fELF\x02\x01\x01" + bytes(9) + bytes(0x40 - 16))
    struct.pack_into("<Q", header, 0x20, phoff)

    dynamic_offset = phoff + phentsize * (len(segments) + (1 if dynamic else 0))
    if dynamic:
        segments.append((2, dynamic_offset, len(dynamic)))  # PT_DYNAMIC
    struct.pack_into("<HH", header, 0x36, phentsize, len(segments))

    table = b""
    for p_type, p_offset, p_filesz in segments:
        table += struct.pack(
            "<IIQQQQQQ", p_type, 0, p_offset, 0, 0, p_filesz, p_filesz, 8
        )

    return bytes(header) + table + dynamic


def tar_asset(path: Path, target: str, payload: bytes, names=None) -> Path:
    """A release tarball, nesting the binary the way dist's do."""
    names = names or [f"data-dict-cli-{target}/data-dict"]
    archive = path / make_wheels.asset_name(target)
    with tarfile.open(archive, "w:xz") as tf:
        info = tarfile.TarInfo(f"data-dict-cli-{target}/README.md")
        info.size = 3
        tf.addfile(info, io.BytesIO(b"hi\n"))
        for name in names:
            info = tarfile.TarInfo(name)
            info.size = len(payload)
            info.mode = 0o755
            tf.addfile(info, io.BytesIO(payload))
    return archive


def zip_asset(path: Path, target: str, payload: bytes) -> Path:
    """A release zip, keeping the binary at the root the way dist's does."""
    archive = path / make_wheels.asset_name(target)
    with zipfile.ZipFile(archive, "w") as zf:
        zf.writestr("README.md", "hi\n")
        zf.writestr("data-dict.exe", payload)
    return archive


class TestVersion(unittest.TestCase):
    def test_strips_tag_prefix(self):
        self.assertEqual(make_wheels.pep440_version("v0.0.2"), "0.0.2")
        self.assertEqual(make_wheels.pep440_version("0.0.2"), "0.0.2")

    def test_normalizes_prereleases(self):
        self.assertEqual(make_wheels.pep440_version("v0.1.0-rc.1"), "0.1.0rc1")
        self.assertEqual(make_wheels.pep440_version("v0.1.0-alpha.2"), "0.1.0a2")
        self.assertEqual(make_wheels.pep440_version("v0.1.0-beta1"), "0.1.0b1")
        self.assertEqual(make_wheels.pep440_version("v1.2.3-rc"), "1.2.3rc0")

    def test_rejects_unmappable_tags(self):
        for tag in ["v0.1.0-nightly.3", "v0.1.0+build.5", "release-2", ""]:
            with self.assertRaises(Error):
                make_wheels.pep440_version(tag)


class TestStaticCheck(unittest.TestCase):
    def test_accepts_static_binary(self):
        make_wheels.check_static(elf(), "x86_64-unknown-linux-musl")

    def test_rejects_interpreter(self):
        with self.assertRaisesRegex(Error, "PT_INTERP"):
            make_wheels.check_static(elf(interp=True), "x86_64-unknown-linux-musl")

    def test_rejects_shared_libraries(self):
        with self.assertRaisesRegex(Error, "DT_NEEDED"):
            make_wheels.check_static(elf(needed=True), "x86_64-unknown-linux-musl")

    def test_rejects_non_elf(self):
        with self.assertRaisesRegex(Error, "not an ELF binary"):
            make_wheels.check_static(b"MZ\x90\x00", "x86_64-unknown-linux-musl")


class TestReadBinary(unittest.TestCase):
    def setUp(self):
        self.dir = Path(tempfile.mkdtemp())

    def test_finds_nested_binary_in_tarball(self):
        target = "x86_64-unknown-linux-musl"
        archive = tar_asset(self.dir, target, b"payload")
        self.assertEqual(make_wheels.read_binary(archive, target), b"payload")

    def test_finds_flat_binary_in_zip(self):
        target = "x86_64-pc-windows-msvc"
        archive = zip_asset(self.dir, target, b"payload")
        self.assertEqual(make_wheels.read_binary(archive, target), b"payload")

    def test_rejects_ambiguous_archive(self):
        target = "x86_64-unknown-linux-musl"
        archive = tar_asset(
            self.dir, target, b"payload", names=["a/data-dict", "b/data-dict"]
        )
        with self.assertRaisesRegex(Error, "found 2"):
            make_wheels.read_binary(archive, target)

    def test_rejects_archive_without_binary(self):
        target = "x86_64-unknown-linux-musl"
        archive = tar_asset(self.dir, target, b"payload", names=["some/other-tool"])
        with self.assertRaisesRegex(Error, "found 0"):
            make_wheels.read_binary(archive, target)


class TestBuildWheel(unittest.TestCase):
    def setUp(self):
        self.dir = Path(tempfile.mkdtemp())

    def build(self, target: str, binary: bytes = b"binary-bytes") -> Path:
        return make_wheels.build_wheel(
            self.dir, "0.1.0", target, binary, DESCRIPTION
        )

    def test_filenames_carry_the_platform_tags(self):
        cases = {
            "aarch64-apple-darwin": "data_dict-0.1.0-py3-none-macosx_11_0_arm64.whl",
            "x86_64-apple-darwin": "data_dict-0.1.0-py3-none-macosx_10_12_x86_64.whl",
            "x86_64-pc-windows-msvc": "data_dict-0.1.0-py3-none-win_amd64.whl",
            "x86_64-unknown-linux-musl": (
                "data_dict-0.1.0-py3-none-manylinux_2_17_x86_64.manylinux2014_x86_64"
                ".musllinux_1_1_x86_64.musllinux_1_2_x86_64.whl"
            ),
        }
        for target, expected in cases.items():
            self.assertEqual(self.build(target).name, expected)

    def test_contains_only_the_binary_and_metadata(self):
        with zipfile.ZipFile(self.build("x86_64-apple-darwin")) as zf:
            self.assertEqual(
                sorted(zf.namelist()),
                [
                    "data_dict-0.1.0.data/scripts/data-dict",
                    "data_dict-0.1.0.dist-info/METADATA",
                    "data_dict-0.1.0.dist-info/RECORD",
                    "data_dict-0.1.0.dist-info/WHEEL",
                ],
            )

    def test_windows_wheel_keeps_the_exe_extension(self):
        with zipfile.ZipFile(self.build("x86_64-pc-windows-msvc")) as zf:
            self.assertIn("data_dict-0.1.0.data/scripts/data-dict.exe", zf.namelist())

    def test_binary_is_executable(self):
        with zipfile.ZipFile(self.build("x86_64-apple-darwin")) as zf:
            info = zf.getinfo("data_dict-0.1.0.data/scripts/data-dict")
            self.assertEqual(info.external_attr >> 16, 0o755)
            self.assertEqual(info.create_system, 3)
            metadata = zf.getinfo("data_dict-0.1.0.dist-info/METADATA")
            self.assertEqual(metadata.external_attr >> 16, 0o644)

    def test_record_hashes_match_the_payload(self):
        with zipfile.ZipFile(self.build("x86_64-apple-darwin")) as zf:
            record = zf.read("data_dict-0.1.0.dist-info/RECORD").decode()
            entries = dict(
                (line.split(",")[0], line.split(",")[1:])
                for line in record.splitlines()
            )
            self.assertEqual(entries["data_dict-0.1.0.dist-info/RECORD"], ["", ""])
            for path, (digest, size) in entries.items():
                if not digest:
                    continue
                data = zf.read(path)
                self.assertEqual(digest, make_wheels.record_hash(data), path)
                self.assertEqual(int(size), len(data), path)

    def test_wheel_lists_every_tag(self):
        target = "aarch64-unknown-linux-musl"
        with zipfile.ZipFile(self.build(target)) as zf:
            wheel = zf.read("data_dict-0.1.0.dist-info/WHEEL").decode()
        tags = [line for line in wheel.splitlines() if line.startswith("Tag: ")]
        self.assertEqual(
            tags, [f"Tag: py3-none-{tag}" for tag in make_wheels.TARGETS[target]]
        )
        self.assertIn("Root-Is-Purelib: false", wheel)

    def test_metadata_carries_the_description(self):
        with zipfile.ZipFile(self.build("x86_64-apple-darwin")) as zf:
            metadata = zf.read("data_dict-0.1.0.dist-info/METADATA").decode()
        self.assertIn("Name: data-dict", metadata)
        self.assertIn("Version: 0.1.0", metadata)
        self.assertIn("Description-Content-Type: text/markdown", metadata)
        self.assertTrue(metadata.endswith(DESCRIPTION))

    def test_output_is_deterministic(self):
        first = self.build("x86_64-apple-darwin").read_bytes()
        second = self.build("x86_64-apple-darwin").read_bytes()
        self.assertEqual(first, second)


class TestMakeWheels(unittest.TestCase):
    def setUp(self):
        self.artifacts = Path(tempfile.mkdtemp())
        self.out = Path(tempfile.mkdtemp())

    def populate(self, binary: bytes = b"binary-bytes"):
        for target in make_wheels.TARGETS:
            payload = elf() if "linux" in target else binary
            if make_wheels.is_windows(target):
                zip_asset(self.artifacts, target, payload)
            else:
                tar_asset(self.artifacts, target, payload)

    def test_builds_one_wheel_per_target(self):
        self.populate()
        wheels = make_wheels.make_wheels("0.1.0", self.artifacts, self.out, DESCRIPTION)
        self.assertEqual(len(wheels), len(make_wheels.TARGETS))
        self.assertEqual(sorted(p.name for p in self.out.iterdir()),
                         sorted(p.name for p in wheels))

    def test_missing_archive_is_an_error(self):
        self.populate()
        (self.artifacts / make_wheels.asset_name("x86_64-apple-darwin")).unlink()
        with self.assertRaisesRegex(Error, "missing release archive"):
            make_wheels.make_wheels("0.1.0", self.artifacts, self.out, DESCRIPTION)

    def test_dynamically_linked_linux_binary_is_an_error(self):
        self.populate()
        tar_asset(self.artifacts, "x86_64-unknown-linux-musl", elf(interp=True))
        with self.assertRaisesRegex(Error, "dynamically linked"):
            make_wheels.make_wheels("0.1.0", self.artifacts, self.out, DESCRIPTION)


if __name__ == "__main__":
    unittest.main()
