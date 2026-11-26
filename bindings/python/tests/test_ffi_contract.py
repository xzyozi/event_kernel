from __future__ import annotations

import ctypes
import gc
import hashlib
import struct
import sys
import time
from collections import deque
from pathlib import Path
from typing import Any

import polars as pl
import pyarrow as pa
import pytest

import event_kernel as ek
from event_kernel._core import (
    EventBatchView,
    EventRecordView,
    HistoryView,
    StrategyArenaView,
    StrategyContext,
)

REPO_ROOT = Path(__file__).resolve().parents[3]


def _load_capi_lib() -> ctypes.CDLL:
    if sys.platform == "darwin":
        lib_name = "libek_capi.dylib"
    elif sys.platform.startswith("linux"):
        lib_name = "libek_capi.so"
    else:
        lib_name = "ek_capi.dll"

    candidates = [
        REPO_ROOT / "target" / "release" / lib_name,
        REPO_ROOT / "target" / "debug" / lib_name,
    ]
    for cand in candidates:
        if cand.exists():
            return ctypes.CDLL(str(cand))
    raise FileNotFoundError(f"Could not locate {lib_name} in {[str(c) for c in candidates]}")


class EkKernelConfig(ctypes.Structure):
    _fields_ = [
        ("struct_size", ctypes.c_uint32),
        ("history_ring_len", ctypes.c_uint32),
        ("arena_size", ctypes.c_uint64),
        ("strategy_arena_offset", ctypes.c_uint64),
        ("strategy_arena_size", ctypes.c_uint64),
        ("action_queue_capacity", ctypes.c_uint32),
        ("payload_record_size", ctypes.c_uint32),
        ("price_scale_exp", ctypes.c_int32),
        ("qty_scale_exp", ctypes.c_int32),
        ("prng_seed", ctypes.c_uint64),
        ("exec_state_max_bytes", ctypes.c_uint32),
        ("feed_token_max_bytes", ctypes.c_uint32),
        ("hash_interval", ctypes.c_uint64),
        ("trace_enabled", ctypes.c_uint8),
        ("reserved", ctypes.c_uint8 * 7),
        ("max_total_bytes", ctypes.c_uint64),
    ]


class EkEventHeader(ctypes.Structure):
    _fields_ = [
        ("timestamp_ns", ctypes.c_int64),
        ("sequence_id", ctypes.c_uint64),
        ("stream_id", ctypes.c_uint32),
        ("event_type", ctypes.c_uint16),
        ("flags", ctypes.c_uint16),
        ("payload_idx", ctypes.c_uint64),
    ]


class EkStepResult(ctypes.Structure):
    _fields_ = [
        ("status", ctypes.c_int32),
        ("detail", ctypes.c_int32),
        ("processed_event_count", ctypes.c_uint64),
        ("now_ns", ctypes.c_int64),
    ]


class EkAction(ctypes.Structure):
    _fields_ = [
        ("action_kind", ctypes.c_uint16),
        ("reserved", ctypes.c_uint8 * 6),
        ("latency_ns", ctypes.c_int64),
        ("payload_ptr", ctypes.POINTER(ctypes.c_uint8)),
        ("payload_len", ctypes.c_uint32),
    ]


class EkStrategyCtx(ctypes.Structure):
    _fields_ = [
        ("now_ns", ctypes.c_int64),
        ("history_headers", ctypes.POINTER(EkEventHeader)),
        ("history_len", ctypes.c_uint32),
        ("strategy_arena_ptr", ctypes.POINTER(ctypes.c_uint8)),
        ("strategy_arena_len", ctypes.c_uint64),
        ("prng", ctypes.c_void_p),
        ("action_ctx", ctypes.c_void_p),
    ]


