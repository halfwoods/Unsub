#!/usr/bin/env python3
"""Audit bundled Mach-O deployment targets and known newer hard imports.

Runs on any Python 3 host, without executing the application. This is a build
gate, not proof of runtime compatibility with every macOS API or WebKit version.
"""
import argparse
from dataclasses import dataclass
import mmap
from pathlib import Path
import plistlib
import struct
import sys


ARCHITECTURES = {0x01000007: "x86_64", 0x0100000C: "arm64"}
NEW_IMPORTS = {
    "_AudioHardwareCreateProcessTap": (14, 2, 0),
    "_AudioHardwareDestroyProcessTap": (14, 2, 0),
    "_OBJC_CLASS_$_CATapDescription": (14, 2, 0),
    "_SecTrustEvaluateWithError": (10, 14, 0),
    "_SecTrustSetSignedCertificateTimestamps": (10, 14, 2),
}
THIN = {
    b"\xcf\xfa\xed\xfe": ("<", True), b"\xfe\xed\xfa\xcf": (">", True),
    b"\xce\xfa\xed\xfe": ("<", False), b"\xfe\xed\xfa\xce": (">", False),
}
FAT = {
    b"\xca\xfe\xba\xbe": (">", False), b"\xbe\xba\xfe\xca": ("<", False),
    b"\xca\xfe\xba\xbf": (">", True), b"\xbf\xba\xfe\xca": ("<", True),
}


class AuditError(ValueError):
    pass


def version(text):
    parts = text.split(".")
    if not 1 <= len(parts) <= 3 or any(not p.isdigit() for p in parts):
        raise AuditError("invalid macOS version: " + text)
    return tuple(int(p) for p in parts) + (0,) * (3 - len(parts))


def display(value):
    return ".".join(str(p) for p in value)


def packed_version(number):
    return (number >> 16, (number >> 8) & 255, number & 255)


@dataclass(frozen=True)
class Slice:
    architecture: str
    minimum: tuple
    strong_imports: frozenset


def bind_imports(data, begin, end, lazy=False):
    """Read LC_DYLD_INFO bind bytecode (mach-o/loader.h), even without an nlist table.

    Addresses and addends do not affect symbol availability. We still decode every operand
    so unsupported or truncated encodings cannot silently hide a later import.
    """
    strong, symbol, weak = set(), None, False
    offset = begin

    def leb128(signed=False):
        nonlocal offset
        value, shift = 0, 0
        for _ in range(10):
            if offset >= end:
                raise AuditError("truncated dyld bind LEB128 operand")
            byte = data[offset]
            offset += 1
            value |= (byte & 0x7F) << shift
            shift += 7
            if not byte & 0x80:
                if signed and byte & 0x40:
                    value -= 1 << shift
                if not (-(1 << 63) <= value < (1 << 63) if signed else 0 <= value < (1 << 64)):
                    raise AuditError("overflowing dyld bind LEB128 operand")
                return value
        raise AuditError("overflowing dyld bind LEB128 operand")

    def bind():
        if not symbol:
            raise AuditError("dyld bind has no symbol")
        if not weak:
            strong.add(symbol)

    while offset < end:
        byte = data[offset]
        offset += 1
        opcode, immediate = byte & 0xF0, byte & 0x0F
        if opcode in (0x20, 0x60, 0x80, 0x90, 0xA0, 0xC0) and immediate:
            raise AuditError("unsupported dyld bind opcode flags: " + hex(byte))
        if opcode == 0:  # BIND_OPCODE_DONE; lazy streams contain multiple records.
            if immediate:
                raise AuditError("unsupported dyld bind DONE opcode")
            if not lazy:
                if any(data[offset:end]):
                    raise AuditError("non-padding data after dyld bind DONE")
                return strong
            symbol, weak = None, False
            if offset == end:
                return strong
        elif opcode in (0x10, 0x30):  # SET_DYLIB_ORDINAL_IMM / SET_DYLIB_SPECIAL_IMM
            pass
        elif opcode in (0x20, 0x70, 0x80):  # ordinal, segment offset, address offset
            leb128()
        elif opcode == 0x40:  # SET_SYMBOL_TRAILING_FLAGS_IMM
            if immediate & ~0x09:  # WEAK_IMPORT and NON_WEAK_DEFINITION are the known flags.
                raise AuditError("unsupported dyld bind symbol flags")
            stop = data.find(b"\0", offset, end)
            if stop < 0:
                raise AuditError("unterminated dyld bind symbol")
            symbol = bytes(data[offset:stop]).decode("utf-8", errors="replace")
            weak = bool(immediate & 1)
            offset = stop + 1
        elif opcode == 0x50:  # SET_TYPE_IMM
            if immediate not in (1, 2, 3):
                raise AuditError("unsupported dyld bind type")
        elif opcode == 0x60:  # SET_ADDEND_SLEB
            leb128(signed=True)
        elif opcode in (0x90, 0xA0, 0xB0):
            bind()
            if opcode == 0xA0:  # DO_BIND_ADD_ADDR_ULEB
                leb128()
        elif opcode == 0xC0:  # DO_BIND_ULEB_TIMES_SKIPPING_ULEB
            count = leb128()
            leb128()
            if count:
                bind()
        else:
            # In particular, old arm64e THREADED bind opcodes need pointer-chain decoding.
            # Reject rather than claiming a complete audit of an unimplemented format.
            raise AuditError("unsupported dyld bind opcode: " + hex(byte))
    if end > begin:
        raise AuditError("unterminated dyld bind stream")
    return strong


