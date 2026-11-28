"""Deterministic Event Kernel Python バインディング（EK-IF-001 IF-002 / IF-003）。"""

from __future__ import annotations

from enum import IntEnum

from event_kernel._core import (
    OK,
    RUN_LIMIT_REACHED,
    RUN_SNAPSHOT_REQUESTED,
    STEP_FINISHED,
    ActionQueueFullError,
    ArrowResultStream,
    CausalityViolationError,
    DiskSnapshotPort,
    DiskTraceSink,
    EventBatchView,
    EventRecordView,
    HistoryView,
    InMemoryMarketFeed,
    Kernel,
    KernelCapacityError,
    KernelConfig,
    KernelError,
    KernelFatalError,
    KernelPanicError,
    KernelPoisonedError,
    PortFailureError,
    ReentrantError,
    ResultOverflowError,
    SequenceViolationError,
    SimExecution,
    SnapshotIoError,
    SnapshotRestoreError,
    SnapshotUnsupportedError,
    StepResult,
    StrategyArenaView,
    StrategyContext,
    StrategyFailureError,
)


class StepStatus(IntEnum):
    """`StepResult.status` の正常終了コード。"""

    OK = OK
    FINISHED = STEP_FINISHED
    LIMIT_REACHED = RUN_LIMIT_REACHED
    SNAPSHOT_REQUESTED = RUN_SNAPSHOT_REQUESTED


__all__ = [
    "ActionQueueFullError",
    "ArrowResultStream",
    "CausalityViolationError",
    "DiskSnapshotPort",
    "DiskTraceSink",
    "EventBatchView",
    "EventRecordView",
    "HistoryView",
    "InMemoryMarketFeed",
    "Kernel",
    "KernelCapacityError",
    "KernelConfig",
    "KernelError",
    "KernelFatalError",
    "KernelPanicError",
    "KernelPoisonedError",
    "PortFailureError",
    "ReentrantError",
    "ResultOverflowError",
    "SequenceViolationError",
    "SimExecution",
    "SnapshotIoError",
    "SnapshotRestoreError",
    "SnapshotUnsupportedError",
    "StepResult",
    "StepStatus",
    "StrategyArenaView",
    "StrategyContext",
    "StrategyFailureError",
]
