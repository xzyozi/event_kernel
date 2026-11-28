//! 決定論的シミュレーション約定アダプタ（`ExecutionPort` 実装）。

use std::collections::VecDeque;

use ek_core::{
    ERR_PORT_FAILURE, EventHeader, EventRecord, ExecutionPort, HistoryView, PortError, Prng,
    event_type,
};

const EXEC_STATE_MAGIC: [u8; 4] = *b"EKEX";

/// 遅延アクションを受理し、次の `poll_result` で決定論的に `Fill` を返すシミュレータ。
#[derive(Clone, Debug)]
pub struct SimExecutionAdapter {
    delay_ns: i64,
    default_price: i64,
    default_qty: i64,
    pending: VecDeque<(EventHeader, Vec<u8>)>,
}

impl Default for SimExecutionAdapter {
    fn default() -> Self {
        Self::new(1, 10_000, 1)
    }
}

impl SimExecutionAdapter {
    /// 約定遅延（ナノ秒）と既定約定価格・数量を指定して生成する。
    pub fn new(delay_ns: i64, default_price: i64, default_qty: i64) -> Self {
        Self {
            delay_ns: delay_ns.max(0),
            default_price,
            default_qty,
            pending: VecDeque::new(),
        }
    }

    /// 待機中約定結果件数を返す。
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }
}

impl ExecutionPort for SimExecutionAdapter {
    fn on_arrival(&mut self, event: EventRecord<'_>, rng: &mut Prng) -> Result<(), PortError> {
        let rnd = rng.next_u64();
        let mut payload = vec![0u8; event.payload.len()];
        if payload.len() >= 8 {
            payload[..8].copy_from_slice(&rnd.to_le_bytes());
        }
        if payload.len() >= 16 {
            payload[8..16].copy_from_slice(&event.header.sequence_id.to_le_bytes());
        }
        if payload.len() >= 24 {
            payload[16..24].copy_from_slice(&self.default_price.to_le_bytes());
        }
        if payload.len() >= 32 {
            payload[24..32].copy_from_slice(&self.default_qty.to_le_bytes());
        }
        let mut hdr = event.header;
        hdr.timestamp_ns = event.header.timestamp_ns.saturating_add(self.delay_ns);
        hdr.event_type = event_type::FILL;
        hdr.payload_idx = 0;
        self.pending.push_back((hdr, payload));
        Ok(())
    }

    fn on_market_event(
        &mut self,
        _event: EventRecord<'_>,
        _history: HistoryView<'_>,
        _rng: &mut Prng,
    ) -> Result<(), PortError> {
        Ok(())
    }

    fn poll_result(
        &mut self,
        header: &mut EventHeader,
        payload: &mut [u8],
    ) -> Result<bool, PortError> {
        let Some((hdr, pl)) = self.pending.pop_front() else {
            return Ok(false);
        };
        *header = hdr;
        payload.fill(0);
        let copy_len = payload.len().min(pl.len());
        payload[..copy_len].copy_from_slice(&pl[..copy_len]);
        Ok(true)
    }

    fn capture_state(&self, out: &mut [u8]) -> Result<usize, PortError> {
        let count = u32::try_from(self.pending.len()).map_err(|_| PortError(ERR_PORT_FAILURE))?;
        let mut needed = 32usize;
        for (_, pl) in &self.pending {
            let _ = u32::try_from(pl.len()).map_err(|_| PortError(ERR_PORT_FAILURE))?;
            needed = needed
                .checked_add(28)
                .and_then(|v| v.checked_add(pl.len()))
                .ok_or(PortError(ERR_PORT_FAILURE))?;
        }
        if needed > out.len() {
            return Err(PortError(ERR_PORT_FAILURE));
        }
        out[0..4].copy_from_slice(&EXEC_STATE_MAGIC);
        out[4..8].copy_from_slice(&count.to_le_bytes());
        out[8..16].copy_from_slice(&self.delay_ns.to_le_bytes());
        out[16..24].copy_from_slice(&self.default_price.to_le_bytes());
        out[24..32].copy_from_slice(&self.default_qty.to_le_bytes());
        let mut pos = 32usize;
        for (hdr, pl) in &self.pending {
            let pl_len = u32::try_from(pl.len()).map_err(|_| PortError(ERR_PORT_FAILURE))?;
            out[pos..pos + 8].copy_from_slice(&hdr.timestamp_ns.to_le_bytes());
            out[pos + 8..pos + 16].copy_from_slice(&hdr.sequence_id.to_le_bytes());
            out[pos + 16..pos + 20].copy_from_slice(&hdr.stream_id.to_le_bytes());
            out[pos + 20..pos + 22].copy_from_slice(&hdr.event_type.to_le_bytes());
            out[pos + 22..pos + 24].copy_from_slice(&hdr.flags.to_le_bytes());
            out[pos + 24..pos + 28].copy_from_slice(&pl_len.to_le_bytes());
            pos += 28;
            out[pos..pos + pl.len()].copy_from_slice(pl);
            pos += pl.len();
        }
        Ok(pos)
    }

