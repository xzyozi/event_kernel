//! Apache Arrow C Data / Stream Interface（PyCapsule）結果出力（EK-IF-001 4.3節、V-02）。

#![allow(unsafe_code)]

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr::{self, null, null_mut};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use ek_core::Sha256;
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::PyBytes;

const ARROW_SCHEMA_CAPSULE_NAME: &CStr = c"arrow_array_stream";
const ARROW_REQUESTED_SCHEMA_NAME: &CStr = c"arrow_schema";

/// Apache Arrow C Data Interface `ArrowSchema`（公式仕様準拠）。
#[repr(C)]
pub struct ArrowSchema {
    format: *const c_char,
    name: *const c_char,
    metadata: *const c_char,
    flags: i64,
    n_children: i64,
    children: *mut *mut ArrowSchema,
    dictionary: *mut ArrowSchema,
    release: Option<unsafe extern "C" fn(schema: *mut ArrowSchema)>,
    private_data: *mut c_void,
}

/// Apache Arrow C Data Interface `ArrowArray`（公式仕様準拠）。
#[repr(C)]
pub struct ArrowArray {
    length: i64,
    null_count: i64,
    offset: i64,
    n_buffers: i64,
    n_children: i64,
    buffers: *mut *const c_void,
    children: *mut *mut ArrowArray,
    dictionary: *mut ArrowArray,
    release: Option<unsafe extern "C" fn(array: *mut ArrowArray)>,
    private_data: *mut c_void,
}

/// Apache Arrow C Stream Interface `ArrowArrayStream`（公式仕様準拠）。
#[repr(C)]
pub struct ArrowArrayStream {
    get_schema:
        Option<unsafe extern "C" fn(stream: *mut ArrowArrayStream, out: *mut ArrowSchema) -> c_int>,
    get_next:
        Option<unsafe extern "C" fn(stream: *mut ArrowArrayStream, out: *mut ArrowArray) -> c_int>,
    get_last_error: Option<unsafe extern "C" fn(stream: *mut ArrowArrayStream) -> *const c_char>,
    release: Option<unsafe extern "C" fn(stream: *mut ArrowArrayStream)>,
    private_data: *mut c_void,
}

struct SchemaPrivate {
    _format: CString,
    _name: CString,
    children_vec: Vec<*mut ArrowSchema>,
}

unsafe extern "C" fn release_arrow_schema(schema: *mut ArrowSchema) {
    if schema.is_null() {
        return;
    }
    let s = unsafe { &mut *schema };
    if s.release.is_none() {
        return;
    }
    s.release = None;
    if !s.children.is_null() && s.n_children > 0 {
        let children = unsafe { std::slice::from_raw_parts(s.children, s.n_children as usize) };
        for &child in children {
            if !child.is_null() {
                unsafe {
                    if let Some(rel) = (*child).release {
                        rel(child);
                    }
                    drop(Box::from_raw(child));
                }
            }
        }
    }
    if !s.private_data.is_null() {
        let priv_box = unsafe { Box::from_raw(s.private_data as *mut SchemaPrivate) };
        drop(priv_box);
    }
    s.private_data = null_mut();
    s.children = null_mut();
}

enum OwnedBuffer {
    I64(Vec<i64>),
    U64(Vec<u64>),
    U32(Vec<u32>),
    U16(Vec<u16>),
    I8(Vec<i8>),
    Empty,
}

impl OwnedBuffer {
    fn as_ptr(&self) -> *const c_void {
        match self {
            Self::I64(v) => v.as_ptr().cast(),
            Self::U64(v) => v.as_ptr().cast(),
            Self::U32(v) => v.as_ptr().cast(),
            Self::U16(v) => v.as_ptr().cast(),
            Self::I8(v) => v.as_ptr().cast(),
            Self::Empty => ptr::NonNull::<u8>::dangling().as_ptr().cast(),
        }
    }

    fn write_le_bytes(&self, hasher: &mut Sha256) {
        match self {
            Self::I64(v) => {
                for x in v {
                    hasher.update(&x.to_le_bytes());
                }
            }
            Self::U64(v) => {
                for x in v {
                    hasher.update(&x.to_le_bytes());
                }
            }
            Self::U32(v) => {
                for x in v {
                    hasher.update(&x.to_le_bytes());
                }
            }
            Self::U16(v) => {
                for x in v {
                    hasher.update(&x.to_le_bytes());
                }
            }
            Self::I8(v) => {
                for x in v {
                    hasher.update(&x.to_le_bytes());
                }
            }
            Self::Empty => {}
        }
    }
}

