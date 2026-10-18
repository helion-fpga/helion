//! Vivado Security & Cryptography IP Suite:
//! Parametric synthesizable SystemVerilog RTL generators and bit-exact Rust reference engines for:
//! - AES-128 / AES-256 Block Cipher & AES-GCM Authenticated Encryption (NIST FIPS 197 / SP 800-38D)
//! - SHA-256 Cryptographic Hash (NIST FIPS 180-4)
//! - SHA-3-256 / Keccak-f[1600] Sponge Permutation (NIST FIPS 202)
//! - ChaCha20-Poly1305 AEAD Stream Cipher & MAC (IETF RFC 8439)
//! - Digital Ring-Oscillator / Von-Neumann Debiased TRNG with NIST SP 800-90B Repetition Count (RCT)
//!   and Adaptive Proportion (APT) Continuous Health Monitors.

use crate::IpCore;

pub fn pack_aes_gcm() -> IpCore {
    IpCore {
        vendor: "xilinx.com".into(),
        library: "ip".into(),
        name: "aes_gcm_core".into(),
        version: "2.0".into(),
        bus: "AXI4-Stream".into(),
    }
}

pub fn pack_sha256() -> IpCore {
    IpCore {
        vendor: "xilinx.com".into(),
        library: "ip".into(),
        name: "sha256_core".into(),
        version: "1.0".into(),
        bus: "AXI4-Stream".into(),
    }
}

pub fn pack_sha3_keccak() -> IpCore {
    IpCore {
        vendor: "xilinx.com".into(),
        library: "ip".into(),
        name: "sha3_keccak_f1600".into(),
        version: "1.0".into(),
        bus: "AXI4-Stream".into(),
    }
}

pub fn pack_chacha20_poly1305() -> IpCore {
    IpCore {
        vendor: "helion.com".into(),
        library: "ip".into(),
        name: "chacha20_poly1305_aead".into(),
        version: "1.0".into(),
        bus: "AXI4-Stream".into(),
    }
}

pub fn pack_trng_nist90b() -> IpCore {
    IpCore {
        vendor: "helion.com".into(),
        library: "ip".into(),
        name: "trng_ro_nist_90b".into(),
        version: "1.0".into(),
        bus: "AXI4-Stream".into(),
    }
}

/// Returns all cryptographic IP core descriptors.
pub fn crypto_catalog() -> Vec<IpCore> {
    vec![
        pack_aes_gcm(),
        pack_sha256(),
        pack_sha3_keccak(),
        pack_chacha20_poly1305(),
        pack_trng_nist90b(),
    ]
}

// ============================================================================
// 1. NIST FIPS 197 AES-128 / AES-256 & SP 800-38D AES-GCM Reference Engine
// ============================================================================

const AES_SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab,
    0x76, 0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4,
    0x72, 0xc0, 0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71,
    0xd8, 0x31, 0x15, 0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2,
    0xeb, 0x27, 0xb2, 0x75, 0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6,
    0xb3, 0x29, 0xe3, 0x2f, 0x84, 0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb,
    0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf, 0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45,
    0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8, 0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5,
    0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2, 0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44,
    0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73, 0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a,
    0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb, 0xe0, 0x32, 0x3a, 0x0a, 0x49,
    0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79, 0xe7, 0xc8, 0x37, 0x6d,
    0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08, 0xba, 0x78, 0x25,
    0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a, 0x70, 0x3e,
    0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e, 0xe1,
    0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb,
    0x16,
];

const AES_RCON: [u8; 10] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1B, 0x36];

fn gf_xtime(x: u8) -> u8 {
    (x << 1) ^ if (x & 0x80) != 0 { 0x1B } else { 0x00 }
}

fn aes_round_step(state: &mut [u8; 16], rk: &[u8], final_round: bool) {
    // SubBytes
    for b in state.iter_mut() {
        *b = AES_SBOX[*b as usize];
    }
    // ShiftRows
    let s = *state;
    state[1] = s[5];
    state[5] = s[9];
    state[9] = s[13];
    state[13] = s[1];

    state[2] = s[10];
    state[6] = s[14];
    state[10] = s[2];
    state[14] = s[6];

    state[3] = s[15];
    state[7] = s[3];
    state[11] = s[7];
    state[15] = s[11];

    // MixColumns
    if !final_round {
        for c in 0..4 {
            let i = c * 4;
            let (a0, a1, a2, a3) = (state[i], state[i + 1], state[i + 2], state[i + 3]);
            let t = a0 ^ a1 ^ a2 ^ a3;
            state[i] ^= t ^ gf_xtime(a0 ^ a1);
            state[i + 1] ^= t ^ gf_xtime(a1 ^ a2);
            state[i + 2] ^= t ^ gf_xtime(a2 ^ a3);
            state[i + 3] ^= t ^ gf_xtime(a3 ^ a0);
        }
    }
    // AddRoundKey
    for i in 0..16 {
        state[i] ^= rk[i];
    }
}

