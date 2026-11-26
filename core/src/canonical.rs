//! 正準バイト列の生成（EK-DS-001 2.3節、2.5節）。
//!
//! ステップごとのトレースハッシュ（XXH3-64、セクション 2, 3, 4, 5, 6, 8, 9）と、
//! 最終状態の SHA-256（全セクション 1..=9）で、同じセクション構成・同じバイト列を使う。

use crate::config::KernelConfig;
use crate::event::EventHeader;
use crate::history::HistoryRing;
use crate::ports::{EventFeedPort, ExecutionPort, PortError};
use crate::prng::Prng;
use crate::queue::ActionQueue;
use crate::status::{ERR_PANIC, ERR_PORT_FAILURE};

const SECTION_HEADER_SIZE: usize = 16;
const SECTION_FLAG_REQUIRED: u16 = 0x0001;

const SECTION_CONFIG: u16 = 1;
const SECTION_CLOCK: u16 = 2;
const SECTION_PRNG: u16 = 3;
const SECTION_STATE_ARENA: u16 = 4;
const SECTION_MERGE_HEADS: u16 = 5;
const SECTION_ACTION_QUEUE: u16 = 6;
const SECTION_HISTORY_RING: u16 = 7;
const SECTION_FEED_STATES: u16 = 8;
const SECTION_EXEC_STATE: u16 = 9;

/// Xoshiro256++（EK-DD-001 2.6節、EK-DS-001 2.2節 `CONFIG_SECTION`）。
const PRNG_ALGORITHM_XOSHIRO256: u16 = 1;
const PRNG_STATE_LEN: u16 = 32;

pub(crate) type Failure = (i32, i32);

fn port_failure(error: PortError) -> Failure {
    (ERR_PORT_FAILURE, error.0)
}

pub(crate) struct CanonicalInputs<'a, F, E> {
    pub(crate) config: &'a KernelConfig,
    pub(crate) now_ns: i64,
    pub(crate) processed_event_count: u64,
    pub(crate) next_action_sequence_id: u64,
    pub(crate) rng: &'a Prng,
    pub(crate) arena: &'a [u8],
    pub(crate) head_present: bool,
    pub(crate) head_header: EventHeader,
    pub(crate) head_payload: &'a [u8],
    pub(crate) feed_stream_id: u32,
    pub(crate) queue: &'a ActionQueue,
    pub(crate) history: &'a HistoryRing,
    pub(crate) feed: &'a F,
    pub(crate) exec: &'a E,
}

pub(crate) struct CanonicalScratch<'a> {
    pub(crate) buf: &'a mut [u8],
    pub(crate) action_sort: &'a mut [usize],
    pub(crate) feed_token: &'a mut [u8],
    pub(crate) exec_state: &'a mut [u8],
}

/// 正準セクション列を `scratch.buf` へ書き込み、書き込んだバイト数を返す。
pub(crate) fn write_canonical_sections<F: EventFeedPort, E: ExecutionPort>(
    inputs: &CanonicalInputs<'_, F, E>,
    include_config_and_history: bool,
    scratch: &mut CanonicalScratch<'_>,
) -> Result<usize, Failure> {
    let mut cursor = Cursor::new(scratch.buf);

    if include_config_and_history {
        cursor.write_section(SECTION_CONFIG, |c| write_config(c, inputs.config, 1))?;
    }

    cursor.write_section(SECTION_CLOCK, |c| {
        c.write_i64(inputs.now_ns)?;
        c.write_u64(inputs.processed_event_count)?;
        c.write_u64(inputs.next_action_sequence_id)
    })?;

    cursor.write_section(SECTION_PRNG, |c| {
        c.write_u16(PRNG_ALGORITHM_XOSHIRO256)?;
        c.write_u16(PRNG_STATE_LEN)?;
        c.write_u32(0)?;
        for word in inputs.rng.state() {
            c.write_u64(word)?;
        }
        Ok(())
    })?;

    cursor.write_section(SECTION_STATE_ARENA, |c| c.write_bytes(inputs.arena))?;

    cursor.write_section(SECTION_MERGE_HEADS, |c| {
        if inputs.head_present {
            c.write_u8(1)?;
            c.write_zeros(7)?;
            write_event_record(c, inputs.head_header, inputs.head_payload)
        } else {
            c.write_u8(0)?;
            c.write_zeros(7)
        }
    })?;

    cursor.write_section(SECTION_ACTION_QUEUE, |c| {
        c.write_u64(inputs.queue.len() as u64)?;
        inputs
            .queue
            .write_sorted_records(scratch.action_sort, |header, payload| {
                write_event_record(c, header, payload)
            })
    })?;

    if include_config_and_history {
        cursor.write_section(SECTION_HISTORY_RING, |c| {
            let count = inputs.history.len();
            c.write_u64(count as u64)?;
            for i in 0..count {
                let Some(record) = inputs.history.get(i) else {
                    return Err((ERR_PANIC, 0));
                };
                write_event_record(c, record.header, record.payload)?;
            }
            Ok(())
        })?;
    }

    cursor.write_section(SECTION_FEED_STATES, |c| {
        c.write_u64(1)?;
        let pos = inputs
            .feed
            .position(scratch.feed_token)
            .map_err(port_failure)?;
        let (entry_flags, token_len) = if pos.seekable {
            if pos.len > scratch.feed_token.len() {
                return Err((ERR_PORT_FAILURE, 0));
            }
            (1u16, pos.len)
        } else {
            (0u16, 0usize)
        };
        let token_u32 = u32::try_from(token_len).map_err(|_| (ERR_PORT_FAILURE, 0))?;
        c.write_u32(inputs.feed_stream_id)?;
        c.write_u32(token_u32)?;
        c.write_u16(entry_flags)?;
        c.write_zeros(6)?;
        if let Some(token) = scratch.feed_token.get(..token_len) {
            c.write_bytes(token)?;
        } else {
            return Err((ERR_PORT_FAILURE, 0));
        }
        c.write_zeros(pad_to_8(token_len))
    })?;

    cursor.write_section(SECTION_EXEC_STATE, |c| {
        let len = inputs
            .exec
            .capture_state(scratch.exec_state)
            .map_err(port_failure)?;
        if len > scratch.exec_state.len() {
            return Err((ERR_PORT_FAILURE, 0));
        }
        c.write_u64(len as u64)?;
        if let Some(bytes) = scratch.exec_state.get(..len) {
            c.write_bytes(bytes)?;
        } else {
            return Err((ERR_PORT_FAILURE, 0));
        }
        c.write_zeros(pad_to_8(len))
    })?;

    Ok(cursor.pos)
}

