//! XXH3-64、SHA-256、および `TraceHeader` / `TraceRecord` バイト表現の検証（EK-DS-001 2.2節、2.5節）。

use ek_core::{
    Sha256, TRACE_FLAG_HASH_PRESENT, TRACE_FORMAT_MAJOR, TRACE_FORMAT_MINOR,
    TRACE_HASH_ALGORITHM_XXH3_64, TRACE_MAGIC, TraceHeader, TraceRecord, sha256, xxh3_64,
};

fn pattern(len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| ((i.wrapping_mul(37).wrapping_add(13)) & 0xff) as u8)
        .collect()
}

fn hex_to_bytes32(hex: &str) -> [u8; 32] {
    assert_eq!(hex.len(), 64);
    let mut out = [0u8; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
    }
    out
}

#[test]
fn xxh3_64_and_sha256_match_reference_vectors() {
    let vectors: &[(usize, u64, &str)] = &[
        (
            0,
            0x2d06_8005_38d3_94c2,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
        (
            1,
            0x8a21_d78b_1538_b1c0,
            "9d1e0e2d9459d06523ad13e28a4093c2316baafe7aec5b25f30eba2e113599c4",
        ),
        (
            2,
            0xf7ee_8473_4093_23fb,
            "d371376a337b5c558b52611350eca28ba18bdd2d3315cd9568e00afcf81e08e1",
        ),
        (
            3,
            0xa7cb_df98_d7cd_e625,
            "4820e978977641a57f59d05109f5989620f6a9bff43ae2599bc7345d910cfbfa",
        ),
        (
            4,
            0x505e_63b0_2f46_b63e,
            "c2337b330a0ba4d9b6d6fa57eb88ba044d721fc58ce90d8586981153f30a0f81",
        ),
        (
            7,
            0x3380_cf8f_8d1d_4c41,
            "09f8f0d8bd18b13d747601fec452938b42aaadad389d2099b9388e87a40f58f2",
        ),
        (
            8,
            0x8420_85e7_7c82_c646,
            "5b103dfe3697c4101d8f5f53e1f1b4e54497c067b2957b47a3d20a52ef33e721",
        ),
        (
            9,
            0xe574_1a67_33f9_2496,
            "3b7e6995e07baa6f615608786d61c57d2232e7f8652d917ba825b47a07a47929",
        ),
        (
            15,
            0x8cbe_46d3_48ff_2d7d,
            "623a9923be0743f1c3ef96dba68aa2bd5b95df19f8133d504f0838f8b78e0f22",
        ),
        (
            16,
            0xa016_72dc_f82f_6241,
            "8762e307f883f3e9f857f21f1a8c961f438c01a3da352577dba4a21cc3f387db",
        ),
        (
            17,
            0xd21b_b4ed_7c09_bb5f,
            "cf81cfc48e4b75a414b3ade34c5900f434cbb2a15dae7309f75f35d6cf9c6fc8",
        ),
        (
            32,
            0x3698_a36d_25c0_b4dc,
            "d77e13f1159ba07584076a6869f6cdc034a239882ab3bf3f32bb9beffd216608",
        ),
        (
            64,
            0xbee2_af0a_d04e_d842,
            "b3789bd66f7c59090ef934647fa6fd1b0c559d4b883df09800c88e543cc538c0",
        ),
        (
            128,
            0xcc60_2770_c19f_71a8,
            "88d13d4dadf6d93ee67daf4dba4a7dd9c99ffde5629cc0b986a48dbfb641f536",
        ),
        (
            129,
            0xac1d_c6a0_16a6_887a,
            "6650615569f0ad8efb248abe2d04e951e947a07404be85d464ec947899e01ebd",
        ),
        (
            240,
            0x805f_9212_ca09_0ad1,
            "4c6535b819fd59e68290c0a4d1a1faf3eef3802c522407c0896911eafef030a1",
        ),
        (
            241,
            0x45cb_5dec_13e0_32e2,
            "156a8ccc929ac50869719f015747a3121efd3bd1382bd34c57d01e3cb4b179eb",
        ),
        (
            512,
            0xb484_c5f9_fe42_52db,
            "384f39a7b42806a60ba129d0bad9c5bf090398574669e9aa16e09ab788308d09",
        ),
        (
            1024,
            0x1be5_01a2_3bf1_9bca,
            "15c7be47d93f2f2786bda2b188de3c42c35432ce7ce48b15a8b3d56beacdf896",
        ),
        (
            1025,
            0x5417_52dd_8f49_4711,
            "22a7bf35dade4ff361cd1f391926f23d5a72fa8ef042edd10812e2d410d64719",
        ),
        (
            2048,
            0x302b_f1f4_b1f1_5b70,
            "27856cfadd8adc73a7c5c0ee89286eaba8a257c4fb435b3f79f05d81322d9c6c",
        ),
        (
            4096,
            0x0ed8_48d5_f773_7ec6,
            "5b8d1e559b0f247e85c97ea1fe466b280de16d01b9b18545afbf2f4579dc868b",
        ),
    ];

    for &(len, expected_xxh3, expected_sha256) in vectors {
        let data = pattern(len);
        assert_eq!(xxh3_64(&data), expected_xxh3, "xxh3_64 len={len}");
        let expected_digest = hex_to_bytes32(expected_sha256);
        assert_eq!(sha256(&data), expected_digest, "sha256 len={len}");

        // ストリーミング SHA-256 もチャンク境界に関わらず一致する。
        let mut streaming = Sha256::new();
        for chunk in data.chunks(17) {
            streaming.update(chunk);
        }
        assert_eq!(streaming.finalize(), expected_digest, "streaming len={len}");
    }
}

#[test]
fn trace_header_and_record_binary_layout_matches_ds() {
    let header = TraceHeader::new(10_000);
    let bytes = header.to_le_bytes();
    assert_eq!(&bytes[0..4], &TRACE_MAGIC);
    assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), TRACE_FORMAT_MAJOR);
    assert_eq!(u16::from_le_bytes([bytes[6], bytes[7]]), TRACE_FORMAT_MINOR);
    assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), 40);
    assert_eq!(
        u16::from_le_bytes([bytes[12], bytes[13]]),
        TRACE_HASH_ALGORITHM_XXH3_64
    );
    assert_eq!(u16::from_le_bytes([bytes[14], bytes[15]]), 0);
    assert_eq!(
        u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
        10_000
    );
    assert_eq!(&bytes[24..32], &[0u8; 8]);

    let record = TraceRecord {
        timestamp_ns: -42,
        sequence_id: 99,
        stream_id: 0xFFFF_FF00,
        event_type: 18,
        event_flags: 1,
        state_hash: 0x1234_5678_9abc_def0,
        trace_flags: TRACE_FLAG_HASH_PRESENT,
        reserved: [0; 6],
    };
    let rec = record.to_le_bytes();
    assert_eq!(i64::from_le_bytes(rec[0..8].try_into().unwrap()), -42);
    assert_eq!(u64::from_le_bytes(rec[8..16].try_into().unwrap()), 99);
    assert_eq!(
        u32::from_le_bytes(rec[16..20].try_into().unwrap()),
        0xFFFF_FF00
    );
    assert_eq!(u16::from_le_bytes([rec[20], rec[21]]), 18);
    assert_eq!(u16::from_le_bytes([rec[22], rec[23]]), 1);
    assert_eq!(
        u64::from_le_bytes(rec[24..32].try_into().unwrap()),
        0x1234_5678_9abc_def0
    );
    assert_eq!(
        u16::from_le_bytes([rec[32], rec[33]]),
        TRACE_FLAG_HASH_PRESENT
    );
    assert_eq!(&rec[34..40], &[0u8; 6]);
}
