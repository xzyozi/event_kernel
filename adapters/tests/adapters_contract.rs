use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use ek_adapters::{
    BinarySoaFeed, CsvFeed, DiskSnapshotStore, DiskTraceSink, EventInputRecord,
    MemorySnapshotStore, ParquetFeed, SimExecutionAdapter, TraceLogReader, VecFeedAdapter,
    validate_run_id, write_parquet_events,
};
use ek_core::{
    Action, Context, ERR_PORT_FAILURE, ERR_SNAPSHOT_INVALID, ERR_SNAPSHOT_IO, EventHeader,
    EventRecord, Kernel, KernelConfig, RUN_SNAPSHOT_REQUESTED, RunLimit, STEP_FINISHED,
    SnapshotPort, Strategy, StrategyStatus, TraceHeader, event_type, flags,
};

struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(prefix: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(1);
        let id = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "ek_adapters_test_{prefix}_{}_{}",
            std::process::id(),
            id
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        Self { path: dir }
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct OrderEveryTickStrategy;

impl Strategy for OrderEveryTickStrategy {
    fn on_event(&mut self, ctx: &mut Context<'_>, event: EventRecord<'_>) -> StrategyStatus {
        if event.header.event_type == event_type::TICK {
            let rnd = ctx.rng().next_u64();
            let arena = ctx.strategy_arena();
            if arena.len() >= 8 {
                arena[..8].copy_from_slice(&rnd.to_le_bytes());
            }
            let _ = ctx.submit_action(&Action {
                action_kind: 1,
                latency_ns: 5,
                payload: &rnd.to_le_bytes(),
            });
        }
        0
    }
}

fn sample_records() -> Vec<EventInputRecord> {
    (0..6u64)
        .map(|i| {
            let ts = (i as i64 + 1) * 100;
            let mut payload = vec![0u8; 32];
            payload[..8].copy_from_slice(&(10_000 + i as i64).to_le_bytes());
            EventInputRecord {
                header: EventHeader {
                    timestamp_ns: ts,
                    sequence_id: i + 1,
                    stream_id: 0,
                    event_type: event_type::TICK,
                    flags: if i == 2 { flags::SNAPSHOT_TRIGGER } else { 0 },
                    payload_idx: 0,
                },
                payload,
            }
        })
        .collect()
}

fn test_config(trace_enabled: bool) -> KernelConfig {
    KernelConfig {
        history_ring_len: 16,
        arena_size: 4096,
        strategy_arena_offset: 0,
        strategy_arena_size: 256,
        action_queue_capacity: 64,
        payload_record_size: 32,
        price_scale_exp: -2,
        qty_scale_exp: 0,
        prng_seed: 12345,
        exec_state_max_bytes: 1024,
        feed_token_max_bytes: 64,
        hash_interval: 1,
        trace_enabled,
        max_total_bytes: 1 << 24,
    }
}

#[test]
fn tc016_disk_snapshot_atomic_write_collision_and_scoped_cleanup() {
    assert_eq!(validate_run_id(""), Err(ERR_SNAPSHOT_IO));
    assert_eq!(validate_run_id("run_bad"), Err(ERR_SNAPSHOT_IO));
    assert_eq!(validate_run_id("run/bad"), Err(ERR_SNAPSHOT_IO));
    assert_eq!(validate_run_id("run-ok-01"), Ok(()));

    let temp = TempDirGuard::new("snap");
    // 同じ接頭辞だが別 run_id (`run` vs `run-2`) の .tmp 誤削除を防止すること（DS 3.1節手順5）。
    fs::write(temp.path.join(".run_100_1.eksnap.tmp"), b"stale").unwrap();
    fs::write(temp.path.join(".run-2_100_1.eksnap.tmp"), b"keep-other").unwrap();
    fs::write(temp.path.join("unrelated.txt"), b"keep-unrelated").unwrap();

    let mut store = DiskSnapshotStore::open(&temp.path, "run").expect("open store");
    assert_eq!(store.cleaned_tmp_files(), 1);
    assert!(!temp.path.join(".run_100_1.eksnap.tmp").exists());
    assert!(temp.path.join(".run-2_100_1.eksnap.tmp").exists());
    assert!(temp.path.join("unrelated.txt").exists());

    let feed = VecFeedAdapter::new(sample_records());
    let kernel = Kernel::create(
        test_config(false),
        feed,
        SimExecutionAdapter::default(),
        OrderEveryTickStrategy,
    )
    .expect("create");

    // SNAPSHOT_TRIGGER (i=2 -> ts=300) で停止し、capture 実行。
    let r1 = kernel.run(RunLimit::default());
    assert_eq!(r1.now_ns, 300);
    assert_eq!(kernel.capture(&mut store), 0);

    let snap_path = store.snapshot_path(r1.now_ns, r1.processed_event_count);
    assert!(snap_path.exists());
    assert!(
        !temp
            .path
            .join(format!(
                ".run_{}_{}.eksnap.tmp",
                r1.now_ns, r1.processed_event_count
            ))
            .exists()
    );

    let original_bytes = fs::read(&snap_path).unwrap();
    // 同一 (run_id, logical_time_ns, event_count) への再保存は失敗し既存ファイルを変えない。
    assert_eq!(kernel.capture(&mut store), ERR_SNAPSHOT_IO);
    assert_eq!(fs::read(&snap_path).unwrap(), original_bytes);

    // load() は DS 4.3節の形式検査を行う。破損時は ERR_SNAPSHOT_INVALID で拒否する。
    let loaded = store.load(r1.now_ns, r1.processed_event_count).unwrap();
    assert_eq!(loaded, original_bytes);

    let mut corrupt = original_bytes.clone();
    let last_idx = corrupt.len() - 1;
    corrupt[last_idx] ^= 0xFF;
    let bad_path = temp.path.join("corrupt.eksnap");
    fs::write(&bad_path, corrupt).unwrap();
    assert_eq!(
        DiskSnapshotStore::load_from_path(&bad_path),
        Err(ERR_SNAPSHOT_INVALID)
    );

    // インメモリストアも重複書込拒否・形式検査を行う。
    let mut mem_store = MemorySnapshotStore::new("mem-run-1").unwrap();
    assert!(
        mem_store
            .store(&original_bytes, r1.now_ns, r1.processed_event_count)
            .is_ok()
    );
    assert!(
        mem_store
            .store(&original_bytes, r1.now_ns, r1.processed_event_count)
            .is_err()
    );
    assert_eq!(
        mem_store.load(r1.now_ns, r1.processed_event_count).unwrap(),
        original_bytes
    );
}

#[test]
fn tc016_disk_trace_exclusive_create_and_partial_tail_truncation() {
    let temp = TempDirGuard::new("trace");
    let trace = DiskTraceSink::create(&temp.path, "trace-run-1").expect("create trace");
    let trace_path = trace.path().to_path_buf();

    // 同一 run_id で再作成すると上書きせず失敗する（DS 3.1節）。
    assert_eq!(
        DiskTraceSink::create(&temp.path, "trace-run-1").err(),
        Some(ERR_PORT_FAILURE)
    );

    let feed = VecFeedAdapter::new(sample_records());
    let kernel = Kernel::create_with_trace(
        test_config(true),
        feed,
        SimExecutionAdapter::default(),
        OrderEveryTickStrategy,
        trace,
    )
    .expect("create kernel with trace");
    let res_mid = kernel.run(RunLimit::default());
    assert_eq!(res_mid.status, RUN_SNAPSHOT_REQUESTED);
    let res = kernel.run(RunLimit::default());
    assert_eq!(res.status, STEP_FINISHED);
    let (_, _, _, mut trace_back) = kernel.destroy_with_trace();
    trace_back.sync().unwrap();
    drop(trace_back);

    let expected_records = usize::try_from(res.processed_event_count).unwrap();
    let parsed_full = TraceLogReader::read_file(&trace_path).unwrap();
    assert_eq!(parsed_full.records.len(), expected_records);
    assert_eq!(parsed_full.truncated_tail_bytes, 0);

    // 異常終了を模擬し末尾へ 13 バイトの不完全レコードを追記 -> 端数破棄で復元できること。
    let mut f = OpenOptions::new().append(true).open(&trace_path).unwrap();
    f.write_all(&[0xAB; 13]).unwrap();
    drop(f);

    let parsed_partial = TraceLogReader::read_file(&trace_path).unwrap();
    assert_eq!(parsed_partial.records, parsed_full.records);
    assert_eq!(parsed_partial.truncated_tail_bytes, 13);

    // DS 4.3節ヘッダ違反検査（不正 record_size）。
    let mut bad_hdr = TraceHeader::new(1).to_le_bytes();
    bad_hdr[8..12].copy_from_slice(&44u32.to_le_bytes()); // 8の倍数でない
    assert_eq!(
        TraceLogReader::from_bytes(&bad_hdr),
        Err(ERR_SNAPSHOT_INVALID)
    );
}

#[test]
fn parquet_csv_binary_feeds_and_restore_determinism() {
    let temp = TempDirGuard::new("feeds");
    let records = sample_records();

    // Parquet 生成と読込
    let pq_path = temp.path.join("events.parquet");
    write_parquet_events(&pq_path, &records).expect("write parquet");
    let pq_feed = ParquetFeed::from_path(&pq_path).expect("read parquet");

    // バイナリ SoA 生成と読込
    let bin_path = temp.path.join("events.ekev");
    BinarySoaFeed::write_file(&bin_path, &records, 32).expect("write binary");
    let bin_feed = BinarySoaFeed::from_path(&bin_path).expect("read binary");

    // CSV 生成と読込
    let mut csv_text =
        String::from("timestamp_ns,sequence_id,stream_id,event_type,flags,payload_hex\n");
    for rec in &records {
        let hex: String = rec.payload.iter().map(|b| format!("{b:02x}")).collect();
        csv_text.push_str(&format!(
            "{},{},{},{},{},{hex}\n",
            rec.header.timestamp_ns,
            rec.header.sequence_id,
            rec.header.stream_id,
            rec.header.event_type,
            rec.header.flags
        ));
    }
    let csv_path = temp.path.join("events.csv");
    fs::write(&csv_path, csv_text).unwrap();
    let csv_feed = CsvFeed::from_path(&csv_path).expect("read csv");

    // 1. ParquetFeed で途中スナップショット → 最後まで完走 → 分岐後 restore → 再完走
    let mut snap_store = DiskSnapshotStore::open(&temp.path, "pq-run").unwrap();
    let trace_pq = DiskTraceSink::create(&temp.path, "pq-trace").unwrap();
    let k_pq = Kernel::create_with_trace(
        test_config(true),
        pq_feed,
        SimExecutionAdapter::new(5, 25_000, 2),
        OrderEveryTickStrategy,
        trace_pq,
    )
    .unwrap();
    let stop_mid = k_pq.run(RunLimit::default());
    assert_eq!(stop_mid.status, RUN_SNAPSHOT_REQUESTED);
    assert_eq!(k_pq.capture(&mut snap_store), 0);
    let end_pq = k_pq.run(RunLimit::default());
    assert_eq!(end_pq.status, STEP_FINISHED);
    let digest_pq = k_pq.state_digest().unwrap();
    let (_, _, _, mut trace_pq_done) = k_pq.destroy_with_trace();
    trace_pq_done.sync().unwrap();
    let trace_pq_bytes = fs::read(trace_pq_done.path()).unwrap();

    // 新規カーネルへスナップショットから復元して完走しても同一 state_digest になること
    let snap_blob = snap_store
        .load(stop_mid.now_ns, stop_mid.processed_event_count)
        .unwrap();
    let pq_feed_restored = ParquetFeed::from_path(&pq_path).expect("read parquet restored");
    let k_pq_restored = Kernel::create_with_trace(
        test_config(true),
        pq_feed_restored,
        SimExecutionAdapter::new(5, 25_000, 2),
        OrderEveryTickStrategy,
        DiskTraceSink::create(&temp.path, "pq-restored-trace").unwrap(),
    )
    .unwrap();
    assert_eq!(k_pq_restored.restore(&snap_blob), 0);
    let end_restored = k_pq_restored.run(RunLimit::default());
    assert_eq!(end_restored.status, STEP_FINISHED);
    assert_eq!(k_pq_restored.state_digest().unwrap(), digest_pq);

    // 2. BinarySoaFeed と CsvFeed の最終状態ハッシュおよびトレースファイルが ParquetFeed と完全一致すること
    let trace_bin = DiskTraceSink::create(&temp.path, "bin-trace").unwrap();
    let k_bin = Kernel::create_with_trace(
        test_config(true),
        bin_feed,
        SimExecutionAdapter::new(5, 25_000, 2),
        OrderEveryTickStrategy,
        trace_bin,
    )
    .unwrap();
    assert_eq!(
        k_bin.run(RunLimit::default()).status,
        RUN_SNAPSHOT_REQUESTED
    );
    assert_eq!(k_bin.run(RunLimit::default()).status, STEP_FINISHED);
    assert_eq!(k_bin.state_digest().unwrap(), digest_pq);
    let (_, _, _, mut trace_bin_done) = k_bin.destroy_with_trace();
    trace_bin_done.sync().unwrap();
    assert_eq!(fs::read(trace_bin_done.path()).unwrap(), trace_pq_bytes);

    let trace_csv = DiskTraceSink::create(&temp.path, "csv-trace").unwrap();
    let k_csv = Kernel::create_with_trace(
        test_config(true),
        csv_feed,
        SimExecutionAdapter::new(5, 25_000, 2),
        OrderEveryTickStrategy,
        trace_csv,
    )
    .unwrap();
    assert_eq!(
        k_csv.run(RunLimit::default()).status,
        RUN_SNAPSHOT_REQUESTED
    );
    assert_eq!(k_csv.run(RunLimit::default()).status, STEP_FINISHED);
    assert_eq!(k_csv.state_digest().unwrap(), digest_pq);
    let (_, _, _, mut trace_csv_done) = k_csv.destroy_with_trace();
    trace_csv_done.sync().unwrap();
    assert_eq!(fs::read(trace_csv_done.path()).unwrap(), trace_pq_bytes);
}
