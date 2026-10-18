//! Bitstream Encryption & Authentication (`bitstream_crypto`, UG470 chapter 6):
//! - FIPS 180-4 SHA-256 & RFC 2104 HMAC-SHA256 authentication framing.
//! - FIPS 197 AES-256 + AES-256-GCM / CTR encryption with per-segment key rolling.
//! - Key source descriptors (`KeySource::Bbram`, `KeySource::Efuse`, `KeySource::HmacOnly`).
//! - Hardware tamper-rejection status bits (`DEC_ERROR`, `HMAC_ERROR`, `KEY_SOURCE_MISMATCH`, `SEC_VIOLATION`).

use crate::{Bitstream, Stat, Tap};

pub const AUTH_BITSTREAM_MAGIC: [u8; 8] = *b"HLNSEC01";

/// Non-volatile / battery-backed key storage source selector (UG470 `CTL0.DEC` / `KEY_SOURCE`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum KeySource {
    Bbram = 1,
    Efuse = 2,
    HmacOnly = 3,
}

impl KeySource {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Bbram),
            2 => Some(Self::Efuse),
            3 => Some(Self::HmacOnly),
            _ => None,
        }
    }
}

/// Device-side provisioned key store (BBRAM + eFUSE + HMAC key).
#[derive(Clone, Debug)]
pub struct DeviceKeyStore {
    pub bbram_key: Option<[u8; 32]>,
    pub efuse_key: Option<[u8; 32]>,
    pub hmac_key: [u8; 32],
    /// When set (`eFUSE.ENC_ONLY = 1`), unencrypted or wrong-key-source bitstreams are rejected.
    pub force_efuse_key: bool,
}

impl DeviceKeyStore {
    pub fn new(hmac_key: [u8; 32]) -> Self {
        Self {
            bbram_key: None,
            efuse_key: None,
            hmac_key,
            force_efuse_key: false,
        }
    }

    pub fn with_bbram_key(mut self, key: [u8; 32]) -> Self {
        self.bbram_key = Some(key);
        self
    }

    pub fn with_efuse_key(mut self, key: [u8; 32]) -> Self {
        self.efuse_key = Some(key);
        self
    }

    pub fn with_force_efuse_key(mut self, force: bool) -> Self {
        self.force_efuse_key = force;
        self
    }
}

/// Hardware configuration security & tamper-rejection status bits (UG470 `STAT` & `BOOTSTS`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CryptoStatBits {
    pub dec_error: bool,
    pub hmac_error: bool,
    pub key_source_mismatch: bool,
    pub sec_violation: bool,
    pub key_source_active: Option<KeySource>,
    pub keys_rolled: u32,
    pub done: bool,
}

impl CryptoStatBits {
    pub fn has_tamper(&self) -> bool {
        self.dec_error || self.hmac_error || self.key_source_mismatch || self.sec_violation
    }
}

// ============================================================================
// FIPS 180-4 SHA-256 & RFC 2104 HMAC-SHA256
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

/// Compute FIPS 180-4 SHA-256 digest over `data`.
pub fn sha256_digest(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity(data.len() + 72);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0x00);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[4 * i],
                chunk[4 * i + 1],
                chunk[4 * i + 2],
                chunk[4 * i + 3],
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

        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];

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
    for (i, &word) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// Compute RFC 2104 HMAC-SHA256 over `message` using a 256-bit `key`.
pub fn hmac_sha256(key: &[u8; 32], message: &[u8]) -> [u8; 32] {
    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5Cu8; 64];
    for i in 0..32 {
        ipad[i] ^= key[i];
        opad[i] ^= key[i];
    }
    let mut inner = Vec::with_capacity(64 + message.len());
    inner.extend_from_slice(&ipad);
    inner.extend_from_slice(message);
    let inner_hash = sha256_digest(&inner);

    let mut outer = Vec::with_capacity(64 + 32);
    outer.extend_from_slice(&opad);
    outer.extend_from_slice(&inner_hash);
    sha256_digest(&outer)
}

// ============================================================================
// FIPS 197 AES-256 Block Cipher & Key-Rolling AES-256-GCM/CTR
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

const AES_RCON: [u8; 15] = [
    0x00, 0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36, 0x6c, 0xd8, 0xab, 0x4d,
];

fn gf_mul2(x: u8) -> u8 {
    if (x & 0x80) != 0 {
        (x << 1) ^ 0x1b
    } else {
        x << 1
    }
}