FEED_PEEK_FN = ctypes.CFUNCTYPE(
    ctypes.c_int32, ctypes.c_void_p, ctypes.POINTER(ctypes.c_uint8), ctypes.POINTER(ctypes.c_int64)
)
FEED_POP_FN = ctypes.CFUNCTYPE(
    ctypes.c_int32,
    ctypes.c_void_p,
    ctypes.POINTER(EkEventHeader),
    ctypes.POINTER(ctypes.c_uint8),
    ctypes.c_size_t,
)
FEED_POSITION_FN = ctypes.CFUNCTYPE(
    ctypes.c_int32,
    ctypes.c_void_p,
    ctypes.POINTER(ctypes.c_uint8),
    ctypes.POINTER(ctypes.c_uint8),
    ctypes.c_size_t,
    ctypes.POINTER(ctypes.c_size_t),
)
FEED_VALIDATE_FN = ctypes.CFUNCTYPE(
    ctypes.c_int32, ctypes.c_void_p, ctypes.POINTER(ctypes.c_uint8), ctypes.c_size_t
)
FEED_SEEK_FN = ctypes.CFUNCTYPE(
    ctypes.c_int32, ctypes.c_void_p, ctypes.POINTER(ctypes.c_uint8), ctypes.c_size_t
)


class EkFeedVTable(ctypes.Structure):
    _fields_ = [
        ("user_data", ctypes.c_void_p),
        ("peek_next_timestamp", FEED_PEEK_FN),
        ("pop_next", FEED_POP_FN),
        ("position", FEED_POSITION_FN),
        ("validate_position", FEED_VALIDATE_FN),
        ("seek", FEED_SEEK_FN),
    ]


EXEC_ON_ARRIVAL_FN = ctypes.CFUNCTYPE(
    ctypes.c_int32,
    ctypes.c_void_p,
    ctypes.POINTER(EkEventHeader),
    ctypes.POINTER(ctypes.c_uint8),
    ctypes.c_size_t,
    ctypes.c_void_p,
)
EXEC_ON_MARKET_FN = ctypes.CFUNCTYPE(
    ctypes.c_int32,
    ctypes.c_void_p,
    ctypes.POINTER(EkEventHeader),
    ctypes.POINTER(ctypes.c_uint8),
    ctypes.c_size_t,
    ctypes.POINTER(EkEventHeader),
    ctypes.c_uint32,
    ctypes.c_void_p,
)
EXEC_POLL_FN = ctypes.CFUNCTYPE(
    ctypes.c_int32,
    ctypes.c_void_p,
    ctypes.POINTER(ctypes.c_uint8),
    ctypes.POINTER(EkEventHeader),
    ctypes.POINTER(ctypes.c_uint8),
    ctypes.c_size_t,
)
EXEC_CAPTURE_FN = ctypes.CFUNCTYPE(
    ctypes.c_int32,
    ctypes.c_void_p,
    ctypes.POINTER(ctypes.c_uint8),
    ctypes.c_size_t,
    ctypes.POINTER(ctypes.c_size_t),
)
EXEC_STATE_FN = ctypes.CFUNCTYPE(
    ctypes.c_int32, ctypes.c_void_p, ctypes.POINTER(ctypes.c_uint8), ctypes.c_size_t
)


class EkExecVTable(ctypes.Structure):
    _fields_ = [
        ("user_data", ctypes.c_void_p),
        ("on_arrival", EXEC_ON_ARRIVAL_FN),
        ("on_market_event", EXEC_ON_MARKET_FN),
        ("poll_result", EXEC_POLL_FN),
        ("capture_state", EXEC_CAPTURE_FN),
        ("validate_state", EXEC_STATE_FN),
        ("restore_state", EXEC_STATE_FN),
    ]


STRATEGY_ON_EVENT_FN = ctypes.CFUNCTYPE(
    ctypes.c_uint32,
    ctypes.c_void_p,
    ctypes.POINTER(EkStrategyCtx),
    ctypes.POINTER(EkEventHeader),
    ctypes.POINTER(ctypes.c_uint8),
    ctypes.c_size_t,
)


class EkStrategyVTable(ctypes.Structure):
    _fields_ = [
        ("user_data", ctypes.c_void_p),
        ("on_event", STRATEGY_ON_EVENT_FN),
    ]


SNAP_STORE_FN = ctypes.CFUNCTYPE(
    ctypes.c_int32,
    ctypes.c_void_p,
    ctypes.POINTER(ctypes.c_uint8),
    ctypes.c_size_t,
    ctypes.c_int64,
    ctypes.c_uint64,
)


