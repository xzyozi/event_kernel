//! 決定論的ハッシュ関数（EK-DS-001 2.5節、OPEN-05）。
//!
//! 外部クレートを使わず、`#![no_std]` かつ `#![forbid(unsafe_code)]` で
//! XXH3-64（シード 0、ステップハッシュ用）と SHA-256（最終状態・トレースログ用）を実装する。

const PRIME32_1: u64 = 0x9E37_79B1;
const PRIME32_2: u64 = 0x85EB_CA77;
const PRIME32_3: u64 = 0xC2B2_AE3D;

const PRIME64_1: u64 = 0x9E37_79B1_85EB_CA87;
const PRIME64_2: u64 = 0xC2B2_AE3D_27D4_EB4F;
const PRIME64_3: u64 = 0x1656_67B1_9E37_79F9;
const PRIME64_4: u64 = 0x85EB_CA77_C2B2_AE63;
const PRIME64_5: u64 = 0x27D4_EB2F_1656_67C5;

const STRIPE_LEN: usize = 64;
const ACC_NB: usize = 8;
const SECRET_CONSUME_RATE: usize = 8;
const SECRET_DEFAULT_SIZE: usize = 192;
const NB_STRIPES_PER_BLOCK: usize = (SECRET_DEFAULT_SIZE - STRIPE_LEN) / SECRET_CONSUME_RATE;
const BLOCK_LEN: usize = STRIPE_LEN * NB_STRIPES_PER_BLOCK;

/// xxHash v0.8.x 標準の既定シークレット（192バイト）。
const SECRET_DEFAULT: [u8; SECRET_DEFAULT_SIZE] = [
    0xb8, 0xfe, 0x6c, 0x39, 0x23, 0xa4, 0x4b, 0xbe, 0x7c, 0x01, 0x81, 0x2c, 0xf7, 0x21, 0xad, 0x1c,
    0xde, 0xd4, 0x6d, 0xe9, 0x83, 0x90, 0x97, 0xdb, 0x72, 0x40, 0xa4, 0xa4, 0xb7, 0xb3, 0x67, 0x1f,
    0xcb, 0x79, 0xe6, 0x4e, 0xcc, 0xc0, 0xe5, 0x78, 0x82, 0x5a, 0xd0, 0x7d, 0xcc, 0xff, 0x72, 0x21,
    0xb8, 0x08, 0x46, 0x74, 0xf7, 0x43, 0x24, 0x8e, 0xe0, 0x35, 0x90, 0xe6, 0x81, 0x3a, 0x26, 0x4c,
    0x3c, 0x28, 0x52, 0xbb, 0x91, 0xc3, 0x00, 0xcb, 0x88, 0xd0, 0x65, 0x8b, 0x1b, 0x53, 0x2e, 0xa3,
    0x71, 0x64, 0x48, 0x97, 0xa2, 0x0d, 0xf9, 0x4e, 0x38, 0x19, 0xef, 0x46, 0xa9, 0xde, 0xac, 0xd8,
    0xa8, 0xfa, 0x76, 0x3f, 0xe3, 0x9c, 0x34, 0x3f, 0xf9, 0xdc, 0xbb, 0xc7, 0xc7, 0x0b, 0x4f, 0x1d,
    0x8a, 0x51, 0xe0, 0x4b, 0xcd, 0xb4, 0x59, 0x31, 0xc8, 0x9f, 0x7e, 0xc9, 0xd9, 0x78, 0x73, 0x64,
    0xea, 0xc5, 0xac, 0x83, 0x34, 0xd3, 0xeb, 0xc3, 0xc5, 0x81, 0xa0, 0xff, 0xfa, 0x13, 0x63, 0xeb,
    0x17, 0x0d, 0xdd, 0x51, 0xb7, 0xf0, 0xda, 0x49, 0xd3, 0x16, 0x55, 0x26, 0x29, 0xd4, 0x68, 0x9e,
    0x2b, 0x16, 0xbe, 0x58, 0x7d, 0x47, 0xa1, 0xfc, 0x8f, 0xf8, 0xb8, 0xd1, 0x7a, 0xd0, 0x31, 0xce,
    0x45, 0xcb, 0x3a, 0x8f, 0x95, 0x16, 0x04, 0x28, 0xaf, 0xd7, 0xfb, 0xca, 0xbb, 0x4b, 0x40, 0x7e,
];

