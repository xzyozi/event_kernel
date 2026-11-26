//! Deterministic Event Kernel のカーネルコア。
//!
//! 仕様の正本は EK-DD-001（制御）と EK-DS-001（データの配置）である。

#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::disallowed_types
)]

extern crate alloc;

mod canonical;
mod config;
pub mod event;
mod hash;
mod history;
mod kernel;
mod merge;
mod ports;
mod prng;
mod queue;
mod snapshot;
pub mod status;
mod trace;
mod util;
pub mod view;

pub use config::{KernelConfig, MAX_STREAMS};
pub use event::{
    ACTION_PAYLOAD_OFFSET, ACTION_STREAM_ID, EventHeader, EventRecord, event_type, flags,
};
pub use hash::{Sha256, sha256, xxh3_64};
pub use history::HistoryView;
pub use kernel::{Action, CLOCK_UNSET_NS, Context, HandleState, Kernel, RunLimit, StepResult};
pub use ports::{
    EventFeedPort, ExecutionPort, FeedPosition, NoTraceSink, PortError, SnapshotPort, Strategy,
    StrategyStatus, TraceSink,
};
pub use prng::Prng;
pub use snapshot::{
    SNAPSHOT_FORMAT_MAJOR, SNAPSHOT_FORMAT_MINOR, SNAPSHOT_HEADER_SIZE, SNAPSHOT_MAGIC,
    SNAPSHOT_REQUIRED_SECTION_COUNT,
};
pub use status::*;
pub use trace::{
    TRACE_FLAG_HASH_PRESENT, TRACE_FORMAT_MAJOR, TRACE_FORMAT_MINOR, TRACE_HASH_ALGORITHM_XXH3_64,
    TRACE_MAGIC, TraceHeader, TraceRecord,
};
pub use view::{ViewGuard, ViewLease, ViewToken};