class EkSnapVTable(ctypes.Structure):
    _fields_ = [
        ("user_data", ctypes.c_void_p),
        ("store", SNAP_STORE_FN),
    ]


def _setup_capi_signatures(lib: ctypes.CDLL) -> ctypes.CDLL:
    lib.ek_capi_abi_version.argtypes = [ctypes.POINTER(ctypes.c_uint32), ctypes.POINTER(ctypes.c_uint32)]
    lib.ek_capi_abi_version.restype = ctypes.c_int32

    lib.ek_prng_next_u64.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_uint64)]
    lib.ek_prng_next_u64.restype = ctypes.c_int32

    lib.ek_kernel_create.argtypes = [
        ctypes.POINTER(EkKernelConfig),
        ctypes.POINTER(EkFeedVTable),
        ctypes.c_size_t,
        ctypes.POINTER(EkExecVTable),
        ctypes.c_void_p,
        ctypes.POINTER(EkStrategyVTable),
        ctypes.POINTER(ctypes.c_void_p),
    ]
    lib.ek_kernel_create.restype = ctypes.c_int32

    lib.ek_kernel_run.argtypes = [ctypes.c_void_p, ctypes.c_uint64, ctypes.POINTER(EkStepResult)]
    lib.ek_kernel_run.restype = ctypes.c_int32

    lib.ek_kernel_step.argtypes = [ctypes.c_void_p, ctypes.POINTER(EkStepResult)]
    lib.ek_kernel_step.restype = ctypes.c_int32

    lib.ek_kernel_submit_action.argtypes = [ctypes.c_void_p, ctypes.POINTER(EkAction)]
    lib.ek_kernel_submit_action.restype = ctypes.c_int32

    lib.ek_kernel_capture.argtypes = [ctypes.c_void_p, ctypes.POINTER(EkSnapVTable)]
    lib.ek_kernel_capture.restype = ctypes.c_int32

    lib.ek_kernel_restore.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_uint8), ctypes.c_size_t]
    lib.ek_kernel_restore.restype = ctypes.c_int32

    lib.ek_kernel_state_digest.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_uint8)]
    lib.ek_kernel_state_digest.restype = ctypes.c_int32

    lib.ek_kernel_destroy.argtypes = [ctypes.c_void_p]
    lib.ek_kernel_destroy.restype = ctypes.c_int32
    return lib


def test_tc018_tc019_capi_abi_version_layout_and_null_guards() -> None:
    lib = _setup_capi_signatures(_load_capi_lib())
    assert ctypes.sizeof(EkEventHeader) == 32

    major = ctypes.c_uint32(0)
    minor = ctypes.c_uint32(0)
    rc = lib.ek_capi_abi_version(ctypes.byref(major), ctypes.byref(minor))
    assert rc == 0
    assert (major.value, minor.value) == (0, 1)

    assert lib.ek_capi_abi_version(None, ctypes.byref(minor)) == -1
    assert lib.ek_kernel_step(None, None) == -1
    assert lib.ek_kernel_state_digest(None, None) == -1


class _SubmitActionStrategy:
    def on_event(self, ctx: ek.StrategyContext, event: EventRecordView) -> int:
        if event.event_type == 1:
            rnd = ctx.prng_next_u64()
            arena = ctx.strategy_arena
            arena[0:8] = int(event.timestamp_ns).to_bytes(8, "little", signed=True)
            payload = struct.pack("<Q", rnd) + b"\x00" * 8 + struct.pack("<qqq", 1000, 1, 2)
            ctx.submit_action(1, 5, payload)
        return 0


class _BatchNoopStrategy:
    def on_events(self, ctx: ek.StrategyContext, batch: EventBatchView) -> int:
        mv = memoryview(batch)
        try:
            _ = len(mv)
        finally:
            mv.release()
        return 0


