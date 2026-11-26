//! 複数外部フィード（K-way マージ、EK-BD-001 C-003、EK-DD-001 2.3/2.5/3.1節、TR-003/TR-021）。

mod common;

use std::sync::Arc;
use std::thread;

use common::*;
use ek_core::*;

fn feed_event(timestamp_ns: i64, stream_id: u32, sequence_id: u64) -> EventHeader {
    EventHeader {
        timestamp_ns,
        sequence_id,
        stream_id,
        event_type: event_type::TICK,
        flags: 0,
        payload_idx: 0,
    }
}

struct StreamFeed {
    events: Vec<EventHeader>,
    next: usize,
    seekable: bool,
}

impl StreamFeed {
    fn new(events: Vec<EventHeader>, seekable: bool) -> Self {
        Self {
            events,
            next: 0,
            seekable,
        }
    }
}

impl EventFeedPort for StreamFeed {
    fn peek_next_timestamp(&mut self) -> Result<Option<i64>, PortError> {
        Ok(self.events.get(self.next).map(|h| h.timestamp_ns))
    }

    fn pop_next(&mut self, header: &mut EventHeader, payload: &mut [u8]) -> Result<(), PortError> {
        let ev = *self.events.get(self.next).ok_or(PortError(1))?;
        *header = ev;
        payload[..8].copy_from_slice(&ev.timestamp_ns.to_le_bytes());
        payload[8..12].copy_from_slice(&ev.stream_id.to_le_bytes());
        self.next += 1;
        Ok(())
    }

    fn position(&self, out: &mut [u8]) -> Result<FeedPosition, PortError> {
        if !self.seekable {
            return Ok(FeedPosition {
                seekable: false,
                len: 0,
            });
        }
        out[..8].copy_from_slice(&(self.next as u64).to_le_bytes());
        Ok(FeedPosition {
            seekable: true,
            len: 8,
        })
    }
}

struct VecTraceSink {
    records: Vec<TraceRecord>,
    bytes: Vec<u8>,
}

impl VecTraceSink {
    fn new() -> Self {
        Self {
            records: Vec::new(),
            bytes: Vec::new(),
        }
    }
}

impl TraceSink for VecTraceSink {
    fn write_header(&mut self, header: &TraceHeader) -> Result<(), PortError> {
        self.bytes.extend_from_slice(&header.to_le_bytes());
        Ok(())
    }

    fn write(&mut self, record: &TraceRecord) -> Result<(), PortError> {
        self.records.push(*record);
        self.bytes.extend_from_slice(&record.to_le_bytes());
        Ok(())
    }
}

#[test]
fn multi_feed_validation_limits_and_wrong_stream_id() {
    // 0 本は ERR_INVALID_ARGUMENT。
    assert_eq!(
        Kernel::create_multi(
            config(),
            Vec::<StreamFeed>::new(),
            FillNextExec::new(),
            RecordingStrategy::new(),
        )
        .err(),
        Some(ERR_INVALID_ARGUMENT)
    );

    // MAX_STREAMS + 1 本は ERR_CAPACITY_EXCEEDED。
    let mut cfg = config();
    cfg.max_total_bytes = u64::MAX;
    let too_many: Vec<StreamFeed> = (0..=MAX_STREAMS)
        .map(|_| StreamFeed::new(Vec::new(), true))
        .collect();
    assert_eq!(
        Kernel::create_multi(cfg, too_many, FillNextExec::new(), RecordingStrategy::new(),).err(),
        Some(ERR_CAPACITY_EXCEEDED)
    );

    // 登録順 1 のフィードが stream_id = 0 を返すと ERR_PORT_FAILURE。
    let feeds = vec![
        StreamFeed::new(vec![feed_event(10, 0, 0)], true),
        StreamFeed::new(vec![feed_event(10, 0, 0)], true),
    ];
    assert_eq!(
        Kernel::create_multi(
            config(),
            feeds,
            FillNextExec::new(),
            RecordingStrategy::new()
        )
        .err(),
        Some(ERR_PORT_FAILURE)
    );
}

