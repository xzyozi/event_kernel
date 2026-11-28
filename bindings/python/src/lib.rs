//! PyO3 による Python 拡張モジュール `event_kernel._core`（EK-IF-001 4.2〜4.3節、ADR-003）。
//!
//! `core` の安全な Rust API（`ek_core`）を直接呼び出し、Python 戦略コールバック・
//! `ViewGuard` 寿命制御・例外変換・Arrow PyCapsule 結果出力を担う。

#![allow(unsafe_code)]

mod arrow;

use std::cell::{Cell, RefCell, UnsafeCell};
use std::collections::VecDeque;
use std::ffi::{c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use arrow::{ArrowColumn, ArrowColumnData, PyArrowResultStream};
use ek_adapters::{DiskSnapshotStore, DiskTraceSink};
use ek_core::{
    ACTION_PAYLOAD_OFFSET, Action, Context, ERR_CAPACITY_EXCEEDED, ERR_CAUSALITY_VIOLATION,
    ERR_INVALID_ARGUMENT, ERR_PANIC, ERR_POISONED, ERR_PORT_FAILURE, ERR_QUEUE_FULL, ERR_REENTRANT,
    ERR_RESULT_OVERFLOW, ERR_SEQUENCE_VIOLATION, ERR_SNAPSHOT_INVALID, ERR_SNAPSHOT_IO,
    ERR_SNAPSHOT_MISMATCH, ERR_SNAPSHOT_UNSUPPORTED, ERR_STRATEGY_FAILURE, EventFeedPort,
    EventHeader, EventRecord, ExecutionPort, FeedPosition, HandleState, HistoryView, Kernel,
    KernelConfig, OK, PortError, Prng, RUN_LIMIT_REACHED, RUN_SNAPSHOT_REQUESTED, RunLimit,
    STEP_FINISHED, SnapshotPort, StepResult, Strategy, StrategyStatus, TraceHeader, TraceRecord,
    TraceSink, ViewLease, ViewToken, event_type,
};
use pyo3::exceptions::{PyException, PyIndexError, PyRuntimeError, PyValueError};
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyBytes, PySequence, PySlice};

pyo3::create_exception!(_core, KernelError, PyException);
pyo3::create_exception!(_core, KernelFatalError, KernelError);
pyo3::create_exception!(_core, KernelCapacityError, KernelError);
pyo3::create_exception!(_core, ActionQueueFullError, KernelError);
pyo3::create_exception!(_core, KernelPoisonedError, KernelError);
pyo3::create_exception!(_core, ReentrantError, KernelError);
pyo3::create_exception!(_core, SnapshotUnsupportedError, KernelError);
pyo3::create_exception!(_core, SnapshotIoError, KernelError);
pyo3::create_exception!(_core, SnapshotRestoreError, KernelError);
pyo3::create_exception!(_core, CausalityViolationError, KernelFatalError);
pyo3::create_exception!(_core, SequenceViolationError, KernelFatalError);
pyo3::create_exception!(_core, PortFailureError, KernelFatalError);
pyo3::create_exception!(_core, ResultOverflowError, KernelFatalError);
pyo3::create_exception!(_core, StrategyFailureError, KernelFatalError);
pyo3::create_exception!(_core, KernelPanicError, KernelFatalError);

fn map_error_code(py: Python<'_>, code: i32, detail: i32, cause: Option<PyErr>) -> PyErr {
    match code {
        ERR_INVALID_ARGUMENT => PyValueError::new_err(format!("ERR_INVALID_ARGUMENT ({code})")),
        ERR_CAPACITY_EXCEEDED => {
            KernelCapacityError::new_err(format!("ERR_CAPACITY_EXCEEDED ({code})"))
        }
        ERR_QUEUE_FULL => ActionQueueFullError::new_err(format!("ERR_QUEUE_FULL ({code})")),
        ERR_CAUSALITY_VIOLATION => {
            CausalityViolationError::new_err(format!("ERR_CAUSALITY_VIOLATION ({code})"))
        }
        ERR_SEQUENCE_VIOLATION => {
            SequenceViolationError::new_err(format!("ERR_SEQUENCE_VIOLATION ({code})"))
        }
        ERR_POISONED => KernelPoisonedError::new_err(format!("ERR_POISONED ({code})")),
        ERR_REENTRANT => ReentrantError::new_err(format!("ERR_REENTRANT ({code})")),
        ERR_SNAPSHOT_UNSUPPORTED => {
            SnapshotUnsupportedError::new_err(format!("ERR_SNAPSHOT_UNSUPPORTED ({code})"))
        }
        ERR_SNAPSHOT_IO => SnapshotIoError::new_err(format!("ERR_SNAPSHOT_IO ({code})")),
        ERR_SNAPSHOT_INVALID | ERR_SNAPSHOT_MISMATCH => {
            SnapshotRestoreError::new_err(format!("ERR_SNAPSHOT_RESTORE ({code})"))
        }
        ERR_PORT_FAILURE => {
            PortFailureError::new_err(format!("ERR_PORT_FAILURE ({code}, detail={detail})"))
        }
        ERR_RESULT_OVERFLOW => {
            ResultOverflowError::new_err(format!("ERR_RESULT_OVERFLOW ({code})"))
        }
        ERR_STRATEGY_FAILURE => {
            let err = StrategyFailureError::new_err(format!(
                "ERR_STRATEGY_FAILURE ({code}, detail={detail})"
            ));
            if let Some(orig) = cause {
                err.set_cause(py, Some(orig));
            }
            err
        }
        ERR_PANIC => KernelPanicError::new_err(format!("ERR_PANIC ({code})")),
        other => KernelError::new_err(format!("Kernel error ({other}, detail={detail})")),
    }
}

/// ビューの有効期限（コールバック外アクセス時 `RuntimeError`）をチェックする。
fn ensure_valid(token: &ViewToken) -> PyResult<()> {
    if token.is_valid() {
        Ok(())
    } else {
        Err(PyRuntimeError::new_err(
            "View accessed after callback exited; retained views are invalidated by ViewGuard",
        ))
    }
}

/// `KernelConfig`（EK-IF-001 4.2.1節）。
#[pyclass(
    name = "KernelConfig",
    module = "event_kernel._core",
    frozen,
    from_py_object
)]
#[derive(Clone, Copy)]
pub struct PyKernelConfig {
    inner: KernelConfig,
}

#[pymethods]
impl PyKernelConfig {
    #[new]
    #[pyo3(signature = (
        history_ring_len,
        arena_size,
        strategy_arena_offset=0,
        strategy_arena_size=0,
        action_queue_capacity=1024,
        payload_record_size=64,
        price_scale_exp=-2,
        qty_scale_exp=-2,
        prng_seed=0,
        exec_state_max_bytes=1024,
        feed_token_max_bytes=4096,
        hash_interval=0,
        trace_enabled=false,
        max_total_bytes=268_435_456,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        history_ring_len: u32,
        arena_size: u64,
        strategy_arena_offset: u64,
        strategy_arena_size: u64,
        action_queue_capacity: u32,
        payload_record_size: u32,
        price_scale_exp: i32,
        qty_scale_exp: i32,
        prng_seed: u64,
        exec_state_max_bytes: u32,
        feed_token_max_bytes: u32,
        hash_interval: u64,
        trace_enabled: bool,
        max_total_bytes: u64,
    ) -> PyResult<Self> {
        if history_ring_len == 0
            || !arena_size.is_multiple_of(8)
            || !strategy_arena_offset.is_multiple_of(8)
            || !strategy_arena_size.is_multiple_of(8)
            || action_queue_capacity == 0
            || !payload_record_size.is_multiple_of(8)
            || payload_record_size < 16
            || price_scale_exp > 0
            || qty_scale_exp > 0
            || !exec_state_max_bytes.is_multiple_of(8)
            || feed_token_max_bytes == 0
        {
            return Err(PyValueError::new_err(
                "Invalid KernelConfig field constraint",
            ));
        }
        let end = strategy_arena_offset
            .checked_add(strategy_arena_size)
            .ok_or_else(|| PyValueError::new_err("strategy_arena overflow"))?;
        if end > arena_size {
            return Err(PyValueError::new_err(
                "strategy_arena_offset + strategy_arena_size exceeds arena_size",
            ));
        }
        Ok(Self {
            inner: KernelConfig {
                history_ring_len,
                arena_size,
                strategy_arena_offset,
                strategy_arena_size,
                action_queue_capacity,
                payload_record_size,
                price_scale_exp,
                qty_scale_exp,
                prng_seed,
                exec_state_max_bytes,
                feed_token_max_bytes,
                hash_interval,
                trace_enabled,
                max_total_bytes,
            },
        })
    }

    #[getter]
    fn history_ring_len(&self) -> u32 {
        self.inner.history_ring_len
    }

    #[getter]
    fn arena_size(&self) -> u64 {
        self.inner.arena_size
    }

    #[getter]
    fn strategy_arena_offset(&self) -> u64 {
        self.inner.strategy_arena_offset
    }

    #[getter]
    fn strategy_arena_size(&self) -> u64 {
        self.inner.strategy_arena_size
    }

    #[getter]
    fn action_queue_capacity(&self) -> u32 {
        self.inner.action_queue_capacity
    }

    #[getter]
    fn payload_record_size(&self) -> u32 {
        self.inner.payload_record_size
    }

    #[getter]
    fn price_scale_exp(&self) -> i32 {
        self.inner.price_scale_exp
    }

    #[getter]
    fn qty_scale_exp(&self) -> i32 {
        self.inner.qty_scale_exp
    }

    #[getter]
    fn prng_seed(&self) -> u64 {
        self.inner.prng_seed
    }

    #[getter]
    fn exec_state_max_bytes(&self) -> u32 {
        self.inner.exec_state_max_bytes
    }

    #[getter]
    fn feed_token_max_bytes(&self) -> u32 {
        self.inner.feed_token_max_bytes
    }

    #[getter]
    fn hash_interval(&self) -> u64 {
        self.inner.hash_interval
    }

    #[getter]
    fn trace_enabled(&self) -> bool {
        self.inner.trace_enabled
    }

    #[getter]
    fn max_total_bytes(&self) -> u64 {
        self.inner.max_total_bytes
    }
}