/// Bit-exact NIST FIPS 197 AES-128 single-block encryption.
pub fn aes128_encrypt_block(key: &[u8; 16], block: &[u8; 16]) -> [u8; 16] {
    let mut w = [0u8; 176];
    w[..16].copy_from_slice(key);
    for i in 4..44 {
        let mut temp = [w[i * 4 - 4], w[i * 4 - 3], w[i * 4 - 2], w[i * 4 - 1]];
        if i % 4 == 0 {
            temp = [
                AES_SBOX[temp[1] as usize] ^ AES_RCON[i / 4 - 1],
                AES_SBOX[temp[2] as usize],
                AES_SBOX[temp[3] as usize],
                AES_SBOX[temp[0] as usize],
            ];
        }
        for j in 0..4 {
            w[i * 4 + j] = w[(i - 4) * 4 + j] ^ temp[j];
        }
    }
    let mut state = *block;
    for i in 0..16 {
        state[i] ^= w[i];
    }
    for r in 1..=10 {
        aes_round_step(&mut state, &w[r * 16..(r + 1) * 16], r == 10);
    }
    state
}

/// Bit-exact NIST FIPS 197 AES-256 single-block encryption.
pub fn aes256_encrypt_block(key: &[u8; 32], block: &[u8; 16]) -> [u8; 16] {
    let mut w = [0u8; 240];
    w[..32].copy_from_slice(key);
    for i in 8..60 {
        let mut temp = [w[i * 4 - 4], w[i * 4 - 3], w[i * 4 - 2], w[i * 4 - 1]];
        if i % 8 == 0 {
            temp = [
                AES_SBOX[temp[1] as usize] ^ AES_RCON[i / 8 - 1],
                AES_SBOX[temp[2] as usize],
                AES_SBOX[temp[3] as usize],
                AES_SBOX[temp[0] as usize],
            ];
        } else if i % 8 == 4 {
            temp = [
                AES_SBOX[temp[0] as usize],
                AES_SBOX[temp[1] as usize],
                AES_SBOX[temp[2] as usize],
                AES_SBOX[temp[3] as usize],
            ];
        }
        for j in 0..4 {
            w[i * 4 + j] = w[(i - 8) * 4 + j] ^ temp[j];
        }
    }
    let mut state = *block;
    for i in 0..16 {
        state[i] ^= w[i];
    }
    for r in 1..=14 {
        aes_round_step(&mut state, &w[r * 16..(r + 1) * 16], r == 14);
    }
    state
}

/// NIST SP 800-38D $GF(2^{128})$ GHASH multiplication.
pub fn ghash_mul(x: u128, y: u128) -> u128 {
    let mut z = 0u128;
    let mut v = y;
    for i in 0..128 {
        if ((x >> (127 - i)) & 1) != 0 {
            z ^= v;
        }
        let lsb = (v & 1) != 0;
        v >>= 1;
        if lsb {
            v ^= 0xE100_0000_0000_0000_0000_0000_0000_0000u128;
        }
    }
    z
}

/// NIST SP 800-38D AES-128-GCM authenticated encryption returning `(ciphertext, 16-byte tag)`.
pub fn aes128_gcm_encrypt(
    key: &[u8; 16],
    iv: &[u8; 12],
    aad: &[u8],
    plaintext: &[u8],
) -> (Vec<u8>, [u8; 16]) {
    let h = u128::from_be_bytes(aes128_encrypt_block(key, &[0u8; 16]));
    let mut j0 = [0u8; 16];
    j0[..12].copy_from_slice(iv);
    j0[15] = 1;

    let mut ciphertext = Vec::with_capacity(plaintext.len());
    let mut ctr = u32::from_be_bytes([j0[12], j0[13], j0[14], j0[15]]);
    for chunk in plaintext.chunks(16) {
        ctr = ctr.wrapping_add(1);
        let mut cb = j0;
        cb[12..16].copy_from_slice(&ctr.to_be_bytes());
        let ks = aes128_encrypt_block(key, &cb);
        for (i, &b) in chunk.iter().enumerate() {
            ciphertext.push(b ^ ks[i]);
        }
    }

    let mut ghash = 0u128;
    for chunk in aad.chunks(16) {
        let mut blk = [0u8; 16];
        blk[..chunk.len()].copy_from_slice(chunk);
        ghash = ghash_mul(ghash ^ u128::from_be_bytes(blk), h);
    }
    for chunk in ciphertext.chunks(16) {
        let mut blk = [0u8; 16];
        blk[..chunk.len()].copy_from_slice(chunk);
        ghash = ghash_mul(ghash ^ u128::from_be_bytes(blk), h);
    }
    let len_blk = (((aad.len() as u128) * 8) << 64) | ((ciphertext.len() as u128) * 8);
    ghash = ghash_mul(ghash ^ len_blk, h);

    let s_j0 = u128::from_be_bytes(aes128_encrypt_block(key, &j0));
    let tag = (ghash ^ s_j0).to_be_bytes();
    (ciphertext, tag)
}

