#!/usr/bin/env python3
"""
Pairs of different event IDs made to have the same 64-bit hash in the fast
pipeline (fast/src/ids.rs as of exp-v5 296b0dd3: `hash33` for IDs of 33 bytes,
`hash9` for IDs of up to 9), each pair on two events of one user at one
instant. By the rules these are two events. A pipeline that takes equal hashes
of the same person at the same time for copies of one event, without a look at
the IDs, counts one.

This is not input the generator can make, and nobody can make it without
reading the hash function: it shows where exactness rests on the hash, no more.

hash33(first, w0..w3) = finish(a ^ rotl(b, 32) ^ rotl(c, 17) ^ rotl(d, 47)) with
a = (w0 ^ first * K0) * K1, b = w1 * K2, ...: every word goes through a
multiplication by an odd number, which can be undone, so for any other w1 there
is a w0 that gives the same sum. hash9(first, rest) = mix9(rest ^ first * K0),
one to one in `rest`: two IDs collide only with different first bytes, when
rest' = rest ^ first * K0 ^ first' * K0.

Usage: collide.py TEMPLATE_DATA_DIR OUT_DIR [PAIRS] [SEED]
The template gives the window, products.csv and campaigns.csv.
"""
import json
import os
import random
import shutil
import sys

M = (1 << 64) - 1
K0 = 0xD6E8_FEB8_6659_FD93
K1 = 0x9E37_79B9_7F4A_7C15
K2 = 0xC2B2_AE3D_27D4_EB4F
K3 = 0x1656_67B1_9E37_79F9
K4 = 0xFF51_AFD7_ED55_8CCD
K1_INV = pow(K1, -1, 1 << 64)
PLAIN = [c for c in range(0x21, 0x7F) if chr(c) not in ',"']


def rotl(x, n):
    return (x << n | x >> (64 - n)) & M


def finish(h):
    h ^= h >> 29
    h = h * 0xBF58_476D_1CE4_E5B9 & M
    return h ^ (h >> 32)


def hash33(b):
    w = [int.from_bytes(b[1 + 8 * k:9 + 8 * k], "little") for k in range(4)]
    a = (w[0] ^ b[0] * K0 & M) * K1 & M
    return finish(a ^ rotl(w[1] * K2 & M, 32) ^ rotl(w[2] * K3 & M, 17) ^ rotl(w[3] * K4 & M, 47))


def hash9(b):
    rest = int.from_bytes(b[1:].ljust(8, b"\0"), "little")
    a = (rest ^ b[0] * K0 & M) * K1 & M
    return a ^ (a >> 32)


def pair33(R):
    """Two IDs of 33 plain ASCII bytes, `e` and 32 more, with one hash33."""
    first = ord("e")
    one = bytes([first]) + bytes(R.choice(b"0123456789abcdef") for _ in range(32))
    w0, w1 = int.from_bytes(one[1:9], "little"), int.from_bytes(one[9:17], "little")
    a = (w0 ^ first * K0 & M) * K1 & M
    while True:
        other1 = bytes(R.choice(PLAIN) for _ in range(8))
        w1b = int.from_bytes(other1, "little")
        if w1b == w1:
            continue
        ab = a ^ rotl(w1 * K2 & M, 32) ^ rotl(w1b * K2 & M, 32)
        w0b = (ab * K1_INV & M) ^ (first * K0 & M)
        other0 = w0b.to_bytes(8, "little")
        if all(c in PLAIN for c in other0):
            two = bytes([first]) + other0 + other1 + one[17:]
            assert two != one and hash33(one) == hash33(two) and len(two) == 33
            return one, two


def pair9(R):
    """Two IDs of 9 bytes with different first letters and one hash9 (some
    bytes are not ASCII: the difference between the two is a constant)."""
    while True:
        f1, f2 = R.sample(PLAIN, 2)
        c = (f1 * K0 ^ f2 * K0) & M
        diff = c.to_bytes(8, "little")
        rest = bytearray()
        for d in diff:
            options = [x for x in PLAIN if (x ^ d) not in (0, 0x0A, 0x22, 0x2C, 0x0D)]
            rest.append(R.choice(options))
        one = bytes([f1]) + bytes(rest)
        two = bytes([f2]) + bytes(x ^ d for x, d in zip(rest, diff))
        if hash9(one) == hash9(two) and one != two:
            return one, two


def main():
    template, out = sys.argv[1], sys.argv[2]
    pairs = int(sys.argv[3]) if len(sys.argv) > 3 else 4
    R = random.Random(int(sys.argv[4]) if len(sys.argv) > 4 else 1)
    if os.path.exists(out):
        shutil.rmtree(out)
    for folder in ("events", "clicks", "links"):
        os.makedirs(os.path.join(out, folder))
    for name in ("products.csv", "campaigns.csv", "manifest.json"):
        shutil.copy(os.path.join(template, name), os.path.join(out, name))
    with open(os.path.join(template, "manifest.json")) as f:
        start = json.load(f)["window_start"]
    day = start[:10]
    with open(os.path.join(template, "products.csv"), "rb") as f:
        product = f.read().split(b"\n")[1].split(b",")[0]
    lines = [b"event_id,user_id,ts,ingest_ts,event_type,product_id,price_cents"]
    made = []
    for k in range(pairs):
        one, two = pair33(R) if k % 2 == 0 else pair9(R)
        made.append((one, two))
        ts = f"{day}T{10 + k % 10:02d}:00:00Z".encode()
        user = f"uc{k}".encode()
        # Two events of one user at one instant, ingested a second apart.
        lines.append(b",".join([one, user, ts, f"{day}T{10 + k % 10:02d}:00:05Z".encode(), b"view", product, b""]))
        lines.append(b",".join([two, user, ts, f"{day}T{10 + k % 10:02d}:00:06Z".encode(), b"view", product, b""]))
    with open(os.path.join(out, "events", "part-00000.csv"), "wb") as f:
        f.write(b"\n".join(lines) + b"\n")
    with open(os.path.join(out, "clicks", "part-00000.csv"), "wb") as f:
        f.write(b"click_id,user_id,campaign_id,ts\n")
    with open(os.path.join(out, "links", "part-00000.csv"), "wb") as f:
        f.write(b"user_id_a,user_id_b\n")
    for one, two in made:
        print(f"  {one!r} and {two!r}: one hash ({len(one)} bytes each)")
    print(f"collide: {pairs} pairs, {2 * pairs} events of {pairs} users in {out}; by the rules every user has 2 events")


if __name__ == "__main__":
    main()