#[inline]
fn read_u32_le(bytes: &[u8], offset: usize) -> u32 {
    let b0 = u32::from(bytes.get(offset).copied().unwrap_or(0));
    let b1 = u32::from(bytes.get(offset + 1).copied().unwrap_or(0));
    let b2 = u32::from(bytes.get(offset + 2).copied().unwrap_or(0));
    let b3 = u32::from(bytes.get(offset + 3).copied().unwrap_or(0));
    b0 | (b1 << 8) | (b2 << 16) | (b3 << 24)
}

#[inline]
fn read_u64_le(bytes: &[u8], offset: usize) -> u64 {
    let lo = u64::from(read_u32_le(bytes, offset));
    let hi = u64::from(read_u32_le(bytes, offset + 4));
    lo | (hi << 32)
}

#[inline]
fn read_u32_be(bytes: &[u8], offset: usize) -> u32 {
    let b0 = u32::from(bytes.get(offset).copied().unwrap_or(0));
    let b1 = u32::from(bytes.get(offset + 1).copied().unwrap_or(0));
    let b2 = u32::from(bytes.get(offset + 2).copied().unwrap_or(0));
    let b3 = u32::from(bytes.get(offset + 3).copied().unwrap_or(0));
    (b0 << 24) | (b1 << 16) | (b2 << 8) | b3
}

#[inline]
const fn mult128_fold64(lhs: u64, rhs: u64) -> u64 {
    let product = (lhs as u128).wrapping_mul(rhs as u128);
    (product as u64) ^ ((product >> 64) as u64)
}

#[inline]
const fn xxh64_avalanche(mut h: u64) -> u64 {
    h ^= h >> 33;
    h = h.wrapping_mul(PRIME64_2);
    h ^= h >> 29;
    h = h.wrapping_mul(PRIME64_3);
    h ^ (h >> 32)
}

#[inline]
const fn avalanche(mut h: u64) -> u64 {
    h ^= h >> 37;
    h = h.wrapping_mul(0x1656_6791_9E37_79F9);
    h ^ (h >> 32)
}

#[inline]
const fn rrmxmx(mut h: u64, len: u64) -> u64 {
    h ^= h.rotate_left(49) ^ h.rotate_left(24);
    h = h.wrapping_mul(0x9FB2_1C65_1E98_DF25);
    h ^= (h >> 35).wrapping_add(len);
    h = h.wrapping_mul(0x9FB2_1C65_1E98_DF25);
    h ^ (h >> 28)
}

#[inline]
fn mix16b(input: &[u8], input_off: usize, secret_off: usize) -> u64 {
    let il = read_u64_le(input, input_off);
    let ih = read_u64_le(input, input_off + 8);
    let kl = read_u64_le(&SECRET_DEFAULT, secret_off);
    let kh = read_u64_le(&SECRET_DEFAULT, secret_off + 8);
    mult128_fold64(il ^ kl, ih ^ kh)
}

#[inline]
fn accumulate_512(acc: &mut [u64; ACC_NB], stripe: &[u8], secret_off: usize) {
    for i in 0..ACC_NB {
        let data_val = read_u64_le(stripe, i * 8);
        let data_key = data_val ^ read_u64_le(&SECRET_DEFAULT, secret_off + i * 8);
        if let Some(other) = acc.get_mut(i ^ 1) {
            *other = other.wrapping_add(data_val);
        }
        let product = (data_key & 0xFFFF_FFFF).wrapping_mul(data_key >> 32);
        if let Some(slot) = acc.get_mut(i) {
            *slot = slot.wrapping_add(product);
        }
    }
}

#[inline]
fn scramble_acc(acc: &mut [u64; ACC_NB], secret_off: usize) {
    for i in 0..ACC_NB {
        let key = read_u64_le(&SECRET_DEFAULT, secret_off + i * 8);
        if let Some(slot) = acc.get_mut(i) {
            *slot ^= *slot >> 47;
            *slot ^= key;
            *slot = slot.wrapping_mul(PRIME32_1);
        }
    }
}