/// `StepResult`（EK-IF-001 4.1.2 / 4.2.1節）。
#[pyclass(name = "StepResult", module = "event_kernel._core", frozen)]
#[derive(Clone, Copy)]
pub struct PyStepResult {
    #[pyo3(get)]
    pub status: i32,
    #[pyo3(get)]
    pub detail: i32,
    #[pyo3(get)]
    pub processed_event_count: u64,
    #[pyo3(get)]
    pub now_ns: i64,
}

impl From<StepResult> for PyStepResult {
    fn from(r: StepResult) -> Self {
        Self {
            status: r.status,
            detail: r.detail,
            processed_event_count: r.processed_event_count,
            now_ns: r.now_ns,
        }
    }
}

/// コールバック内のみ有効なイベントレコードビュー（V-05 / QG-013）。
#[pyclass(name = "EventRecordView", module = "event_kernel._core", unsendable)]
pub struct PyEventRecordView {
    header: EventHeader,
    payload: Vec<u8>,
    token: ViewToken,
}

#[pymethods]
impl PyEventRecordView {
    #[getter]
    fn timestamp_ns(&self) -> PyResult<i64> {
        ensure_valid(&self.token)?;
        Ok(self.header.timestamp_ns)
    }

    #[getter]
    fn sequence_id(&self) -> PyResult<u64> {
        ensure_valid(&self.token)?;
        Ok(self.header.sequence_id)
    }

    #[getter]
    fn stream_id(&self) -> PyResult<u32> {
        ensure_valid(&self.token)?;
        Ok(self.header.stream_id)
    }

    #[getter]
    fn event_type(&self) -> PyResult<u16> {
        ensure_valid(&self.token)?;
        Ok(self.header.event_type)
    }

    #[getter]
    fn flags(&self) -> PyResult<u16> {
        ensure_valid(&self.token)?;
        Ok(self.header.flags)
    }

    #[getter]
    fn payload_idx(&self) -> PyResult<u64> {
        ensure_valid(&self.token)?;
        Ok(self.header.payload_idx)
    }

    #[getter]
    fn payload<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        ensure_valid(&self.token)?;
        Ok(PyBytes::new(py, &self.payload))
    }
}

/// 連続メモリに保持したイベントバッチビュー（`on_events(ctx, batch)` / V-01 / V-05）。
#[pyclass(name = "EventBatchView", module = "event_kernel._core", unsendable)]
pub struct PyEventBatchView {
    headers: Vec<EventHeader>,
    payloads: Vec<u8>,
    payload_stride: usize,
    timestamps_buf: Vec<i64>,
    active_exports: Cell<usize>,
    token: ViewToken,
}

#[pymethods]
impl PyEventBatchView {
    fn __len__(&self) -> PyResult<usize> {
        ensure_valid(&self.token)?;
        Ok(self.headers.len())
    }

    fn __getitem__(&self, index: isize) -> PyResult<PyEventRecordView> {
        ensure_valid(&self.token)?;
        let len = self.headers.len() as isize;
        let idx = if index < 0 { len + index } else { index };
        if idx < 0 || idx >= len {
            return Err(PyIndexError::new_err("EventBatchView index out of range"));
        }
        let u = idx as usize;
        let start = u * self.payload_stride;
        let end = start + self.payload_stride;
        Ok(PyEventRecordView {
            header: self.headers[u],
            payload: self.payloads[start..end].to_vec(),
            token: self.token.clone(),
        })
    }

    /// ゼロコピー NumPy 閲覧向けバッファプロトコル（タイムスタンプ `int64` 連続配列）。
    unsafe fn __getbuffer__(
        slf: Bound<'_, Self>,
        view: *mut ffi::Py_buffer,
        flags: c_int,
    ) -> PyResult<()> {
        let this = slf.borrow();
        ensure_valid(&this.token)?;
        if (flags & ffi::PyBUF_WRITABLE) != 0 {
            return Err(PyRuntimeError::new_err(
                "EventBatchView buffer is read-only",
            ));
        }
        if view.is_null() {
            return Err(PyValueError::new_err("Null Py_buffer"));
        }
        let byte_len = std::mem::size_of_val(this.timestamps_buf.as_slice()) as ffi::Py_ssize_t;
        let rc = unsafe {
            ffi::PyBuffer_FillInfo(
                view,
                slf.as_ptr(),
                this.timestamps_buf.as_ptr() as *mut c_void,
                byte_len,
                1,
                flags,
            )
        };
        if rc != 0 {
            return Err(PyErr::fetch(slf.py()));
        }
        this.active_exports.set(this.active_exports.get() + 1);
        Ok(())
    }

    unsafe fn __releasebuffer__(&self, _view: *mut ffi::Py_buffer) {
        self.active_exports
            .set(self.active_exports.get().saturating_sub(1));
    }
}

/// 履歴ウィンドウの読み取り専用ビュー（V-05 / QG-013）。
#[pyclass(name = "HistoryView", module = "event_kernel._core", unsendable)]
pub struct PyHistoryView {
    records: Vec<(EventHeader, Vec<u8>)>,
    token: ViewToken,
}

#[pymethods]
impl PyHistoryView {
    fn __len__(&self) -> PyResult<usize> {
        ensure_valid(&self.token)?;
        Ok(self.records.len())
    }

    fn __getitem__(&self, index: isize) -> PyResult<PyEventRecordView> {
        ensure_valid(&self.token)?;
        let len = self.records.len() as isize;
        let idx = if index < 0 { len + index } else { index };
        if idx < 0 || idx >= len {
            return Err(PyIndexError::new_err("HistoryView index out of range"));
        }
        let (header, payload) = &self.records[idx as usize];
        Ok(PyEventRecordView {
            header: *header,
            payload: payload.clone(),
            token: self.token.clone(),
        })
    }

    fn latest(&self) -> PyResult<Option<PyEventRecordView>> {
        ensure_valid(&self.token)?;
        Ok(self
            .records
            .last()
            .map(|(header, payload)| PyEventRecordView {
                header: *header,
                payload: payload.clone(),
                token: self.token.clone(),
            }))
    }
}

/// `StrategyArena` バッファ（`Py_buffer` 公開時もメモリ安全を保証）。
struct ArenaBufferBox {
    bytes: UnsafeCell<Vec<u8>>,
    active_exports: Cell<usize>,
}

/// 書き込み可能 `StrategyArena` ビュー（V-05 / QG-013）。
#[pyclass(name = "StrategyArenaView", module = "event_kernel._core", unsendable)]
pub struct PyStrategyArenaView {
    backing: Rc<ArenaBufferBox>,
    token: ViewToken,
}