def test_tc020_v01_single_and_batch_callback_paths() -> None:
    n_events = 2_000
    timestamps = [1_000 + i * 10 for i in range(n_events)]
    cfg = ek.KernelConfig(history_ring_len=16, arena_size=4096, strategy_arena_size=128)
    feed = ek.InMemoryMarketFeed(timestamps)
    exec_port = ek.SimExecution(delay_ns=5)

    class _NoopStrat:
        def on_event(self, ctx: StrategyContext, event: EventRecordView) -> int:
            return 0

    t0 = time.perf_counter()
    kernel = ek.Kernel(cfg, [feed], exec_port, _NoopStrat())
    res = kernel.run()
    t_single = time.perf_counter() - t0
    assert res.status == ek.StepStatus.FINISHED
    assert res.processed_event_count == n_events

    t1 = time.perf_counter()
    batch_processed = ek.Kernel.benchmark_batch_callbacks(_BatchNoopStrategy(), timestamps, batch_size=64)
    t_batch = time.perf_counter() - t1
    assert batch_processed == n_events
    assert t_single > 0.0 and t_batch > 0.0


def _compute_expected_arrow_digest(table_name: str, table: pa.Table) -> bytes:
    hasher = hashlib.sha256()
    hasher.update(table_name.encode("utf-8"))
    hasher.update(struct.pack("<Q", table.num_rows))
    hasher.update(struct.pack("<I", table.num_columns))
    for field, col in zip(table.schema, table.columns, strict=True):
        name_bytes = field.name.encode("utf-8")
        hasher.update(struct.pack("<I", len(name_bytes)))
        hasher.update(name_bytes)
        arr = col.combine_chunks().to_numpy(zero_copy_only=False)
        hasher.update(arr.tobytes(order="C"))
    return hasher.digest()


def test_tc021_v02_arrow_pycapsule_zero_copy_release_and_digest() -> None:
    cfg = ek.KernelConfig(
        history_ring_len=16,
        arena_size=4096,
        strategy_arena_offset=0,
        strategy_arena_size=128,
        action_queue_capacity=64,
        payload_record_size=64,
        prng_seed=777,
    )
    kernel = ek.Kernel(
        cfg,
        [ek.InMemoryMarketFeed([1000, 2000, 3000])],
        ek.SimExecution(delay_ns=5),
        _SubmitActionStrategy(),
    )
    res = kernel.run()
    assert res.status == ek.StepStatus.FINISHED

    fills_stream = kernel.export_fills()
    expected_addrs = fills_stream.buffer_addresses()
    expected_digest = fills_stream.digest()
    assert fills_stream.row_count == 3
    assert fills_stream.release_count() == 0

    reader = pa.RecordBatchReader.from_stream(fills_stream)
    pa_table = reader.read_all()
    assert pa_table.num_rows == 3
    # ゼロコピー検証: timestamp_ns 列の C Data Interface バッファポインタが一致する
    col0_buf = pa_table.column(0).chunk(0).buffers()[1]
    assert col0_buf is not None
    assert col0_buf.address == expected_addrs[0]
    assert _compute_expected_arrow_digest("fills", pa_table) == expected_digest

    del col0_buf
    del pa_table
    del reader
    gc.collect()
    assert fills_stream.release_count() == 1

    # Polars でもゼロコピー PyCapsule Consumer 経由で読み込めること
    pl_df = pl.DataFrame(kernel.export_fills())
    assert isinstance(pl_df, pl.DataFrame)
    assert pl_df.height == 3
    assert pl_df.columns[0] == "timestamp_ns"

    eq_stream = kernel.export_equity()
    assert eq_stream.row_count > 0
    tr_stream = kernel.export_transitions()
    assert tr_stream.row_count == 0