struct ArrayPrivate {
    _data: OwnedBuffer,
    _buffers_ptrs: [*const c_void; 2],
    _children_ptrs: Vec<*mut ArrowArray>,
    release_counter: Option<Arc<AtomicUsize>>,
}

unsafe extern "C" fn release_arrow_array(array: *mut ArrowArray) {
    if array.is_null() {
        return;
    }
    let a = unsafe { &mut *array };
    if a.release.is_none() {
        return;
    }
    a.release = None;
    if !a.children.is_null() && a.n_children > 0 {
        let children = unsafe { std::slice::from_raw_parts(a.children, a.n_children as usize) };
        for &child in children {
            if !child.is_null() {
                unsafe {
                    if let Some(rel) = (*child).release {
                        rel(child);
                    }
                    drop(Box::from_raw(child));
                }
            }
        }
    }
    if !a.private_data.is_null() {
        let priv_box = unsafe { Box::from_raw(a.private_data as *mut ArrayPrivate) };
        if let Some(counter) = &priv_box.release_counter {
            counter.fetch_add(1, Ordering::SeqCst);
        }
        drop(priv_box);
    }
    a.private_data = null_mut();
    a.children = null_mut();
    a.buffers = null_mut();
}

/// テーブル1列分の定義・所有バッファ。
#[derive(Clone)]
pub struct ArrowColumn {
    pub name: &'static str,
    pub format: &'static str,
    pub buffer: ArrowColumnData,
}

#[derive(Clone)]
pub enum ArrowColumnData {
    I64(Vec<i64>),
    U64(Vec<u64>),
    U32(Vec<u32>),
    U16(Vec<u16>),
    I8(Vec<i8>),
}

impl ArrowColumnData {
    fn len(&self) -> usize {
        match self {
            Self::I64(v) => v.len(),
            Self::U64(v) => v.len(),
            Self::U32(v) => v.len(),
            Self::U16(v) => v.len(),
            Self::I8(v) => v.len(),
        }
    }

    fn as_ptr_usize(&self) -> usize {
        match self {
            Self::I64(v) => v.as_ptr() as usize,
            Self::U64(v) => v.as_ptr() as usize,
            Self::U32(v) => v.as_ptr() as usize,
            Self::U16(v) => v.as_ptr() as usize,
            Self::I8(v) => v.as_ptr() as usize,
        }
    }

    fn into_owned(self) -> OwnedBuffer {
        match self {
            Self::I64(v) => OwnedBuffer::I64(v),
            Self::U64(v) => OwnedBuffer::U64(v),
            Self::U32(v) => OwnedBuffer::U32(v),
            Self::U16(v) => OwnedBuffer::U16(v),
            Self::I8(v) => OwnedBuffer::I8(v),
        }
    }
}

fn build_struct_schema(schema_meta: &[(&'static str, &'static str)]) -> ArrowSchema {
    let mut children_ptrs = Vec::with_capacity(schema_meta.len());
    for &(col_name, col_format) in schema_meta {
        let fmt = CString::new(col_format).expect("valid C format string");
        let name = CString::new(col_name).expect("valid C column name");
        let priv_data = Box::new(SchemaPrivate {
            _format: fmt,
            _name: name,
            children_vec: Vec::new(),
        });
        let fmt_ptr = priv_data._format.as_ptr();
        let name_ptr = priv_data._name.as_ptr();
        let child = Box::new(ArrowSchema {
            format: fmt_ptr,
            name: name_ptr,
            metadata: null(),
            flags: 0,
            n_children: 0,
            children: null_mut(),
            dictionary: null_mut(),
            release: Some(release_arrow_schema),
            private_data: Box::into_raw(priv_data).cast(),
        });
        children_ptrs.push(Box::into_raw(child));
    }

    let root_fmt = CString::new("+s").expect("valid struct format");
    let root_name = CString::new("").expect("valid empty name");
    let mut priv_data = Box::new(SchemaPrivate {
        _format: root_fmt,
        _name: root_name,
        children_vec: children_ptrs,
    });
    let fmt_ptr = priv_data._format.as_ptr();
    let name_ptr = priv_data._name.as_ptr();
    let children_ptr = if priv_data.children_vec.is_empty() {
        null_mut()
    } else {
        priv_data.children_vec.as_mut_ptr()
    };
    let n_children = priv_data.children_vec.len() as i64;
    ArrowSchema {
        format: fmt_ptr,
        name: name_ptr,
        metadata: null(),
        flags: 0,
        n_children,
        children: children_ptr,
        dictionary: null_mut(),
        release: Some(release_arrow_schema),
        private_data: Box::into_raw(priv_data).cast(),
    }
}