#[test]
fn k_way_merge_orders_by_timestamp_stream_id_sequence_id() {
    // 4本の外部フィード（うち #2 は空）＋アクションキュー（0xFFFF_FF00）。
    let f0 = StreamFeed::new(vec![feed_event(10, 0, 0), feed_event(20, 0, 5)], true);
    let f1 = StreamFeed::new(vec![feed_event(10, 1, 0), feed_event(15, 1, 0)], true);
    let f2 = StreamFeed::new(Vec::new(), true);
    let f3 = StreamFeed::new(vec![feed_event(10, 3, 0), feed_event(12, 3, 1)], true);

    let mut strategy = RecordingStrategy::new();
    // t=10（最初に配送される stream 0）で latency=5 のアクションを提出 -> t=15 に到達し OrderAck が t=15 に生成。
    strategy.submit_on = Some((10, 5));

    let kernel = Kernel::create_multi(
        config(),
        vec![f0, f1, f2, f3],
        FillNextExec::new(),
        strategy,
    )
    .unwrap();
    let res = kernel.run(RunLimit::default());
    assert_eq!(res.status, STEP_FINISHED);

    // 配送順の期待値:
    // t=10: stream 0, 1, 3 の順（それぞれ latency=5 のアクションを提出し ActionArrival seq 0,1,2 が登録される）
    // t=12: stream 3 (seq 1)
    // t=15: まず stream 1 (seq 0) [市場], その後に system 0xFFFF_FF00 の OrderAck 3件 (seq 3, 4, 5)
    // t=20: まず stream 0 (seq 5) [市場], その後に system 0xFFFF_FF00 の Fill 3件 (seq 6, 7, 8)
    let observed: Vec<(u16, i64, u32, u64)> = kernel
        .inspect_feeds(|_, _, s| {
            s.seen
                .iter()
                .map(|&(kind, ts, seq, sid, _, _)| (kind, ts, sid, seq))
                .collect()
        })
        .unwrap();

    assert_eq!(
        observed,
        vec![
            (event_type::TICK, 10, 0, 0),
            (event_type::TICK, 10, 1, 0),
            (event_type::TICK, 10, 3, 0),
            (event_type::TICK, 12, 3, 1),
            (event_type::TICK, 15, 1, 0),
            (event_type::ORDER_ACK, 15, ACTION_STREAM_ID, 3),
            (event_type::ORDER_ACK, 15, ACTION_STREAM_ID, 4),
            (event_type::ORDER_ACK, 15, ACTION_STREAM_ID, 5),
            (event_type::TICK, 20, 0, 5),
            (event_type::FILL, 20, ACTION_STREAM_ID, 6),
            (event_type::FILL, 20, ACTION_STREAM_ID, 7),
            (event_type::FILL, 20, ACTION_STREAM_ID, 8),
        ]
    );
}

#[test]
fn per_stream_causality_and_sequence_validation_across_multiple_feeds() {
    // feed 0 は t=10 -> 30、feed 1 は t=20 -> 15（feed 1 のみに着目すると逆行）。
    let f0 = StreamFeed::new(vec![feed_event(10, 0, 0), feed_event(30, 0, 1)], true);
    let f1 = StreamFeed::new(vec![feed_event(20, 1, 0), feed_event(15, 1, 1)], true);
    let kernel = Kernel::create_multi(
        config(),
        vec![f0, f1],
        FillNextExec::new(),
        RecordingStrategy::new(),
    )
    .unwrap();
    assert_eq!(kernel.step().status, OK); // t=10 (stream 0)
    let second = kernel.step(); // t=20 (stream 1) の手順8で t=15 を取り込み ERR_CAUSALITY_VIOLATION
    assert_eq!(second.status, ERR_CAUSALITY_VIOLATION);
    assert_eq!(kernel.state(), HandleState::Poisoned);

    // 同一時刻の連番違反（stream 1 内で seq=2 -> seq=2）。
    let f0 = StreamFeed::new(vec![feed_event(10, 0, 0)], true);
    let f1 = StreamFeed::new(vec![feed_event(20, 1, 2), feed_event(20, 1, 2)], true);
    let kernel = Kernel::create_multi(
        config(),
        vec![f0, f1],
        FillNextExec::new(),
        RecordingStrategy::new(),
    )
    .unwrap();
    assert_eq!(kernel.step().status, OK);
    let second = kernel.step();
    assert_eq!(second.status, ERR_SEQUENCE_VIOLATION);
    assert_eq!(kernel.state(), HandleState::Poisoned);
}

