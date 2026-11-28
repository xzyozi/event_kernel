//! C ABI 契約テスト（TR-IF-001〜TR-IF-005、V-03 / V-04 / V-06）。

use core::ffi::c_void;
use core::mem::size_of;

use ek_capi::{
    EK_CAPI_ABI_MAJOR, EK_CAPI_ABI_MINOR, EK_ERR_INVALID_ARGUMENT, EK_ERR_PANIC, EK_ERR_POISONED,
    EK_ERR_STRATEGY_FAILURE, EK_OK, EK_STEP_FINISHED, EkAction, EkEventHeader, EkExecVTable,
    EkFeedVTable, EkKernelConfig, EkKernelHandle, EkPrng, EkSnapVTable, EkStepResult,
    EkStrategyCtx, EkStrategyVTable, ek_capi_abi_version, ek_kernel_capture, ek_kernel_create,
    ek_kernel_destroy, ek_kernel_init_strategy_arena, ek_kernel_read_strategy_arena,
    ek_kernel_restore, ek_kernel_run, ek_kernel_state_digest, ek_kernel_step,
    ek_kernel_submit_action, ek_prng_next_u64,
};

struct FeedData {
    timestamps: Vec<i64>,
    pos: usize,
}

unsafe extern "C-unwind" fn feed_peek(
    user_data: *mut c_void,
    out_has_next: *mut u8,
    out_timestamp_ns: *mut i64,
) -> i32 {
    let data = unsafe { &mut *(user_data as *mut FeedData) };
    if data.pos < data.timestamps.len() {
        unsafe {
            *out_has_next = 1;
            *out_timestamp_ns = data.timestamps[data.pos];
        }
    } else {
        unsafe {
            *out_has_next = 0;
        }
    }
    0
}

unsafe extern "C-unwind" fn feed_pop(
    user_data: *mut c_void,
    out_header: *mut EkEventHeader,
    _out_payload: *mut u8,
    _payload_cap: usize,
) -> i32 {
    let data = unsafe { &mut *(user_data as *mut FeedData) };
    let ts = data.timestamps[data.pos];
    let seq = (data.pos + 1) as u64;
    data.pos += 1;
    unsafe {
        *out_header = EkEventHeader {
            timestamp_ns: ts,
            sequence_id: seq,
            stream_id: 0,
            event_type: 1,
            flags: 0,
            payload_idx: 0,
        };
    }
    0
}

unsafe extern "C-unwind" fn feed_position(
    user_data: *mut c_void,
    out_seekable: *mut u8,
    out_token: *mut u8,
    token_cap: usize,
    out_token_len: *mut usize,
) -> i32 {
    let data = unsafe { &*(user_data as *const FeedData) };
    if token_cap < 8 {
        return -1;
    }
    let bytes = (data.pos as u64).to_le_bytes();
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), out_token, 8);
        *out_seekable = 1;
        *out_token_len = 8;
    }
    0
}

unsafe extern "C-unwind" fn feed_seek(
    user_data: *mut c_void,
    token_ptr: *const u8,
    token_len: usize,
) -> i32 {
    if token_len != 8 {
        return -1;
    }
    let mut buf = [0u8; 8];
    unsafe {
        core::ptr::copy_nonoverlapping(token_ptr, buf.as_mut_ptr(), 8);
    }
    let data = unsafe { &mut *(user_data as *mut FeedData) };
    data.pos = u64::from_le_bytes(buf) as usize;
    0
}

struct ExecData {
    pending: Option<(i64, u64)>,
    seq: u64,
}

unsafe extern "C-unwind" fn exec_on_arrival(
    user_data: *mut c_void,
    header: *const EkEventHeader,
    _payload: *const u8,
    _payload_len: usize,
    prng: *mut EkPrng,
) -> i32 {
    let data = unsafe { &mut *(user_data as *mut ExecData) };
    let mut r = 0u64;
    assert_eq!(unsafe { ek_prng_next_u64(prng, &mut r) }, EK_OK);
    data.seq += 1;
    let h = unsafe { &*header };
    data.pending = Some((h.timestamp_ns + 5, r));
    0
}

