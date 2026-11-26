//! 決定論 PRNG（TR-017 の一部、EK-DD-001 2.6節）。

use ek_core::Prng;

#[test]
fn seed_expansion_matches_splitmix64_reference() {
    // SplitMix64 の参照値（seed = 0 の最初の4つ）。
    assert_eq!(
        Prng::from_seed(0).state(),
        [
            0xE220_A839_7B1D_CDAF,
            0x6E78_9E6A_A1B9_65F4,
            0x06C4_5D18_8009_454F,
            0xF88B_B8A8_724C_81EC
        ]
    );
}

#[test]
fn xoshiro256_plus_plus_matches_reference_sequence() {
    // 参照実装で、状態 [1, 2, 3, 4] から出る最初の値。
    let mut prng = Prng::from_state([1, 2, 3, 4]);
    let drawn: Vec<u64> = (0..4).map(|_| prng.next_u64()).collect();
    assert_eq!(
        drawn,
        vec![
            41_943_041,
            58_720_359,
            3_588_806_011_781_223,
            3_591_011_842_654_386
        ]
    );
}

#[test]
fn same_seed_gives_same_sequence_and_different_seed_differs() {
    let draw = |seed| {
        let mut prng = Prng::from_seed(seed);
        (0..16).map(|_| prng.next_u64()).collect::<Vec<_>>()
    };
    assert_eq!(draw(7), draw(7));
    assert_ne!(draw(7), draw(8));
}

#[test]
fn saved_state_resumes_the_same_stream() {
    let mut original = Prng::from_seed(42);
    for _ in 0..5 {
        original.next_u64();
    }
    let mut resumed = Prng::from_state(original.state());
    for _ in 0..10 {
        assert_eq!(original.next_u64(), resumed.next_u64());
    }
}