def chained_imports(data, begin, end, endian):
    """Read every chained import record, per mach-o/fixup-chains.h.

    The import table is independent of the pointer-chain encoding and survives symbol stripping.
    Inspecting all records is conservative, including any record not used by a live chain.
    """
    def unpack(fmt, offset):
        if offset < begin or offset + struct.calcsize(endian + fmt) > end:
            raise AuditError("truncated chained import metadata")
        return struct.unpack_from(endian + fmt, data, offset)

    if endian != "<":
        raise AuditError("unsupported big-endian chained imports")
    version_, starts, imports, symbols, count, format_, symbols_format = unpack("7I", begin)
    if version_ != 0:
        raise AuditError("unsupported chained fixups version")
    if symbols_format != 0:
        raise AuditError("unsupported chained symbol encoding (including compressed pools)")
    if format_ not in (1, 2, 3):
        raise AuditError("unsupported chained import format")
    if starts:
        if starts < 28:
            raise AuditError("invalid chained starts offset")
        segments = unpack("I", begin + starts)[0]
        if starts + 4 + 4 * segments > end - begin:
            raise AuditError("truncated chained starts table")
    entry_size = {1: 4, 2: 8, 3: 16}[format_]
    if imports < 28 or imports + count * entry_size > end - begin:
        raise AuditError("invalid chained import-table bounds")
    if symbols < 28 or symbols > end - begin or (count and symbols < imports + count * entry_size):
        raise AuditError("invalid chained symbol-pool bounds")
    strong = set()
    for index in range(count):
        entry = begin + imports + index * entry_size
        if format_ == 3:
            packed, _ = unpack("2Q", entry)
            if (packed >> 17) & 0x7FFF:
                raise AuditError("unsupported chained import reserved bits")
            weak, name = bool(packed & (1 << 16)), packed >> 32
        else:
            packed = unpack("I", entry)[0]
            weak, name = bool(packed & (1 << 8)), packed >> 9
            if format_ == 2:
                unpack("i", entry + 4)  # Validate the addend even though it does not affect the name.
        offset = begin + symbols + name
        if offset >= end:
            raise AuditError("invalid chained import name offset")
        stop = data.find(b"\0", offset, end)
        if stop < 0:
            raise AuditError("unterminated chained import name")
        if stop == offset:
            raise AuditError("empty chained import name")
        if not weak:
            strong.add(bytes(data[offset:stop]).decode("utf-8", errors="replace"))
    return strong