#[test]
fn heterogeneous_boxed_feeds_and_multi_feed_trace_determinism() {
    struct StepGenFeed {
        stream_id: u32,
        count: usize,
        idx: usize,
        step_ns: i64,
    }
    impl EventFeedPort for StepGenFeed {
        fn peek_next_timestamp(&mut self) -> Result<Option<i64>, PortError> {
            Ok((self.idx < self.count).then_some((self.idx as i64 + 1) * self.step_ns))
        }
        fn pop_next(
            &mut self,
            header: &mut EventHeader,
            payload: &mut [u8],
        ) -> Result<(), PortError> {
            let ts = (self.idx as i64 + 1) * self.step_ns;
            *header = feed_event(ts, self.stream_id, self.idx as u64);
            payload[..8].copy_from_slice(&ts.to_le_bytes());
            self.idx += 1;
            Ok(())
        }
        fn position(&self, out: &mut [u8]) -> Result<FeedPosition, PortError> {
            out[..8].copy_from_slice(&(self.idx as u64).to_le_bytes());
            Ok(FeedPosition {
                seekable: true,
                len: 8,
            })
        }
    }

    let run_once = || {
        let mut cfg = config();
        cfg.trace_enabled = true;
        cfg.hash_interval = 5;
        let feeds: Vec<Box<dyn EventFeedPort>> = vec![
            Box::new(StepGenFeed {
                stream_id: 0,
                count: 50,
                idx: 0,
                step_ns: 2,
            }),
            Box::new(StreamFeed::new(
                (1..=40).map(|i| feed_event(i * 3, 1, i as u64)).collect(),
                false,
            )),
            Box::new(StepGenFeed {
                stream_id: 2,
                count: 30,
                idx: 0,
                step_ns: 5,
            }),
        ];
        let mut strategy = RecordingStrategy::new();
        strategy.submit_on = Some((10, 4));
        let kernel = Kernel::create_multi_with_trace(
            cfg,
            feeds,
            FillNextExec::new(),
            strategy,
            VecTraceSink::new(),
        )
        .unwrap();
        let res = kernel.run(RunLimit::default());
        assert_eq!(res.status, STEP_FINISHED);

        // TR-021: 最終状態の正準ステップバイト列の XXH3-64 が、最後の hash_interval 倍数ステップと整合し、
        // CONFIG セクションに stream_count = 3 が入っていることを確認する。
        let mut full = vec![0u8; 32768];
        let len = kernel.write_canonical_state(true, &mut full).unwrap();
        // SECTION 1 (CONFIG): 16バイトヘッダ + CONFIG_SECTION (stream_count は +4..+8 = オフセット 20..24)
        assert_eq!(u32::from_le_bytes(full[20..24].try_into().unwrap()), 3);
        let digest = kernel.state_digest().unwrap();
        assert_eq!(digest, sha256(&full[..len]));

        let (_, _, _, trace) = kernel.destroy_multi_with_trace();
        (digest, sha256(&trace.bytes), trace.records.len())
    };

    let baseline = Arc::new(run_once());
    let workers: Vec<_> = thread::scope(|scope| {
        (0..8)
            .map(|_| scope.spawn(run_once))
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect()
    });
    for w in workers {
        assert_eq!(w, *baseline);
    }
}
