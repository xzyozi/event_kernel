//! 決定論 PRNG（EK-DD-001 2.6節）。Xoshiro256++ で、シードは SplitMix64 で展開する。

/// Xoshiro256++。状態は32バイト（`u64` が4つ）。
///
/// 状態がすべて0になると、以後すべて0を返す。`from_seed` では起きない。
/// `from_state` には、すべて0ではない状態を渡す。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prng {
    state: [u64; 4],
}

impl Prng {
    /// 64ビットのシードから、SplitMix64 を4回呼んで状態を作る。
    pub fn from_seed(seed: u64) -> Self {
        let mut expander = seed;
        let mut state = [0u64; 4];
        for slot in &mut state {
            *slot = splitmix64(&mut expander);
        }
        Self { state }
    }

    /// 保存した状態から再開する（スナップショットの復元用）。
    pub const fn from_state(state: [u64; 4]) -> Self {
        Self { state }
    }

    /// 現在の状態（スナップショットへ保存する値）。
    pub const fn state(&self) -> [u64; 4] {
        self.state
    }

    /// 次の64ビットの乱数。
    pub fn next_u64(&mut self) -> u64 {
        let [s0, s1, s2, s3] = self.state;
        let result = s0.wrapping_add(s3).rotate_left(23).wrapping_add(s0);
        let t = s1 << 17;
        let s2 = s2 ^ s0;
        let s3 = s3 ^ s1;
        let s1 = s1 ^ s2;
        let s0 = s0 ^ s3;
        let s2 = s2 ^ t;
        let s3 = s3.rotate_left(45);
        self.state = [s0, s1, s2, s3];
        result
    }
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