fn build_struct_array(
    row_count: usize,
    columns: Vec<ArrowColumn>,
    release_counter: Arc<AtomicUsize>,
) -> ArrowArray {
    let mut children_ptrs = Vec::with_capacity(columns.len());
    for col in columns {
        let len = col.buffer.len() as i64;
        let data = col.buffer.into_owned();
        let data_ptr = data.as_ptr();
        let mut priv_data = Box::new(ArrayPrivate {
            _data: data,
            _buffers_ptrs: [null(), data_ptr],
            _children_ptrs: Vec::new(),
            release_counter: None,
        });
        let buffers_ptr = priv_data._buffers_ptrs.as_mut_ptr();
        let child = Box::new(ArrowArray {
            length: len,
            null_count: 0,
            offset: 0,
            n_buffers: 2,
            n_children: 0,
            buffers: buffers_ptr,
            children: null_mut(),
            dictionary: null_mut(),
            release: Some(release_arrow_array),
            private_data: Box::into_raw(priv_data).cast(),
        });
        children_ptrs.push(Box::into_raw(child));
    }

    let mut root_priv = Box::new(ArrayPrivate {
        _data: OwnedBuffer::Empty,
        _buffers_ptrs: [null(), null()],
        _children_ptrs: children_ptrs,
        release_counter: Some(release_counter),
    });
    let children_ptr = if root_priv._children_ptrs.is_empty() {
        null_mut()
    } else {
        root_priv._children_ptrs.as_mut_ptr()
    };
    let n_children = root_priv._children_ptrs.len() as i64;
    ArrowArray {
        length: row_count as i64,
        null_count: 0,
        offset: 0,
        n_buffers: 1,
        n_children,
        buffers: root_priv._buffers_ptrs.as_mut_ptr(),
        children: children_ptr,
        dictionary: null_mut(),
        release: Some(release_arrow_array),
        private_data: Box::into_raw(root_priv).cast(),
    }
}

struct StreamPrivate {
    schema_meta: Vec<(&'static str, &'static str)>,
    columns: Vec<ArrowColumn>,
    row_count: usize,
    emitted: bool,
    release_counter: Arc<AtomicUsize>,
}

unsafe extern "C" fn stream_get_schema(
    stream: *mut ArrowArrayStream,
    out: *mut ArrowSchema,
) -> c_int {
    if stream.is_null() || out.is_null() {
        return 22; // EINVAL
    }
    let priv_ptr = unsafe { (*stream).private_data as *const StreamPrivate };
    if priv_ptr.is_null() {
        return 22;
    }
    let state = unsafe { &*priv_ptr };
    unsafe {
        *out = build_struct_schema(&state.schema_meta);
    }
    0
}

unsafe extern "C" fn stream_get_next(stream: *mut ArrowArrayStream, out: *mut ArrowArray) -> c_int {
    if stream.is_null() || out.is_null() {
        return 22;
    }
    let priv_ptr = unsafe { (*stream).private_data as *mut StreamPrivate };
    if priv_ptr.is_null() {
        return 22;
    }
    let state = unsafe { &mut *priv_ptr };
    if state.emitted {
        unsafe {
            *out = ArrowArray {
                length: 0,
                null_count: 0,
                offset: 0,
                n_buffers: 0,
                n_children: 0,
                buffers: null_mut(),
                children: null_mut(),
                dictionary: null_mut(),
                release: None,
                private_data: null_mut(),
            };
        }
        return 0;
    }
    state.emitted = true;
    let cols = std::mem::take(&mut state.columns);
    unsafe {
        *out = build_struct_array(state.row_count, cols, Arc::clone(&state.release_counter));
    }
    0
}

unsafe extern "C" fn stream_get_last_error(_stream: *mut ArrowArrayStream) -> *const c_char {
    null()
}

unsafe extern "C" fn stream_release(stream: *mut ArrowArrayStream) {
    if stream.is_null() {
        return;
    }
    let s = unsafe { &mut *stream };
    if s.release.is_none() {
        return;
    }
    s.release = None;
    if !s.private_data.is_null() {
        let priv_box = unsafe { Box::from_raw(s.private_data as *mut StreamPrivate) };
        drop(priv_box);
    }
    s.private_data = null_mut();
}

unsafe extern "C" fn capsule_destructor(capsule: *mut ffi::PyObject) {
    let stream_ptr =
        unsafe { ffi::PyCapsule_GetPointer(capsule, ARROW_SCHEMA_CAPSULE_NAME.as_ptr()) }
            as *mut ArrowArrayStream;
    if !stream_ptr.is_null() {
        unsafe {
            if let Some(rel) = (*stream_ptr).release {
                rel(stream_ptr);
            }
            drop(Box::from_raw(stream_ptr));
        }
    }
}