#[pymethods]
impl PyStrategyArenaView {
    fn __len__(&self) -> PyResult<usize> {
        ensure_valid(&self.token)?;
        let len = unsafe { (*self.backing.bytes.get()).len() };
        Ok(len)
    }

    fn tobytes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        ensure_valid(&self.token)?;
        let slice = unsafe { &*self.backing.bytes.get() };
        Ok(PyBytes::new(py, slice))
    }

    fn __getitem__(&self, index: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        ensure_valid(&self.token)?;
        let py = index.py();
        let slice = unsafe { &*self.backing.bytes.get() };
        if let Ok(slice_obj) = index.cast::<PySlice>() {
            let indices = slice_obj.indices(slice.len() as isize)?;
            let mut out = Vec::with_capacity(indices.slicelength);
            let mut cur = indices.start;
            for _ in 0..indices.slicelength {
                out.push(slice[cur as usize]);
                cur += indices.step;
            }
            return Ok(PyBytes::new(py, &out).into_any().unbind());
        }
        let raw_idx: isize = index.extract()?;
        let len = slice.len() as isize;
        let idx = if raw_idx < 0 { len + raw_idx } else { raw_idx };
        if idx < 0 || idx >= len {
            return Err(PyIndexError::new_err(
                "StrategyArenaView index out of range",
            ));
        }
        Ok(slice[idx as usize].into_pyobject(py)?.into_any().unbind())
    }

    fn __setitem__(&self, index: &Bound<'_, PyAny>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        ensure_valid(&self.token)?;
        let slice = unsafe { &mut *self.backing.bytes.get() };
        if let Ok(slice_obj) = index.cast::<PySlice>() {
            let indices = slice_obj.indices(slice.len() as isize)?;
            let src: Vec<u8> = value.extract()?;
            if src.len() != indices.slicelength {
                return Err(PyValueError::new_err("slice assignment length mismatch"));
            }
            let mut cur = indices.start;
            for byte in src {
                slice[cur as usize] = byte;
                cur += indices.step;
            }
            return Ok(());
        }
        let raw_idx: isize = index.extract()?;
        let len = slice.len() as isize;
        let idx = if raw_idx < 0 { len + raw_idx } else { raw_idx };
        if idx < 0 || idx >= len {
            return Err(PyIndexError::new_err(
                "StrategyArenaView index out of range",
            ));
        }
        let byte: u8 = value.extract()?;
        slice[idx as usize] = byte;
        Ok(())
    }

    unsafe fn __getbuffer__(
        slf: Bound<'_, Self>,
        view: *mut ffi::Py_buffer,
        flags: c_int,
    ) -> PyResult<()> {
        let this = slf.borrow();
        ensure_valid(&this.token)?;
        if view.is_null() {
            return Err(PyValueError::new_err("Null Py_buffer"));
        }
        let ptr = unsafe { (*this.backing.bytes.get()).as_mut_ptr() };
        let len = unsafe { (*this.backing.bytes.get()).len() as ffi::Py_ssize_t };
        let rc = unsafe { ffi::PyBuffer_FillInfo(view, slf.as_ptr(), ptr.cast(), len, 0, flags) };
        if rc != 0 {
            return Err(PyErr::fetch(slf.py()));
        }
        this.backing
            .active_exports
            .set(this.backing.active_exports.get() + 1);
        Ok(())
    }

    unsafe fn __releasebuffer__(&self, _view: *mut ffi::Py_buffer) {
        self.backing
            .active_exports
            .set(self.backing.active_exports.get().saturating_sub(1));
    }
}

/// 戦略コールバック内へ渡されるコンテキスト（EK-IF-001 4.2.2節）。
#[pyclass(name = "StrategyContext", module = "event_kernel._core", unsendable)]
pub struct PyStrategyContext {
    now_ns: i64,
    history: Py<PyHistoryView>,
    strategy_arena: Py<PyStrategyArenaView>,
    prng_ptr: *mut Prng,
    submit_fn: *mut dyn FnMut(&Action<'_>) -> i32,
    token: ViewToken,
}

#[pymethods]
impl PyStrategyContext {
    #[getter]
    fn now_ns(&self) -> PyResult<i64> {
        ensure_valid(&self.token)?;
        Ok(self.now_ns)
    }

    #[getter]
    fn history(&self, py: Python<'_>) -> PyResult<Py<PyHistoryView>> {
        ensure_valid(&self.token)?;
        Ok(self.history.clone_ref(py))
    }

    #[getter]
    fn strategy_arena(&self, py: Python<'_>) -> PyResult<Py<PyStrategyArenaView>> {
        ensure_valid(&self.token)?;
        Ok(self.strategy_arena.clone_ref(py))
    }

    fn prng_next_u64(&self) -> PyResult<u64> {
        ensure_valid(&self.token)?;
        let rng = unsafe { &mut *self.prng_ptr };
        Ok(rng.next_u64())
    }

    #[pyo3(signature = (action_kind, latency_ns, payload=None))]
    fn submit_action(
        &self,
        action_kind: u16,
        latency_ns: i64,
        payload: Option<&[u8]>,
    ) -> PyResult<i32> {
        ensure_valid(&self.token)?;
        let action = Action {
            action_kind,
            latency_ns,
            payload: payload.unwrap_or(&[]),
        };
        let submit = unsafe { &mut *self.submit_fn };
        let rc = submit(&action);
        match rc {
            OK => Ok(OK),
            ERR_QUEUE_FULL => Err(ActionQueueFullError::new_err("Action queue is full")),
            ERR_INVALID_ARGUMENT => Err(PyValueError::new_err("Invalid action arguments")),
            other => Err(KernelError::new_err(format!("submit_action error {other}"))),
        }
    }
}

/// 約定・資産推移・状態遷移の結果蓄積バッファ（IF-003 出力生成用）。
#[derive(Default)]
struct ResultTables {
    fills_ts: Vec<i64>,
    fills_seq: Vec<u64>,
    fills_action_id: Vec<u64>,
    fills_instrument: Vec<u32>,
    fills_side: Vec<i8>,
    fills_price: Vec<i64>,
    fills_qty: Vec<i64>,
    fills_fee: Vec<i64>,

    eq_ts: Vec<i64>,
    eq_count: Vec<u64>,
    eq_cash: Vec<i64>,
    eq_pos: Vec<i64>,
    eq_total: Vec<i64>,

    tr_ts: Vec<i64>,
    tr_seq: Vec<u64>,
    tr_from: Vec<u16>,
    tr_to: Vec<u16>,
    tr_reason: Vec<u16>,
}

impl ResultTables {
    fn record_fill(&mut self, header: EventHeader, payload: &[u8]) {
        self.fills_ts.push(header.timestamp_ns);
        self.fills_seq.push(header.sequence_id);
        // ペイロード先頭に i64/u64 等があれば読み出し、不足時は0とする決定論的マッピング。
        let action_id = payload
            .get(0..8)
            .and_then(|b| b.try_into().ok())
            .map_or(header.sequence_id, u64::from_le_bytes);
        let instrument_id = payload
            .get(8..12)
            .and_then(|b| b.try_into().ok())
            .map_or(header.stream_id, u32::from_le_bytes);
        let side = payload
            .get(12)
            .map_or(1i8, |&b| if b == 0 { 1 } else { b as i8 });
        let fill_price = payload
            .get(16..24)
            .and_then(|b| b.try_into().ok())
            .map_or(0, i64::from_le_bytes);
        let fill_qty = payload
            .get(24..32)
            .and_then(|b| b.try_into().ok())
            .map_or(0, i64::from_le_bytes);
        let fee = payload
            .get(32..40)
            .and_then(|b| b.try_into().ok())
            .map_or(0, i64::from_le_bytes);
        self.fills_action_id.push(action_id);
        self.fills_instrument.push(instrument_id);
        self.fills_side.push(side);
        self.fills_price.push(fill_price);
        self.fills_qty.push(fill_qty);
        self.fills_fee.push(fee);
    }
}

/// 高速ネイティブ補助フィード（V-01 計測 / 決定論テスト用、純 Python フィードと併存可能）。
#[pyclass(name = "InMemoryMarketFeed", module = "event_kernel._core")]
#[derive(Clone)]
pub struct PyInMemoryMarketFeed {
    timestamps: Vec<i64>,
    stream_id: u32,
    event_type: u16,
    pos: usize,
}

#[pymethods]
impl PyInMemoryMarketFeed {
    #[new]
    #[pyo3(signature = (timestamps, stream_id=0, event_type=1))]
    fn new(timestamps: Vec<i64>, stream_id: u32, event_type: u16) -> Self {
        Self {
            timestamps,
            stream_id,
            event_type,
            pos: 0,
        }
    }
}

enum PyFeedKind {
    Native(PyInMemoryMarketFeed),
    PythonObj(Py<PyAny>),
}

struct PyFeedPortAdapter {
    kind: PyFeedKind,
}

impl EventFeedPort for PyFeedPortAdapter {
    fn peek_next_timestamp(&mut self) -> Result<Option<i64>, PortError> {
        match &mut self.kind {
            PyFeedKind::Native(n) => Ok(n.timestamps.get(n.pos).copied()),
            PyFeedKind::PythonObj(obj) => Python::attach(|py| {
                let res = obj
                    .call_method0(py, "peek_next_timestamp")
                    .map_err(|_| PortError(ERR_PORT_FAILURE))?;
                if res.is_none(py) {
                    Ok(None)
                } else {
                    res.extract::<i64>(py)
                        .map(Some)
                        .map_err(|_| PortError(ERR_PORT_FAILURE))
                }
            }),
        }
    }

