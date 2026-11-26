//! アクションキュー（EK-DD-001 2.5節）。`ActionArrival` と `ExecutionPort` の結果を、
//! `(timestamp_ns, stream_id, sequence_id)` の昇順で取り出す二分ヒープ。

use alloc::vec;
use alloc::vec::Vec;

use crate::event::EventHeader;
use crate::util::copy_bytes;

type Key = (i64, u32, u64);
const MAX_KEY: Key = (i64::MAX, u32::MAX, u64::MAX);

/// 固定容量のアクションキュー。領域は `create` で確保し、以後は確保しない。
pub(crate) struct ActionQueue {
    headers: Vec<EventHeader>,
    payloads: Vec<u8>,
    record: usize,
    heap: Vec<usize>,
    heap_len: usize,
    free: Vec<usize>,
    free_len: usize,
}

impl ActionQueue {
    pub(crate) fn new(capacity: usize, record: usize) -> Option<Self> {
        let bytes = capacity.checked_mul(record)?;
        let mut free = Vec::with_capacity(capacity);
        free.extend((0..capacity).rev());
        Some(Self {
            headers: vec![EventHeader::default(); capacity],
            payloads: vec![0; bytes],
            record,
            heap: vec![0; capacity],
            heap_len: 0,
            free,
            free_len: capacity,
        })
    }

    pub(crate) fn len(&self) -> usize {
        self.heap_len
    }

    pub(crate) fn clear(&mut self) {
        self.heap_len = 0;
        let cap = self.free.len();
        self.free_len = cap;
        for (i, slot) in self.free.iter_mut().enumerate() {
            *slot = cap - 1 - i;
        }
    }

    pub(crate) fn record_size(&self) -> usize {
        self.record
    }

    pub(crate) fn write_sorted_records(
        &self,
        sort_scratch: &mut [usize],
        mut write_one: impl FnMut(EventHeader, &[u8]) -> Result<(), (i32, i32)>,
    ) -> Result<(), (i32, i32)> {
        let len = self.heap_len;
        let Some(indices) = sort_scratch.get_mut(..len) else {
            return Err((crate::status::ERR_PANIC, 0));
        };
        let Some(active) = self.heap.get(..len) else {
            return Err((crate::status::ERR_PANIC, 0));
        };
        indices.copy_from_slice(active);
        indices.sort_unstable_by_key(|&slot| {
            self.headers
                .get(slot)
                .map_or(MAX_KEY, EventHeader::order_key)
        });
        for &slot in indices.iter() {
            let Some(&header) = self.headers.get(slot) else {
                return Err((crate::status::ERR_PANIC, 0));
            };
            let offset = slot * self.record;
            let Some(payload) = self.payloads.get(offset..offset + self.record) else {
                return Err((crate::status::ERR_PANIC, 0));
            };
            write_one(header, payload)?;
        }
        Ok(())
    }

    pub(crate) fn is_full(&self) -> bool {
        self.free_len == 0
    }

    pub(crate) fn peek_header(&self) -> Option<&EventHeader> {
        if self.heap_len == 0 {
            return None;
        }
        self.headers.get(*self.heap.first()?)
    }

    /// 1件追加する。`fill` がペイロードを書く。満杯なら false を返し、状態を変えない。
    pub(crate) fn push_with(&mut self, header: EventHeader, fill: impl FnOnce(&mut [u8])) -> bool {
        let Some(free_index) = self.free_len.checked_sub(1) else {
            return false;
        };
        let Some(&slot) = self.free.get(free_index) else {
            return false;
        };
        let offset = slot * self.record;
        let (Some(stored_header), Some(stored_payload), Some(position)) = (
            self.headers.get_mut(slot),
            self.payloads.get_mut(offset..offset + self.record),
            self.heap.get_mut(self.heap_len),
        ) else {
            return false;
        };
        *stored_header = header;
        fill(stored_payload);
        *position = slot;
        self.free_len = free_index;
        self.heap_len += 1;
        self.sift_up(self.heap_len - 1);
        true
    }

    /// 最小の1件を取り出して、`header` と `payload` へコピーする。空なら false を返す。
    pub(crate) fn pop_into(&mut self, header: &mut EventHeader, payload: &mut [u8]) -> bool {
        let Some(last) = self.heap_len.checked_sub(1) else {
            return false;
        };
        let Some(&slot) = self.heap.first() else {
            return false;
        };
        let Some(stored) = self.headers.get(slot) else {
            return false;
        };
        *header = *stored;
        let offset = slot * self.record;
        if let Some(stored) = self.payloads.get(offset..offset + self.record) {
            copy_bytes(payload, stored);
        }
        self.heap.swap(0, last);
        self.heap_len = last;
        self.sift_down(0);
        if let Some(free) = self.free.get_mut(self.free_len) {
            *free = slot;
        }
        self.free_len += 1;
        true
    }

    #[inline]
    fn key(&self, position: usize) -> Key {
        self.heap
            .get(position)
            .and_then(|&slot| self.headers.get(slot))
            .map_or(MAX_KEY, EventHeader::order_key)
    }

    fn sift_up(&mut self, mut position: usize) {
        while position > 0 {
            let parent = (position - 1) / 2;
            if self.key(position) < self.key(parent) {
                self.heap.swap(position, parent);
                position = parent;
            } else {
                break;
            }
        }
    }

    fn sift_down(&mut self, mut position: usize) {
        loop {
            let left = 2 * position + 1;
            if left >= self.heap_len {
                break;
            }
            let right = left + 1;
            let child = if right < self.heap_len && self.key(right) < self.key(left) {
                right
            } else {
                left
            };
            if self.key(child) < self.key(position) {
                self.heap.swap(position, child);
                position = child;
            } else {
                break;
            }
        }
    }
}
