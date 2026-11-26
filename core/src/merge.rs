//! K-way マージキュー（EK-BD-001 2.2節 C-003、EK-DD-001 2.5節）。
//!
//! 各外部フィードの先頭1イベントだけを保持し、`(timestamp_ns, stream_id, sequence_id)` の
//! 最小のイベントを二分ヒープで選ぶ。領域は `create` で確保し、以後は確保しない。

use alloc::vec;
use alloc::vec::Vec;

use crate::event::EventHeader;

type Key = (i64, u32, u64);
const MAX_KEY: Key = (i64::MAX, u32::MAX, u64::MAX);

pub(crate) struct MergeQueue {
    present: Vec<bool>,
    headers: Vec<EventHeader>,
    payloads: Vec<u8>,
    record: usize,
    heap: Vec<usize>,
    heap_len: usize,
}

impl MergeQueue {
    pub(crate) fn new(stream_count: usize, record: usize) -> Option<Self> {
        let bytes = stream_count.checked_mul(record)?;
        Some(Self {
            present: vec![false; stream_count],
            headers: vec![EventHeader::default(); stream_count],
            payloads: vec![0; bytes],
            record,
            heap: vec![0; stream_count],
            heap_len: 0,
        })
    }

    pub(crate) fn stream_count(&self) -> usize {
        self.present.len()
    }

    pub(crate) fn slot_mut(&mut self, feed_idx: usize) -> Option<(&mut EventHeader, &mut [u8])> {
        let header = self.headers.get_mut(feed_idx)?;
        let offset = feed_idx.checked_mul(self.record)?;
        let payload = self.payloads.get_mut(offset..offset + self.record)?;
        Some((header, payload))
    }

    pub(crate) fn set_initial_present(&mut self, feed_idx: usize, is_present: bool) {
        if let Some(slot) = self.present.get_mut(feed_idx) {
            *slot = is_present;
        }
        if is_present && let Some(pos) = self.heap.get_mut(self.heap_len) {
            *pos = feed_idx;
            let idx = self.heap_len;
            self.heap_len += 1;
            self.sift_up(idx);
        }
    }

    #[inline]
    pub(crate) fn peek(&self) -> Option<(usize, &EventHeader)> {
        if self.heap_len == 0 {
            return None;
        }
        let &feed_idx = self.heap.first()?;
        let header = self.headers.get(feed_idx)?;
        Some((feed_idx, header))
    }

    pub(crate) fn head_payload(&self, feed_idx: usize) -> &[u8] {
        let Some(offset) = feed_idx.checked_mul(self.record) else {
            return &[];
        };
        self.payloads
            .get(offset..offset + self.record)
            .unwrap_or(&[])
    }

    pub(crate) fn head_entry(&self, feed_idx: usize) -> Option<(bool, EventHeader, &[u8])> {
        let present = *self.present.get(feed_idx)?;
        let header = *self.headers.get(feed_idx)?;
        let offset = feed_idx.checked_mul(self.record)?;
        let payload = self.payloads.get(offset..offset + self.record)?;
        Some((present, header, payload))
    }

    pub(crate) fn update_root_after_refill(&mut self, is_present: bool) {
        let Some(&feed_idx) = self.heap.first() else {
            return;
        };
        if let Some(slot) = self.present.get_mut(feed_idx) {
            *slot = is_present;
        }
        if is_present {
            self.sift_down(0);
        } else if let Some(last) = self.heap_len.checked_sub(1) {
            self.heap.swap(0, last);
            self.heap_len = last;
            self.sift_down(0);
        }
    }

    #[inline]
    fn key(&self, position: usize) -> Key {
        self.heap
            .get(position)
            .and_then(|&feed_idx| self.headers.get(feed_idx))
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