def test_tc022_v03_capi_and_python_pyo3_state_digest_parity() -> None:
    timestamps = [100, 200, 300, 400]
    py_cfg = ek.KernelConfig(
        history_ring_len=16,
        arena_size=4096,
        strategy_arena_offset=0,
        strategy_arena_size=256,
        action_queue_capacity=128,
        payload_record_size=64,
        price_scale_exp=-2,
        qty_scale_exp=-2,
        prng_seed=42,
        exec_state_max_bytes=1024,
        feed_token_max_bytes=4096,
        hash_interval=0,
        trace_enabled=False,
    )
    py_kernel = ek.Kernel(
        py_cfg,
        [ek.InMemoryMarketFeed(timestamps, stream_id=0, event_type=1)],
        ek.SimExecution(delay_ns=5),
        _SubmitActionStrategy(),
    )
    py_res = py_kernel.run()
    assert py_res.status == ek.StepStatus.FINISHED
    py_digest = py_kernel.state_digest()
    assert len(py_digest) == 32

    # 同一設定・同一処理列を core-capi (`ctypes`) 経由で実行し SHA-256 を比較する
    lib = _setup_capi_signatures(_load_capi_lib())
    c_cfg = EkKernelConfig(
        struct_size=ctypes.sizeof(EkKernelConfig),
        history_ring_len=16,
        arena_size=4096,
        strategy_arena_offset=0,
        strategy_arena_size=256,
        action_queue_capacity=128,
        payload_record_size=64,
        price_scale_exp=-2,
        qty_scale_exp=-2,
        prng_seed=42,
        exec_state_max_bytes=1024,
        feed_token_max_bytes=4096,
        hash_interval=0,
        trace_enabled=0,
        reserved=(ctypes.c_uint8 * 7)(),
        max_total_bytes=268_435_456,
    )

    feed_state: dict[str, int] = {"pos": 0}
    pending_fills: deque[tuple[EkEventHeader, bytes]] = deque()

    @FEED_PEEK_FN
    def c_peek(_ud: Any, out_has: Any, out_ts: Any) -> int:
        pos = feed_state["pos"]
        if pos < len(timestamps):
            out_ts[0] = timestamps[pos]
            out_has[0] = 1
        else:
            out_has[0] = 0
        return 0

    @FEED_POP_FN
    def c_pop(_ud: Any, out_hdr: Any, out_pl: Any, pl_len: int) -> int:
        pos = feed_state["pos"]
        if pos >= len(timestamps):
            return -20
        ts = timestamps[pos]
        feed_state["pos"] = pos + 1
        out_hdr[0] = EkEventHeader(ts, feed_state["pos"], 0, 1, 0, 0)
        ctypes.memset(out_pl, 0, pl_len)
        return 0

    @FEED_POSITION_FN
    def c_pos(_ud: Any, out_seek: Any, out_tok: Any, _tok_cap: int, out_len: Any) -> int:
        out_seek[0] = 1
        out_len[0] = 8
        raw = struct.pack("<Q", feed_state["pos"])
        ctypes.memmove(out_tok, raw, 8)
        return 0

    @FEED_VALIDATE_FN
    def c_val(_ud: Any, _tok: Any, tok_len: int) -> int:
        return 0 if tok_len == 8 else -20

    @FEED_SEEK_FN
    def c_seek(_ud: Any, tok: Any, tok_len: int) -> int:
        if tok_len != 8:
            return -20
        raw = ctypes.string_at(tok, 8)
        feed_state["pos"] = struct.unpack("<Q", raw)[0]
        return 0

    @EXEC_ON_ARRIVAL_FN
    def c_on_arrival(_ud: Any, hdr_ptr: Any, _pl_ptr: Any, pl_len: int, prng_ptr: Any) -> int:
        rnd_val = ctypes.c_uint64(0)
        if lib.ek_prng_next_u64(prng_ptr, ctypes.byref(rnd_val)) != 0:
            return -20
        buf = bytearray(pl_len)
        if pl_len >= 8:
            buf[0:8] = struct.pack("<Q", rnd_val.value)
        if pl_len >= 24:
            buf[16:24] = struct.pack("<q", 1000)
        if pl_len >= 32:
            buf[24:32] = struct.pack("<q", 1)
        src = hdr_ptr[0]
        fill_hdr = EkEventHeader(
            src.timestamp_ns + 5,
            src.sequence_id,
            src.stream_id,
            18,  # event_type::FILL
            src.flags,
            src.payload_idx,
        )
        pending_fills.append((fill_hdr, bytes(buf)))
        return 0

    @EXEC_ON_MARKET_FN
    def c_on_market(*_args: Any) -> int:
        return 0

    @EXEC_POLL_FN
    def c_poll(_ud: Any, out_has: Any, out_hdr: Any, out_pl: Any, pl_len: int) -> int:
        if not pending_fills:
            out_has[0] = 0
            return 0
        hdr, payload = pending_fills.popleft()
        out_hdr[0] = hdr
        ctypes.memset(out_pl, 0, pl_len)
        n = min(pl_len, len(payload))
        ctypes.memmove(out_pl, payload, n)
        out_has[0] = 1
        return 0

    @EXEC_CAPTURE_FN
    def c_cap_exec(_ud: Any, out_buf: Any, _buf_cap: int, out_len: Any) -> int:
        out_len[0] = 8
        ctypes.memmove(out_buf, struct.pack("<Q", len(pending_fills)), 8)
        return 0

    @EXEC_STATE_FN
    def c_val_exec(_ud: Any, _buf: Any, buf_len: int) -> int:
        return 0 if buf_len >= 8 else -20

    @EXEC_STATE_FN
    def c_rest_exec(_ud: Any, _buf: Any, _buf_len: int) -> int:
        pending_fills.clear()
        return 0

    @STRATEGY_ON_EVENT_FN
    def c_on_event(_ud: Any, ctx_ptr: Any, hdr_ptr: Any, _pl_ptr: Any, _pl_len: int) -> int:
        hdr = hdr_ptr[0]
        if hdr.event_type == 1:
            ctx = ctx_ptr[0]
            rnd_val = ctypes.c_uint64(0)
            if lib.ek_prng_next_u64(ctx.prng, ctypes.byref(rnd_val)) != 0:
                return 1
            ts_bytes = int(hdr.timestamp_ns).to_bytes(8, "little", signed=True)
            ctypes.memmove(ctx.strategy_arena_ptr, ts_bytes, 8)
            payload = struct.pack("<Q", rnd_val.value) + b"\x00" * 8 + struct.pack("<qqq", 1000, 1, 2)
            raw_buf = (ctypes.c_uint8 * len(payload)).from_buffer_copy(payload)
            act = EkAction(1, (ctypes.c_uint8 * 6)(), 5, raw_buf, len(payload))
            rc = lib.ek_kernel_submit_action(ctx.action_ctx, ctypes.byref(act))
            if rc != 0:
                return 1
        return 0

    feed_vt = EkFeedVTable(None, c_peek, c_pop, c_pos, c_val, c_seek)
    exec_vt = EkExecVTable(None, c_on_arrival, c_on_market, c_poll, c_cap_exec, c_val_exec, c_rest_exec)
    strat_vt = EkStrategyVTable(None, c_on_event)

    handle = ctypes.c_void_p()
    rc = lib.ek_kernel_create(
        ctypes.byref(c_cfg),
        ctypes.byref(feed_vt),
        1,
        ctypes.byref(exec_vt),
        None,
        ctypes.byref(strat_vt),
        ctypes.byref(handle),
    )
    assert rc == 0
    try:
        step_res = EkStepResult()
        rc = lib.ek_kernel_run(handle, 0, ctypes.byref(step_res))
        assert rc == ek.StepStatus.FINISHED
        assert step_res.status == 1  # STEP_FINISHED
        assert step_res.processed_event_count == py_res.processed_event_count

        c_digest_buf = (ctypes.c_uint8 * 32)()
        assert lib.ek_kernel_state_digest(handle, c_digest_buf) == 0
        c_digest = bytes(c_digest_buf)
        assert c_digest == py_digest
    finally:
        lib.ek_kernel_destroy(handle)