/// XXH3-64（シード 0、標準既定シークレット）。
pub fn xxh3_64(input: &[u8]) -> u64 {
    let len = input.len();
    if len == 0 {
        return xxh64_avalanche(
            read_u64_le(&SECRET_DEFAULT, 56) ^ read_u64_le(&SECRET_DEFAULT, 64),
        );
    }
    if len <= 3 {
        let c1 = u32::from(input.first().copied().unwrap_or(0));
        let c2 = u32::from(input.get(len >> 1).copied().unwrap_or(0));
        let c3 = u32::from(input.get(len - 1).copied().unwrap_or(0));
        let combined = (c1 << 16) | (c2 << 24) | c3 | ((len as u32) << 8);
        let bitflip = u64::from(read_u32_le(&SECRET_DEFAULT, 0) ^ read_u32_le(&SECRET_DEFAULT, 4));
        return xxh64_avalanche(u64::from(combined) ^ bitflip);
    }
    if len <= 8 {
        let il = u64::from(read_u32_le(input, 0));
        let ih = u64::from(read_u32_le(input, len - 4));
        let combined = ih | (il << 32);
        let bitflip = read_u64_le(&SECRET_DEFAULT, 8) ^ read_u64_le(&SECRET_DEFAULT, 16);
        return rrmxmx(combined ^ bitflip, len as u64);
    }
    if len <= 16 {
        let bitflip1 = read_u64_le(&SECRET_DEFAULT, 24) ^ read_u64_le(&SECRET_DEFAULT, 32);
        let bitflip2 = read_u64_le(&SECRET_DEFAULT, 40) ^ read_u64_le(&SECRET_DEFAULT, 48);
        let il = read_u64_le(input, 0) ^ bitflip1;
        let ih = read_u64_le(input, len - 8) ^ bitflip2;
        let acc = (len as u64)
            .wrapping_add(il.swap_bytes())
            .wrapping_add(ih)
            .wrapping_add(mult128_fold64(il, ih));
        return avalanche(acc);
    }
    if len <= 128 {
        let mut acc = (len as u64).wrapping_mul(PRIME64_1);
        if len > 32 {
            if len > 64 {
                if len > 96 {
                    acc = acc.wrapping_add(mix16b(input, 48, 96));
                    acc = acc.wrapping_add(mix16b(input, len - 64, 112));
                }
                acc = acc.wrapping_add(mix16b(input, 32, 64));
                acc = acc.wrapping_add(mix16b(input, len - 48, 80));
            }
            acc = acc.wrapping_add(mix16b(input, 16, 32));
            acc = acc.wrapping_add(mix16b(input, len - 32, 48));
        }
        acc = acc.wrapping_add(mix16b(input, 0, 0));
        acc = acc.wrapping_add(mix16b(input, len - 16, 16));
        return avalanche(acc);
    }
    if len <= 240 {
        let mut acc = (len as u64).wrapping_mul(PRIME64_1);
        let nb_rounds = len / 16;
        for i in 0..8 {
            acc = acc.wrapping_add(mix16b(input, i * 16, i * 16));
        }
        acc = avalanche(acc);
        for i in 8..nb_rounds {
            acc = acc.wrapping_add(mix16b(input, i * 16, (i - 8) * 16 + 3));
        }
        acc = acc.wrapping_add(mix16b(input, len - 16, 136 - 17));
        return avalanche(acc);
    }

    let mut acc: [u64; ACC_NB] = [
        PRIME32_3, PRIME64_1, PRIME64_2, PRIME64_3, PRIME64_4, PRIME32_2, PRIME64_5, PRIME32_1,
    ];
    let nb_blocks = (len - 1) / BLOCK_LEN;
    for b in 0..nb_blocks {
        let block_off = b * BLOCK_LEN;
        for s in 0..NB_STRIPES_PER_BLOCK {
            if let Some(stripe) = input.get(block_off + s * STRIPE_LEN..) {
                accumulate_512(&mut acc, stripe, s * SECRET_CONSUME_RATE);
            }
        }
        scramble_acc(&mut acc, SECRET_DEFAULT_SIZE - STRIPE_LEN);
    }

    let last_block_off = nb_blocks * BLOCK_LEN;
    let nb_stripes = ((len - 1) - last_block_off) / STRIPE_LEN;
    for s in 0..nb_stripes {
        if let Some(stripe) = input.get(last_block_off + s * STRIPE_LEN..) {
            accumulate_512(&mut acc, stripe, s * SECRET_CONSUME_RATE);
        }
    }
    if let Some(last_stripe) = input.get(len - STRIPE_LEN..) {
        accumulate_512(&mut acc, last_stripe, SECRET_DEFAULT_SIZE - STRIPE_LEN - 7);
    }

    let mut result = (len as u64).wrapping_mul(PRIME64_1);
    for i in 0..4 {
        let lhs = acc.get(i * 2).copied().unwrap_or(0) ^ read_u64_le(&SECRET_DEFAULT, 11 + i * 16);
        let rhs = acc.get(i * 2 + 1).copied().unwrap_or(0)
            ^ read_u64_le(&SECRET_DEFAULT, 11 + i * 16 + 8);
        result = result.wrapping_add(mult128_fold64(lhs, rhs));
    }
    avalanche(result)
}