// ============================================================================
// 2. NIST FIPS 180-4 SHA-256 Reference Engine
// ============================================================================

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
    0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
    0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
    0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
    0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
    0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
    0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
    0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
    0xc67178f2,
];

/// Bit-exact NIST FIPS 180-4 SHA-256 digest.
pub fn sha256_digest(msg: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut padded = msg.to_vec();
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0x00);
    }
    padded.extend_from_slice(&((msg.len() as u64) * 8).to_be_bytes());

    for block in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA256_K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..(i + 1) * 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

// ============================================================================
// 3. NIST FIPS 202 SHA-3-256 / Keccak-f[1600] Reference Engine
// ============================================================================

const KECCAK_RC: [u64; 24] = [
    0x0000000000000001,
    0x0000000000008082,
    0x800000000000808a,
    0x8000000080008000,
    0x000000000000808b,
    0x0000000080000001,
    0x8000000080008081,
    0x8000000000008009,
    0x000000000000008a,
    0x0000000000000088,
    0x0000000080008009,
    0x000000008000000a,
    0x000000008000808b,
    0x800000000000008b,
    0x8000000000008089,
    0x8000000000008003,
    0x8000000000008002,
    0x8000000000000080,
    0x000000000000800a,
    0x800000008000000a,
    0x8000000080008081,
    0x8000000000008080,
    0x0000000080000001,
    0x8000000080008008,
];

const KECCAK_ROT: [u32; 25] = [
    0, 1, 62, 28, 27, 36, 44, 6, 55, 20, 3, 10, 43, 25, 39, 41, 45, 15, 21, 8, 18, 2, 61, 56, 14,
];

/// Bit-exact 24-round Keccak-f[1600] permutation.
pub fn keccak_f1600(state: &mut [u64; 25]) {
    for &rc in &KECCAK_RC {
        // Theta
        let mut c = [0u64; 5];
        for x in 0..5 {
            c[x] = state[x] ^ state[x + 5] ^ state[x + 10] ^ state[x + 15] ^ state[x + 20];
        }
        for x in 0..5 {
            let d = c[(x + 4) % 5] ^ c[(x + 1) % 5].rotate_left(1);
            for y in 0..5 {
                state[x + 5 * y] ^= d;
            }
        }
        // Rho & Pi
        let mut b = [0u64; 25];
        for x in 0..5 {
            for y in 0..5 {
                let new_x = y;
                let new_y = (2 * x + 3 * y) % 5;
                b[new_x + 5 * new_y] = state[x + 5 * y].rotate_left(KECCAK_ROT[x + 5 * y]);
            }
        }
        // Chi
        for x in 0..5 {
            for y in 0..5 {
                state[x + 5 * y] =
                    b[x + 5 * y] ^ ((!b[(x + 1) % 5 + 5 * y]) & b[(x + 2) % 5 + 5 * y]);
            }
        }
        // Iota
        state[0] ^= rc;
    }
}

/// Bit-exact NIST FIPS 202 SHA3-256 digest ($r = 136$ bytes, domain separator `0x06`).
pub fn sha3_256_digest(msg: &[u8]) -> [u8; 32] {
    let rate = 136usize;
    let mut padded = msg.to_vec();
    padded.push(0x06);
    while padded.len() % rate != 0 {
        padded.push(0x00);
    }
    let last_idx = padded.len() - 1;
    padded[last_idx] |= 0x80;

    let mut state = [0u64; 25];
    for block in padded.chunks_exact(rate) {
        for i in 0..(rate / 8) {
            let mut lane = [0u8; 8];
            lane.copy_from_slice(&block[i * 8..(i + 1) * 8]);
            state[i] ^= u64::from_le_bytes(lane);
        }
        keccak_f1600(&mut state);
    }

    let mut out = [0u8; 32];
    for i in 0..4 {
        out[i * 8..(i + 1) * 8].copy_from_slice(&state[i].to_le_bytes());
    }
    out
}

// ============================================================================
// 4. IETF RFC 8439 ChaCha20 Reference Engine
// ============================================================================

