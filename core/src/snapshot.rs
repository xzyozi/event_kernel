//! スナップショットのヘッダ・形式検査・セクション解析（EK-DS-001 2.2, 2.3, 4.1, 4.3節）。

use crate::config::KernelConfig;
use crate::event::{EventHeader, event_type, flags};
use crate::hash::sha256;
use crate::status::{ERR_SNAPSHOT_INVALID, ERR_SNAPSHOT_MISMATCH};

/// スナップショットの先頭マジックバイト（`EKSN`）。
pub const SNAPSHOT_MAGIC: [u8; 4] = *b"EKSN";
/// スナップショットヘッダのバイト数。
pub const SNAPSHOT_HEADER_SIZE: usize = 64;
/// スナップショット形式の主版数。
pub const SNAPSHOT_FORMAT_MAJOR: u16 = 1;
/// スナップショット形式の副版数。
pub const SNAPSHOT_FORMAT_MINOR: u16 = 0;
/// 初版スナップショットに含まれる必須セクション数。
pub const SNAPSHOT_REQUIRED_SECTION_COUNT: u32 = 9;

const SECTION_HEADER_SIZE: usize = 16;
const SECTION_FLAG_REQUIRED: u16 = 0x0001;

const PRNG_ALGORITHM_XOSHIRO256: u16 = 1;
const PRNG_STATE_LEN: u16 = 32;

/// 64バイトの `SNAPSHOT_HEADER` を書き込む。
pub(crate) fn write_snapshot_header(
    out: &mut [u8],
    body_len: usize,
    body_sha256: &[u8; 32],
) -> bool {
    let Some(header) = out.get_mut(..SNAPSHOT_HEADER_SIZE) else {
        return false;
    };
    header.fill(0);
    if let Some(slot) = header.get_mut(0..4) {
        slot.copy_from_slice(&SNAPSHOT_MAGIC);
    }
    if let Some(slot) = header.get_mut(4..6) {
        slot.copy_from_slice(&SNAPSHOT_FORMAT_MAJOR.to_le_bytes());
    }
    if let Some(slot) = header.get_mut(6..8) {
        slot.copy_from_slice(&SNAPSHOT_FORMAT_MINOR.to_le_bytes());
    }
    // flags = 0 (offset 8..12)
    if let Some(slot) = header.get_mut(12..16) {
        slot.copy_from_slice(&SNAPSHOT_REQUIRED_SECTION_COUNT.to_le_bytes());
    }
    if let Some(slot) = header.get_mut(16..24) {
        slot.copy_from_slice(&(body_len as u64).to_le_bytes());
    }
    if let Some(slot) = header.get_mut(24..56) {
        slot.copy_from_slice(body_sha256);
    }
    true
}

pub(crate) struct ParsedSnapshot<'a> {
    pub(crate) now_ns: i64,
    pub(crate) processed_event_count: u64,
    pub(crate) next_action_sequence_id: u64,
    pub(crate) prng_state: [u64; 4],
    pub(crate) state_arena: &'a [u8],
    pub(crate) merge_heads: &'a [u8],
    pub(crate) action_queue: &'a [u8],
    pub(crate) history_ring: &'a [u8],
    pub(crate) feed_states: &'a [u8],
    pub(crate) exec_state: &'a [u8],
}