    fn validate_state(&self, state: &[u8]) -> Result<(), PortError> {
        decode_exec_state(state).map(|_| ())
    }

    fn restore_state(&mut self, state: &[u8]) -> Result<(), PortError> {
        let (delay_ns, default_price, default_qty, pending) = decode_exec_state(state)?;
        self.delay_ns = delay_ns;
        self.default_price = default_price;
        self.default_qty = default_qty;
        self.pending = pending;
        Ok(())
    }
}

type DecodedExecState = (i64, i64, i64, VecDeque<(EventHeader, Vec<u8>)>);

fn decode_exec_state(state: &[u8]) -> Result<DecodedExecState, PortError> {
    if state.len() < 32 || state[0..4] != EXEC_STATE_MAGIC {
        return Err(PortError(ERR_PORT_FAILURE));
    }
    let count = u32::from_le_bytes(
        state[4..8]
            .try_into()
            .map_err(|_| PortError(ERR_PORT_FAILURE))?,
    ) as usize;
    let delay_ns = i64::from_le_bytes(
        state[8..16]
            .try_into()
            .map_err(|_| PortError(ERR_PORT_FAILURE))?,
    );
    let default_price = i64::from_le_bytes(
        state[16..24]
            .try_into()
            .map_err(|_| PortError(ERR_PORT_FAILURE))?,
    );
    let default_qty = i64::from_le_bytes(
        state[24..32]
            .try_into()
            .map_err(|_| PortError(ERR_PORT_FAILURE))?,
    );

    let min_body_bytes = count.checked_mul(28).ok_or(PortError(ERR_PORT_FAILURE))?;
    if min_body_bytes > state.len() - 32 {
        return Err(PortError(ERR_PORT_FAILURE));
    }

    let mut pos = 32usize;
    let mut pending = VecDeque::with_capacity(count);
    for _ in 0..count {
        let entry_end = pos.checked_add(28).ok_or(PortError(ERR_PORT_FAILURE))?;
        if entry_end > state.len() {
            return Err(PortError(ERR_PORT_FAILURE));
        }
        let timestamp_ns = i64::from_le_bytes(
            state[pos..pos + 8]
                .try_into()
                .map_err(|_| PortError(ERR_PORT_FAILURE))?,
        );
        let sequence_id = u64::from_le_bytes(
            state[pos + 8..pos + 16]
                .try_into()
                .map_err(|_| PortError(ERR_PORT_FAILURE))?,
        );
        let stream_id = u32::from_le_bytes(
            state[pos + 16..pos + 20]
                .try_into()
                .map_err(|_| PortError(ERR_PORT_FAILURE))?,
        );
        let ev_type = u16::from_le_bytes(
            state[pos + 20..pos + 22]
                .try_into()
                .map_err(|_| PortError(ERR_PORT_FAILURE))?,
        );
        let flags = u16::from_le_bytes(
            state[pos + 22..pos + 24]
                .try_into()
                .map_err(|_| PortError(ERR_PORT_FAILURE))?,
        );
        let pl_len = u32::from_le_bytes(
            state[pos + 24..pos + 28]
                .try_into()
                .map_err(|_| PortError(ERR_PORT_FAILURE))?,
        ) as usize;
        pos = entry_end;
        let pl_end = pos.checked_add(pl_len).ok_or(PortError(ERR_PORT_FAILURE))?;
        if pl_end > state.len() {
            return Err(PortError(ERR_PORT_FAILURE));
        }
        let payload = state[pos..pl_end].to_vec();
        pos = pl_end;
        pending.push_back((
            EventHeader {
                timestamp_ns,
                sequence_id,
                stream_id,
                event_type: ev_type,
                flags,
                payload_idx: 0,
            },
            payload,
        ));
    }
    if pos != state.len() {
        return Err(PortError(ERR_PORT_FAILURE));
    }
    Ok((delay_ns, default_price, default_qty, pending))
}