def parse_macho(data):
    """Read load commands and undefined symbols, including every universal slice."""
    def unpack(fmt, offset, end):
        if offset < 0 or offset + struct.calcsize(fmt) > end:
            raise AuditError("truncated Mach-O structure")
        return struct.unpack_from(fmt, data, offset)

    def thin(start, size):
        end = start + size
        magic = bytes(data[start:start + 4])
        if magic not in THIN:
            raise AuditError("universal slice is not Mach-O")
        endian, wide = THIN[magic]
        header_size = 32 if wide else 28
        header = unpack(endian + ("8I" if wide else "7I"), start, end)
        cpu, commands, command_bytes = header[1], header[4], header[5]
        command_end = start + header_size + command_bytes
        if command_end > end or commands > command_bytes // 8:
            raise AuditError("invalid Mach-O load-command bounds")
        minimums, symbols, bindings, chained = [], None, [], None
        has_dyld_info = False
        offset = start + header_size
        for _ in range(commands):
            cmd, length = unpack(endian + "2I", offset, command_end)
            if length < 8 or offset + length > command_end:
                raise AuditError("invalid Mach-O load-command size")
            if cmd == 0x24:  # LC_VERSION_MIN_MACOSX
                minimums.append(packed_version(unpack(endian + "4I", offset, offset + length)[2]))
            elif cmd == 0x32:  # LC_BUILD_VERSION
                fields = unpack(endian + "6I", offset, offset + length)
                if fields[2] != 1:
                    raise AuditError("non-macOS platform in application bundle")
                minimums.append(packed_version(fields[3]))
            elif cmd == 0x2:  # LC_SYMTAB; offsets are relative to this slice
                symbols = unpack(endian + "6I", offset, offset + length)[2:]
            elif cmd in (0x22, 0x80000022):  # LC_DYLD_INFO / LC_DYLD_INFO_ONLY
                fields = unpack(endian + "12I", offset, offset + length)
                has_dyld_info = True
                bindings.extend((fields[index], fields[index + 1], kind)
                                for index, kind in ((4, "normal"), (6, "weak"), (8, "lazy")))
            elif cmd == 0x80000034:  # LC_DYLD_CHAINED_FIXUPS
                if chained is not None:
                    raise AuditError("duplicate chained fixups command")
                chained = unpack(endian + "4I", offset, offset + length)[2:]
            offset += length
        if not minimums or any(v == (0, 0, 0) for v in minimums):
            raise AuditError("missing macOS deployment target")
        if symbols is None and not has_dyld_info and chained is None:
            raise AuditError("missing import metadata; cannot audit stripped image")
        if header[6] & 0x04000000 and not has_dyld_info and chained is None:
            raise AuditError("nlist imports are incomplete but dyld import metadata is missing")
        strong = set()
        if symbols:
            symoff, count, stroff, strsize = symbols
            entry_size = 16 if wide else 12
            if symoff + count * entry_size > size or stroff + strsize > size:
                raise AuditError("invalid Mach-O symbol-table bounds")
            for i in range(count):
                string_index, kind, _, description, _ = unpack(
                    endian + ("IBBHQ" if wide else "IBBHI"), start + symoff + i * entry_size, end)
                # External, undefined, non-debug, non-weak symbols can fail at load time.
                if kind & 0xE0 or kind & 0x0E or not kind & 1 or description & 0x40:
                    continue
                if string_index >= strsize:
                    raise AuditError("invalid Mach-O string-table index")
                begin = start + stroff + string_index
                stop = data.find(b"\0", begin, start + stroff + strsize)
                if stop < 0:
                    raise AuditError("unterminated Mach-O symbol")
                strong.add(bytes(data[begin:stop]).decode("utf-8", errors="replace"))
        for relative, length, kind in bindings:
            if relative + length > size:
                raise AuditError("invalid dyld bind-stream bounds")
            imports = bind_imports(data, start + relative, start + relative + length, lazy=kind == "lazy")
            # The weak-bind stream coalesces definitions; these are not required imports.
            if kind != "weak":
                strong.update(imports)
        if chained is not None:
            relative, length = chained
            if relative + length > size:
                raise AuditError("invalid chained fixups bounds")
            strong.update(chained_imports(data, start + relative, start + relative + length, endian))
        return Slice(ARCHITECTURES.get(cpu, "cpu-" + hex(cpu)), max(minimums), frozenset(strong))

    magic = bytes(data[:4])
    if magic in THIN:
        return [thin(0, len(data))]
    if magic not in FAT:
        raise AuditError("not a Mach-O file")
    endian, wide = FAT[magic]
    count = unpack(endian + "2I", 0, len(data))[1]
    entry_size = 32 if wide else 20
    if not count or count > (len(data) - 8) // entry_size:
        raise AuditError("invalid universal architecture table")
    slices = []
    for index in range(count):
        fields = unpack(endian + ("IIQQII" if wide else "5I"), 8 + index * entry_size, len(data))
        start, size = fields[2:4]
        if start < 8 + count * entry_size or start + size > len(data):
            raise AuditError("invalid universal slice bounds")
        slices.append(thin(start, size))
    if len({s.architecture for s in slices}) != len(slices):
        raise AuditError("duplicate universal architecture")
    return slices