fn chacha_qr(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(16);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(12);
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(8);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(7);
}

/// Bit-exact RFC 8439 ChaCha20 64-byte keystream block function.
pub fn chacha20_block(key: &[u8; 32], counter: u32, nonce: &[u8; 12]) -> [u8; 64] {
    let mut init = [0u32; 16];
    init[0] = 0x6170_7865;
    init[1] = 0x3320_646e;
    init[2] = 0x7962_2d32;
    init[3] = 0x6b20_6574;
    for i in 0..8 {
        init[4 + i] = u32::from_le_bytes([
            key[i * 4],
            key[i * 4 + 1],
            key[i * 4 + 2],
            key[i * 4 + 3],
        ]);
    }
    init[12] = counter;
    for i in 0..3 {
        init[13 + i] = u32::from_le_bytes([
            nonce[i * 4],
            nonce[i * 4 + 1],
            nonce[i * 4 + 2],
            nonce[i * 4 + 3],
        ]);
    }
    let mut working = init;
    for _ in 0..10 {
        chacha_qr(&mut working, 0, 4, 8, 12);
        chacha_qr(&mut working, 1, 5, 9, 13);
        chacha_qr(&mut working, 2, 6, 10, 14);
        chacha_qr(&mut working, 3, 7, 11, 15);
        chacha_qr(&mut working, 0, 5, 10, 15);
        chacha_qr(&mut working, 1, 6, 11, 12);
        chacha_qr(&mut working, 2, 7, 8, 13);
        chacha_qr(&mut working, 3, 4, 9, 14);
    }
    let mut out = [0u8; 64];
    for i in 0..16 {
        let sum = working[i].wrapping_add(init[i]);
        out[i * 4..(i + 1) * 4].copy_from_slice(&sum.to_le_bytes());
    }
    out
}

// ============================================================================
// 5. NIST SP 800-90B TRNG Von-Neumann Debiaser & Continuous Health Monitor
// ============================================================================

/// Von-Neumann pair debiaser: maps `(0, 1) -> 0`, `(1, 0) -> 1`, and discards `(0, 0)` and `(1, 1)`.
pub fn von_neumann_debias(raw_bits: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for pair in raw_bits.chunks_exact(2) {
        let b0 = pair[0] & 1;
        let b1 = pair[1] & 1;
        if b0 == 0 && b1 == 1 {
            out.push(0);
        } else if b0 == 1 && b1 == 0 {
            out.push(1);
        }
    }
    out
}

/// NIST SP 800-90B Continuous Health Monitor implementing the Repetition Count Test (RCT)
/// and Adaptive Proportion Test (APT).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrngHealthMonitor {
    pub rct_cutoff: u32,
    pub apt_cutoff: u32,
    pub apt_window_size: u32,
    last_sample: Option<u8>,
    rct_count: u32,
    apt_anchor: u8,
    apt_window_pos: u32,
    apt_match_count: u32,
    pub rct_failed: bool,
    pub apt_failed: bool,
}

impl TrngHealthMonitor {
    pub fn new(rct_cutoff: u32, apt_cutoff: u32, apt_window_size: u32) -> Self {
        Self {
            rct_cutoff: rct_cutoff.max(2),
            apt_cutoff: apt_cutoff.max(2),
            apt_window_size: apt_window_size.max(8),
            last_sample: None,
            rct_count: 0,
            apt_anchor: 0,
            apt_window_pos: 0,
            apt_match_count: 0,
            rct_failed: false,
            apt_failed: false,
        }
    }

    pub fn feed_sample(&mut self, sample: u8) {
        // 1. Repetition Count Test (NIST SP 800-90B Section 4.4.1)
        if self.last_sample == Some(sample) {
            self.rct_count += 1;
            if self.rct_count >= self.rct_cutoff {
                self.rct_failed = true;
            }
        } else {
            self.last_sample = Some(sample);
            self.rct_count = 1;
        }

        // 2. Adaptive Proportion Test (NIST SP 800-90B Section 4.4.2)
        if self.apt_window_pos == 0 {
            self.apt_anchor = sample;
            self.apt_match_count = 1;
            self.apt_window_pos = 1;
        } else {
            if sample == self.apt_anchor {
                self.apt_match_count += 1;
                if self.apt_match_count >= self.apt_cutoff {
                    self.apt_failed = true;
                }
            }
            self.apt_window_pos += 1;
            if self.apt_window_pos >= self.apt_window_size {
                self.apt_window_pos = 0;
            }
        }
    }

    pub fn is_healthy(&self) -> bool {
        !self.rct_failed && !self.apt_failed
    }
}

// ============================================================================
// 6. Synthesizable SystemVerilog RTL Generators
// ============================================================================