const SHA256_K: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];

/// ストリーミング SHA-256（FIPS 180-4）。
#[derive(Clone, Debug)]
pub struct Sha256 {
    state: [u32; 8],
    bit_len: u64,
    block: [u8; 64],
    block_len: usize,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    /// 初期状態を作成する。
    pub const fn new() -> Self {
        Self {
            state: [
                0x6a09_e667,
                0xbb67_ae85,
                0x3c6e_f372,
                0xa54f_f53a,
                0x510e_527f,
                0x9b05_688c,
                0x1f83_d9ab,
                0x5be0_cd19,
            ],
            bit_len: 0,
            block: [0; 64],
            block_len: 0,
        }
    }

    /// バイト列を入力へ追加する。
    pub fn update(&mut self, mut data: &[u8]) {
        self.bit_len = self.bit_len.wrapping_add((data.len() as u64) << 3);
        if self.block_len > 0 {
            let need = 64 - self.block_len;
            let take = need.min(data.len());
            if let (Some(dst), Some(src)) = (
                self.block.get_mut(self.block_len..self.block_len + take),
                data.get(..take),
            ) {
                dst.copy_from_slice(src);
            }
            self.block_len += take;
            data = data.get(take..).unwrap_or(&[]);
            if self.block_len == 64 {
                let block = self.block;
                compress(&mut self.state, &block);
                self.block_len = 0;
            }
        }
        while data.len() >= 64 {
            if let Some(chunk) = data.get(..64) {
                compress(&mut self.state, chunk);
            }
            data = data.get(64..).unwrap_or(&[]);
        }
        if !data.is_empty() {
            if let Some(dst) = self.block.get_mut(..data.len()) {
                dst.copy_from_slice(data);
            }
            self.block_len = data.len();
        }
    }

    /// 32バイトの SHA-256 ダイジェストを確定して返す。
    pub fn finalize(mut self) -> [u8; 32] {
        let total_bits = self.bit_len;
        self.update(&[0x80]);
        if self.block_len > 56 {
            if let Some(pad) = self.block.get_mut(self.block_len..64) {
                pad.fill(0);
            }
            let block = self.block;
            compress(&mut self.state, &block);
            self.block_len = 0;
        }
        if let Some(pad) = self.block.get_mut(self.block_len..56) {
            pad.fill(0);
        }
        if let Some(len_slot) = self.block.get_mut(56..64) {
            len_slot.copy_from_slice(&total_bits.to_be_bytes());
        }
        let block = self.block;
        compress(&mut self.state, &block);

        let mut out = [0u8; 32];
        for (i, &word) in self.state.iter().enumerate() {
            if let Some(slot) = out.get_mut(i * 4..i * 4 + 4) {
                slot.copy_from_slice(&word.to_be_bytes());
            }
        }
        out
    }
}

/// 一括で SHA-256 を計算する。
pub fn sha256(input: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(input);
    hasher.finalize()
}

fn compress(state: &mut [u32; 8], block: &[u8]) {
    let mut w = [0u32; 64];
    for i in 0..16 {
        if let Some(slot) = w.get_mut(i) {
            *slot = read_u32_be(block, i * 4);
        }
    }
    for i in 16..64 {
        let w15 = w.get(i - 15).copied().unwrap_or(0);
        let w2 = w.get(i - 2).copied().unwrap_or(0);
        let w16 = w.get(i - 16).copied().unwrap_or(0);
        let w7 = w.get(i - 7).copied().unwrap_or(0);
        let s0 = w15.rotate_right(7) ^ w15.rotate_right(18) ^ (w15 >> 3);
        let s1 = w2.rotate_right(17) ^ w2.rotate_right(19) ^ (w2 >> 10);
        let val = w16.wrapping_add(s0).wrapping_add(w7).wrapping_add(s1);
        if let Some(slot) = w.get_mut(i) {
            *slot = val;
        }
    }

    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ ((!e) & g);
        let ki = SHA256_K.get(i).copied().unwrap_or(0);
        let wi = w.get(i).copied().unwrap_or(0);
        let temp1 = h
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(ki)
            .wrapping_add(wi);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let temp2 = s0.wrapping_add(maj);

        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(temp1);
        d = c;
        c = b;
        b = a;
        a = temp1.wrapping_add(temp2);
    }

    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
    state[5] = state[5].wrapping_add(f);
    state[6] = state[6].wrapping_add(g);
    state[7] = state[7].wrapping_add(h);
}