unsafe extern "C-unwind" fn exec_on_market(
    _user_data: *mut c_void,
    _header: *const EkEventHeader,
    _payload: *const u8,
    _payload_len: usize,
    _history_headers: *const EkEventHeader,
    _history_len: u32,
    _prng: *mut EkPrng,
) -> i32 {
    0
}

unsafe extern "C-unwind" fn exec_poll(
    user_data: *mut c_void,
    out_has_result: *mut u8,
    out_header: *mut EkEventHeader,
    out_payload: *mut u8,
    payload_cap: usize,
) -> i32 {
    let data = unsafe { &mut *(user_data as *mut ExecData) };
    if let Some((ts, rnd)) = data.pending.take() {
        unsafe {
            *out_has_result = 1;
            *out_header = EkEventHeader {
                timestamp_ns: ts,
                sequence_id: 0,
                stream_id: 0,
                event_type: 18,
                flags: 0,
                payload_idx: 0,
            };
            if payload_cap >= 8 {
                core::ptr::copy_nonoverlapping(rnd.to_le_bytes().as_ptr(), out_payload, 8);
            }
        }
    } else {
        unsafe {
            *out_has_result = 0;
        }
    }
    0
}

unsafe extern "C-unwind" fn snap_store(
    user_data: *mut c_void,
    blob_ptr: *const u8,
    blob_len: usize,
    _logical_time_ns: i64,
    _event_count: u64,
) -> i32 {
    let out = unsafe { &mut *(user_data as *mut Vec<u8>) };
    let bytes = unsafe { core::slice::from_raw_parts(blob_ptr, blob_len) };
    out.clear();
    out.extend_from_slice(bytes);
    0
}

struct StratData {
    should_panic: bool,
    should_fail: bool,
    fills_seen: u32,
}

unsafe extern "C-unwind" fn strat_on_event(
    user_data: *mut c_void,
    ctx_ptr: *const EkStrategyCtx,
    header_ptr: *const EkEventHeader,
    _payload: *const u8,
    _payload_len: usize,
) -> u32 {
    let data = unsafe { &mut *(user_data as *mut StratData) };
    if data.should_panic {
        panic!("simulated C ABI strategy panic");
    }
    if data.should_fail {
        return 7;
    }
    let ctx = unsafe { &*ctx_ptr };
    let hdr = unsafe { &*header_ptr };
    if hdr.event_type == 1 {
        let mut r = 0u64;
        assert_eq!(unsafe { ek_prng_next_u64(ctx.prng, &mut r) }, EK_OK);
        let payload = [42u8; 4];
        let action = EkAction {
            action_kind: 1,
            reserved: [0u8; 6],
            latency_ns: 2,
            payload_ptr: payload.as_ptr(),
            payload_len: 4,
        };
        assert_eq!(
            unsafe { ek_kernel_submit_action(ctx.action_ctx, &action) },
            EK_OK
        );
        if ctx.strategy_arena_len >= 8 {
            let arena = unsafe {
                core::slice::from_raw_parts_mut(
                    ctx.strategy_arena_ptr,
                    ctx.strategy_arena_len as usize,
                )
            };
            arena[..8].copy_from_slice(&r.to_le_bytes());
        }
    } else if hdr.event_type == 18 {
        data.fills_seen += 1;
    }
    0
}

fn default_cfg() -> EkKernelConfig {
    EkKernelConfig {
        struct_size: size_of::<EkKernelConfig>() as u32,
        history_ring_len: 16,
        arena_size: 64,
        strategy_arena_offset: 0,
        strategy_arena_size: 32,
        action_queue_capacity: 16,
        payload_record_size: 32,
        price_scale_exp: -2,
        qty_scale_exp: -2,
        prng_seed: 12345,
        exec_state_max_bytes: 64,
        feed_token_max_bytes: 64,
        hash_interval: 1,
        trace_enabled: 0,
        reserved: [0u8; 7],
        max_total_bytes: 16 * 1024 * 1024,
    }
}