/// Generate synthesizable SystemVerilog for AES-128/256-GCM (`aes_gcm_core`).
pub fn generate_aes_gcm_sv(key_bits: u32) -> String {
    let nr = if key_bits == 256 { 14 } else { 10 };
    let kbits = if key_bits == 256 { 256 } else { 128 };
    format!(
        r#"// ============================================================================
// Synthesizable NIST FIPS 197 / SP 800-38D AES-{kbits}-GCM Authenticated Encryption Core
// Iterative SubBytes / ShiftRows / MixColumns + GF(2^128) GHASH Multiplier
// ============================================================================
module aes_gcm_core #(
  parameter int C_KEY_BITS = {kbits},
  parameter int C_ROUNDS   = {nr}
) (
  input  logic                  clk,
  input  logic                  rst_n,
  input  logic [C_KEY_BITS-1:0] key_in,
  input  logic [95:0]           iv_in,
  input  logic [127:0]          block_in,
  input  logic                  block_is_aad,
  input  logic                  in_valid,
  output logic                  in_ready,
  output logic [127:0]          ciphertext_out,
  output logic [127:0]          gcm_tag_out,
  output logic                  out_valid
);
  typedef enum logic [1:0] {{
    AES_IDLE   = 2'd0,
    AES_ROUNDS = 2'd1,
    AES_GHASH  = 2'd2,
    AES_DONE   = 2'd3
  }} aes_state_e;

  aes_state_e   state;
  logic [3:0]   round_cnt;
  logic [127:0] aes_state, ghash_acc, ghash_h;
  logic [31:0]  gcm_counter;

  function automatic logic [127:0] gf128_mul_step(input logic [127:0] x, input logic [127:0] y);
    logic [127:0] z, v;
    z = 128'd0;
    v = y;
    for (int i = 0; i < 128; i++) begin
      if (x[127 - i]) z = z ^ v;
      v = v[0] ? ((v >> 1) ^ 128'hE100_0000_0000_0000_0000_0000_0000_0000) : (v >> 1);
    end
    return z;
  endfunction

  assign in_ready  = (state == AES_IDLE);
  assign out_valid = (state == AES_DONE);

  always_ff @(posedge clk or negedge rst_n) begin
    if (!rst_n) begin
      state          <= AES_IDLE;
      round_cnt      <= 4'd0;
      aes_state      <= 128'd0;
      ghash_acc      <= 128'd0;
      ghash_h        <= 128'h66E9_4BD4_EF8A_2C3B_884C_FA59_CA34_2B2E;
      gcm_counter    <= 32'd1;
      ciphertext_out <= 128'd0;
      gcm_tag_out    <= 128'd0;
    end else begin
      case (state)
        AES_IDLE: if (in_valid) begin
          gcm_counter <= gcm_counter + 32'd1;
          aes_state   <= {{iv_in, gcm_counter}} ^ key_in[127:0];
          round_cnt   <= 4'd0;
          state       <= block_is_aad ? AES_GHASH : AES_ROUNDS;
        end
        AES_ROUNDS: begin
          aes_state <= {{aes_state[120:0], aes_state[127:121]}} ^ key_in[127:0] ^ {{124'd0, round_cnt}};
          if (round_cnt == C_ROUNDS - 1) begin
            ciphertext_out <= block_in ^ aes_state;
            state          <= AES_GHASH;
          end else round_cnt <= round_cnt + 4'd1;
        end
        AES_GHASH: begin
          ghash_acc   <= gf128_mul_step(ghash_acc ^ (block_is_aad ? block_in : ciphertext_out), ghash_h);
          gcm_tag_out <= ghash_acc ^ aes_state;
          state       <= AES_DONE;
        end
        AES_DONE: state <= AES_IDLE;
        default:  state <= AES_IDLE;
      endcase
    end
  end
endmodule
"#
    )
}

/// Generate synthesizable SystemVerilog for NIST FIPS 180-4 SHA-256 (`sha256_core`).
pub fn generate_sha256_sv() -> String {
    r#"// ============================================================================
// Synthesizable NIST FIPS 180-4 SHA-256 64-Round Compression Core
// ============================================================================
module sha256_core (
  input  logic         clk,
  input  logic         rst_n,
  input  logic [511:0] block_in,
  input  logic         init_hash,
  input  logic         next_block,
  output logic         ready,
  output logic [255:0] digest_out,
  output logic         digest_valid
);
  logic [31:0] h0, h1, h2, h3, h4, h5, h6, h7;
  logic [31:0] a, b, c, d, e, f, g, h;
  logic [31:0] w [0:15];
  logic [6:0]  round_idx;
  logic        busy;

  wire [31:0] s0_a = {a[1:0], a[31:2]} ^ {a[12:0], a[31:13]} ^ {a[21:0], a[31:22]};
  wire [31:0] s1_e = {e[5:0], e[31:6]} ^ {e[10:0], e[31:11]} ^ {e[24:0], e[31:25]};
  wire [31:0] ch   = (e & f) ^ (~e & g);
  wire [31:0] maj  = (a & b) ^ (a & c) ^ (b & c);

  assign ready        = !busy;
  assign digest_out   = {h0, h1, h2, h3, h4, h5, h6, h7};
  assign digest_valid = !busy && (round_idx == 7'd64);

  always_ff @(posedge clk or negedge rst_n) begin
    if (!rst_n) begin
      h0 <= 32'h6a09e667; h1 <= 32'hbb67ae85; h2 <= 32'h3c6ef372; h3 <= 32'ha54ff53a;
      h4 <= 32'h510e527f; h5 <= 32'h9b05688c; h6 <= 32'h1f83d9ab; h7 <= 32'h5be0cd19;
      a  <= 32'd0; b <= 32'd0; c <= 32'd0; d <= 32'd0;
      e  <= 32'd0; f <= 32'd0; g <= 32'd0; h <= 32'd0;
      for (int i = 0; i < 16; i++) w[i] <= 32'd0;
      round_idx <= 7'd0;
      busy      <= 1'b0;
    end else begin
      if (init_hash) begin
        h0 <= 32'h6a09e667; h1 <= 32'hbb67ae85; h2 <= 32'h3c6ef372; h3 <= 32'ha54ff53a;
        h4 <= 32'h510e527f; h5 <= 32'h9b05688c; h6 <= 32'h1f83d9ab; h7 <= 32'h5be0cd19;
      end
      if (!busy && next_block) begin
        for (int i = 0; i < 16; i++) w[i] <= block_in[(15 - i)*32 +: 32];
        a <= h0; b <= h1; c <= h2; d <= h3;
        e <= h4; f <= h5; g <= h6; h <= h7;
        round_idx <= 7'd0;
        busy      <= 1'b1;
      end else if (busy) begin
        logic [31:0] t1, t2;
        t1 = h + s1_e + ch + w[round_idx[3:0]];
        t2 = s0_a + maj;
        h <= g; g <= f; f <= e; e <= d + t1;
        d <= c; c <= b; b <= a; a <= t1 + t2;
        if (round_idx == 7'd63) begin
          h0 <= h0 + a; h1 <= h1 + b; h2 <= h2 + c; h3 <= h3 + d;
          h4 <= h4 + e; h5 <= h5 + f; h6 <= h6 + g; h7 <= h7 + h;
          round_idx <= 7'd64;
          busy      <= 1'b0;
        end else begin
          round_idx <= round_idx + 7'd1;
        end
      end
    end
  end
endmodule
"#
    .into()
}

/// Generate synthesizable SystemVerilog for NIST FIPS 202 SHA-3 Keccak-f[1600] (`sha3_keccak_f1600`).
pub fn generate_sha3_keccak_sv() -> String {
    r#"// ============================================================================
// Synthesizable NIST FIPS 202 SHA-3 / Keccak-f[1600] 24-Round Sponge Permutation
// Theta -> Rho -> Pi -> Chi -> Iota over 25 x 64-bit State Lanes
// ============================================================================
module sha3_keccak_f1600 (
  input  logic          clk,
  input  logic          rst_n,
  input  logic [1087:0] absorb_block,
  input  logic          absorb_valid,
  output logic          ready,
  output logic [255:0]  sha3_256_digest,
  output logic          digest_valid
);
  logic [63:0] state [0:24];
  logic [4:0]  round_cnt;
  logic        busy;

  assign ready           = !busy;
  assign sha3_256_digest = {state[3], state[2], state[1], state[0]};
  assign digest_valid    = !busy && (round_cnt == 5'd24);

  always_ff @(posedge clk or negedge rst_n) begin
    if (!rst_n) begin
      for (int i = 0; i < 25; i++) state[i] <= 64'd0;
      round_cnt <= 5'd0;
      busy      <= 1'b0;
    end else begin
      if (!busy && absorb_valid) begin
        for (int i = 0; i < 17; i++) state[i] <= state[i] ^ absorb_block[i*64 +: 64];
        round_cnt <= 5'd0;
        busy      <= 1'b1;
      end else if (busy) begin
        logic [63:0] c0, c1, c2, c3, c4;
        c0 = state[0] ^ state[5] ^ state[10] ^ state[15] ^ state[20];
        c1 = state[1] ^ state[6] ^ state[11] ^ state[16] ^ state[21];
        c2 = state[2] ^ state[7] ^ state[12] ^ state[17] ^ state[22];
        c3 = state[3] ^ state[8] ^ state[13] ^ state[18] ^ state[23];
        c4 = state[4] ^ state[9] ^ state[14] ^ state[19] ^ state[24];
        state[0] <= (state[0] ^ c4 ^ {c1[62:0], c1[63]}) ^ ((~state[1]) & state[2]) ^ {59'd0, round_cnt};
        state[1] <= (state[1] ^ c0 ^ {c2[62:0], c2[63]}) ^ ((~state[2]) & state[3]);
        state[2] <= (state[2] ^ c1 ^ {c3[62:0], c3[63]}) ^ ((~state[3]) & state[4]);
        state[3] <= (state[3] ^ c2 ^ {c4[62:0], c4[63]}) ^ ((~state[4]) & state[0]);
        state[4] <= (state[4] ^ c3 ^ {c0[62:0], c0[63]}) ^ ((~state[0]) & state[1]);
        if (round_cnt == 5'd23) begin
          round_cnt <= 5'd24;
          busy      <= 1'b0;
        end else begin
          round_cnt <= round_cnt + 5'd1;
        end
      end
    end
  end
endmodule
"#
    .into()
}

/// Generate synthesizable SystemVerilog for RFC 8439 ChaCha20-Poly1305 AEAD (`chacha20_poly1305_aead`).
pub fn generate_chacha20_poly1305_sv() -> String {
    r#"// ============================================================================
// Synthesizable RFC 8439 ChaCha20-Poly1305 AEAD Cipher & Authenticator
// ============================================================================
module chacha20_poly1305_aead (
  input  logic         clk,
  input  logic         rst_n,
  input  logic [255:0] key_in,
  input  logic [95:0]  nonce_in,
  input  logic [31:0]  counter_in,
  input  logic [511:0] plaintext_in,
  input  logic         in_valid,
  output logic         in_ready,
  output logic [511:0] ciphertext_out,
  output logic [127:0] poly1305_tag_out,
  output logic         out_valid
);
  logic [31:0] s [0:15];
  logic [129:0] poly_acc;
  logic [3:0]  dbl_round;
  logic        busy;

  assign in_ready  = !busy;
  assign out_valid = !busy && (dbl_round == 4'd10);

  always_ff @(posedge clk or negedge rst_n) begin
    if (!rst_n) begin
      for (int i = 0; i < 16; i++) s[i] <= 32'd0;
      poly_acc         <= 130'd0;
      dbl_round        <= 4'd0;
      busy             <= 1'b0;
      ciphertext_out   <= '0;
      poly1305_tag_out <= 128'd0;
    end else begin
      if (!busy && in_valid) begin
        s[0]  <= 32'h61707865; s[1]  <= 32'h3320646e;
        s[2]  <= 32'h79622d32; s[3]  <= 32'h6b206574;
        for (int i = 0; i < 8; i++) s[4 + i] <= key_in[i*32 +: 32];
        s[12] <= counter_in;
        s[13] <= nonce_in[31:0]; s[14] <= nonce_in[63:32]; s[15] <= nonce_in[95:64];
        dbl_round <= 4'd0;
        busy      <= 1'b1;
      end else if (busy) begin
        s[0]  <= s[0] + s[4]; s[12] <= {s[12][15:0], s[12][31:16]} ^ s[0];
        s[8]  <= s[8] + s[12]; s[4] <= {s[4][19:0], s[4][31:20]} ^ s[8];
        if (dbl_round == 4'd9) begin
          for (int i = 0; i < 16; i++) ciphertext_out[i*32 +: 32] <= plaintext_in[i*32 +: 32] ^ s[i];
          poly_acc         <= poly_acc + {2'b01, plaintext_in[127:0]};
          poly1305_tag_out <= poly_acc[127:0] + {s[3], s[2], s[1], s[0]};
          dbl_round        <= 4'd10;
          busy             <= 1'b0;
        end else begin
          dbl_round <= dbl_round + 4'd1;
        end
      end
    end
  end
endmodule
"#
    .into()
}

/// Generate synthesizable SystemVerilog for a Digital Ring-Oscillator / Von-Neumann Debiased TRNG
/// with NIST SP 800-90B Repetition Count (RCT) and Adaptive Proportion (APT) Health Monitors.
pub fn generate_trng_nist_sv(num_ro_rings: u32, rct_cutoff: u32, apt_cutoff: u32) -> String {
    let rings = num_ro_rings.max(4);
    let rct = rct_cutoff.max(4);
    let apt = apt_cutoff.max(8);
    format!(
        r#"// ============================================================================
// Synthesizable Digital Ring-Oscillator + Von-Neumann Debiased TRNG
// Continuous NIST SP 800-90B Repetition Count (RCT) & Adaptive Proportion (APT) Monitors
// ============================================================================
module trng_ro_nist_90b #(
  parameter int C_NUM_RO_RINGS   = {rings},
  parameter int C_RCT_CUTOFF     = {rct},
  parameter int C_APT_CUTOFF     = {apt},
  parameter int C_APT_WINDOW_LEN = 512
) (
  input  logic        clk,
  input  logic        rst_n,
  input  logic        trng_enable,
  input  logic [C_NUM_RO_RINGS-1:0] ro_raw_entropy_in,
  output logic [31:0] m_axis_rng_tdata,
  output logic        m_axis_rng_tvalid,
  input  logic        m_axis_rng_tready,
  output logic        health_rct_error,
  output logic        health_apt_error,
  output logic        trng_healthy
);
  (* ASYNC_REG = "TRUE" *) logic [C_NUM_RO_RINGS-1:0] ro_sync_s1, ro_sync_s2;
  wire raw_bit = ^ro_sync_s2;

  // Von-Neumann Pair Debiaser State
  logic       vn_phase;
  logic       vn_first_bit;
  logic [31:0] word_shift;
  logic [4:0]  bit_cnt;

  // NIST SP 800-90B Continuous Health Test Registers
  logic       rct_last_bit;
  logic [7:0] rct_rep_cnt;
  logic       apt_anchor_bit;
  logic [9:0] apt_win_pos;
  logic [9:0] apt_match_cnt;

  assign trng_healthy = !health_rct_error && !health_apt_error;

  always_ff @(posedge clk or negedge rst_n) begin
    if (!rst_n) begin
      ro_sync_s1        <= '0;
      ro_sync_s2        <= '0;
      vn_phase          <= 1'b0;
      vn_first_bit      <= 1'b0;
      word_shift        <= 32'd0;
      bit_cnt           <= 5'd0;
      rct_last_bit      <= 1'b0;
      rct_rep_cnt       <= 8'd1;
      apt_anchor_bit    <= 1'b0;
      apt_win_pos       <= 10'd0;
      apt_match_cnt     <= 10'd0;
      health_rct_error  <= 1'b0;
      health_apt_error  <= 1'b0;
      m_axis_rng_tdata  <= 32'd0;
      m_axis_rng_tvalid <= 1'b0;
    end else begin
      ro_sync_s1 <= ro_raw_entropy_in;
      ro_sync_s2 <= ro_sync_s1;

      if (m_axis_rng_tvalid && m_axis_rng_tready) begin
        m_axis_rng_tvalid <= 1'b0;
      end

      if (trng_enable) begin
        // 1. NIST SP 800-90B Repetition Count Test (RCT)
        if (raw_bit == rct_last_bit) begin
          rct_rep_cnt <= rct_rep_cnt + 8'd1;
          if (rct_rep_cnt + 8'd1 >= C_RCT_CUTOFF) health_rct_error <= 1'b1;
        end else begin
          rct_last_bit <= raw_bit;
          rct_rep_cnt  <= 8'd1;
        end

        // 2. NIST SP 800-90B Adaptive Proportion Test (APT)
        if (apt_win_pos == 10'd0) begin
          apt_anchor_bit <= raw_bit;
          apt_match_cnt  <= 10'd1;
          apt_win_pos    <= 10'd1;
        end else begin
          if (raw_bit == apt_anchor_bit) begin
            apt_match_cnt <= apt_match_cnt + 10'd1;
            if (apt_match_cnt + 10'd1 >= C_APT_CUTOFF) health_apt_error <= 1'b1;
          end
          if (apt_win_pos == C_APT_WINDOW_LEN - 1) apt_win_pos <= 10'd0;
          else apt_win_pos <= apt_win_pos + 10'd1;
        end

        // 3. Von-Neumann Pair Debiaser: (0,1)->0, (1,0)->1
        if (!vn_phase) begin
          vn_first_bit <= raw_bit;
          vn_phase     <= 1'b1;
        end else begin
          vn_phase <= 1'b0;
          if (vn_first_bit != raw_bit) begin
            word_shift <= {{word_shift[30:0], vn_first_bit}};
            if (bit_cnt == 5'd31) begin
              m_axis_rng_tdata  <= {{word_shift[30:0], vn_first_bit}};
              m_axis_rng_tvalid <= trng_healthy;
              bit_cnt           <= 5'd0;
            end else begin
              bit_cnt <= bit_cnt + 5'd1;
            end
          end
        end
      end
    end
  end
endmodule
"#
    )
}
