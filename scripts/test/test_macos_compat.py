"""Synthetic binary regressions for the release gate; no Apple tools required."""
import importlib.util
from pathlib import Path
import plistlib
import struct
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "check-macos-compat.py"
SPEC = importlib.util.spec_from_file_location("macos_compat", SCRIPT)
audit = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = audit
SPEC.loader.exec_module(audit)


def binary(minimum=(10, 13, 0), cpu=0x01000007, symbols=(), modern=False, endian="<", wide=True):
    pack = lambda fmt, *values: struct.pack(endian + fmt, *values)
    packed = minimum[0] << 16 | minimum[1] << 8 | minimum[2]
    platform = pack("6I", 0x32, 24, 1, packed, 0x0F0000, 0) if modern else pack("4I", 0x24, 16, packed, 0x0F0000)
    header_size = 32 if wide else 28
    symbol_offset = header_size + len(platform) + 24
    string_offset = symbol_offset + len(symbols) * (16 if wide else 12)
    strings, entries = b"\0", b""
    for name, weak in symbols:
        entries += pack("IBBHQ" if wide else "IBBHI", len(strings), 1, 0, 0x40 if weak else 0, 0)
        strings += name.encode() + b"\0"
    symtab = pack("6I", 2, 24, symbol_offset, len(symbols), string_offset, len(strings))
    header = pack("7I", 0xFEEDFACF if wide else 0xFEEDFACE, cpu, 3, 2, 2, len(platform) + 24, 0)
    if wide:
        header += pack("I", 0)
    return header + platform + symtab + entries + strings


def universal(blobs, wide=False):
    entry_size = 32 if wide else 20
    offset = 8 + entry_size * len(blobs)
    table = struct.pack(">2I", 0xCAFEBABF if wide else 0xCAFEBABE, len(blobs))
    for cpu, blob in blobs:
        table += struct.pack(">IIQQII", cpu, 3, offset, len(blob), 0, 0) if wide else struct.pack(">5I", cpu, 3, offset, len(blob), 0)
        offset += len(blob)
    return table + b"".join(blob for _, blob in blobs)


def bind_stream(symbols):
    return b"\x11\x51\x70\0" + b"".join(
        bytes([0x41 if weak else 0x40]) + name.encode() + b"\0\x90" for name, weak in symbols
    ) + b"\0"


def binding_binary(normal=b"", weak=b"", lazy=b"", empty_symtab=False, endian="<"):
    pack = lambda fmt, *values: struct.pack(endian + fmt, *values)
    commands_size = 16 + 48 + (24 if empty_symtab else 0)
    offset = 32 + commands_size
    streams = []
    payload = b""
    for stream in (normal, weak, lazy):
        streams.extend((offset if stream else 0, len(stream)))
        payload += stream
        offset += len(stream)
    minimum = pack("4I", 0x24, 16, 0x0A0D00, 0x0F0000)
    bindings = pack("12I", 0x80000022, 48, 0, 0, *streams, 0, 0)
    # A present but empty nlist table must not hide imports from dyld metadata.
    symtab = pack("6I", 2, 24, 0, 0, 0, 0) if empty_symtab else b""
    header = pack("8I", 0xFEEDFACF, 0x01000007, 3, 2, 2 + int(empty_symtab), commands_size,
                  0x04000000, 0)  # MH_NLIST_OUTOFSYNC_WITH_DYLDINFO
    return header + minimum + bindings + symtab + payload


def chained_binary(symbols, import_format=1, symbols_format=0, version=0):
    names, records = b"", b""
    for name, weak in symbols:
        if import_format == 3:
            records += struct.pack("<2Q", 1 | (int(weak) << 16) | (len(names) << 32), 0)
        else:
            records += struct.pack("<I", 1 | (int(weak) << 8) | (len(names) << 9))
            if import_format == 2:
                records += struct.pack("<i", -7)
        names += name.encode() + b"\0"
    payload = struct.pack("<7I", version, 28, 32, 32 + len(records), len(symbols), import_format, symbols_format)
    payload += struct.pack("<I", 0) + records + names  # zero segments; only import metadata is relevant here
    minimum = struct.pack("<4I", 0x24, 16, 0x0A0D00, 0x0F0000)
    chained = struct.pack("<4I", 0x80000034, 16, 64, len(payload))
    header = struct.pack("<8I", 0xFEEDFACF, 0x01000007, 3, 2, 2, 32, 0, 0)
    return header + minimum + chained + payload