    fn pop_next(&mut self, header: &mut EventHeader, payload: &mut [u8]) -> Result<(), PortError> {
        match &mut self.kind {
            PyFeedKind::Native(n) => {
                let Some(&ts) = n.timestamps.get(n.pos) else {
                    return Err(PortError(ERR_PORT_FAILURE));
                };
                n.pos += 1;
                *header = EventHeader {
                    timestamp_ns: ts,
                    sequence_id: n.pos as u64,
                    stream_id: n.stream_id,
                    event_type: n.event_type,
                    flags: 0,
                    payload_idx: 0,
                };
                payload.fill(0);
                Ok(())
            }
            PyFeedKind::PythonObj(obj) => Python::attach(|py| {
                let tuple = obj
                    .call_method0(py, "pop_next")
                    .map_err(|_| PortError(ERR_PORT_FAILURE))?;
                let bound = tuple.bind(py);
                let (ts, seq, stream_id, ev_type, flags, data): (i64, u64, u32, u16, u16, Vec<u8>) =
                    bound.extract().map_err(|_| PortError(ERR_PORT_FAILURE))?;
                *header = EventHeader {
                    timestamp_ns: ts,
                    sequence_id: seq,
                    stream_id,
                    event_type: ev_type,
                    flags,
                    payload_idx: 0,
                };
                payload.fill(0);
                let n = payload.len().min(data.len());
                payload[..n].copy_from_slice(&data[..n]);
                Ok(())
            }),
        }
    }

    fn position(&self, out: &mut [u8]) -> Result<FeedPosition, PortError> {
        match &self.kind {
            PyFeedKind::Native(n) => {
                if out.len() < 8 {
                    return Err(PortError(ERR_PORT_FAILURE));
                }
                out[..8].copy_from_slice(&(n.pos as u64).to_le_bytes());
                Ok(FeedPosition {
                    seekable: true,
                    len: 8,
                })
            }
            PyFeedKind::PythonObj(obj) => Python::attach(|py| {
                if !obj.bind(py).hasattr("position").unwrap_or(false) {
                    return Ok(FeedPosition {
                        seekable: false,
                        len: 0,
                    });
                }
                let res = obj
                    .call_method0(py, "position")
                    .map_err(|_| PortError(ERR_PORT_FAILURE))?;
                let (seekable, token): (bool, Vec<u8>) =
                    res.extract(py).map_err(|_| PortError(ERR_PORT_FAILURE))?;
                if !seekable {
                    return Ok(FeedPosition {
                        seekable: false,
                        len: 0,
                    });
                }
                if token.len() > out.len() {
                    return Err(PortError(ERR_PORT_FAILURE));
                }
                out[..token.len()].copy_from_slice(&token);
                Ok(FeedPosition {
                    seekable: true,
                    len: token.len(),
                })
            }),
        }
    }

    fn validate_position(&self, token: &[u8]) -> Result<(), PortError> {
        match &self.kind {
            PyFeedKind::Native(n) => {
                if token.len() != 8 {
                    return Err(PortError(ERR_PORT_FAILURE));
                }
                let idx = u64::from_le_bytes(token[..8].try_into().unwrap()) as usize;
                if idx <= n.timestamps.len() {
                    Ok(())
                } else {
                    Err(PortError(ERR_PORT_FAILURE))
                }
            }
            PyFeedKind::PythonObj(obj) => Python::attach(|py| {
                if !obj.bind(py).hasattr("validate_position").unwrap_or(false) {
                    return Ok(());
                }
                obj.call_method1(py, "validate_position", (PyBytes::new(py, token),))
                    .map(|_| ())
                    .map_err(|_| PortError(ERR_PORT_FAILURE))
            }),
        }
    }

    fn seek(&mut self, token: &[u8]) -> Result<(), PortError> {
        match &mut self.kind {
            PyFeedKind::Native(n) => {
                if token.len() != 8 {
                    return Err(PortError(ERR_PORT_FAILURE));
                }
                let idx = u64::from_le_bytes(token[..8].try_into().unwrap()) as usize;
                if idx > n.timestamps.len() {
                    return Err(PortError(ERR_PORT_FAILURE));
                }
                n.pos = idx;
                Ok(())
            }
            PyFeedKind::PythonObj(obj) => Python::attach(|py| {
                if !obj.bind(py).hasattr("seek").unwrap_or(false) {
                    return Err(PortError(ERR_PORT_FAILURE));
                }
                obj.call_method1(py, "seek", (PyBytes::new(py, token),))
                    .map(|_| ())
                    .map_err(|_| PortError(ERR_PORT_FAILURE))
            }),
        }
    }
}

/// 高速ネイティブ約定モデル（V-01〜V-03 計測 / テスト用）。
#[pyclass(name = "SimExecution", module = "event_kernel._core")]
#[derive(Clone, Default)]
pub struct PySimExecution {
    pending: VecDeque<(EventHeader, Vec<u8>)>,
    delay_ns: i64,
}

#[pymethods]
impl PySimExecution {
    #[new]
    #[pyo3(signature = (delay_ns=5))]
    fn new(delay_ns: i64) -> Self {
        Self {
            pending: VecDeque::new(),
            delay_ns: delay_ns.max(0),
        }
    }
}

enum PyExecKind {
    Native(PySimExecution),
    PythonObj(Py<PyAny>),
}

struct PyExecPortAdapter {
    kind: PyExecKind,
    tables: Rc<RefCell<ResultTables>>,
}

impl ExecutionPort for PyExecPortAdapter {
    fn on_arrival(&mut self, event: EventRecord<'_>, rng: &mut Prng) -> Result<(), PortError> {
        match &mut self.kind {
            PyExecKind::Native(sim) => {
                let rnd = rng.next_u64();
                let mut payload = vec![0u8; event.payload.len()];
                if payload.len() >= 8 {
                    payload[..8].copy_from_slice(&rnd.to_le_bytes());
                }
                if payload.len() >= 24 {
                    payload[16..24].copy_from_slice(&1000i64.to_le_bytes());
                }
                if payload.len() >= 32 {
                    payload[24..32].copy_from_slice(&1i64.to_le_bytes());
                }
                let mut hdr = event.header;
                hdr.timestamp_ns = event.header.timestamp_ns.saturating_add(sim.delay_ns);
                hdr.event_type = event_type::FILL;
                sim.pending.push_back((hdr, payload));
                Ok(())
            }
            PyExecKind::PythonObj(obj) => Python::attach(|py| {
                let rnd = rng.next_u64();
                let hdr = (
                    event.header.timestamp_ns,
                    event.header.sequence_id,
                    event.header.stream_id,
                    event.header.event_type,
                    event.header.flags,
                );
                obj.call_method1(
                    py,
                    "on_arrival",
                    (hdr, PyBytes::new(py, event.payload), rnd),
                )
                .map(|_| ())
                .map_err(|_| PortError(ERR_PORT_FAILURE))
            }),
        }
    }

    fn on_market_event(
        &mut self,
        event: EventRecord<'_>,
        history: HistoryView<'_>,
        rng: &mut Prng,
    ) -> Result<(), PortError> {
        match &mut self.kind {
            PyExecKind::Native(_) => Ok(()),
            PyExecKind::PythonObj(obj) => Python::attach(|py| {
                if !obj.bind(py).hasattr("on_market_event").unwrap_or(false) {
                    return Ok(());
                }
                let rnd = rng.next_u64();
                let hdr = (
                    event.header.timestamp_ns,
                    event.header.sequence_id,
                    event.header.stream_id,
                    event.header.event_type,
                    event.header.flags,
                );
                obj.call_method1(
                    py,
                    "on_market_event",
                    (hdr, PyBytes::new(py, event.payload), history.len(), rnd),
                )
                .map(|_| ())
                .map_err(|_| PortError(ERR_PORT_FAILURE))
            }),
        }
    }

