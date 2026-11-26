//! Deterministic Event Kernel のカーネルコア（縦切り）。
//!
//! 仕様の正本は EK-DD-001（制御）と EK-DS-001（データの配置）である。
//! 実装の範囲は `docs/analysis/コア縦切り実装計画.md` の2章とする。

#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::disallowed_types
)]

extern crate alloc;

mod config;
pub mod event;
mod history;
mod kernel;
pub mod ports;
mod queue;
pub mod status;
mod util;

pub use config::{KernelConfig, MAX_STREAMS};
pub use event::{
    ACTION_PAYLOAD_OFFSET, ACTION_STREAM_ID, EventHeader, EventRecord, event_type, flags,
};
pub use history::HistoryView;
pub use kernel::{Action, CLOCK_UNSET_NS, Context, HandleState, Kernel, RunLimit, StepResult};
pub use ports::{EventFeedPort, ExecutionPort, PortError, Strategy, StrategyStatus};
pub use status::*;