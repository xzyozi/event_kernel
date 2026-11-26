//! コールバック終了時にビューを無効化するスコープガード（ADR-003 5.1-7、V-05、QG-009）。
//!
//! `core` は `#![no_std]` + `#![forbid(unsafe_code)]` で安全に管理し、
//! `Drop` によって正常終了・早期 return・パニック（unwind）のいずれの経路でも無効化する。

use alloc::rc::Rc;
use core::cell::Cell;

#[derive(Debug)]
struct ViewLeaseState {
    generation: Cell<u64>,
    active: Cell<bool>,
}

/// ビューの有効性を照合する軽量トークン。
#[derive(Clone, Debug)]
pub struct ViewToken {
    state: Rc<ViewLeaseState>,
    generation: u64,
}

impl ViewToken {
    /// このトークンに対応するコールバック呼び出しが現在も有効であれば true。
    #[inline]
    pub fn is_valid(&self) -> bool {
        self.state.active.get() && self.state.generation.get() == self.generation
    }

    /// トークン発行時の世代番号（監視・デバッグ用）。
    #[inline]
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

/// コールバック呼び出しの寿命を制御する `ViewGuard` の生成元。
#[derive(Debug)]
pub struct ViewLease {
    state: Rc<ViewLeaseState>,
}

impl Default for ViewLease {
    fn default() -> Self {
        Self::new()
    }
}

impl ViewLease {
    /// 新しいビューリースを作成する（初期状態は無効）。
    pub fn new() -> Self {
        Self {
            state: Rc::new(ViewLeaseState {
                generation: Cell::new(0),
                active: Cell::new(false),
            }),
        }
    }

    /// 1回のコールバック区間を開始し、終了時に必ず無効化するガードとトークンを返す。
    pub fn enter(&self) -> (ViewGuard, ViewToken) {
        let next_gen = self.state.generation.get().wrapping_add(1);
        self.state.generation.set(next_gen);
        self.state.active.set(true);
        let token = ViewToken {
            state: Rc::clone(&self.state),
            generation: next_gen,
        };
        let guard = ViewGuard {
            state: Rc::clone(&self.state),
            generation: next_gen,
        };
        (guard, token)
    }
}

/// スコープ離脱時（`Drop`）に `ViewToken` を無効化するガード。
#[derive(Debug)]
pub struct ViewGuard {
    state: Rc<ViewLeaseState>,
    generation: u64,
}

impl ViewGuard {
    /// 明示的にビューを無効化する（以後の操作も安全）。
    #[inline]
    pub fn invalidate(&self) {
        if self.state.generation.get() == self.generation {
            self.state.active.set(false);
        }
    }
}

impl Drop for ViewGuard {
    fn drop(&mut self) {
        self.invalidate();
    }
}