    fn poll_result(
        &mut self,
        header: &mut EventHeader,
        payload: &mut [u8],
    ) -> Result<bool, PortError> {
        match &mut self.kind {
            PyExecKind::Native(sim) => {
                let Some((hdr, pl)) = sim.pending.pop_front() else {
                    return Ok(false);
                };
                *header = hdr;
                payload.fill(0);
                let n = payload.len().min(pl.len());
                payload[..n].copy_from_slice(&pl[..n]);
                if hdr.event_type == event_type::FILL {
                    self.tables.borrow_mut().record_fill(hdr, payload);
                }
                Ok(true)
            }
            PyExecKind::PythonObj(obj) => Python::attach(|py| {
                let res = obj
                    .call_method0(py, "poll_result")
                    .map_err(|_| PortError(ERR_PORT_FAILURE))?;
                if res.is_none(py) {
                    return Ok(false);
                }
                let (ts, seq, stream_id, ev_type, flags, data): (i64, u64, u32, u16, u16, Vec<u8>) =
                    res.extract(py).map_err(|_| PortError(ERR_PORT_FAILURE))?;
                *header = EventHeader {
                    timestamp_ns: ts,
                    sequence_id: seq,
                    stream_id,
                    event_type: ev_type,
                    flags,
                    payload_idx: 0,
                };
                payload.fill(0);
                let n = payload.len().min(data.len());
                payload[..n].copy_from_slice(&data[..n]);
                if ev_type == event_type::FILL {
                    self.tables.borrow_mut().record_fill(*header, payload);
                }
                Ok(true)
            }),
        }
    }

    fn capture_state(&self, out: &mut [u8]) -> Result<usize, PortError> {
        match &self.kind {
            PyExecKind::Native(sim) => {
                let count = sim.pending.len() as u64;
                if out.len() < 8 {
                    return Err(PortError(ERR_PORT_FAILURE));
                }
                out[..8].copy_from_slice(&count.to_le_bytes());
                Ok(8)
            }
            PyExecKind::PythonObj(obj) => Python::attach(|py| {
                if !obj.bind(py).hasattr("capture_state").unwrap_or(false) {
                    return Ok(0);
                }
                let res = obj
                    .call_method0(py, "capture_state")
                    .map_err(|_| PortError(ERR_PORT_FAILURE))?;
                let bytes: Vec<u8> = res.extract(py).map_err(|_| PortError(ERR_PORT_FAILURE))?;
                if bytes.len() > out.len() {
                    return Err(PortError(ERR_PORT_FAILURE));
                }
                out[..bytes.len()].copy_from_slice(&bytes);
                Ok(bytes.len())
            }),
        }
    }

    fn validate_state(&self, state: &[u8]) -> Result<(), PortError> {
        match &self.kind {
            PyExecKind::Native(_) => {
                if state.len() >= 8 {
                    Ok(())
                } else {
                    Err(PortError(ERR_PORT_FAILURE))
                }
            }
            PyExecKind::PythonObj(obj) => Python::attach(|py| {
                if !obj.bind(py).hasattr("validate_state").unwrap_or(false) {
                    return Ok(());
                }
                obj.call_method1(py, "validate_state", (PyBytes::new(py, state),))
                    .map(|_| ())
                    .map_err(|_| PortError(ERR_PORT_FAILURE))
            }),
        }
    }

    fn restore_state(&mut self, state: &[u8]) -> Result<(), PortError> {
        match &mut self.kind {
            PyExecKind::Native(sim) => {
                if state.len() < 8 {
                    return Err(PortError(ERR_PORT_FAILURE));
                }
                sim.pending.clear();
                Ok(())
            }
            PyExecKind::PythonObj(obj) => Python::attach(|py| {
                if !obj.bind(py).hasattr("restore_state").unwrap_or(false) {
                    return Ok(());
                }
                obj.call_method1(py, "restore_state", (PyBytes::new(py, state),))
                    .map(|_| ())
                    .map_err(|_| PortError(ERR_PORT_FAILURE))
            }),
        }
    }
}

enum PyTraceKind {
    None,
    NativeDisk(Rc<RefCell<DiskTraceSink>>),
    PythonObj(Py<PyAny>),
}

struct PyTraceSinkAdapter {
    kind: PyTraceKind,
}

impl TraceSink for PyTraceSinkAdapter {
    fn is_present(&self) -> bool {
        !matches!(self.kind, PyTraceKind::None)
    }

    fn write_header(&mut self, header: &TraceHeader) -> Result<(), PortError> {
        match &self.kind {
            PyTraceKind::None => Err(PortError(ERR_PORT_FAILURE)),
            PyTraceKind::NativeDisk(inner) => inner
                .try_borrow_mut()
                .map_err(|_| PortError(ERR_REENTRANT))?
                .write_header(header),
            PyTraceKind::PythonObj(obj) => Python::attach(|py| {
                obj.call_method1(
                    py,
                    "write_header",
                    (PyBytes::new(py, &header.to_le_bytes()),),
                )
                .map(|_| ())
                .map_err(|_| PortError(ERR_PORT_FAILURE))
            }),
        }
    }

    fn write(&mut self, record: &TraceRecord) -> Result<(), PortError> {
        match &self.kind {
            PyTraceKind::None => Err(PortError(ERR_PORT_FAILURE)),
            PyTraceKind::NativeDisk(inner) => inner
                .try_borrow_mut()
                .map_err(|_| PortError(ERR_REENTRANT))?
                .write(record),
            PyTraceKind::PythonObj(obj) => Python::attach(|py| {
                obj.call_method1(py, "write", (PyBytes::new(py, &record.to_le_bytes()),))
                    .map(|_| ())
                    .map_err(|_| PortError(ERR_PORT_FAILURE))
            }),
        }
    }
}

struct PySnapPortAdapter {
    obj: Py<PyAny>,
}

impl SnapshotPort for PySnapPortAdapter {
    fn store(
        &mut self,
        blob: &[u8],
        logical_time_ns: i64,
        event_count: u64,
    ) -> Result<(), PortError> {
        Python::attach(|py| {
            self.obj
                .call_method1(
                    py,
                    "store",
                    (PyBytes::new(py, blob), logical_time_ns, event_count),
                )
                .map(|_| ())
                .map_err(|_| PortError(ERR_SNAPSHOT_IO))
        })
    }
}

/// `adapters/` 原子ディスク永続化スナップショットポートの Python ラッパ。
#[pyclass(name = "DiskSnapshotPort", module = "event_kernel._core", unsendable)]
pub struct PyDiskSnapshotPort {
    inner: DiskSnapshotStore,
}

#[pymethods]
impl PyDiskSnapshotPort {
    #[new]
    fn new(py: Python<'_>, dir: &str, run_id: &str) -> PyResult<Self> {
        let store =
            DiskSnapshotStore::open(dir, run_id).map_err(|rc| map_error_code(py, rc, 0, None))?;
        Ok(Self { inner: store })
    }

    fn cleaned_tmp_files(&self) -> usize {
        self.inner.cleaned_tmp_files()
    }

    fn snapshot_path(&self, logical_time_ns: i64, event_count: u64) -> String {
        self.inner
            .snapshot_path(logical_time_ns, event_count)
            .to_string_lossy()
            .into_owned()
    }

    fn store(
        &mut self,
        py: Python<'_>,
        blob: &[u8],
        logical_time_ns: i64,
        event_count: u64,
    ) -> PyResult<()> {
        self.inner
            .store(blob, logical_time_ns, event_count)
            .map_err(|e| map_error_code(py, e.0, 0, None))
    }

    fn load<'py>(
        &self,
        py: Python<'py>,
        logical_time_ns: i64,
        event_count: u64,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let bytes = self
            .inner
            .load(logical_time_ns, event_count)
            .map_err(|rc| map_error_code(py, rc, 0, None))?;
        Ok(PyBytes::new(py, &bytes))
    }
}