/// EK-DS-001 4.3節の形式検査と、EK-DD-001 3.3節の設定・PRNG 一致検査を行う（状態を変更しない）。
pub(crate) fn parse_and_validate<'a>(
    blob: &'a [u8],
    expected_config: &KernelConfig,
    expected_stream_count: u32,
) -> Result<ParsedSnapshot<'a>, i32> {
    if blob.len() < SNAPSHOT_HEADER_SIZE {
        return Err(ERR_SNAPSHOT_INVALID);
    }
    let magic = blob.get(0..4).ok_or(ERR_SNAPSHOT_INVALID)?;
    if magic != SNAPSHOT_MAGIC {
        return Err(ERR_SNAPSHOT_INVALID);
    }
    let format_major = read_u16(blob, 4)?;
    if format_major != SNAPSHOT_FORMAT_MAJOR {
        return Err(ERR_SNAPSHOT_INVALID);
    }
    let _format_minor = read_u16(blob, 6)?;
    let header_flags = read_u32(blob, 8)?;
    if header_flags != 0 {
        return Err(ERR_SNAPSHOT_INVALID);
    }
    let section_count = read_u32(blob, 12)?;
    let body_length = read_u64(blob, 16)?;
    let expected_sha = blob.get(24..56).ok_or(ERR_SNAPSHOT_INVALID)?;
    let reserved = blob.get(56..64).ok_or(ERR_SNAPSHOT_INVALID)?;
    if reserved != [0u8; 8] {
        return Err(ERR_SNAPSHOT_INVALID);
    }

    let actual_body_len = blob.len() - SNAPSHOT_HEADER_SIZE;
    if body_length != actual_body_len as u64 {
        return Err(ERR_SNAPSHOT_INVALID);
    }
    let body = blob
        .get(SNAPSHOT_HEADER_SIZE..)
        .ok_or(ERR_SNAPSHOT_INVALID)?;
    if sha256(body) != expected_sha {
        return Err(ERR_SNAPSHOT_INVALID);
    }

    // セクションの走査: `section_id` 昇順、重複なし、長さ合計一致。
    let mut sections: [Option<&'a [u8]>; 9] = [None; 9];
    let mut pos = 0usize;
    let mut parsed_count = 0u32;
    let mut prev_id: Option<u16> = None;

    while pos < body.len() {
        if pos + SECTION_HEADER_SIZE > body.len() {
            return Err(ERR_SNAPSHOT_INVALID);
        }
        let section_id = read_u16(body, pos)?;
        let section_flags = read_u16(body, pos + 2)?;
        let section_reserved = read_u32(body, pos + 4)?;
        let length = read_u64(body, pos + 8)?;

        if section_reserved != 0 || (section_flags & !SECTION_FLAG_REQUIRED) != 0 {
            return Err(ERR_SNAPSHOT_INVALID);
        }
        if !length.is_multiple_of(8) {
            return Err(ERR_SNAPSHOT_INVALID);
        }
        if let Some(prev) = prev_id
            && section_id <= prev
        {
            return Err(ERR_SNAPSHOT_INVALID);
        }
        prev_id = Some(section_id);

        let length_usize = usize::try_from(length).map_err(|_| ERR_SNAPSHOT_INVALID)?;
        let body_start = pos + SECTION_HEADER_SIZE;
        let body_end = body_start
            .checked_add(length_usize)
            .ok_or(ERR_SNAPSHOT_INVALID)?;
        let section_body = body.get(body_start..body_end).ok_or(ERR_SNAPSHOT_INVALID)?;

        if (1..=9).contains(&section_id) {
            if let Some(slot) = sections.get_mut(usize::from(section_id - 1)) {
                *slot = Some(section_body);
            }
        } else if (section_flags & SECTION_FLAG_REQUIRED) != 0 {
            // 未知の必須セクションは拒否、非必須は `length` で読み飛ばす（DS 4.1節）。
            return Err(ERR_SNAPSHOT_INVALID);
        }

        pos = body_end;
        parsed_count = parsed_count.checked_add(1).ok_or(ERR_SNAPSHOT_INVALID)?;
    }

    if pos != body.len() || parsed_count != section_count {
        return Err(ERR_SNAPSHOT_INVALID);
    }

    let config_sec = sections[0].ok_or(ERR_SNAPSHOT_INVALID)?;
    let clock_sec = sections[1].ok_or(ERR_SNAPSHOT_INVALID)?;
    let prng_sec = sections[2].ok_or(ERR_SNAPSHOT_INVALID)?;
    let arena_sec = sections[3].ok_or(ERR_SNAPSHOT_INVALID)?;
    let merge_sec = sections[4].ok_or(ERR_SNAPSHOT_INVALID)?;
    let action_sec = sections[5].ok_or(ERR_SNAPSHOT_INVALID)?;
    let history_sec = sections[6].ok_or(ERR_SNAPSHOT_INVALID)?;
    let feed_states_sec = sections[7].ok_or(ERR_SNAPSHOT_INVALID)?;
    let exec_state_sec = sections[8].ok_or(ERR_SNAPSHOT_INVALID)?;

    // 1. 形式の内部構造と予約領域の検査（ERR_SNAPSHOT_INVALID）。
    let snap_cfg = parse_config_section(config_sec)?;
    let (now_ns, processed_event_count, next_action_sequence_id) = parse_clock_section(clock_sec)?;
    let (prng_alg, prng_state) = parse_prng_section(prng_sec)?;

    let record_size = 32usize
        .checked_add(
            usize::try_from(snap_cfg.payload_record_size).map_err(|_| ERR_SNAPSHOT_INVALID)?,
        )
        .ok_or(ERR_SNAPSHOT_INVALID)?;

    validate_merge_heads(merge_sec, snap_cfg.stream_count, record_size)?;
    validate_event_list(
        action_sec,
        record_size,
        snap_cfg.action_queue_capacity,
        false,
    )?;
    validate_event_list(history_sec, record_size, snap_cfg.history_ring_len, true)?;
    validate_feed_states(
        feed_states_sec,
        snap_cfg.stream_count,
        snap_cfg.feed_token_max_bytes,
    )?;
    let exec_state = parse_exec_state(exec_state_sec, snap_cfg.exec_state_max_bytes)?;

    // 2. 現在の設定・PRNG との一致検査（ERR_SNAPSHOT_MISMATCH、DD 3.3節）。
    if !snap_cfg.matches(expected_config, expected_stream_count)
        || snap_cfg.prng_algorithm != PRNG_ALGORITHM_XOSHIRO256
        || prng_alg != PRNG_ALGORITHM_XOSHIRO256
        || arena_sec.len() as u64 != expected_config.arena_size
    {
        return Err(ERR_SNAPSHOT_MISMATCH);
    }

    Ok(ParsedSnapshot {
        now_ns,
        processed_event_count,
        next_action_sequence_id,
        prng_state,
        state_arena: arena_sec,
        merge_heads: merge_sec,
        action_queue: action_sec,
        history_ring: history_sec,
        feed_states: feed_states_sec,
        exec_state,
    })
}