class MachOAuditTests(unittest.TestCase):
    def test_old_and_new_load_commands_and_byte_orders(self):
        for modern in (False, True):
            for endian in ("<", ">"):
                for wide in (False, True):
                    with self.subTest(modern=modern, endian=endian, wide=wide):
                        slices = audit.parse_macho(binary(modern=modern, endian=endian, wide=wide))
                        self.assertEqual(slices[0].minimum, (10, 13, 0))
                        self.assertEqual(audit.validate_slices(slices, (10, 13, 0), ["x86_64"]), [])

    def test_new_hard_import_fails_but_weak_import_is_allowed(self):
        for name, minimum in audit.NEW_IMPORTS.items():
            with self.subTest(name=name):
                strong = audit.parse_macho(binary(symbols=[(name, False)]))
                weak = audit.parse_macho(binary(symbols=[(name, True)]))
                self.assertIn(name, audit.validate_slices(strong, (10, 13, 0), ["x86_64"])[0])
                self.assertEqual(audit.validate_slices(weak, (10, 13, 0), ["x86_64"]), [])
                self.assertEqual(audit.validate_slices(strong, minimum, ["x86_64"]), [])

    def test_bind_imports_survive_absent_or_incomplete_symbol_table(self):
        for empty_symtab in (False, True):
            for endian in ("<", ">"):
                with self.subTest(empty_symtab=empty_symtab, endian=endian):
                    data = binding_binary(bind_stream([
                        ("_SecTrustEvaluateWithError", False), ("_AudioHardwareCreateProcessTap", True),
                    ]), empty_symtab=empty_symtab, endian=endian)
                    slices = audit.parse_macho(data)
                    self.assertEqual(slices[0].strong_imports, frozenset({"_SecTrustEvaluateWithError"}))
                    self.assertIn("SecTrust", audit.validate_slices(slices, (10, 13, 0), ["x86_64"])[0])

    def test_lazy_records_and_weak_coalescing_definitions(self):
        data = binding_binary(
            weak=bind_stream([("_AudioHardwareCreateProcessTap", False)]),
            lazy=bind_stream([("_malloc", False)]) + bind_stream([("_SecTrustEvaluateWithError", False)]),
        )
        self.assertEqual(audit.parse_macho(data)[0].strong_imports,
                         frozenset({"_malloc", "_SecTrustEvaluateWithError"}))

    def test_binding_operands_are_consumed_without_skipping_later_symbols(self):
        stream = (b"\x20\x81\x01\x3e\x51\x70\x80\x01\x60\x7f\x80\x80\x01"
                  b"\x40_malloc\0\xa0\x08\xb2\xc0\x02\x08"
                  b"\x40_SecTrustEvaluateWithError\0\x90\0")
        self.assertEqual(audit.parse_macho(binding_binary(stream))[0].strong_imports,
                         frozenset({"_malloc", "_SecTrustEvaluateWithError"}))

    def test_bad_bind_streams_fail_closed(self):
        streams = [b"\xe0", b"\xd0\0", b"\x40unterminated", b"\x90\0", b"\x20\x80",
                   b"\x20" + b"\x81" * 11, b"\x40_name\0\x90", b"\0\x90",
                   b"\x44_name\0\x90\0", b"\x54\0", b"\x91\0"]
        for stream in streams:
            with self.subTest(stream=stream), self.assertRaises(audit.AuditError):
                audit.parse_macho(binding_binary(stream))

    def test_stripped_chained_import_formats_preserve_weakness(self):
        for format_ in (1, 2, 3):
            with self.subTest(format=format_):
                data = chained_binary([
                    ("_SecTrustEvaluateWithError", False), ("_AudioHardwareCreateProcessTap", True),
                ], import_format=format_)
                slices = audit.parse_macho(data)
                self.assertEqual(slices[0].strong_imports, frozenset({"_SecTrustEvaluateWithError"}))
                self.assertIn("SecTrust", audit.validate_slices(slices, (10, 13, 0), ["x86_64"])[0])
                # Chained offsets are relative to their slice, including in fat binaries.
                self.assertEqual(audit.parse_macho(universal([(0x01000007, data)])), slices)

    def test_bad_chained_metadata_fails_closed(self):
        for field, value in ((0, 1), (2, 1), (3, 1), (4, 10000), (5, 4), (6, 1)):
            data = bytearray(chained_binary([("_malloc", False)]))
            struct.pack_into("<I", data, 64 + 4 * field, value)
            with self.subTest(field=field), self.assertRaises(audit.AuditError):
                audit.parse_macho(data)
        invalid_name = bytearray(chained_binary([("_malloc", False)]))
        struct.pack_into("<I", invalid_name, 64 + 32, 0xFFFFFE00)
        with self.assertRaisesRegex(audit.AuditError, "name offset"):
            audit.parse_macho(invalid_name)
        missing_terminator = bytearray(chained_binary([("_malloc", False)]))
        missing_terminator[-1] = ord("x")
        with self.assertRaisesRegex(audit.AuditError, "unterminated"):
            audit.parse_macho(missing_terminator)
        for length in (64, 70, 95, 99):
            with self.subTest(length=length), self.assertRaises(audit.AuditError):
                audit.parse_macho(chained_binary([("_malloc", False)])[:length])

    def test_missing_import_metadata_fails_closed(self):
        data = bytearray(binary())
        struct.pack_into("<I", data, 48, 0x99)  # remove the only import metadata command
        with self.assertRaisesRegex(audit.AuditError, "missing import metadata"):
            audit.parse_macho(data)
        data = bytearray(binary())
        struct.pack_into("<I", data, 24, 0x04000000)  # nlist explicitly says it is incomplete
        with self.assertRaisesRegex(audit.AuditError, "incomplete"):
            audit.parse_macho(data)

    def test_every_universal_slice_and_required_architecture_checked(self):
        for wide in (False, True):
            slices = audit.parse_macho(universal([
                (0x01000007, binary()),
                (0x0100000C, binary(minimum=(11, 0, 0), cpu=0x0100000C)),
            ], wide=wide))
            self.assertEqual(len(slices), 2)
            self.assertIn("arm64 needs macOS 11.0.0", audit.validate_slices(slices, (10, 13, 0), ["x86_64"])[0])
        self.assertIn("missing arm64", audit.validate_slices(audit.parse_macho(binary()), (11, 0, 0), ["arm64"])[0])

    def test_truncated_and_missing_target_fail_closed(self):
        data = binary()
        for length in (0, 4, 20, 33, len(data) - 1):
            with self.subTest(length=length), self.assertRaises(audit.AuditError):
                audit.parse_macho(data[:length])
        missing = bytearray(data)
        struct.pack_into("<I", missing, 32, 0x99)  # unrecognized rather than a deployment command
        with self.assertRaisesRegex(audit.AuditError, "missing macOS"):
            audit.parse_macho(missing)

    def test_bundle_checks_main_and_nested_frameworks_and_plist(self):
        with tempfile.TemporaryDirectory() as directory:
            app = Path(directory) / "Test.app"
            contents = app / "Contents"
            (contents / "MacOS").mkdir(parents=True)
            framework = contents / "Frameworks/Example.framework/Versions/A"
            framework.mkdir(parents=True)
            (contents / "Info.plist").write_bytes(plistlib.dumps({
                "CFBundleExecutable": "Test", "LSMinimumSystemVersion": "10.13",
            }))
            (contents / "MacOS/Test").write_bytes(binary())
            (framework / "Example").write_bytes(binary())
            report, errors = audit.audit_bundle(app, "10.13", "x86_64")
            self.assertEqual(len(report), 2)
            self.assertEqual(errors, [])
            (contents / "MacOS/Test").write_bytes(binary(minimum=(11, 0, 0)))
            (framework / "Example").write_bytes(binary(symbols=[("_SecTrustEvaluateWithError", False)]))
            _, errors = audit.audit_bundle(app, "10.13", "x86_64")
            self.assertEqual(len(errors), 2)
            self.assertTrue(any("MacOS/Test" in error and "11.0.0" in error for error in errors))
            self.assertTrue(any("Example.framework" in error and "SecTrust" in error for error in errors))
            self.assertIn("Info.plist", audit.audit_bundle(app, "11.0")[1][0])


if __name__ == "__main__":
    unittest.main()