/// `adapters/` 追記専用トレースシンクの Python ラッパ。
#[pyclass(name = "DiskTraceSink", module = "event_kernel._core", unsendable)]
pub struct PyDiskTraceSink {
    inner: Rc<RefCell<DiskTraceSink>>,
    path: String,
}

#[pymethods]
impl PyDiskTraceSink {
    #[new]
    fn new(py: Python<'_>, dir: &str, run_id: &str) -> PyResult<Self> {
        let sink =
            DiskTraceSink::create(dir, run_id).map_err(|rc| map_error_code(py, rc, 0, None))?;
        let path = sink.path().to_string_lossy().into_owned();
        Ok(Self {
            inner: Rc::new(RefCell::new(sink)),
            path,
        })
    }

    fn path(&self) -> &str {
        &self.path
    }

    fn write_header(&self, py: Python<'_>, header_bytes: &[u8]) -> PyResult<()> {
        if header_bytes.len() != TraceHeader::SIZE {
            return Err(map_error_code(py, ERR_PORT_FAILURE, 0, None));
        }
        let hdr = TraceHeader::from_le_bytes_validated(header_bytes)
            .ok_or_else(|| map_error_code(py, ERR_PORT_FAILURE, 0, None))?;
        self.inner
            .try_borrow_mut()
            .map_err(|_| map_error_code(py, ERR_REENTRANT, 0, None))?
            .write_header(&hdr)
            .map_err(|e| map_error_code(py, e.0, 0, None))
    }

    fn write(&self, py: Python<'_>, record_bytes: &[u8]) -> PyResult<()> {
        if record_bytes.len() != TraceRecord::SIZE {
            return Err(map_error_code(py, ERR_PORT_FAILURE, 0, None));
        }
        let rec = TraceRecord::from_le_bytes(record_bytes)
            .ok_or_else(|| map_error_code(py, ERR_PORT_FAILURE, 0, None))?;
        self.inner
            .try_borrow_mut()
            .map_err(|_| map_error_code(py, ERR_REENTRANT, 0, None))?
            .write(&rec)
            .map_err(|e| map_error_code(py, e.0, 0, None))
    }

    fn sync(&self, py: Python<'_>) -> PyResult<()> {
        self.inner
            .try_borrow_mut()
            .map_err(|_| map_error_code(py, ERR_REENTRANT, 0, None))?
            .sync()
            .map_err(|rc| map_error_code(py, rc, 0, None))
    }
}

struct PyStrategyAdapter {
    obj: Py<PyAny>,
    lease: ViewLease,
    arena_box: Rc<ArenaBufferBox>,
    last_py_err: Rc<RefCell<Option<PyErr>>>,
    leaked_buffer: Rc<Cell<bool>>,
    tables: Rc<RefCell<ResultTables>>,
}

impl Strategy for PyStrategyAdapter {
    fn on_event(&mut self, context: &mut Context<'_>, event: EventRecord<'_>) -> StrategyStatus {
        let mut status: StrategyStatus = 0;
        let panic_res = catch_unwind(AssertUnwindSafe(|| {
            Python::attach(|py| {
                // `StrategyArena` を一時バッファへ同期し、終了時に戻す（リーク時は UB 防止）。
                {
                    let dst = unsafe { &mut *self.arena_box.bytes.get() };
                    let src = context.strategy_arena();
                    if dst.len() != src.len() {
                        dst.resize(src.len(), 0);
                    }
                    dst.copy_from_slice(src);
                }

                let (guard, token) = self.lease.enter();
                let mut hist_records = Vec::with_capacity(context.history().len());
                for i in 0..context.history().len() {
                    if let Some(rec) = context.history().get(i) {
                        hist_records.push((rec.header, rec.payload.to_vec()));
                    }
                }
                let py_hist = match Py::new(
                    py,
                    PyHistoryView {
                        records: hist_records,
                        token: token.clone(),
                    },
                ) {
                    Ok(h) => h,
                    Err(e) => {
                        *self.last_py_err.borrow_mut() = Some(e);
                        status = 1;
                        return;
                    }
                };
                let py_arena = match Py::new(
                    py,
                    PyStrategyArenaView {
                        backing: Rc::clone(&self.arena_box),
                        token: token.clone(),
                    },
                ) {
                    Ok(a) => a,
                    Err(e) => {
                        *self.last_py_err.borrow_mut() = Some(e);
                        status = 1;
                        return;
                    }
                };
                let py_event = match Py::new(
                    py,
                    PyEventRecordView {
                        header: event.header,
                        payload: event.payload.to_vec(),
                        token: token.clone(),
                    },
                ) {
                    Ok(ev) => ev,
                    Err(e) => {
                        *self.last_py_err.borrow_mut() = Some(e);
                        status = 1;
                        return;
                    }
                };

                let now_ns = context.now_ns();
                let mut prng_copy = context.rng().clone();
                let mut submit_closure = |action: &Action<'_>| context.submit_action(action);
                let submit_dyn: &mut dyn FnMut(&Action<'_>) -> i32 = &mut submit_closure;
                let submit_ptr: *mut dyn FnMut(&Action<'_>) -> i32 = unsafe {
                    std::mem::transmute::<
                        *mut dyn FnMut(&Action<'_>) -> i32,
                        *mut (dyn FnMut(&Action<'_>) -> i32 + 'static),
                    >(submit_dyn)
                };
                let py_ctx = match Py::new(
                    py,
                    PyStrategyContext {
                        now_ns,
                        history: py_hist,
                        strategy_arena: py_arena,
                        prng_ptr: &mut prng_copy,
                        submit_fn: submit_ptr,
                        token: token.clone(),
                    },
                ) {
                    Ok(c) => c,
                    Err(e) => {
                        *self.last_py_err.borrow_mut() = Some(e);
                        status = 1;
                        return;
                    }
                };

                let call_res = self.obj.call_method1(py, "on_event", (py_ctx, py_event));
                // ViewGuard を明示的に drop し、残存エクスポートを確認する。
                drop(guard);
                *context.rng() = prng_copy;
                {
                    let src = unsafe { &*self.arena_box.bytes.get() };
                    let dst = context.strategy_arena();
                    if src.len() == dst.len() {
                        dst.copy_from_slice(src);
                    }
                }
                if self.arena_box.active_exports.get() > 0 {
                    self.leaked_buffer.set(true);
                    status = 2;
                    return;
                }
                match call_res {
                    Ok(ret) => {
                        let code = ret.extract::<u32>(py).unwrap_or(0);
                        status = code;
                    }
                    Err(e) => {
                        *self.last_py_err.borrow_mut() = Some(e);
                        status = 1;
                    }
                }
                self.tables.borrow_mut().eq_ts.push(now_ns);
                self.tables.borrow_mut().eq_count.push(1);
                self.tables.borrow_mut().eq_cash.push(0);
                self.tables.borrow_mut().eq_pos.push(0);
                self.tables.borrow_mut().eq_total.push(0);
            });
        }));
        if let Err(payload) = panic_res {
            std::panic::resume_unwind(payload);
        }
        status
    }
}

/// `Kernel` Python ラッパ（EK-IF-001 4.2.1節）。
#[pyclass(name = "Kernel", module = "event_kernel._core", unsendable)]
pub struct PyKernel {
    kernel:
        Option<Kernel<PyFeedPortAdapter, PyExecPortAdapter, PyStrategyAdapter, PyTraceSinkAdapter>>,
    last_py_err: Rc<RefCell<Option<PyErr>>>,
    leaked_buffer: Rc<Cell<bool>>,
    tables: Rc<RefCell<ResultTables>>,
    config: KernelConfig,
}

impl PyKernel {
    fn kernel_ref(
        &self,
    ) -> PyResult<
        &Kernel<PyFeedPortAdapter, PyExecPortAdapter, PyStrategyAdapter, PyTraceSinkAdapter>,
    > {
        self.kernel
            .as_ref()
            .ok_or_else(|| KernelError::new_err("Kernel has already been closed"))
    }

    fn handle_step_outcome(&self, py: Python<'_>, res: StepResult) -> PyResult<PyStepResult> {
        let leaked = self.leaked_buffer.replace(false);
        let py_err = self.last_py_err.borrow_mut().take();
        if leaked {
            return Err(PyRuntimeError::new_err(
                "Derived memoryview/buffer was retained past callback exit",
            ));
        }
        if res.status >= 0 {
            return Ok(PyStepResult::from(res));
        }
        Err(map_error_code(py, res.status, res.detail, py_err))
    }
}

#[pymethods]
impl PyKernel {
    #[new]
    #[pyo3(signature = (config, feeds, execution, strategy, trace_sink=None))]
    fn new(
        py: Python<'_>,
        config: PyKernelConfig,
        feeds: &Bound<'_, PySequence>,
        execution: Py<PyAny>,
        strategy: Py<PyAny>,
        trace_sink: Option<Py<PyAny>>,
    ) -> PyResult<Self> {
        let mut feed_adapters = Vec::with_capacity(feeds.len()?);
        for i in 0..feeds.len()? {
            let item = feeds.get_item(i)?;
            if let Ok(native) = item.extract::<PyRef<'_, PyInMemoryMarketFeed>>() {
                feed_adapters.push(PyFeedPortAdapter {
                    kind: PyFeedKind::Native(native.clone()),
                });
            } else {
                feed_adapters.push(PyFeedPortAdapter {
                    kind: PyFeedKind::PythonObj(item.unbind()),
                });
            }
        }
        let tables = Rc::new(RefCell::new(ResultTables::default()));
        let exec_kind =
            if let Ok(native) = execution.bind(py).extract::<PyRef<'_, PySimExecution>>() {
                PyExecKind::Native(native.clone())
            } else {
                PyExecKind::PythonObj(execution)
            };
        let exec_adapter = PyExecPortAdapter {
            kind: exec_kind,
            tables: Rc::clone(&tables),
        };
        let arena_len = usize::try_from(config.inner.strategy_arena_size).unwrap_or(0);
        let arena_box = Rc::new(ArenaBufferBox {
            bytes: UnsafeCell::new(vec![0u8; arena_len]),
            active_exports: Cell::new(0),
        });
        let last_py_err = Rc::new(RefCell::new(None));
        let leaked_buffer = Rc::new(Cell::new(false));
        let strat_adapter = PyStrategyAdapter {
            obj: strategy,
            lease: ViewLease::new(),
            arena_box,
            last_py_err: Rc::clone(&last_py_err),
            leaked_buffer: Rc::clone(&leaked_buffer),
            tables: Rc::clone(&tables),
        };
        let trace_kind = match trace_sink {
            None => PyTraceKind::None,
            Some(obj) => {
                if let Ok(native) = obj.bind(py).extract::<PyRef<'_, PyDiskTraceSink>>() {
                    PyTraceKind::NativeDisk(Rc::clone(&native.inner))
                } else {
                    PyTraceKind::PythonObj(obj)
                }
            }
        };
        let trace_adapter = PyTraceSinkAdapter { kind: trace_kind };
        let kernel = Kernel::create_multi_with_trace(
            config.inner,
            feed_adapters,
            exec_adapter,
            strat_adapter,
            trace_adapter,
        )
        .map_err(|code| map_error_code(py, code, 0, None))?;
        Ok(Self {
            kernel: Some(kernel),
            last_py_err,
            leaked_buffer,
            tables,
            config: config.inner,
        })
    }