struct SnapshotConfig {
    history_ring_len: u32,
    stream_count: u32,
    arena_size: u64,
    strategy_arena_offset: u64,
    strategy_arena_size: u64,
    action_queue_capacity: u32,
    payload_record_size: u32,
    price_scale_exp: i32,
    qty_scale_exp: i32,
    prng_algorithm: u16,
    exec_state_max_bytes: u32,
    feed_token_max_bytes: u32,
}

impl SnapshotConfig {
    fn matches(&self, cfg: &KernelConfig, stream_count: u32) -> bool {
        self.history_ring_len == cfg.history_ring_len
            && self.stream_count == stream_count
            && self.arena_size == cfg.arena_size
            && self.strategy_arena_offset == cfg.strategy_arena_offset
            && self.strategy_arena_size == cfg.strategy_arena_size
            && self.action_queue_capacity == cfg.action_queue_capacity
            && self.payload_record_size == cfg.payload_record_size
            && self.price_scale_exp == cfg.price_scale_exp
            && self.qty_scale_exp == cfg.qty_scale_exp
            && self.exec_state_max_bytes == cfg.exec_state_max_bytes
            && self.feed_token_max_bytes == cfg.feed_token_max_bytes
    }
}

fn parse_config_section(sec: &[u8]) -> Result<SnapshotConfig, i32> {
    if sec.len() != 64 {
        return Err(ERR_SNAPSHOT_INVALID);
    }
    let reserved0 = read_u16(sec, 50)?;
    let reserved1 = read_u32(sec, 60)?;
    if reserved0 != 0 || reserved1 != 0 {
        return Err(ERR_SNAPSHOT_INVALID);
    }
    Ok(SnapshotConfig {
        history_ring_len: read_u32(sec, 0)?,
        stream_count: read_u32(sec, 4)?,
        arena_size: read_u64(sec, 8)?,
        strategy_arena_offset: read_u64(sec, 16)?,
        strategy_arena_size: read_u64(sec, 24)?,
        action_queue_capacity: read_u32(sec, 32)?,
        payload_record_size: read_u32(sec, 36)?,
        price_scale_exp: read_i32(sec, 40)?,
        qty_scale_exp: read_i32(sec, 44)?,
        prng_algorithm: read_u16(sec, 48)?,
        exec_state_max_bytes: read_u32(sec, 52)?,
        feed_token_max_bytes: read_u32(sec, 56)?,
    })
}

fn parse_clock_section(sec: &[u8]) -> Result<(i64, u64, u64), i32> {
    if sec.len() != 24 {
        return Err(ERR_SNAPSHOT_INVALID);
    }
    Ok((read_i64(sec, 0)?, read_u64(sec, 8)?, read_u64(sec, 16)?))
}

fn parse_prng_section(sec: &[u8]) -> Result<(u16, [u64; 4]), i32> {
    if sec.len() < 8 {
        return Err(ERR_SNAPSHOT_INVALID);
    }
    let algorithm = read_u16(sec, 0)?;
    let state_len = read_u16(sec, 2)?;
    let reserved = read_u32(sec, 4)?;
    if reserved != 0 || state_len != PRNG_STATE_LEN {
        return Err(ERR_SNAPSHOT_INVALID);
    }
    if sec.len() != 8 + usize::from(PRNG_STATE_LEN) {
        return Err(ERR_SNAPSHOT_INVALID);
    }
    Ok((
        algorithm,
        [
            read_u64(sec, 8)?,
            read_u64(sec, 16)?,
            read_u64(sec, 24)?,
            read_u64(sec, 32)?,
        ],
    ))
}