def read_macho(path):
    with path.open("rb") as source:
        if source.read(4) not in {*THIN, *FAT}:
            return None
        with mmap.mmap(source.fileno(), 0, access=mmap.ACCESS_READ) as data:
            return parse_macho(data)


def validate_slices(slices, minimum, required_arches):
    errors = []
    available = {s.architecture for s in slices}
    for arch in sorted(set(required_arches) - available):
        errors.append("missing " + arch + " architecture")
    for item in slices:
        if item.minimum > minimum:
            errors.append(item.architecture + " needs macOS " + display(item.minimum)
                          + ", above declared " + display(minimum))
        for name in sorted(item.strong_imports & NEW_IMPORTS.keys()):
            introduced = NEW_IMPORTS[name]
            if introduced > minimum:
                errors.append(item.architecture + " strongly imports " + name
                              + " (requires macOS " + display(introduced) + ")")
    return errors


def audit_bundle(app, expected_minimum=None, required_arch=None):
    with (app / "Contents/Info.plist").open("rb") as source:
        info = plistlib.load(source)
    minimum = version(info.get("LSMinimumSystemVersion", ""))
    errors = []
    if expected_minimum is not None and minimum != version(expected_minimum):
        errors.append("Info.plist minimum differs from requested " + expected_minimum)
    main = app / "Contents/MacOS" / info["CFBundleExecutable"]
    main_slices = read_macho(main)
    if not main_slices:
        raise AuditError("main executable is not Mach-O")
    arches = {s.architecture for s in main_slices}
    if required_arch:
        arches.add(required_arch)
    report = []
    # Inspect all embedded code, including framework executables without a dylib extension.
    for path in sorted((app / "Contents").rglob("*")):
        if path.is_symlink() or not path.is_file():
            continue
        try:
            slices = read_macho(path)
            if slices is None:
                continue
            relative = str(path.relative_to(app))
            report.append(relative + ": " + ", ".join(s.architecture + " min " + display(s.minimum) for s in slices))
            errors.extend(relative + ": " + error for error in validate_slices(slices, minimum, arches))
        except AuditError as error:
            errors.append(str(path.relative_to(app)) + ": " + str(error))
    return report, errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("app", type=Path)
    parser.add_argument("--minimum", help="require this exact Info.plist minimum")
    parser.add_argument("--arch", choices=sorted(ARCHITECTURES.values()))
    args = parser.parse_args()
    try:
        report, errors = audit_bundle(args.app, args.minimum, args.arch)
    except (OSError, ValueError, KeyError, plistlib.InvalidFileException) as error:
        print("Compatibility audit failed: " + str(error), file=sys.stderr)
        return 1
    for line in report:
        print(line)
    for error in errors:
        print("ERROR: " + error, file=sys.stderr)
    if errors:
        return 1
    print("Deployment targets, architectures and known newer hard imports passed. Runtime testing is still required.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
