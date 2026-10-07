// Ported from CodexBar Tests/CodexBarTests/CodexOAuthExpiryPipelineTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexOAuthCredentialReadTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar TestsLinux/ClaudeOAuthDelegatedRefreshLinuxTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::oauth::{ExpireFailureReason, ExpiredAction, TokenOwner, decide_on_expired};
use ariadusage_core::pipeline::{FetchInteraction, FetchRuntime};

// CodexBar: Tests/CodexBarTests/CodexOAuthExpiryPipelineTests.swift:246
#[test]
fn test_expired_native_tokens_retain_cli_ownership() {
    let action = decide_on_expired(
        TokenOwner::CliNative,
        true,
        FetchRuntime::Daemon,
        FetchInteraction::UserInitiated,
        true,
    );
    assert_eq!(
        action,
        ExpiredAction::Fail(ExpireFailureReason::NativeRefreshRequired)
    );
}

// CodexBar: Tests/CodexBarTests/CodexOAuthExpiryPipelineTests.swift:331
#[test]
fn test_stale_external_credentials_never_redeemed() {
    let action = decide_on_expired(
        TokenOwner::ReadOnlyExternal,
        true,
        FetchRuntime::Daemon,
        FetchInteraction::UserInitiated,
        true,
    );
    assert_eq!(
        action,
        ExpiredAction::Fail(ExpireFailureReason::ReadOnlySource)
    );
}

// CodexBar: Tests/CodexBarTests/CodexOAuthCredentialReadTests.swift:447
#[test]
fn test_stale_native_probes_never_redeem_refresh_token() {
    for has_refresh in [false, true] {
        let action = decide_on_expired(
            TokenOwner::CliNative,
            has_refresh,
            FetchRuntime::Daemon,
            FetchInteraction::Background,
            false,
        );
        assert_eq!(
            action,
            ExpiredAction::Fail(ExpireFailureReason::NativeRefreshRequired)
        );
    }
}

// CodexBar: TestsLinux/ClaudeOAuthDelegatedRefreshLinuxTests.swift:38
#[test]
fn test_cli_oauth_does_not_delegate_refresh_even_for_user_action() {
    let action = decide_on_expired(
        TokenOwner::Cli,
        true,
        FetchRuntime::Cli,
        FetchInteraction::UserInitiated,
        true,
    );
    assert_eq!(
        action,
        ExpiredAction::Fail(ExpireFailureReason::CliRuntimeNeverDelegates)
    );
}

// CodexBar: TestsLinux/ClaudeOAuthDelegatedRefreshLinuxTests.swift:49
#[test]
fn test_app_oauth_preserves_user_initiated_delegated_refresh() {
    let action = decide_on_expired(
        TokenOwner::Cli,
        true,
        FetchRuntime::Daemon,
        FetchInteraction::UserInitiated,
        false,
    );
    assert_eq!(action, ExpiredAction::Delegate);
}

// CodexBar: TestsLinux/ClaudeOAuthDelegatedRefreshLinuxTests.swift:60
#[test]
fn test_app_oauth_background_respects_prompt_policy() {
    // Without opt-in: background delegation is suppressed
    let without_opt_in = decide_on_expired(
        TokenOwner::Cli,
        true,
        FetchRuntime::Daemon,
        FetchInteraction::Background,
        false,
    );
    assert_eq!(
        without_opt_in,
        ExpiredAction::Fail(ExpireFailureReason::DelegationNotAllowed)
    );

    // With explicit opt-in: background delegation is permitted
    let with_opt_in = decide_on_expired(
        TokenOwner::Cli,
        true,
        FetchRuntime::Daemon,
        FetchInteraction::Background,
        true,
    );
    assert_eq!(with_opt_in, ExpiredAction::Delegate);
}

#[test]
fn test_full_decide_on_expired_truth_table_80_combinations() {
    let owners = [
        TokenOwner::Cli,
        TokenOwner::CliNative,
        TokenOwner::Environment,
        TokenOwner::ReadOnlyExternal,
        TokenOwner::SelfOwned,
    ];
    let refresh_tokens = [false, true];
    let runtimes = [FetchRuntime::Cli, FetchRuntime::Daemon];
    let interactions = [
        FetchInteraction::Background,
        FetchInteraction::UserInitiated,
    ];
    let opt_ins = [false, true];

    let mut count = 0;
    for &owner in &owners {
        for &has_rt in &refresh_tokens {
            for &runtime in &runtimes {
                for &interaction in &interactions {
                    for &opt_in in &opt_ins {
                        count += 1;
                        let action = decide_on_expired(owner, has_rt, runtime, interaction, opt_in);

                        match owner {
                            TokenOwner::Cli => {
                                assert_ne!(action, ExpiredAction::Refresh);
                                if runtime == FetchRuntime::Cli {
                                    assert_eq!(
                                        action,
                                        ExpiredAction::Fail(
                                            ExpireFailureReason::CliRuntimeNeverDelegates
                                        )
                                    );
                                } else if interaction == FetchInteraction::Background && !opt_in {
                                    assert_eq!(
                                        action,
                                        ExpiredAction::Fail(
                                            ExpireFailureReason::DelegationNotAllowed
                                        )
                                    );
                                } else {
                                    assert_eq!(action, ExpiredAction::Delegate);
                                }
                            }
                            TokenOwner::CliNative => {
                                assert_ne!(action, ExpiredAction::Refresh);
                                assert_eq!(
                                    action,
                                    ExpiredAction::Fail(ExpireFailureReason::NativeRefreshRequired)
                                );
                            }
                            TokenOwner::Environment => {
                                assert_ne!(action, ExpiredAction::Refresh);
                                assert_eq!(
                                    action,
                                    ExpiredAction::Fail(ExpireFailureReason::NoRefreshToken)
                                );
                            }
                            TokenOwner::ReadOnlyExternal => {
                                assert_ne!(action, ExpiredAction::Refresh);
                                assert_eq!(
                                    action,
                                    ExpiredAction::Fail(ExpireFailureReason::ReadOnlySource)
                                );
                            }
                            TokenOwner::SelfOwned => {
                                if has_rt {
                                    assert_eq!(action, ExpiredAction::Refresh);
                                } else {
                                    assert_eq!(
                                        action,
                                        ExpiredAction::Fail(ExpireFailureReason::NoRefreshToken)
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    assert_eq!(count, 80, "Truth table must cover exactly 80 permutations");
}

#[test]
fn test_debug_never_leaks_tokens() {
    let invented_token = ["secret", "oauth", "token", "12345"].join("_");
    let owner = TokenOwner::SelfOwned;
    let action = ExpiredAction::Fail(ExpireFailureReason::NoRefreshToken);
    let owner_debug = format!("{owner:?}");
    let action_debug = format!("{action:?}");
    assert!(!owner_debug.contains(&invented_token));
    assert!(!action_debug.contains(&invented_token));
}