fn validate_merge_heads(sec: &[u8], stream_count: u32, record_size: usize) -> Result<(), i32> {
    let mut pos = 0usize;
    for expected_stream in 0..stream_count {
        let present = *sec.get(pos).ok_or(ERR_SNAPSHOT_INVALID)?;
        let reserved = sec.get(pos + 1..pos + 8).ok_or(ERR_SNAPSHOT_INVALID)?;
        if reserved != [0u8; 7] {
            return Err(ERR_SNAPSHOT_INVALID);
        }
        pos += 8;
        match present {
            0 => {}
            1 => {
                let rec = sec
                    .get(pos..pos + record_size)
                    .ok_or(ERR_SNAPSHOT_INVALID)?;
                let h = validate_event_record(rec)?;
                if h.stream_id != expected_stream || !event_type::is_market(h.event_type) {
                    return Err(ERR_SNAPSHOT_INVALID);
                }
                pos += record_size;
            }
            _ => return Err(ERR_SNAPSHOT_INVALID),
        }
    }
    if pos != sec.len() {
        return Err(ERR_SNAPSHOT_INVALID);
    }
    Ok(())
}

fn validate_event_list(
    sec: &[u8],
    record_size: usize,
    max_count: u32,
    market_only: bool,
) -> Result<(), i32> {
    let count = read_u64(sec, 0)?;
    if count > u64::from(max_count) {
        return Err(ERR_SNAPSHOT_INVALID);
    }
    let count_usize = usize::try_from(count).map_err(|_| ERR_SNAPSHOT_INVALID)?;
    let expected = 8usize
        .checked_add(
            count_usize
                .checked_mul(record_size)
                .ok_or(ERR_SNAPSHOT_INVALID)?,
        )
        .ok_or(ERR_SNAPSHOT_INVALID)?;
    if sec.len() != expected {
        return Err(ERR_SNAPSHOT_INVALID);
    }
    for i in 0..count_usize {
        let off = 8 + i * record_size;
        let rec = sec
            .get(off..off + record_size)
            .ok_or(ERR_SNAPSHOT_INVALID)?;
        let h = validate_event_record(rec)?;
        if market_only && !event_type::is_market(h.event_type) {
            return Err(ERR_SNAPSHOT_INVALID);
        }
    }
    Ok(())
}

fn validate_event_record(rec: &[u8]) -> Result<EventHeader, i32> {
    let header = read_event_header(rec)?;
    if !is_known_event_type(header.event_type) {
        return Err(ERR_SNAPSHOT_INVALID);
    }
    let known_flags = flags::SNAPSHOT_TRIGGER | flags::END_OF_STREAM;
    if (header.flags & !known_flags) != 0 {
        return Err(ERR_SNAPSHOT_INVALID);
    }
    Ok(header)
}

pub(crate) fn read_event_header(rec: &[u8]) -> Result<EventHeader, i32> {
    if rec.len() < EventHeader::SIZE {
        return Err(ERR_SNAPSHOT_INVALID);
    }
    Ok(EventHeader {
        timestamp_ns: read_i64(rec, 0)?,
        sequence_id: read_u64(rec, 8)?,
        stream_id: read_u32(rec, 16)?,
        event_type: read_u16(rec, 20)?,
        flags: read_u16(rec, 22)?,
        payload_idx: read_u64(rec, 24)?,
    })
}

fn validate_feed_states(
    sec: &[u8],
    stream_count: u32,
    feed_token_max_bytes: u32,
) -> Result<(), i32> {
    let count = read_u64(sec, 0)?;
    if count != u64::from(stream_count) {
        return Err(ERR_SNAPSHOT_INVALID);
    }
    let mut pos = 8usize;
    for expected_stream_id in 0..stream_count {
        if pos + 16 > sec.len() {
            return Err(ERR_SNAPSHOT_INVALID);
        }
        let stream_id = read_u32(sec, pos)?;
        let token_len = read_u32(sec, pos + 4)?;
        let entry_flags = read_u16(sec, pos + 8)?;
        let reserved = sec.get(pos + 10..pos + 16).ok_or(ERR_SNAPSHOT_INVALID)?;
        if reserved != [0u8; 6] || (entry_flags & !1) != 0 || stream_id != expected_stream_id {
            return Err(ERR_SNAPSHOT_INVALID);
        }
        if (entry_flags == 0 && token_len != 0) || token_len > feed_token_max_bytes {
            return Err(ERR_SNAPSHOT_INVALID);
        }
        let token_usize = usize::try_from(token_len).map_err(|_| ERR_SNAPSHOT_INVALID)?;
        let padded = round_up_8(token_usize).ok_or(ERR_SNAPSHOT_INVALID)?;
        let entry_end = pos + 16 + padded;
        if entry_end > sec.len() {
            return Err(ERR_SNAPSHOT_INVALID);
        }
        if entry_flags == 0 {
            return Err(ERR_SNAPSHOT_MISMATCH);
        }
        pos = entry_end;
    }
    if pos != sec.len() {
        return Err(ERR_SNAPSHOT_INVALID);
    }
    Ok(())
}