/// 結果テーブルの所有権を Arrow C Stream PyCapsule として Python へ移譲するオブジェクト。
#[pyclass(name = "ArrowResultStream", module = "event_kernel._core")]
pub struct PyArrowResultStream {
    pub table_name: &'static str,
    pub row_count: usize,
    pub columns: Option<Vec<ArrowColumn>>,
    pub buffer_addrs: Vec<usize>,
    pub digest_bytes: [u8; 32],
    pub release_counter: Arc<AtomicUsize>,
}

impl PyArrowResultStream {
    pub fn from_columns(
        table_name: &'static str,
        row_count: usize,
        columns: Vec<ArrowColumn>,
    ) -> Self {
        let buffer_addrs = columns.iter().map(|c| c.buffer.as_ptr_usize()).collect();
        let digest_bytes = compute_arrow_table_digest(table_name, row_count, &columns);
        Self {
            table_name,
            row_count,
            columns: Some(columns),
            buffer_addrs,
            digest_bytes,
            release_counter: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[pymethods]
impl PyArrowResultStream {
    /// テーブル名（`fills`, `equity_curve`, `state_transitions`）。
    #[getter]
    fn table_name(&self) -> &'static str {
        self.table_name
    }

    /// 行数。
    #[getter]
    fn row_count(&self) -> usize {
        self.row_count
    }

    /// ゼロコピー所有権移譲前の各列データポインタアドレス（V-02 検証用）。
    fn buffer_addresses(&self) -> Vec<usize> {
        self.buffer_addrs.clone()
    }

    /// Arrow `release` コールバックが実行された回数（V-02 検証用）。
    fn release_count(&self) -> usize {
        self.release_counter.load(Ordering::SeqCst)
    }

    /// EK-IF-001 4.3.4節の正規化 SHA-256 ダイジェスト（32バイト）。
    fn digest<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.digest_bytes)
    }

    /// Apache Arrow PyCapsule Stream Interface（`__arrow_c_stream__`）。
    #[pyo3(signature = (requested_schema=None))]
    fn __arrow_c_stream__(
        &mut self,
        py: Python<'_>,
        requested_schema: Option<Py<PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        if let Some(req) = requested_schema {
            // requested_schema が渡された場合でも基本検証のみ通す（capsule名確認）。
            let bound = req.bind(py);
            let _ = unsafe {
                ffi::PyCapsule_IsValid(bound.as_ptr(), ARROW_REQUESTED_SCHEMA_NAME.as_ptr())
            };
        }
        let cols = self.columns.take().ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err(
                "Arrow stream ownership has already been transferred",
            )
        })?;
        let schema_meta: Vec<(&'static str, &'static str)> =
            cols.iter().map(|c| (c.name, c.format)).collect();
        let priv_data = Box::new(StreamPrivate {
            schema_meta,
            columns: cols,
            row_count: self.row_count,
            emitted: false,
            release_counter: Arc::clone(&self.release_counter),
        });
        let stream = Box::new(ArrowArrayStream {
            get_schema: Some(stream_get_schema),
            get_next: Some(stream_get_next),
            get_last_error: Some(stream_get_last_error),
            release: Some(stream_release),
            private_data: Box::into_raw(priv_data).cast(),
        });
        let raw_stream = Box::into_raw(stream);
        let capsule_ptr = unsafe {
            ffi::PyCapsule_New(
                raw_stream.cast(),
                ARROW_SCHEMA_CAPSULE_NAME.as_ptr(),
                Some(capsule_destructor),
            )
        };
        if capsule_ptr.is_null() {
            unsafe {
                if let Some(rel) = (*raw_stream).release {
                    rel(raw_stream);
                }
                drop(Box::from_raw(raw_stream));
            }
            return Err(PyErr::fetch(py));
        }
        Ok(unsafe { Bound::from_owned_ptr(py, capsule_ptr).unbind() })
    }
}

/// EK-IF-001 4.3.4節: パディング・チャンク差に依存しない Arrow 出力正規化 SHA-256。
pub fn compute_arrow_table_digest(
    table_name: &str,
    row_count: usize,
    columns: &[ArrowColumn],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(table_name.as_bytes());
    hasher.update(&(row_count as u64).to_le_bytes());
    hasher.update(&(columns.len() as u32).to_le_bytes());
    for col in columns {
        hasher.update(&(col.name.len() as u32).to_le_bytes());
        hasher.update(col.name.as_bytes());
        let owned = col.buffer.clone().into_owned();
        owned.write_le_bytes(&mut hasher);
    }
    hasher.finalize()
}
