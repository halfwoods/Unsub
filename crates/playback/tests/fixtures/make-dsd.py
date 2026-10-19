#!/usr/bin/env python3
"""Writes sine-dsd64.dsf and sine-dsd64.dff: 50 ms of a 440 Hz sine as DSD64 stereo.

Left is at amplitude 0.5 and right at 0.25 so tests can tell the channels apart. The bitstream
comes from a second-order sigma-delta modulator, and both containers carry the same bits.
"""
import math
import struct

RATE = 2_822_400
SECONDS = 0.05
FREQ = 440.0
AMPLITUDES = (0.5, 0.25)
BLOCK = 4096  # bytes per channel in a DSF block


def modulate(amplitude, count):
    """Returns `count` one-bit samples (1 for +1, 0 for -1) of a sine."""
    bits = []
    first = second = 0.0
    step = 2.0 * math.pi * FREQ / RATE
    for n in range(count):
        wanted = amplitude * math.sin(step * n)
        out = 1.0 if second >= 0.0 else -1.0
        first += 0.5 * (wanted - out)
        second += 0.5 * (first - out)
        bits.append(1 if out > 0 else 0)
    return bits


def pack(bits, lsb_first):
    """Packs samples into bytes: first sample in bit 0 (DSF) or in bit 7 (DSDIFF)."""
    out = bytearray()
    for i in range(0, len(bits), 8):
        chunk = bits[i : i + 8]
        value = 0
        for position, bit in enumerate(chunk):
            value |= bit << (position if lsb_first else 7 - position)
        out.append(value)
    return bytes(out)


def dsf(channels_bits, count):
    data = [pack(bits, lsb_first=True) for bits in channels_bits]
    blocks = -(-len(data[0]) // BLOCK)
    padded = [d + bytes(blocks * BLOCK - len(d)) for d in data]
    body = b"".join(
        padded[ch][b * BLOCK : (b + 1) * BLOCK] for b in range(blocks) for ch in range(len(padded))
    )
    fmt = struct.pack(
        "<4sQIIIIIIQII", b"fmt ", 52, 1, 0, 2, len(padded), RATE, 1, count, BLOCK, 0
    )
    data_chunk = struct.pack("<4sQ", b"data", 12 + len(body)) + body
    total = 28 + len(fmt) + len(data_chunk)
    return struct.pack("<4sQQQ", b"DSD ", 28, total, 0) + fmt + data_chunk


def dff(channels_bits):
    data = [pack(bits, lsb_first=False) for bits in channels_bits]
    interleaved = bytes(b for frame in zip(*data) for b in frame)

    def chunk(tag, payload):
        pad = b"\0" if len(payload) % 2 else b""
        return tag + struct.pack(">Q", len(payload)) + payload + pad

    cmpr = b"DSD " + bytes([14]) + b"not compressed"
    prop = b"SND " + chunk(b"FS  ", struct.pack(">I", RATE))
    prop += chunk(b"CHNL", struct.pack(">H", 2) + b"SLFTSRGT")
    prop += chunk(b"CMPR", cmpr)
    body = b"DSD " + chunk(b"FVER", struct.pack(">I", 0x01050000))
    body += chunk(b"PROP", prop) + chunk(b"DSD ", interleaved)
    return b"FRM8" + struct.pack(">Q", len(body)) + body


def main():
    count = int(RATE * SECONDS)
    channels = [modulate(a, count) for a in AMPLITUDES]
    with open("sine-dsd64.dsf", "wb") as f:
        f.write(dsf(channels, count))
    with open("sine-dsd64.dff", "wb") as f:
        f.write(dff(channels))


if __name__ == "__main__":
    main()
