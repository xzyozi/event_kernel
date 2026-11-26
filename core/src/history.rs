//! 履歴ウィンドウ（EK-DD-001 2.5節）。市場イベントだけを入れるリングバッファ。

use alloc::vec;
use alloc::vec::Vec;

use crate::event::{EventHeader, EventRecord};
use crate::util::copy_bytes;

/// 履歴のリングバッファ。領域は `create` で確保し、以後は確保しない。
pub(crate) struct HistoryRing {
    headers: Vec<EventHeader>,
    payloads: Vec<u8>,
    record: usize,
    start: usize,
    len: usize,
}

impl HistoryRing {
    pub(crate) fn new(capacity: usize, record: usize) -> Option<Self> {
        let bytes = capacity.checked_mul(record)?;
        Some(Self {
            headers: vec![EventHeader::default(); capacity],
            payloads: vec![0; bytes],
            record,
            start: 0,
            len: 0,
        })
    }

    /// 末尾に追加する。満杯なら、最古を上書きする。
    pub(crate) fn push(&mut self, header: EventHeader, payload: &[u8]) {
        let capacity = self.headers.len();
        if capacity == 0 {
            return;
        }
        let slot = if self.len < capacity {
            let slot = wrap(self.start + self.len, capacity);
            self.len += 1;
            slot
        } else {
            let slot = self.start;
            self.start = wrap(self.start + 1, capacity);
            slot
        };
        if let Some(stored) = self.headers.get_mut(slot) {
            *stored = header;
        }
        let offset = slot * self.record;
        if let Some(stored) = self.payloads.get_mut(offset..offset + self.record) {
            copy_bytes(stored, payload);
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn clear(&mut self) {
        self.start = 0;
        self.len = 0;
    }

    pub(crate) fn get(&self, index: usize) -> Option<EventRecord<'_>> {
        if index >= self.len {
            return None;
        }
        let slot = wrap(self.start + index, self.headers.len());
        let header = *self.headers.get(slot)?;
        let offset = slot * self.record;
        let payload = self.payloads.get(offset..offset + self.record)?;
        Some(EventRecord { header, payload })
    }
}

#[inline]
fn wrap(index: usize, capacity: usize) -> usize {
    if index >= capacity {
        index - capacity
    } else {
        index
    }
}

/// 履歴ウィンドウの読み取り専用ビュー。添字0が最古。
#[derive(Clone, Copy)]
pub struct HistoryView<'a> {
    ring: &'a HistoryRing,
}

impl<'a> HistoryView<'a> {
    pub(crate) fn new(ring: &'a HistoryRing) -> Self {
        Self { ring }
    }

    /// 要素数。
    pub fn len(&self) -> usize {
        self.ring.len
    }

    /// 空かどうか。
    pub fn is_empty(&self) -> bool {
        self.ring.len == 0
    }

    /// 添字 `index`（0が最古）の要素。
    pub fn get(&self, index: usize) -> Option<EventRecord<'a>> {
        self.ring.get(index)
    }

    /// 最新の要素。
    pub fn latest(&self) -> Option<EventRecord<'a>> {
        self.ring
            .len
            .checked_sub(1)
            .and_then(|index| self.ring.get(index))
    }
}