    fn state(&self) -> PyResult<&'static str> {
        let k = self.kernel_ref()?;
        Ok(match k.state() {
            HandleState::Ready => "Ready",
            HandleState::Running => "Running",
            HandleState::Finished => "Finished",
            HandleState::Poisoned => "Poisoned",
        })
    }

    fn step(&self, py: Python<'_>) -> PyResult<PyStepResult> {
        let k = self.kernel_ref()?;
        let outcome = catch_unwind(AssertUnwindSafe(|| k.step()));
        match outcome {
            Ok(res) => self.handle_step_outcome(py, res),
            Err(_) => {
                k.poison();
                Err(map_error_code(py, ERR_PANIC, 0, None))
            }
        }
    }

    #[pyo3(signature = (max_events=None))]
    fn run(&self, py: Python<'_>, max_events: Option<u64>) -> PyResult<PyStepResult> {
        let k = self.kernel_ref()?;
        if matches!(max_events, Some(0)) {
            return Err(PyValueError::new_err("max_events must be >= 1 or None"));
        }
        let limit = RunLimit {
            max_events: max_events.unwrap_or(0),
        };
        let outcome = catch_unwind(AssertUnwindSafe(|| k.run(limit)));
        match outcome {
            Ok(res) => self.handle_step_outcome(py, res),
            Err(_) => {
                k.poison();
                Err(map_error_code(py, ERR_PANIC, 0, None))
            }
        }
    }

    /// バッチ単位コールバック比較用ランナー（ADR-003 V-01）。
    #[pyo3(signature = (strategy, timestamps, batch_size=64, payload_record_size=64))]
    #[staticmethod]
    fn benchmark_batch_callbacks(
        py: Python<'_>,
        strategy: Py<PyAny>,
        timestamps: Vec<i64>,
        batch_size: usize,
        payload_record_size: usize,
    ) -> PyResult<u64> {
        if batch_size == 0 || payload_record_size < 16 {
            return Err(PyValueError::new_err("Invalid batch benchmark arguments"));
        }
        let lease = ViewLease::new();
        let mut processed: u64 = 0;
        let arena_box = Rc::new(ArenaBufferBox {
            bytes: UnsafeCell::new(Vec::new()),
            active_exports: Cell::new(0),
        });
        let mut prng = Prng::from_seed(1);
        let mut noop_submit = |_action: &Action<'_>| OK;
        let submit_dyn: &mut dyn FnMut(&Action<'_>) -> i32 = &mut noop_submit;
        let submit_ptr: *mut dyn FnMut(&Action<'_>) -> i32 = unsafe {
            std::mem::transmute::<
                *mut dyn FnMut(&Action<'_>) -> i32,
                *mut (dyn FnMut(&Action<'_>) -> i32 + 'static),
            >(submit_dyn)
        };

        for chunk in timestamps.chunks(batch_size) {
            let (guard, token) = lease.enter();
            let mut headers = Vec::with_capacity(chunk.len());
            for (i, &ts) in chunk.iter().enumerate() {
                headers.push(EventHeader {
                    timestamp_ns: ts,
                    sequence_id: processed + (i as u64) + 1,
                    stream_id: 0,
                    event_type: 1,
                    flags: 0,
                    payload_idx: 0,
                });
            }
            let py_batch = Py::new(
                py,
                PyEventBatchView {
                    headers,
                    payloads: vec![0u8; chunk.len() * payload_record_size],
                    payload_stride: payload_record_size,
                    timestamps_buf: chunk.to_vec(),
                    active_exports: Cell::new(0),
                    token: token.clone(),
                },
            )?;
            let py_hist = Py::new(
                py,
                PyHistoryView {
                    records: Vec::new(),
                    token: token.clone(),
                },
            )?;
            let py_arena = Py::new(
                py,
                PyStrategyArenaView {
                    backing: Rc::clone(&arena_box),
                    token: token.clone(),
                },
            )?;
            let py_ctx = Py::new(
                py,
                PyStrategyContext {
                    now_ns: *chunk.last().unwrap_or(&0),
                    history: py_hist,
                    strategy_arena: py_arena,
                    prng_ptr: &mut prng,
                    submit_fn: submit_ptr,
                    token: token.clone(),
                },
            )?;
            let res = strategy.call_method1(py, "on_events", (py_ctx, py_batch));
            drop(guard);
            res?;
            processed += chunk.len() as u64;
        }
        Ok(processed)
    }

    fn init_strategy_arena(&self, py: Python<'_>, offset: u64, data: &[u8]) -> PyResult<()> {
        let k = self.kernel_ref()?;
        let rc = k.init_strategy_arena(offset, data);
        if rc == OK {
            Ok(())
        } else {
            Err(map_error_code(py, rc, 0, None))
        }
    }

    #[pyo3(signature = (offset=0, length=None))]
    fn read_strategy_arena<'py>(
        &self,
        py: Python<'py>,
        offset: u64,
        length: Option<usize>,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let k = self.kernel_ref()?;
        let total = usize::try_from(self.config.strategy_arena_size).unwrap_or(0);
        let len = length.unwrap_or_else(|| total.saturating_sub(offset as usize));
        let mut out = vec![0u8; len];
        let rc = k.read_strategy_arena(offset, &mut out);
        if rc == OK {
            Ok(PyBytes::new(py, &out))
        } else {
            Err(map_error_code(py, rc, 0, None))
        }
    }

    fn capture(&self, py: Python<'_>, snapshot_port: Py<PyAny>) -> PyResult<()> {
        let k = self.kernel_ref()?;
        let mut adapter = PySnapPortAdapter { obj: snapshot_port };
        let rc = k.capture(&mut adapter);
        if rc == OK {
            Ok(())
        } else {
            Err(map_error_code(py, rc, 0, None))
        }
    }

    fn restore(&self, py: Python<'_>, blob: &[u8]) -> PyResult<()> {
        let k = self.kernel_ref()?;
        if blob.is_empty() {
            return Err(PyValueError::new_err("snapshot blob cannot be empty"));
        }
        let rc = k.restore(blob);
        if rc == OK {
            Ok(())
        } else {
            Err(map_error_code(py, rc, 0, None))
        }
    }

    fn state_digest<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let k = self.kernel_ref()?;
        match k.state_digest() {
            Ok(d) => Ok(PyBytes::new(py, &d)),
            Err(code) => Err(map_error_code(py, code, 0, None)),
        }
    }

    fn export_fills(&self) -> PyResult<PyArrowResultStream> {
        let k = self.kernel_ref()?;
        if k.state() == HandleState::Running {
            return Err(ReentrantError::new_err("Cannot export while Running"));
        }
        let t = self.tables.borrow();
        let rows = t.fills_ts.len();
        let cols = vec![
            ArrowColumn {
                name: "timestamp_ns",
                format: "l",
                buffer: ArrowColumnData::I64(t.fills_ts.clone()),
            },
            ArrowColumn {
                name: "sequence_id",
                format: "L",
                buffer: ArrowColumnData::U64(t.fills_seq.clone()),
            },
            ArrowColumn {
                name: "action_id",
                format: "L",
                buffer: ArrowColumnData::U64(t.fills_action_id.clone()),
            },
            ArrowColumn {
                name: "instrument_id",
                format: "I",
                buffer: ArrowColumnData::U32(t.fills_instrument.clone()),
            },
            ArrowColumn {
                name: "side",
                format: "c",
                buffer: ArrowColumnData::I8(t.fills_side.clone()),
            },
            ArrowColumn {
                name: "fill_price",
                format: "l",
                buffer: ArrowColumnData::I64(t.fills_price.clone()),
            },
            ArrowColumn {
                name: "fill_qty",
                format: "l",
                buffer: ArrowColumnData::I64(t.fills_qty.clone()),
            },
            ArrowColumn {
                name: "fee",
                format: "l",
                buffer: ArrowColumnData::I64(t.fills_fee.clone()),
            },
        ];
        Ok(PyArrowResultStream::from_columns("fills", rows, cols))
    }

    fn export_equity(&self) -> PyResult<PyArrowResultStream> {
        let k = self.kernel_ref()?;
        if k.state() == HandleState::Running {
            return Err(ReentrantError::new_err("Cannot export while Running"));
        }
        let t = self.tables.borrow();
        let rows = t.eq_ts.len();
        let cols = vec![
            ArrowColumn {
                name: "timestamp_ns",
                format: "l",
                buffer: ArrowColumnData::I64(t.eq_ts.clone()),
            },
            ArrowColumn {
                name: "event_count",
                format: "L",
                buffer: ArrowColumnData::U64(t.eq_count.clone()),
            },
            ArrowColumn {
                name: "cash",
                format: "l",
                buffer: ArrowColumnData::I64(t.eq_cash.clone()),
            },
            ArrowColumn {
                name: "position_value",
                format: "l",
                buffer: ArrowColumnData::I64(t.eq_pos.clone()),
            },
            ArrowColumn {
                name: "total_equity",
                format: "l",
                buffer: ArrowColumnData::I64(t.eq_total.clone()),
            },
        ];
        Ok(PyArrowResultStream::from_columns(
            "equity_curve",
            rows,
            cols,
        ))
    }

    fn export_transitions(&self) -> PyResult<PyArrowResultStream> {
        let k = self.kernel_ref()?;
        if k.state() == HandleState::Running {
            return Err(ReentrantError::new_err("Cannot export while Running"));
        }
        let t = self.tables.borrow();
        let rows = t.tr_ts.len();
        let cols = vec![
            ArrowColumn {
                name: "timestamp_ns",
                format: "l",
                buffer: ArrowColumnData::I64(t.tr_ts.clone()),
            },
            ArrowColumn {
                name: "sequence_id",
                format: "L",
                buffer: ArrowColumnData::U64(t.tr_seq.clone()),
            },
            ArrowColumn {
                name: "from_state",
                format: "S",
                buffer: ArrowColumnData::U16(t.tr_from.clone()),
            },
            ArrowColumn {
                name: "to_state",
                format: "S",
                buffer: ArrowColumnData::U16(t.tr_to.clone()),
            },
            ArrowColumn {
                name: "reason_code",
                format: "S",
                buffer: ArrowColumnData::U16(t.tr_reason.clone()),
            },
        ];
        Ok(PyArrowResultStream::from_columns(
            "state_transitions",
            rows,
            cols,
        ))
    }

    fn close(&mut self) -> PyResult<()> {
        if let Some(k) = self.kernel.take() {
            if k.state() == HandleState::Running {
                self.kernel = Some(k);
                return Err(ReentrantError::new_err("Cannot close while Running"));
            }
            let _ = k.destroy_multi_with_trace();
        }
        Ok(())
    }

    fn __enter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    fn __exit__(
        &mut self,
        _exc_type: Option<Py<PyAny>>,
        _exc_val: Option<Py<PyAny>>,
        _exc_tb: Option<Py<PyAny>>,
    ) -> PyResult<bool> {
        self.close()?;
        Ok(false)
    }

    /// V-04 / V-06 テスト用: 境界内部のパニック捕捉および Poisoned 遷移を強制発火させる。
    fn _debug_trigger_internal_panic(&self, py: Python<'_>) -> PyResult<()> {
        let k = self.kernel_ref()?;
        let outcome = catch_unwind(AssertUnwindSafe(|| -> i32 {
            let _ = ACTION_PAYLOAD_OFFSET;
            panic!("simulated internal Rust panic inside Python boundary");
        }));
        match outcome {
            Ok(_) => Ok(()),
            Err(_) => {
                k.poison();
                Err(map_error_code(py, ERR_PANIC, 0, None))
            }
        }
    }
}