fn write_config(
    c: &mut Cursor<'_>,
    config: &KernelConfig,
    stream_count: u32,
) -> Result<(), Failure> {
    c.write_u32(config.history_ring_len)?;
    c.write_u32(stream_count)?;
    c.write_u64(config.arena_size)?;
    c.write_u64(config.strategy_arena_offset)?;
    c.write_u64(config.strategy_arena_size)?;
    c.write_u32(config.action_queue_capacity)?;
    c.write_u32(config.payload_record_size)?;
    c.write_i32(config.price_scale_exp)?;
    c.write_i32(config.qty_scale_exp)?;
    c.write_u16(PRNG_ALGORITHM_XOSHIRO256)?;
    c.write_u16(0)?;
    c.write_u32(config.exec_state_max_bytes)?;
    c.write_u32(config.feed_token_max_bytes)?;
    c.write_u32(0)
}

fn write_event_record(
    c: &mut Cursor<'_>,
    header: EventHeader,
    payload: &[u8],
) -> Result<(), Failure> {
    c.write_i64(header.timestamp_ns)?;
    c.write_u64(header.sequence_id)?;
    c.write_u32(header.stream_id)?;
    c.write_u16(header.event_type)?;
    c.write_u16(header.flags)?;
    // EK-DS-001 2.3節・2.5節: 正準表現・スナップショットでは `payload_idx` は 0。
    c.write_u64(0)?;
    c.write_bytes(payload)
}

#[inline]
const fn pad_to_8(len: usize) -> usize {
    (8 - (len & 7)) & 7
}

struct Cursor<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), Failure> {
        let Some(end) = self.pos.checked_add(bytes.len()) else {
            return Err((ERR_PANIC, 0));
        };
        let Some(dst) = self.buf.get_mut(self.pos..end) else {
            return Err((ERR_PANIC, 0));
        };
        dst.copy_from_slice(bytes);
        self.pos = end;
        Ok(())
    }

    fn write_zeros(&mut self, count: usize) -> Result<(), Failure> {
        if count == 0 {
            return Ok(());
        }
        let Some(end) = self.pos.checked_add(count) else {
            return Err((ERR_PANIC, 0));
        };
        let Some(dst) = self.buf.get_mut(self.pos..end) else {
            return Err((ERR_PANIC, 0));
        };
        dst.fill(0);
        self.pos = end;
        Ok(())
    }

    fn write_u8(&mut self, v: u8) -> Result<(), Failure> {
        self.write_bytes(&[v])
    }

    fn write_u16(&mut self, v: u16) -> Result<(), Failure> {
        self.write_bytes(&v.to_le_bytes())
    }

    fn write_u32(&mut self, v: u32) -> Result<(), Failure> {
        self.write_bytes(&v.to_le_bytes())
    }

    fn write_u64(&mut self, v: u64) -> Result<(), Failure> {
        self.write_bytes(&v.to_le_bytes())
    }

    fn write_i32(&mut self, v: i32) -> Result<(), Failure> {
        self.write_bytes(&v.to_le_bytes())
    }

    fn write_i64(&mut self, v: i64) -> Result<(), Failure> {
        self.write_bytes(&v.to_le_bytes())
    }

    fn write_section(
        &mut self,
        section_id: u16,
        body: impl FnOnce(&mut Cursor<'_>) -> Result<(), Failure>,
    ) -> Result<(), Failure> {
        let header_start = self.pos;
        self.write_zeros(SECTION_HEADER_SIZE)?;
        let body_start = self.pos;
        body(self)?;
        let body_len = self.pos.saturating_sub(body_start);
        if let Some(slot) = self.buf.get_mut(header_start..header_start + 2) {
            slot.copy_from_slice(&section_id.to_le_bytes());
        }
        if let Some(slot) = self.buf.get_mut(header_start + 2..header_start + 4) {
            slot.copy_from_slice(&SECTION_FLAG_REQUIRED.to_le_bytes());
        }
        if let Some(slot) = self.buf.get_mut(header_start + 8..header_start + 16) {
            slot.copy_from_slice(&(body_len as u64).to_le_bytes());
        }
        Ok(())
    }
}