#[test]
fn tr_if_001_and_002_abi_version_and_invalid_argument_guards() {
    let mut major = 0u32;
    let mut minor = 0u32;
    assert_eq!(
        unsafe { ek_capi_abi_version(&mut major, &mut minor) },
        EK_OK
    );
    assert_eq!((major, minor), (EK_CAPI_ABI_MAJOR, EK_CAPI_ABI_MINOR));
    assert_eq!(
        unsafe { ek_capi_abi_version(core::ptr::null_mut(), &mut minor) },
        EK_ERR_INVALID_ARGUMENT
    );
    assert_eq!(unsafe { ek_kernel_destroy(core::ptr::null_mut()) }, EK_OK);

    let mut bad_cfg = default_cfg();
    bad_cfg.struct_size = 1;
    let mut handle: *mut EkKernelHandle = core::ptr::null_mut();
    let feed = EkFeedVTable {
        user_data: core::ptr::null_mut(),
        peek_next_timestamp: Some(feed_peek),
        pop_next: Some(feed_pop),
        position: Some(feed_position),
        validate_position: None,
        seek: Some(feed_seek),
    };
    let exec = EkExecVTable {
        user_data: core::ptr::null_mut(),
        on_arrival: Some(exec_on_arrival),
        on_market_event: Some(exec_on_market),
        poll_result: Some(exec_poll),
        capture_state: None,
        validate_state: None,
        restore_state: None,
    };
    let strat = EkStrategyVTable {
        user_data: core::ptr::null_mut(),
        on_event: Some(strat_on_event),
    };
    assert_eq!(
        unsafe {
            ek_kernel_create(
                &bad_cfg,
                &feed,
                1,
                &exec,
                core::ptr::null(),
                &strat,
                &mut handle,
            )
        },
        EK_ERR_INVALID_ARGUMENT
    );
    assert!(handle.is_null());

    bad_cfg = default_cfg();
    bad_cfg.trace_enabled = 2;
    assert_eq!(
        unsafe {
            ek_kernel_create(
                &bad_cfg,
                &feed,
                1,
                &exec,
                core::ptr::null(),
                &strat,
                &mut handle,
            )
        },
        EK_ERR_INVALID_ARGUMENT
    );
}