fn aes256_expand_key(key: &[u8; 32]) -> [[u8; 16]; 15] {
    let mut w = [[0u8; 4]; 60];
    for i in 0..8 {
        w[i] = [key[4 * i], key[4 * i + 1], key[4 * i + 2], key[4 * i + 3]];
    }
    for i in 8..60 {
        let mut temp = w[i - 1];
        if i % 8 == 0 {
            temp = [
                AES_SBOX[temp[1] as usize] ^ AES_RCON[i / 8],
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
        let prev = w[i - 8];
        w[i] = [
            prev[0] ^ temp[0],
            prev[1] ^ temp[1],
            prev[2] ^ temp[2],
            prev[3] ^ temp[3],
        ];
    }
    let mut rk = [[0u8; 16]; 15];
    for r in 0..15 {
        for c in 0..4 {
            rk[r][4 * c..4 * c + 4].copy_from_slice(&w[4 * r + c]);
        }
    }
    rk
}

/// Encrypt a single 128-bit block using FIPS 197 AES-256.
pub fn aes256_encrypt_block(key: &[u8; 32], block: &[u8; 16]) -> [u8; 16] {
    let rk = aes256_expand_key(key);
    let mut s = *block;
    for i in 0..16 {
        s[i] ^= rk[0][i];
    }
    for round in 1..=14 {
        for b in &mut s {
            *b = AES_SBOX[*b as usize];
        }
        // ShiftRows
        s = [
            s[0], s[5], s[10], s[15], s[4], s[9], s[14], s[3], s[8], s[13], s[2], s[7], s[12],
            s[1], s[6], s[11],
        ];
        // MixColumns on rounds 1..13
        if round < 14 {
            for col in 0..4 {
                let idx = 4 * col;
                let (s0, s1, s2, s3) = (s[idx], s[idx + 1], s[idx + 2], s[idx + 3]);
                let x0 = gf_mul2(s0) ^ gf_mul2(s1) ^ s1 ^ s2 ^ s3;
                let x1 = s0 ^ gf_mul2(s1) ^ gf_mul2(s2) ^ s2 ^ s3;
                let x2 = s0 ^ s1 ^ gf_mul2(s2) ^ gf_mul2(s3) ^ s3;
                let x3 = gf_mul2(s0) ^ s0 ^ s1 ^ s2 ^ gf_mul2(s3);
                s[idx] = x0;
                s[idx + 1] = x1;
                s[idx + 2] = x2;
                s[idx + 3] = x3;
            }
        }
        for i in 0..16 {
            s[i] ^= rk[round][i];
        }
    }
    s
}

/// Derive a rolled 256-bit AES key for configuration frame segment `segment_idx` (UG470 key rolling).
pub fn derive_rolled_key(master_key: &[u8; 32], segment_idx: u32, iv: &[u8; 12]) -> [u8; 32] {
    let mut msg = [0u8; 20];
    msg[0..4].copy_from_slice(b"KROL");
    msg[4..8].copy_from_slice(&segment_idx.to_be_bytes());
    msg[8..20].copy_from_slice(iv);
    hmac_sha256(master_key, &msg)
}

/// Apply AES-256-CTR stream encryption/decryption using `segment_key` and 96-bit `iv`.
fn aes256_ctr_xor(segment_key: &[u8; 32], iv: &[u8; 12], segment_idx: u32, data: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; data.len()];
    for (blk_i, chunk) in data.chunks(16).enumerate() {
        let mut ctr = [0u8; 16];
        ctr[0..12].copy_from_slice(iv);
        let counter_val = ((segment_idx as u32) << 16).wrapping_add(blk_i as u32 + 1);
        ctr[12..16].copy_from_slice(&counter_val.to_be_bytes());
        let ks = aes256_encrypt_block(segment_key, &ctr);
        let base = blk_i * 16;
        for (j, &b) in chunk.iter().enumerate() {
            out[base + j] = b ^ ks[j];
        }
    }
    out
}

/// Compute a 128-bit per-segment authentication tag (AES-256-GCM / GMAC tag over AAD + ciphertext).
fn compute_segment_auth_tag(
    segment_key: &[u8; 32],
    iv: &[u8; 12],
    segment_idx: u32,
    ciphertext: &[u8],
) -> [u8; 16] {
    let mut j0 = [0u8; 16];
    j0[0..12].copy_from_slice(iv);
    j0[12..16].copy_from_slice(&segment_idx.to_be_bytes());
    let mask = aes256_encrypt_block(segment_key, &j0);

    let mut mac_input = Vec::with_capacity(16 + ciphertext.len());
    mac_input.extend_from_slice(iv);
    mac_input.extend_from_slice(&segment_idx.to_be_bytes());
    mac_input.extend_from_slice(ciphertext);
    let mac = hmac_sha256(segment_key, &mac_input);

    let mut tag = [0u8; 16];
    for i in 0..16 {
        tag[i] = mac[i] ^ mask[i];
    }
    tag
}

/// Encrypt and sign a [`Bitstream`] with AES-256 key rolling (`bytes_per_roll_segment`) and
/// SHA-256 HMAC authentication.
pub fn encrypt_and_sign_bitstream(
    bits: &Bitstream,
    key_source: KeySource,
    aes_key: &[u8; 32],
    hmac_key: &[u8; 32],
    iv: [u8; 12],
    bytes_per_roll_segment: usize,
) -> Vec<u8> {
    let plaintext = &bits.packets;
    let seg_size = bytes_per_roll_segment.max(64);
    let segments: Vec<&[u8]> = if plaintext.is_empty() {
        vec![&[]]
    } else {
        plaintext.chunks(seg_size).collect()
    };

    let mut body = Vec::new();
    // 28-byte header:
    // [0..8]   magic "HLNSEC01"
    // [8]      key_source (1=BBRAM, 2=eFUSE, 3=HmacOnly)
    // [9..12]  reserved 0
    // [12..24] 96-bit IV
    // [24..28] num_segments (u32 BE)
    body.extend_from_slice(&AUTH_BITSTREAM_MAGIC);
    body.push(key_source as u8);
    body.extend_from_slice(&[0u8; 3]);
    body.extend_from_slice(&iv);
    body.extend_from_slice(&(segments.len() as u32).to_be_bytes());

    for (seg_idx, seg_plain) in segments.iter().enumerate() {
        let seg_u32 = seg_idx as u32;
        let rolled_key = derive_rolled_key(aes_key, seg_u32, &iv);
        let ciphertext = if key_source == KeySource::HmacOnly {
            seg_plain.to_vec()
        } else {
            aes256_ctr_xor(&rolled_key, &iv, seg_u32, seg_plain)
        };
        let tag = compute_segment_auth_tag(&rolled_key, &iv, seg_u32, &ciphertext);

        body.extend_from_slice(&(ciphertext.len() as u32).to_be_bytes());
        body.extend_from_slice(&tag);
        body.extend_from_slice(&ciphertext);
    }

    // Append 32-byte outer SHA-256 HMAC over the entire header + encrypted frame segments
    let outer_hmac = hmac_sha256(hmac_key, &body);
    body.extend_from_slice(&outer_hmac);
    body
}

/// Verify outer SHA-256 HMAC, validate key source against `keystore`, verify per-segment
/// authentication tags, and decrypt with AES-256 key rolling.
pub fn verify_and_decrypt_bitstream(
    auth_image: &[u8],
    keystore: &DeviceKeyStore,
) -> (CryptoStatBits, Result<Bitstream, String>) {
    let mut stat = CryptoStatBits::default();

    if auth_image.len() < 28 + 32 {
        stat.hmac_error = true;
        stat.sec_violation = true;
        return (
            stat,
            Err("bitstream_crypto: image shorter than header + HMAC".into()),
        );
    }
    if auth_image[0..8] != AUTH_BITSTREAM_MAGIC {
        stat.hmac_error = true;
        stat.sec_violation = true;
        return (
            stat,
            Err("bitstream_crypto: missing HLNSEC01 magic".into()),
        );
    }

    let body_len = auth_image.len() - 32;
    let body = &auth_image[..body_len];
    let expected_hmac = &auth_image[body_len..];
    let computed_hmac = hmac_sha256(&keystore.hmac_key, body);

    // Constant-time comparison of outer SHA-256 HMAC
    let mut diff = 0u8;
    for (a, b) in expected_hmac.iter().zip(computed_hmac.iter()) {
        diff |= a ^ b;
    }
    if diff != 0 {
        stat.hmac_error = true;
        stat.sec_violation = true;
        return (
            stat,
            Err("bitstream_crypto: SHA-256 HMAC authentication failed (tamper detected)".into()),
        );
    }

    let Some(key_source) = KeySource::from_u8(auth_image[8]) else {
        stat.key_source_mismatch = true;
        stat.sec_violation = true;
        return (
            stat,
            Err("bitstream_crypto: invalid key source descriptor".into()),
        );
    };
    stat.key_source_active = Some(key_source);

    if keystore.force_efuse_key && key_source != KeySource::Efuse {
        stat.key_source_mismatch = true;
        stat.sec_violation = true;
        return (
            stat,
            Err("bitstream_crypto: eFUSE.ENC_ONLY requires KeySource::Efuse".into()),
        );
    }

    let master_key = match key_source {
        KeySource::Bbram => match keystore.bbram_key {
            Some(k) => k,
            None => {
                stat.key_source_mismatch = true;
                stat.sec_violation = true;
                return (
                    stat,
                    Err("bitstream_crypto: BBRAM key not provisioned in device".into()),
                );
            }
        },
        KeySource::Efuse => match keystore.efuse_key {
            Some(k) => k,
            None => {
                stat.key_source_mismatch = true;
                stat.sec_violation = true;
                return (
                    stat,
                    Err("bitstream_crypto: eFUSE key not provisioned in device".into()),
                );
            }
        },
        KeySource::HmacOnly => keystore.hmac_key,
    };

    let mut iv = [0u8; 12];
    iv.copy_from_slice(&auth_image[12..24]);
    let num_segments = u32::from_be_bytes([
        auth_image[24],
        auth_image[25],
        auth_image[26],
        auth_image[27],
    ]);

    let mut cursor = 28usize;
    let mut plaintext = Vec::new();

    for seg_idx in 0..num_segments {
        if cursor + 20 > body_len {
            stat.dec_error = true;
            stat.sec_violation = true;
            return (
                stat,
                Err("bitstream_crypto: truncated encrypted frame segment header".into()),
            );
        }
        let seg_len = u32::from_be_bytes([
            body[cursor],
            body[cursor + 1],
            body[cursor + 2],
            body[cursor + 3],
        ]) as usize;
        let mut expected_tag = [0u8; 16];
        expected_tag.copy_from_slice(&body[cursor + 4..cursor + 20]);
        cursor += 20;

        if cursor + seg_len > body_len {
            stat.dec_error = true;
            stat.sec_violation = true;
            return (
                stat,
                Err("bitstream_crypto: truncated encrypted frame segment payload".into()),
            );
        }
        let ciphertext = &body[cursor..cursor + seg_len];
        cursor += seg_len;

        let rolled_key = derive_rolled_key(&master_key, seg_idx, &iv);
        stat.keys_rolled = stat.keys_rolled.saturating_add(1);

        let actual_tag = compute_segment_auth_tag(&rolled_key, &iv, seg_idx, ciphertext);
        let mut tag_diff = 0u8;
        for (a, b) in expected_tag.iter().zip(actual_tag.iter()) {
            tag_diff |= a ^ b;
        }
        if tag_diff != 0 {
            stat.dec_error = true;
            stat.sec_violation = true;
            return (
                stat,
                Err(format!(
                    "bitstream_crypto: AES-256-GCM authentication tag mismatch in segment {seg_idx}"
                )),
            );
        }

        let seg_plain = if key_source == KeySource::HmacOnly {
            ciphertext.to_vec()
        } else {
            aes256_ctr_xor(&rolled_key, &iv, seg_idx, ciphertext)
        };
        plaintext.extend_from_slice(&seg_plain);
    }

    match Bitstream::from_packets(&plaintext) {
        Ok(bits) => {
            stat.done = true;
            (stat, Ok(bits))
        }
        Err(e) => {
            stat.dec_error = true;
            stat.sec_violation = true;
            (stat, Err(format!("bitstream_crypto decode failed: {e}")))
        }
    }
}

/// Authenticate, decrypt (with key rolling), and program `tap` only if all cryptographic checks pass.
/// On any tamper or key mismatch, leaves `tap`'s fabric unprogrammed and sets the corresponding
/// [`CryptoStatBits`] error flags.
pub fn program_authenticated_bitstream(
    tap: &mut Tap,
    auth_image: &[u8],
    keystore: &DeviceKeyStore,
) -> (CryptoStatBits, Result<Stat, String>) {
    let (mut crypto_stat, res) = verify_and_decrypt_bitstream(auth_image, keystore);
    match res {
        Ok(bits) => match tap.program(&bits) {
            Ok(hw_stat) => {
                crypto_stat.done = hw_stat.done;
                (crypto_stat, Ok(hw_stat))
            }
            Err(e) => {
                crypto_stat.done = false;
                crypto_stat.dec_error = true;
                (crypto_stat, Err(e))
            }
        },
        Err(e) => {
            crypto_stat.done = false;
            (crypto_stat, Err(e))
        }
    }
}