fn parse_exec_state(sec: &[u8], exec_state_max_bytes: u32) -> Result<&[u8], i32> {
    let len = read_u64(sec, 0)?;
    if len > u64::from(exec_state_max_bytes) {
        return Err(ERR_SNAPSHOT_INVALID);
    }
    let len_usize = usize::try_from(len).map_err(|_| ERR_SNAPSHOT_INVALID)?;
    let padded = round_up_8(len_usize).ok_or(ERR_SNAPSHOT_INVALID)?;
    if sec.len() != 8 + padded {
        return Err(ERR_SNAPSHOT_INVALID);
    }
    sec.get(8..8 + len_usize).ok_or(ERR_SNAPSHOT_INVALID)
}

/// `FEED_STATES` セクションから指定位置のトークンを順に取り出すイテレータ補助。
pub(crate) fn feed_token_at(sec: &[u8], target_idx: usize) -> Result<&[u8], i32> {
    let mut pos = 8usize;
    for idx in 0..=target_idx {
        let token_len =
            usize::try_from(read_u32(sec, pos + 4)?).map_err(|_| ERR_SNAPSHOT_INVALID)?;
        let padded = round_up_8(token_len).ok_or(ERR_SNAPSHOT_INVALID)?;
        let token_start = pos + 16;
        if idx == target_idx {
            return sec
                .get(token_start..token_start + token_len)
                .ok_or(ERR_SNAPSHOT_INVALID);
        }
        pos = token_start + padded;
    }
    Err(ERR_SNAPSHOT_INVALID)
}

const fn is_known_event_type(kind: u16) -> bool {
    matches!(
        kind,
        event_type::TICK
            | event_type::BAR
            | event_type::BOOK_UPDATE
            | event_type::INDICATOR
            | event_type::TIMER
            | event_type::ACTION_ARRIVAL
            | event_type::ORDER_ACK
            | event_type::FILL
    )
}

const fn round_up_8(v: usize) -> Option<usize> {
    let Some(added) = v.checked_add(7) else {
        return None;
    };
    Some(added & !7)
}

fn read_u16(bytes: &[u8], off: usize) -> Result<u16, i32> {
    let slice = bytes.get(off..off + 2).ok_or(ERR_SNAPSHOT_INVALID)?;
    let arr: [u8; 2] = slice.try_into().map_err(|_| ERR_SNAPSHOT_INVALID)?;
    Ok(u16::from_le_bytes(arr))
}

fn read_u32(bytes: &[u8], off: usize) -> Result<u32, i32> {
    let slice = bytes.get(off..off + 4).ok_or(ERR_SNAPSHOT_INVALID)?;
    let arr: [u8; 4] = slice.try_into().map_err(|_| ERR_SNAPSHOT_INVALID)?;
    Ok(u32::from_le_bytes(arr))
}

fn read_u64(bytes: &[u8], off: usize) -> Result<u64, i32> {
    let slice = bytes.get(off..off + 8).ok_or(ERR_SNAPSHOT_INVALID)?;
    let arr: [u8; 8] = slice.try_into().map_err(|_| ERR_SNAPSHOT_INVALID)?;
    Ok(u64::from_le_bytes(arr))
}

fn read_i32(bytes: &[u8], off: usize) -> Result<i32, i32> {
    let slice = bytes.get(off..off + 4).ok_or(ERR_SNAPSHOT_INVALID)?;
    let arr: [u8; 4] = slice.try_into().map_err(|_| ERR_SNAPSHOT_INVALID)?;
    Ok(i32::from_le_bytes(arr))
}

fn read_i64(bytes: &[u8], off: usize) -> Result<i64, i32> {
    let slice = bytes.get(off..off + 8).ok_or(ERR_SNAPSHOT_INVALID)?;
    let arr: [u8; 8] = slice.try_into().map_err(|_| ERR_SNAPSHOT_INVALID)?;
    Ok(i64::from_le_bytes(arr))
}