def test_tc023_v04_v06_exception_panic_poison_and_restore_recovery() -> None:
    captured_blobs: list[bytes] = []

    class _SnapPort:
        def store(self, blob: bytes, logical_time_ns: int, event_count: int) -> None:
            captured_blobs.append(blob)

    class _BoomOnThird:
        def __init__(self) -> None:
            self.calls = 0

        def on_event(self, ctx: StrategyContext, event: EventRecordView) -> int:
            self.calls += 1
            if self.calls == 3:
                raise ValueError("strategy failure boom")
            return 0

    cfg = ek.KernelConfig(history_ring_len=8, arena_size=4096, strategy_arena_size=64)
    feed = ek.InMemoryMarketFeed([10, 20, 30, 40])
    strat = _BoomOnThird()
    kernel = ek.Kernel(cfg, [feed], ek.SimExecution(), strat)

    assert kernel.step().status == 0
    kernel.capture(_SnapPort())
    assert len(captured_blobs) == 1

    assert kernel.step().status == 0
    with pytest.raises(ek.StrategyFailureError) as exc_info:
        kernel.step()
    assert isinstance(exc_info.value.__cause__, ValueError)
    assert kernel.state() == "Poisoned"

    with pytest.raises(ek.KernelPoisonedError):
        kernel.step()

    # restore() による Poisoned からの復旧
    kernel.restore(captured_blobs[0])
    assert kernel.state() == "Ready"
    strat.calls = -100  # 再実行時はエラーを発生させない
    end_res = kernel.run()
    assert end_res.status == ek.StepStatus.FINISHED

    # 境界内部 panic 捕捉 -> KernelPanicError + Poisoned
    with pytest.raises(ek.KernelPanicError):
        kernel._debug_trigger_internal_panic()
    assert kernel.state() == "Poisoned"