#[test]
fn tr_if_003_through_005_run_capture_restore_and_panic_poison_recovery() {
    let feed_data = Box::into_raw(Box::new(FeedData {
        timestamps: vec![100, 200],
        pos: 0,
    }));
    let exec_data = Box::into_raw(Box::new(ExecData {
        pending: None,
        seq: 0,
    }));
    let strat_data = Box::into_raw(Box::new(StratData {
        should_panic: false,
        should_fail: false,
        fills_seen: 0,
    }));
    let cfg = default_cfg();
    let feed = EkFeedVTable {
        user_data: feed_data.cast(),
        peek_next_timestamp: Some(feed_peek),
        pop_next: Some(feed_pop),
        position: Some(feed_position),
        validate_position: None,
        seek: Some(feed_seek),
    };
    let exec = EkExecVTable {
        user_data: exec_data.cast(),
        on_arrival: Some(exec_on_arrival),
        on_market_event: Some(exec_on_market),
        poll_result: Some(exec_poll),
        capture_state: None,
        validate_state: None,
        restore_state: None,
    };
    let strat = EkStrategyVTable {
        user_data: strat_data.cast(),
        on_event: Some(strat_on_event),
    };

    let mut handle: *mut EkKernelHandle = core::ptr::null_mut();
    assert_eq!(
        unsafe {
            ek_kernel_create(
                &cfg,
                &feed,
                1,
                &exec,
                core::ptr::null(),
                &strat,
                &mut handle,
            )
        },
        EK_OK
    );

    let init_bytes = [7u8; 8];
    assert_eq!(
        unsafe { ek_kernel_init_strategy_arena(handle, 8, init_bytes.as_ptr(), 8) },
        EK_OK
    );

    let mut step_res = EkStepResult::default();
    assert_eq!(unsafe { ek_kernel_step(handle, &mut step_res) }, EK_OK);

    let snap_blob = Box::into_raw(Box::new(Vec::<u8>::new()));
    let snap = EkSnapVTable {
        user_data: snap_blob.cast(),
        store: Some(snap_store),
    };
    assert_eq!(unsafe { ek_kernel_capture(handle, &snap) }, EK_OK);
    let snap_ref = unsafe { &*snap_blob };
    assert!(!snap_ref.is_empty());

    assert_eq!(
        unsafe { ek_kernel_run(handle, 0, &mut step_res) },
        EK_STEP_FINISHED
    );
    let mut expected_digest = [0u8; 32];
    assert_eq!(
        unsafe { ek_kernel_state_digest(handle, expected_digest.as_mut_ptr()) },
        EK_OK
    );

    let mut arena_out = [0u8; 16];
    assert_eq!(
        unsafe { ek_kernel_read_strategy_arena(handle, 0, arena_out.as_mut_ptr(), 16) },
        EK_OK
    );
    assert_eq!(&arena_out[8..16], &init_bytes);

    // 復元 -> パニック注入 -> EK_ERR_PANIC 捕捉 -> EK_ERR_POISONED 拒否 -> restore 復帰
    // （復元直後の次イベントは t=102 の ActionArrival であり戦略通知なしのため、run で次の通知イベントへ進める）
    assert_eq!(
        unsafe { ek_kernel_restore(handle, snap_ref.as_ptr(), snap_ref.len()) },
        EK_OK
    );
    unsafe { (*strat_data).should_panic = true };
    assert_eq!(
        unsafe { ek_kernel_run(handle, 0, &mut step_res) },
        EK_ERR_PANIC
    );
    assert_eq!(step_res.status, EK_ERR_PANIC);
    assert_eq!(
        unsafe { ek_kernel_step(handle, &mut step_res) },
        EK_ERR_POISONED
    );
    assert_eq!(unsafe { ek_kernel_capture(handle, &snap) }, EK_ERR_POISONED);

    unsafe { (*strat_data).should_panic = false };
    assert_eq!(
        unsafe { ek_kernel_restore(handle, snap_ref.as_ptr(), snap_ref.len()) },
        EK_OK
    );
    assert_eq!(
        unsafe { ek_kernel_run(handle, 0, &mut step_res) },
        EK_STEP_FINISHED
    );
    let mut recovered_digest = [0u8; 32];
    assert_eq!(
        unsafe { ek_kernel_state_digest(handle, recovered_digest.as_mut_ptr()) },
        EK_OK
    );
    assert_eq!(recovered_digest, expected_digest);

    // 戦略エラー時は EK_ERR_STRATEGY_FAILURE + detail を返し Poisoned になる。
    assert_eq!(
        unsafe { ek_kernel_restore(handle, snap_ref.as_ptr(), snap_ref.len()) },
        EK_OK
    );
    unsafe { (*strat_data).should_fail = true };
    assert_eq!(
        unsafe { ek_kernel_run(handle, 0, &mut step_res) },
        EK_ERR_STRATEGY_FAILURE
    );
    assert_eq!(step_res.detail, 7);
    assert_eq!(
        unsafe { ek_kernel_step(handle, &mut step_res) },
        EK_ERR_POISONED
    );

    assert_eq!(
        unsafe { ek_kernel_submit_action(core::ptr::null_mut(), core::ptr::null()) },
        EK_ERR_INVALID_ARGUMENT
    );

    assert_eq!(unsafe { ek_kernel_destroy(handle) }, EK_OK);
    unsafe {
        drop(Box::from_raw(snap_blob));
        drop(Box::from_raw(strat_data));
        drop(Box::from_raw(exec_data));
        drop(Box::from_raw(feed_data));
    }
}
