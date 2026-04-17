#!/usr/bin/env python3
"""Python AES-128-CTR throughput benchmark (PyCryptodome).

Compares with Rust's aes crate performance — both use hardware AES instructions
(AES-NI on x86, ARMv8 crypto extensions on Apple Silicon).

Usage:
    python3 bench_decrypt.py
"""
import time
from Crypto.Cipher import AES

KEY = bytes(range(16))
IV  = bytes(range(16))
SIZES = [64 * 1024, 256 * 1024, 1024 * 1024, 4 * 1024 * 1024]
ITERS = 50

print("Python AES-128-CTR decryption benchmark (PyCryptodome)")
print(f"{'size':>10s}  {'iters':>6s}  {'total':>8s}  {'per_op':>10s}  {'throughput':>12s}")
print("-" * 60)

for size in SIZES:
    data = bytes(range(256)) * (size // 256)
    # Encrypt once (CTR mode is symmetric — decrypt == encrypt)
    enc = AES.new(KEY, AES.MODE_CTR, nonce=b'', initial_value=IV).encrypt(data)

    t0 = time.perf_counter()
    for _ in range(ITERS):
        AES.new(KEY, AES.MODE_CTR, nonce=b'', initial_value=IV).decrypt(enc)
    elapsed = time.perf_counter() - t0

    per_op_us = (elapsed / ITERS) * 1e6
    tput_mbs = (size * ITERS) / elapsed / 1e6

    print(f"{size // 1024:8d}KB  {ITERS:6d}  {elapsed:7.3f}s  {per_op_us:8.0f}µs  {tput_mbs:10.0f}MB/s")
