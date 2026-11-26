//! `ViewGuard` / `ViewToken` の寿命無効化テスト（ADR-003 5.1-7、V-05、QG-009）。

use ek_core::ViewLease;

#[test]
fn view_token_is_valid_only_inside_guard_scope() {
    let lease = ViewLease::new();
    let held_token = {
        let (_guard, token) = lease.enter();
        assert!(token.is_valid());
        assert_eq!(token.generation(), 1);
        token
    };
    assert!(!held_token.is_valid());
}

#[test]
fn previous_generation_token_stays_invalid_across_reentry() {
    let lease = ViewLease::new();
    let old_token = {
        let (_guard, token) = lease.enter();
        assert!(token.is_valid());
        token
    };
    assert!(!old_token.is_valid());

    let (_guard2, new_token) = lease.enter();
    assert!(new_token.is_valid());
    assert_eq!(new_token.generation(), 2);
    // 再入・再発行後も旧世代トークンは無効のままになる。
    assert!(!old_token.is_valid());
}

#[test]
fn view_guard_invalidates_token_on_panic_unwind() {
    let lease = ViewLease::new();
    let mut leaked_token = None;
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let (_guard, token) = lease.enter();
        assert!(token.is_valid());
        leaked_token = Some(token);
        panic!("simulated strategy failure");
    }));
    assert!(outcome.is_err());
    let token = leaked_token.expect("token captured before panic");
    assert!(!token.is_valid());
}