#[pymodule]
fn _core(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("OK", OK)?;
    m.add("STEP_FINISHED", STEP_FINISHED)?;
    m.add("RUN_LIMIT_REACHED", RUN_LIMIT_REACHED)?;
    m.add("RUN_SNAPSHOT_REQUESTED", RUN_SNAPSHOT_REQUESTED)?;

    m.add_class::<PyKernelConfig>()?;
    m.add_class::<PyStepResult>()?;
    m.add_class::<PyEventRecordView>()?;
    m.add_class::<PyEventBatchView>()?;
    m.add_class::<PyHistoryView>()?;
    m.add_class::<PyStrategyArenaView>()?;
    m.add_class::<PyStrategyContext>()?;
    m.add_class::<PyInMemoryMarketFeed>()?;
    m.add_class::<PySimExecution>()?;
    m.add_class::<PyKernel>()?;
    m.add_class::<PyArrowResultStream>()?;
    m.add_class::<PyDiskSnapshotPort>()?;
    m.add_class::<PyDiskTraceSink>()?;

    m.add("KernelError", py.get_type::<KernelError>())?;
    m.add("KernelFatalError", py.get_type::<KernelFatalError>())?;
    m.add("KernelCapacityError", py.get_type::<KernelCapacityError>())?;
    m.add(
        "ActionQueueFullError",
        py.get_type::<ActionQueueFullError>(),
    )?;
    m.add("KernelPoisonedError", py.get_type::<KernelPoisonedError>())?;
    m.add("ReentrantError", py.get_type::<ReentrantError>())?;
    m.add(
        "SnapshotUnsupportedError",
        py.get_type::<SnapshotUnsupportedError>(),
    )?;
    m.add("SnapshotIoError", py.get_type::<SnapshotIoError>())?;
    m.add(
        "SnapshotRestoreError",
        py.get_type::<SnapshotRestoreError>(),
    )?;
    m.add(
        "CausalityViolationError",
        py.get_type::<CausalityViolationError>(),
    )?;
    m.add(
        "SequenceViolationError",
        py.get_type::<SequenceViolationError>(),
    )?;
    m.add("PortFailureError", py.get_type::<PortFailureError>())?;
    m.add("ResultOverflowError", py.get_type::<ResultOverflowError>())?;
    m.add(
        "StrategyFailureError",
        py.get_type::<StrategyFailureError>(),
    )?;
    m.add("KernelPanicError", py.get_type::<KernelPanicError>())?;
    Ok(())
}
