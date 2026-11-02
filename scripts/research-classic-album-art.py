#!/usr/bin/env python3
"""Reproduce bounded Classic 2.0.4 native artwork checks on synthetic memory.

Requires Unicorn 2.1.4 in a separate research Python environment. Accepts only
an explicitly supplied, fingerprinted plaintext image; downloads and device I/O
are absent. No firmware bytes are bundled. This does not boot the OS or render
its screens, and does not establish behavior of another firmware version.

Image source:
https://github.com/iEMU-Android/iPod_35.2.0.4/blob/3747438c43d8a29b12d255c11305f1ba17535e7a/osos.fw.decrypted
Address annotations and relocation profile:
https://github.com/lgreitis/osos-lab/blob/d2c8e357d58591bdaa70bb6be9da191f489554a6/ghidra/profiles/classic7g-2.0.4.json
The image differs from that profile's expected whole-file hash. Runtime startup
constants are checked independently; annotations remain qualified by this fact.
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import sys

try:
    import unicorn
    from unicorn import arm_const as arm
except ImportError:
    unicorn = None
    arm = None


FIRMWARE_SHA256 = "f4368251a58b2fdc7b46acf3178dae1d24bc1e029736240741015851256c65c4"
FIRMWARE_SIZE = 10_599_920
PROFILE_SHA256 = "5841de6479f3a655102745fdb20e153ef99c4ef413314ccdb9a4721cace9a36e"
STARTUP_CONSTANTS = {
    0x4F88: 0x22000000, 0x4F8C: 0xAED8, 0x4F90: 0x2200AED8,
    0x4F94: 0x589C, 0x4FA0: 0x08000000, 0x4FA4: 0xA0FC88,
    0x4FA8: 0x08A0FC88, 0x4FAC: 0xA84, 0x5034: 0x08A1070C,
    0x5038: 0x12244C,
}
# Runtime address, byte count, source offset. None denotes zero-initialized BSS.
REGIONS = (
    (".dram", 0x08000000, 0xA0FC88, 0xB6D8),
    (".dram.data", 0x08A0FC88, 0xA84, 0xA1B360),
    (".dram.bss", 0x08A1070C, 0x12244C, None),
    (".iram", 0x22000000, 0xAED8, 0x800),
    (".iram.bss", 0x2200AED8, 0x589C, None),
)
HEAP = 0x10000000
STACK = 0x70000000
RETURN = 0x0F000000
BROWSER = HEAP + 0x1000
DATABASE = HEAP + 0x3000
VECTOR = HEAP + 0x10000
TRACKS = (HEAP + 0x20000, HEAP + 0x21000)
TRACK_DATA = (HEAP + 0x30000, HEAP + 0x31000)
IMAGES = (HEAP + 0x40000, HEAP + 0x40100, HEAP + 0x40200)
CACHE = 0x089CA58C
ACCUMULATE = 0x080BF474
GET_ALBUM = 0x08047644
GET_RECORD = 0x080464BC
RESOLVE_TRACK = 0x080457E4
FIND_ID = 0x08045540
FIND_OWNER = 0x08045674
DISPATCH = 0x08058E8C
FUNCTIONS = {
    ACCUMULATE: "AccumulateAlbumGroup", GET_ALBUM: "GetAlbumArtworkId",
    GET_RECORD: "GetRecordArtworkId", RESOLVE_TRACK: "ResolveTrackArtwork",
    FIND_ID: "FindById", FIND_OWNER: "FindByOwner",
}
DISPATCH_TARGETS = {
    0x08068F64: "skip-bytes", 0x0808A3EC: "track-import",
    0x08089700: "playlist-import",
}


class ProofError(ValueError):
    pass


def check(condition, message):
    # Proof checks must remain active under python -O.
    if not condition:
        raise ProofError(message)


def load_firmware(path):
    check(path.is_file(), "--firmware must name a regular plaintext firmware file")
    with path.open("rb") as source:
        image = source.read(FIRMWARE_SIZE + 1)
    check(len(image) == FIRMWARE_SIZE, f"expected {FIRMWARE_SIZE} firmware bytes")
    digest = hashlib.sha256(image).hexdigest()
    check(digest == FIRMWARE_SHA256, f"firmware SHA-256 mismatch: {digest}")
    for offset, expected in STARTUP_CONSTANTS.items():
        actual = struct.unpack_from("<I", image, offset)[0]
        check(actual == expected, f"startup constant mismatch at {offset:#x}")
    return image


class Machine:
    def __init__(self, image):
        self.mu = unicorn.Uc(unicorn.UC_ARCH_ARM, unicorn.UC_MODE_ARM)
        self.mu.ctl_set_cpu_model(arm.UC_CPU_ARM_926)
        for prefix in (".dram", ".iram"):
            group = [row for row in REGIONS if row[0].startswith(prefix)]
            start = min(row[1] for row in group) & ~0xFFF
            end = (max(row[1] + row[2] for row in group) + 0xFFF) & ~0xFFF
            self.mu.mem_map(start, end - start)
        for _, address, size, offset in REGIONS:
            if offset is not None:
                self.mu.mem_write(address, image[offset:offset + size])
        self.mu.mem_map(HEAP, 0x100000)
        self.mu.mem_map(STACK, 0x10000)
        self.mu.mem_map(RETURN, 0x1000)
        self.mu.hook_add(unicorn.UC_HOOK_CODE, self.on_code)
        self.mu.hook_add(unicorn.UC_HOOK_MEM_INVALID, self.on_bad_memory)
        self.mu.hook_add(unicorn.UC_HOOK_MEM_READ, self.on_read)
        self.put(BROWSER, 1)  # Native GetOwner accepts kind 1 as its own owner.
        self.put(BROWSER + 0xEB0, VECTOR)
        self.put(BROWSER + 0xEB4, VECTOR)
        self.put(BROWSER + 0xEB8, VECTOR + 24 * 16)
        self.put(BROWSER + 0xF9C, DATABASE)
        self.put(DATABASE, 0x6974696C)
        self.mu.mem_write(DATABASE + 6, b"\x01")  # Linked-list lookup, no lazy index.
        self.mu.mem_write(CACHE, bytes(32))
        for track, data in zip(TRACKS, TRACK_DATA):
            self.put(track, data)
            self.put(data + 4, 0x74646174)  # Checked by native TrackData_IsValid.
            self.put(track + 0x2C, 0x2222)
            self.put(track + 0x30, 0x3333)
            self.put(track + 0x34, 0x1111)

    def put(self, address, value):
        self.mu.mem_write(address, struct.pack("<I", value & 0xFFFFFFFF))

    def get(self, address):
        return struct.unpack("<I", self.mu.mem_read(address, 4))[0]

    def owner(self, track, pid):
        self.put(track + 0x110, pid & 0xFFFFFFFF)
        self.put(track + 0x114, pid >> 32)

    def images(self, items):
        self.put(DATABASE + 0xC, IMAGES[0] if items else 0)
        for index, (image_id, pid) in enumerate(items):
            address = IMAGES[index]
            self.put(address, IMAGES[index + 1] if index + 1 < len(items) else 0)
            self.put(address + 0xC, image_id)
            self.put(address + 0x10, pid & 0xFFFFFFFF)
            self.put(address + 0x14, pid >> 32)

    def on_code(self, machine, address, size, context):
        if address in self.stop_targets:
            self.arrival = {
                "callee": self.stop_targets[address], "address": f"{address:08x}",
                "r0": machine.reg_read(arm.UC_ARM_REG_R0),
                "r1": machine.reg_read(arm.UC_ARM_REG_R1),
            }
            machine.emu_stop()  # Callee arrival is observed; its code is not executed.
            return
        code_region = any(
            name in (".dram", ".iram") and start <= address < start + length
            for name, start, length, _ in REGIONS
        )
        check(code_region, f"unexpected code address {address:08x}")
        self.instructions += 1
        self.trace.append(f"{address:08x}")
        if address in FUNCTIONS:
            self.calls.append(FUNCTIONS[address])

    def on_bad_memory(self, machine, access, address, size, value, context):
        raise ProofError(f"unmapped access {access} at {address:08x}, size {size}")

    def on_read(self, machine, access, address, size, value, context):
        self.reads.append(f"{address:08x}")

    def run(self, entry, registers, stack_words=(), stop_targets=None, budget=20_000):
        self.instructions = 0
        self.trace, self.calls, self.reads = [], [], []
        self.arrival = None
        self.stop_targets = stop_targets or {}
        for register in (
            arm.UC_ARM_REG_R0, arm.UC_ARM_REG_R1, arm.UC_ARM_REG_R2,
            arm.UC_ARM_REG_R3, arm.UC_ARM_REG_R4, arm.UC_ARM_REG_R5,
            arm.UC_ARM_REG_R6, arm.UC_ARM_REG_R7, arm.UC_ARM_REG_R8,
            arm.UC_ARM_REG_R9, arm.UC_ARM_REG_R10, arm.UC_ARM_REG_R11,
            arm.UC_ARM_REG_R12,
        ):
            self.mu.reg_write(register, 0)
        stack = STACK + 0xF000
        self.mu.reg_write(arm.UC_ARM_REG_CPSR, 0x13)
        self.mu.reg_write(arm.UC_ARM_REG_SP, stack)
        self.mu.reg_write(arm.UC_ARM_REG_LR, RETURN)
        for register, value in registers.items():
            self.mu.reg_write(register, value)
        for index, value in enumerate(stack_words):
            self.put(stack + 4 * index, value)
        try:
            self.mu.emu_start(entry, RETURN, count=budget)
        except unicorn.UcError as error:
            raise ProofError(f"native execution failed at {self.mu.reg_read(arm.UC_ARM_REG_PC):08x}: {error}") from error
        if self.stop_targets:
            check(self.arrival is not None, "dispatch did not reach an expected callee")
        else:
            check(self.mu.reg_read(arm.UC_ARM_REG_PC) == RETURN, "native instruction budget exhausted")
            check(self.mu.reg_read(arm.UC_ARM_REG_SP) == stack, "native call did not restore stack")
        return {
            "return": self.mu.reg_read(arm.UC_ARM_REG_R0),
            "instructionCount": self.instructions, "calls": self.calls,
        }

    def call(self, entry, args, stack_words=()):
        registers = dict(zip(
            (arm.UC_ARM_REG_R0, arm.UC_ARM_REG_R1, arm.UC_ARM_REG_R2, arm.UC_ARM_REG_R3),
            args,
        ))
        return self.run(entry, registers, stack_words)

    def row(self):
        return list(struct.unpack("<6I", self.mu.mem_read(VECTOR, 24)))


def album_case(image, name, links, image_items, owners, expected):
    machine = Machine(image)
    machine.images(image_items)
    for track, link, pid in zip(TRACKS, links, owners):
        machine.put(track + 0xF0, link)
        machine.owner(track, pid)
    callbacks, rows = [], []
    for index, track in enumerate(TRACKS):
        # r3=1 selects the ordinary include branch; Track and reset are args 7/8.
        callbacks.append(machine.call(
            ACCUMULATE, (BROWSER, 0, 0, 1), (0, 0, track, int(index == 0)),
        ))
        rows.append(machine.row())
    getter = machine.call(GET_ALBUM, (BROWSER, 0))
    invalid = machine.call(GET_ALBUM, (BROWSER, 1))
    check(machine.get(BROWSER + 0xEB4) == VECTOR + 24, "expected exactly one album row")
    check([row[4] for row in rows] == [1, 2], "album track count did not increase")
    check([row[3] for row in rows] == [expected, expected], "album artwork selection changed")
    check(getter["return"] == expected and invalid["return"] == 0, "album getter mismatch")
    return {
        "name": name, "runtimeNumericLinks": links,
        "rowsAfterEachTrack": rows, "callbacks": callbacks,
        "getter": getter, "invalidIndexGetter": invalid,
    }


def resolver_case(image, name, link, image_items, owner, expected, lookup):
    machine = Machine(image)
    machine.images(image_items)
    machine.put(TRACKS[0] + 0xF0, link)
    machine.owner(TRACKS[0], owner)
    result = machine.call(RESOLVE_TRACK, (DATABASE, TRACKS[0]))
    pointer = result["return"]
    selected = machine.get(pointer + 0xC) if pointer else 0
    check(selected == expected, "track resolver returned the wrong artwork")
    check(result["calls"] == ["ResolveTrackArtwork", lookup], "unexpected artwork lookup route")
    return {"name": name, "runtimeNumericLink": link, "resolvedImage": selected, "execution": result}


def dispatch_cases(image):
    results = []
    for kind, seen_playlist in ((1, 0), (2, 0), (3, 0), (4, 0), (5, 0), (4, 1), (2, 1)):
        machine = Machine(image)
        reader = HEAP + 0x80000
        result = machine.run(
            DISPATCH,
            {arm.UC_ARM_REG_R5: BROWSER, arm.UC_ARM_REG_R6: reader, arm.UC_ARM_REG_R8: seen_playlist},
            (0, 96, 512, kind), DISPATCH_TARGETS, budget=100,
        )
        expected = "track-import" if kind == 1 else (
            "playlist-import" if kind in (2, 3) and not seen_playlist else "skip-bytes"
        )
        check(machine.arrival["callee"] == expected, "unexpected MHSD dispatch target")
        if expected == "skip-bytes":
            check(machine.arrival["r0"] == reader and machine.arrival["r1"] == 416, "wrong MHSD payload skip extent")
        results.append({
            "section": kind, "playlistAlreadyRead": bool(seen_playlist),
            **machine.arrival, "instructionCount": result["instructionCount"],
            "trace": machine.trace, "dataReads": machine.reads,
        })
    return results


def prove(image, path):
    one, two = 0x1122334455667788, 0x8877665544332211
    return {
        "firmware": str(path.resolve()), "firmwareSha256": FIRMWARE_SHA256,
        "firmwareBytes": len(image), "unicornVersion": unicorn.__version__,
        "startupConstantsVerified": True,
        "metadataProfileSha256": PROFILE_SHA256, "metadataProfileWholeHashMatches": False,
        "architecture": "ARM926EJ-S / ARMv5 little endian", "semanticStubs": [],
        "limits": [
            "Bounded native functions on synthetic in-memory objects; no full OS boot, device I/O or screen rendering.",
            "No result establishes physical Cover Flow/Now Playing behavior or compatibility with another firmware or modification.",
            "Public plaintext differs from the saved annotation profile's whole-file hash; only the supplied fingerprinted bytes are executed.",
            "The MHSD experiment begins at its dispatch block and stops on callee arrival; importer and skip callees are not executed.",
        ],
        "assumptions": [
            "Native object signatures are synthesized and checked by the native validity routines.",
            "A preallocated 24-byte vector and linked-list ArtworkDB avoid allocation, locking and lazy-index paths.",
            "Album inputs have adjacent identical nonzero album/artist tokens; r3=1 selects the include branch.",
            "Track +0xf0 is an internal numeric artwork link, and +0x110/+0x114 is an internal owner PID; these are not serialized MHIT offsets.",
            "MHSD header size96/total512 and playlist-seen state are synthetic dispatch inputs.",
        ],
        "albumCases": [
            album_case(image, "first_101_second_202", (101, 202), ((101, one), (202, two)), (one, two), 101),
            album_case(image, "first_no_art_second_202", (0, 202), ((202, two),), (one, two), 0),
            album_case(image, "first_missing_link_second_202", (999, 202), ((202, two),), (one, two), 0),
        ],
        "resolverCases": [
            resolver_case(image, "numeric_wins_over_reverse_owner", 101, ((101, two), (202, one)), one, 101, "FindById"),
            resolver_case(image, "numeric_miss_has_no_reverse_fallback", 999, ((202, one),), one, 0, "FindById"),
            resolver_case(image, "zero_numeric_uses_reverse_owner", 0, ((202, one),), one, 202, "FindByOwner"),
            resolver_case(image, "zero_numeric_owner_miss", 0, ((202, two),), one, 0, "FindByOwner"),
        ],
        "dispatchCases": dispatch_cases(image),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--firmware", required=True, type=Path, help="explicit local 2.0.4 plaintext IMG1")
    parser.add_argument("--report", type=Path, help="write JSON to a new file instead of stdout; never overwrite")
    args = parser.parse_args()
    try:
        image = load_firmware(args.firmware)
        check(unicorn is not None, "requires Unicorn 2.1.4 in a separate research Python environment")
        check(unicorn.__version__ == "2.1.4", "this proof requires the validated Unicorn version 2.1.4")
        report = json.dumps(prove(image, args.firmware), indent=2) + "\n"
        if args.report:
            with args.report.open("x", encoding="utf-8") as target:
                target.write(report)
        else:
            sys.stdout.write(report)
    except (OSError, ProofError) as error:
        parser.exit(1, f"error: {error}\n")


if __name__ == "__main__":
    main()