def test_tc025_v05_qg013_view_guard_lifetime_invalidation() -> None:
    retained_views: dict[str, Any] = {}

    class _RetainingStrategy:
        def __init__(self, raise_exc: bool = False) -> None:
            self.raise_exc = raise_exc

        def on_event(self, ctx: StrategyContext, event: EventRecordView) -> int:
            retained_views["ctx"] = ctx
            retained_views["event"] = event
            retained_views["history"] = ctx.history
            retained_views["arena"] = ctx.strategy_arena
            arena: StrategyArenaView = ctx.strategy_arena
            arena[0:4] = b"\xde\xad\xbe\xef"
            if self.raise_exc:
                raise RuntimeError("abort mid-callback")
            return 0

    cfg = ek.KernelConfig(history_ring_len=8, arena_size=4096, strategy_arena_size=64)
    k_ok = ek.Kernel(cfg, [ek.InMemoryMarketFeed([100])], ek.SimExecution(), _RetainingStrategy(False))
    assert k_ok.run().status == ek.StepStatus.FINISHED
    assert k_ok.read_strategy_arena(0, 4) == b"\xde\xad\xbe\xef"

    ctx_r: StrategyContext = retained_views["ctx"]
    ev_r: EventRecordView = retained_views["event"]
    hist_r: HistoryView = retained_views["history"]
    arena_r: StrategyArenaView = retained_views["arena"]

    with pytest.raises(RuntimeError):
        _ = ctx_r.now_ns
    with pytest.raises(RuntimeError):
        _ = ev_r.timestamp_ns
    with pytest.raises(RuntimeError):
        _ = len(hist_r)
    with pytest.raises(RuntimeError):
        _ = arena_r.tobytes()
    with pytest.raises(RuntimeError):
        memoryview(arena_r)

    # 例外脱出経路でも ViewGuard が無効化されること
    retained_views.clear()
    k_err = ek.Kernel(cfg, [ek.InMemoryMarketFeed([100])], ek.SimExecution(), _RetainingStrategy(True))
    with pytest.raises(ek.StrategyFailureError):
        k_err.step()
    with pytest.raises(RuntimeError):
        _ = retained_views["arena"].tobytes()

    # memoryview リーク検出: release() 忘れ時は RuntimeError
    leaked_mvs: list[memoryview] = []

    class _LeakingMvStrategy:
        def on_event(self, ctx: StrategyContext, event: EventRecordView) -> int:
            leaked_mvs.append(memoryview(ctx.strategy_arena))
            return 0

    k_leak = ek.Kernel(cfg, [ek.InMemoryMarketFeed([100])], ek.SimExecution(), _LeakingMvStrategy())
    try:
        with pytest.raises(RuntimeError):
            k_leak.step()
    finally:
        for mv in leaked_mvs:
            mv.release()

    # バッチビュー (EventBatchView) もコールバック終了後に無効化されること
    retained_batches: list[EventBatchView] = []

    class _RetainBatchStrategy:
        def on_events(self, ctx: StrategyContext, batch: EventBatchView) -> int:
            retained_batches.append(batch)
            return 0

    processed = ek.Kernel.benchmark_batch_callbacks(_RetainBatchStrategy(), [1, 2, 3], batch_size=2)
    assert processed == 3
    with pytest.raises(RuntimeError):
        _ = len(retained_batches[0])
    with pytest.raises(RuntimeError):
        memoryview(retained_batches[0])

