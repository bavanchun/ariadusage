// Ported from CodexBar Tests/CodexBarTests/CodexOAuthTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexAdditionalRateLimitsTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_engine::providers::codex::*;
use ariadusage_protocol::Confidence;
use jiff::Timestamp;
fn credentials() -> CodexOAuthCredentials {
    CodexOAuthCredentials::new(
        "access",
        "refresh",
        None,
        None,
        Some(Timestamp::now()),
        None,
        CodexOAuthCredentialSource::CodexHome,
        false,
    )
}

// upstream: CodexOAuthTests.swift:29 parses O auth credentials
#[test]
fn parses_o_auth_credentials() {
    let raw = br##"{
  "OPENAI_API_KEY": null,
  "tokens": {
    "access_token": "access-token",
    "refresh_token": "refresh-token",
    "id_token": "id-token",
    "account_id": "account-123"
  },
  "last_refresh": "2025-12-20T12:34:56Z"
}"##;
    let creds =
        CodexOAuthCredentialsStore::parse(raw, CodexOAuthCredentialSource::CodexHome).unwrap();
    assert_eq!(creds.access_token_str(), "access-token");
    assert_eq!(creds.refresh_token_str(), "refresh-token");
    assert_eq!(creds.id_token_str(), Some("id-token"));
    assert_eq!(creds.account_id.as_deref(), Some("account-123"));
    assert!(!creds.last_refresh.is_none());
}

// upstream: CodexOAuthTests.swift:51 parses legacy camel case O auth credentials
#[test]
fn parses_legacy_camel_case_o_auth_credentials() {
    let raw = br##"{
  "OPENAI_API_KEY": null,
  "tokens": {
    "accessToken": "access-token",
    "refreshToken": "refresh-token",
    "idToken": "id-token",
    "accountId": "account-123"
  },
  "last_refresh": "2025-12-20T12:34:56Z"
}"##;
    let creds =
        CodexOAuthCredentialsStore::parse(raw, CodexOAuthCredentialSource::CodexHome).unwrap();
    assert_eq!(creds.access_token_str(), "access-token");
    assert_eq!(creds.refresh_token_str(), "refresh-token");
    assert_eq!(creds.id_token_str(), Some("id-token"));
    assert_eq!(creds.account_id.as_deref(), Some("account-123"));
    assert!(!creds.last_refresh.is_none());
}

// upstream: CodexOAuthTests.swift:73 parses API key credentials
#[test]
fn parses_api_key_credentials() {
    let raw = br##"{
  "OPENAI_API_KEY": "sk-test"
}"##;
    let creds =
        CodexOAuthCredentialsStore::parse(raw, CodexOAuthCredentialSource::CodexHome).unwrap();
    assert_eq!(creds.access_token_str(), "sk-test");
    assert!(creds.refresh_token_str().is_empty());
    assert!(creds.id_token_str().is_none());
    assert!(creds.account_id.as_deref().is_none());
    assert!(creds.is_api_key);
    assert!(!creds.needs_refresh());
}

// upstream: CodexOAuthTests.swift:115 decodes credits balance string
#[test]
fn decodes_credits_balance_string() {
    let raw = br##"{
  "plan_type": "pro",
  "rate_limit": {
    "primary_window": {
      "used_percent": 12,
      "reset_at": 1766948068,
      "limit_window_seconds": 18000
    }
  },
  "credits": {
    "has_credits": false,
    "unlimited": false,
    "balance": "0"
  }
}"##;
    let response = CodexOAuthUsageFetcher::decode_usage_response(raw).unwrap();
    assert_eq!(
        response.plan_type.as_ref().map(|v| v.raw_value()),
        Some("pro")
    );
    assert_eq!(response.credits.as_ref().unwrap().balance, Some(0.0));
    assert!(!response.credits.as_ref().unwrap().has_credits);
    assert!(!response.credits.as_ref().unwrap().unlimited);
}

// upstream: CodexOAuthTests.swift:141 decodes prolite plan type without failing usage mapping
#[test]
fn decodes_prolite_plan_type_without_failing_usage_mapping() {
    let raw = br##"{
  "plan_type": "prolite",
  "rate_limit": {
    "primary_window": {
      "used_percent": 12,
      "reset_at": 1766948068,
      "limit_window_seconds": 18000
    }
  }
}"##;
    let snapshot =
        CodexOAuthFetchStrategy::map_usage_for_testing(raw, &credentials(), Timestamp::now())
            .unwrap();
    let response = CodexOAuthUsageFetcher::decode_usage_response(raw).unwrap();
    assert_eq!(
        response.plan_type.as_ref().map(|v| v.raw_value()),
        Some("prolite")
    );
    assert_eq!(
        snapshot
            .as_ref()
            .unwrap()
            .primary
            .as_ref()
            .unwrap()
            .used_percent,
        12.0
    );
}

// upstream: CodexOAuthTests.swift:168 maps usage windows from O auth
#[test]
fn maps_usage_windows_from_o_auth() {
    let raw = br##"{
  "rate_limit": {
    "primary_window": {
      "used_percent": 22,
      "reset_at": 1766948068,
      "limit_window_seconds": 18000
    },
    "secondary_window": {
      "used_percent": 43,
      "reset_at": 1767407914,
      "limit_window_seconds": 604800
    }
  }
}"##;
    let snapshot =
        CodexOAuthFetchStrategy::map_usage_for_testing(raw, &credentials(), Timestamp::now())
            .unwrap();
    assert_eq!(
        snapshot
            .as_ref()
            .unwrap()
            .primary
            .as_ref()
            .unwrap()
            .used_percent,
        22.0
    );
    assert_eq!(
        snapshot
            .as_ref()
            .unwrap()
            .primary
            .as_ref()
            .unwrap()
            .window_minutes,
        Some(300)
    );
    assert_eq!(
        snapshot
            .as_ref()
            .unwrap()
            .secondary
            .as_ref()
            .unwrap()
            .used_percent,
        43.0
    );
    assert_eq!(
        snapshot
            .as_ref()
            .unwrap()
            .secondary
            .as_ref()
            .unwrap()
            .window_minutes,
        Some(10080)
    );
    assert!(
        !snapshot
            .as_ref()
            .unwrap()
            .primary
            .as_ref()
            .unwrap()
            .resets_at
            .is_none()
    );
    assert!(
        !snapshot
            .as_ref()
            .unwrap()
            .secondary
            .as_ref()
            .unwrap()
            .resets_at
            .is_none()
    );
}

// upstream: CodexOAuthTests.swift:202 O auth response with precise windows maps to exact confidence
#[test]
fn o_auth_response_with_precise_windows_maps_to_exact_confidence() {
    let raw = br##"{
  "rate_limit": {
    "primary_window": {
      "used_percent": 22,
      "reset_at": 1766948068,
      "limit_window_seconds": 18000
    },
    "secondary_window": {
      "used_percent": 43,
      "reset_at": 1767407914,
      "limit_window_seconds": 604800
    }
  }
}"##;
    let result = CodexOAuthFetchStrategy::map_result_for_testing(
        raw,
        &credentials(),
        None,
        false,
        true,
        Timestamp::now(),
    )
    .unwrap();
    assert_eq!(result.source_label, "oauth");
    assert_eq!(result.usage.data_confidence, Confidence::Exact);
    assert_eq!(result.usage.primary.as_ref().unwrap().used_percent, 22.0);
    assert_eq!(result.usage.secondary.as_ref().unwrap().used_percent, 43.0);
}

// upstream: CodexOAuthTests.swift:234 O auth response with malformed additional window maps to unknown confidence
#[test]
fn o_auth_response_with_malformed_additional_window_maps_to_unknown_confidence() {
    let raw = br##"{
  "rate_limit": {
    "primary_window": {
      "used_percent": 22,
      "reset_at": 1766948068,
      "limit_window_seconds": 18000
    }
  },
  "additional_rate_limits": [
    {
      "limit_name": "GPT-5.3-Codex-Spark",
      "metered_feature": "gpt_5_3_codex_spark",
      "rate_limit": {
        "primary_window": { "used_percent": "bad" }
      }
    }
  ]
}"##;
    let result = CodexOAuthFetchStrategy::map_result_for_testing(
        raw,
        &credentials(),
        None,
        false,
        true,
        Timestamp::now(),
    )
    .unwrap();
    assert_eq!(result.usage.primary.as_ref().unwrap().used_percent, 22.0);
    assert!(result.usage.extra_rate_windows.is_none());
    assert_eq!(result.usage.data_confidence, Confidence::Unknown);
}

// upstream: CodexOAuthTests.swift:269 maps free weekly only window into secondary
#[test]
fn maps_free_weekly_only_window_into_secondary() {
    let raw = br##"{
  "plan_type": "free",
  "rate_limit": {
    "primary_window": {
      "used_percent": 0,
      "reset_at": 1775468693,
      "limit_window_seconds": 604800
    },
    "secondary_window": null
  }
}"##;
    let snapshot =
        CodexOAuthFetchStrategy::map_usage_for_testing(raw, &credentials(), Timestamp::now())
            .unwrap();
    assert!(snapshot.as_ref().unwrap().primary.is_none());
    assert_eq!(
        snapshot
            .as_ref()
            .unwrap()
            .secondary
            .as_ref()
            .unwrap()
            .used_percent,
        0.0
    );
    assert_eq!(
        snapshot
            .as_ref()
            .unwrap()
            .secondary
            .as_ref()
            .unwrap()
            .window_minutes,
        Some(10080)
    );
}

// upstream: CodexOAuthTests.swift:297 keeps single session window as primary
#[test]
fn keeps_single_session_window_as_primary() {
    let raw = br##"{
  "rate_limit": {
    "primary_window": {
      "used_percent": 9,
      "reset_at": 1766948068,
      "limit_window_seconds": 18000
    },
    "secondary_window": null
  }
}"##;
    let snapshot =
        CodexOAuthFetchStrategy::map_usage_for_testing(raw, &credentials(), Timestamp::now())
            .unwrap();
    assert_eq!(
        snapshot
            .as_ref()
            .unwrap()
            .primary
            .as_ref()
            .unwrap()
            .used_percent,
        9.0
    );
    assert_eq!(
        snapshot
            .as_ref()
            .unwrap()
            .primary
            .as_ref()
            .unwrap()
            .window_minutes,
        Some(300)
    );
    assert!(snapshot.as_ref().unwrap().secondary.is_none());
}

// upstream: CodexOAuthTests.swift:324 preserves unknown single window as primary
#[test]
fn preserves_unknown_single_window_as_primary() {
    let raw = br##"{
  "rate_limit": {
    "primary_window": {
      "used_percent": 17,
      "reset_at": 1766948068,
      "limit_window_seconds": 32400
    },
    "secondary_window": null
  }
}"##;
    let snapshot =
        CodexOAuthFetchStrategy::map_usage_for_testing(raw, &credentials(), Timestamp::now())
            .unwrap();
    assert_eq!(
        snapshot
            .as_ref()
            .unwrap()
            .primary
            .as_ref()
            .unwrap()
            .used_percent,
        17.0
    );
    assert_eq!(
        snapshot
            .as_ref()
            .unwrap()
            .primary
            .as_ref()
            .unwrap()
            .window_minutes,
        Some(540)
    );
    assert!(snapshot.as_ref().unwrap().secondary.is_none());
}

// upstream: CodexOAuthTests.swift:351 preserves unknown secondary only window as primary
#[test]
fn preserves_unknown_secondary_only_window_as_primary() {
    let raw = br##"{
  "rate_limit": {
    "primary_window": null,
    "secondary_window": {
      "used_percent": 17,
      "reset_at": 1766948068,
      "limit_window_seconds": 32400
    }
  }
}"##;
    let snapshot =
        CodexOAuthFetchStrategy::map_usage_for_testing(raw, &credentials(), Timestamp::now())
            .unwrap();
    assert_eq!(
        snapshot
            .as_ref()
            .unwrap()
            .primary
            .as_ref()
            .unwrap()
            .used_percent,
        17.0
    );
    assert_eq!(
        snapshot
            .as_ref()
            .unwrap()
            .primary
            .as_ref()
            .unwrap()
            .window_minutes,
        Some(540)
    );
    assert!(snapshot.as_ref().unwrap().secondary.is_none());
}

// upstream: CodexOAuthTests.swift:378 swaps reversed weekly and unknown windows
#[test]
fn swaps_reversed_weekly_and_unknown_windows() {
    let raw = br##"{
  "rate_limit": {
    "primary_window": {
      "used_percent": 43,
      "reset_at": 1767407914,
      "limit_window_seconds": 604800
    },
    "secondary_window": {
      "used_percent": 17,
      "reset_at": 1766948068,
      "limit_window_seconds": 32400
    }
  }
}"##;
    let snapshot =
        CodexOAuthFetchStrategy::map_usage_for_testing(raw, &credentials(), Timestamp::now())
            .unwrap();
    assert_eq!(
        snapshot
            .as_ref()
            .unwrap()
            .primary
            .as_ref()
            .unwrap()
            .used_percent,
        17.0
    );
    assert_eq!(
        snapshot
            .as_ref()
            .unwrap()
            .primary
            .as_ref()
            .unwrap()
            .window_minutes,
        Some(540)
    );
    assert_eq!(
        snapshot
            .as_ref()
            .unwrap()
            .secondary
            .as_ref()
            .unwrap()
            .used_percent,
        43.0
    );
    assert_eq!(
        snapshot
            .as_ref()
            .unwrap()
            .secondary
            .as_ref()
            .unwrap()
            .window_minutes,
        Some(10080)
    );
}

// upstream: CodexOAuthTests.swift:410 returns nil when O auth usage has no windows
#[test]
fn returns_nil_when_o_auth_usage_has_no_windows() {
    let raw = br##"{
  "rate_limit": {
    "primary_window": null,
    "secondary_window": null
  }
}"##;
    let snapshot =
        CodexOAuthFetchStrategy::map_usage_for_testing(raw, &credentials(), Timestamp::now())
            .unwrap();
    assert!(snapshot.is_none());
}

// upstream: CodexOAuthTests.swift:430 keeps valid window when secondary window is malformed
#[test]
fn keeps_valid_window_when_secondary_window_is_malformed() {
    let raw = br##"{
  "rate_limit": {
    "primary_window": {
      "used_percent": 18,
      "reset_at": 1766948068,
      "limit_window_seconds": 18000
    },
    "secondary_window": {
      "used_percent": "bad",
      "reset_at": 1767407914,
      "limit_window_seconds": 604800
    }
  }
}"##;
    let snapshot =
        CodexOAuthFetchStrategy::map_usage_for_testing(raw, &credentials(), Timestamp::now())
            .unwrap();
    let result = CodexOAuthFetchStrategy::map_result_for_testing(
        raw,
        &credentials(),
        None,
        false,
        true,
        Timestamp::now(),
    )
    .unwrap();
    assert_eq!(
        snapshot
            .as_ref()
            .unwrap()
            .primary
            .as_ref()
            .unwrap()
            .used_percent,
        18.0
    );
    assert!(snapshot.as_ref().unwrap().secondary.is_none());
    assert_eq!(result.usage.primary.as_ref().unwrap().used_percent, 18.0);
    assert!(result.usage.secondary.is_none());
    assert_eq!(result.usage.data_confidence, Confidence::Unknown);
}

// upstream: CodexOAuthTests.swift:464 auto mode keeps weekly window when primary window is malformed
#[test]
fn auto_mode_keeps_weekly_window_when_primary_window_is_malformed() {
    let raw = br##"{
  "rate_limit": {
    "primary_window": {
      "used_percent": "bad",
      "reset_at": 1766948068,
      "limit_window_seconds": 18000
    },
    "secondary_window": {
      "used_percent": 43,
      "reset_at": 1767407914,
      "limit_window_seconds": 604800
    }
  }
}"##;
    let result = CodexOAuthFetchStrategy::map_result_for_testing(
        raw,
        &credentials(),
        None,
        false,
        true,
        Timestamp::now(),
    )
    .unwrap();
    assert!(result.usage.primary.is_none());
    assert_eq!(result.usage.secondary.as_ref().unwrap().used_percent, 43.0);
    assert_eq!(
        result.usage.secondary.as_ref().unwrap().window_minutes,
        Some(10080)
    );
}

// upstream: CodexOAuthTests.swift:499 explicit oauth keeps weekly window when primary window is malformed
#[test]
fn explicit_oauth_keeps_weekly_window_when_primary_window_is_malformed() {
    let raw = br##"{
  "rate_limit": {
    "primary_window": {
      "used_percent": "bad",
      "reset_at": 1766948068,
      "limit_window_seconds": 18000
    },
    "secondary_window": {
      "used_percent": 43,
      "reset_at": 1767407914,
      "limit_window_seconds": 604800
    }
  }
}"##;
    let result = CodexOAuthFetchStrategy::map_result_for_testing(
        raw,
        &credentials(),
        None,
        false,
        true,
        Timestamp::now(),
    )
    .unwrap();
    assert!(result.usage.primary.is_none());
    assert_eq!(result.usage.secondary.as_ref().unwrap().used_percent, 43.0);
    assert_eq!(
        result.usage.secondary.as_ref().unwrap().window_minutes,
        Some(10080)
    );
}

// upstream: CodexOAuthTests.swift:534 auto mode preserves reversed session window when primary window is malformed
#[test]
fn auto_mode_preserves_reversed_session_window_when_primary_window_is_malformed() {
    let raw = br##"{
  "rate_limit": {
    "primary_window": {
      "used_percent": "bad",
      "reset_at": 1767407914,
      "limit_window_seconds": 604800
    },
    "secondary_window": {
      "used_percent": 18,
      "reset_at": 1766948068,
      "limit_window_seconds": 18000
    }
  }
}"##;
    let result = CodexOAuthFetchStrategy::map_result_for_testing(
        raw,
        &credentials(),
        None,
        false,
        true,
        Timestamp::now(),
    )
    .unwrap();
    assert_eq!(result.usage.primary.as_ref().unwrap().used_percent, 18.0);
    assert_eq!(
        result.usage.primary.as_ref().unwrap().window_minutes,
        Some(300)
    );
    assert!(result.usage.secondary.is_none());
}

// upstream: CodexOAuthTests.swift:569 auto mode keeps weekly window when reversed session window is malformed
#[test]
fn auto_mode_keeps_weekly_window_when_reversed_session_window_is_malformed() {
    let raw = br##"{
  "rate_limit": {
    "primary_window": {
      "used_percent": 43,
      "reset_at": 1767407914,
      "limit_window_seconds": 604800
    },
    "secondary_window": {
      "used_percent": "bad",
      "reset_at": 1766948068,
      "limit_window_seconds": 18000
    }
  }
}"##;
    let result = CodexOAuthFetchStrategy::map_result_for_testing(
        raw,
        &credentials(),
        None,
        false,
        true,
        Timestamp::now(),
    )
    .unwrap();
    assert!(result.usage.primary.is_none());
    assert_eq!(result.usage.secondary.as_ref().unwrap().used_percent, 43.0);
    assert_eq!(
        result.usage.secondary.as_ref().unwrap().window_minutes,
        Some(10080)
    );
}

// upstream: CodexOAuthTests.swift:604 explicit oauth keeps weekly window when reversed session window is malformed
#[test]
fn explicit_oauth_keeps_weekly_window_when_reversed_session_window_is_malformed() {
    let raw = br##"{
  "rate_limit": {
    "primary_window": {
      "used_percent": 43,
      "reset_at": 1767407914,
      "limit_window_seconds": 604800
    },
    "secondary_window": {
      "used_percent": "bad",
      "reset_at": 1766948068,
      "limit_window_seconds": 18000
    }
  }
}"##;
    let result = CodexOAuthFetchStrategy::map_result_for_testing(
        raw,
        &credentials(),
        None,
        false,
        true,
        Timestamp::now(),
    )
    .unwrap();
    assert!(result.usage.primary.is_none());
    assert_eq!(result.usage.secondary.as_ref().unwrap().used_percent, 43.0);
    assert_eq!(
        result.usage.secondary.as_ref().unwrap().window_minutes,
        Some(10080)
    );
}

// upstream: CodexOAuthTests.swift:639 ignores malformed credits payload while keeping usage
#[test]
fn ignores_malformed_credits_payload_while_keeping_usage() {
    let raw = br##"{
  "rate_limit": {
    "primary_window": {
      "used_percent": 22,
      "reset_at": 1766948068,
      "limit_window_seconds": 18000
    }
  },
  "credits": {
    "has_credits": false,
    "unlimited": false,
    "balance": []
  }
}"##;
    let snapshot =
        CodexOAuthFetchStrategy::map_usage_for_testing(raw, &credentials(), Timestamp::now())
            .unwrap();
    let response = CodexOAuthUsageFetcher::decode_usage_response(raw).unwrap();
    assert!(!response.credits.as_ref().unwrap().has_credits);
    assert!(!response.credits.as_ref().unwrap().unlimited);
    assert!(response.credits.as_ref().unwrap().balance.is_none());
    assert_eq!(
        snapshot
            .as_ref()
            .unwrap()
            .primary
            .as_ref()
            .unwrap()
            .used_percent,
        22.0
    );
}

// upstream: CodexOAuthTests.swift:672 credits only O auth payload still returns credits result
#[test]
fn credits_only_o_auth_payload_still_returns_credits_result() {
    let raw = br##"{
  "rate_limit": {
    "primary_window": null,
    "secondary_window": null
  },
  "credits": {
    "has_credits": true,
    "unlimited": false,
    "balance": "14.5"
  }
}"##;
    let result = CodexOAuthFetchStrategy::map_result_for_testing(
        raw,
        &credentials(),
        None,
        false,
        true,
        Timestamp::now(),
    )
    .unwrap();
    assert!(result.usage.primary.is_none());
    assert!(result.usage.secondary.is_none());
    assert_eq!(result.credits.as_ref().unwrap().remaining, 14.5);
    assert_eq!(result.source_label, "oauth");
}

// upstream: CodexOAuthTests.swift:702 credits only O auth payload returns credits in auto mode
#[test]
fn credits_only_o_auth_payload_returns_credits_in_auto_mode() {
    let raw = br##"{
  "rate_limit": {
    "primary_window": null,
    "secondary_window": null
  },
  "credits": {
    "has_credits": true,
    "unlimited": false,
    "balance": "14.5"
  }
}"##;
    let result = CodexOAuthFetchStrategy::map_result_for_testing(
        raw,
        &credentials(),
        None,
        false,
        true,
        Timestamp::now(),
    )
    .unwrap();
    assert!(result.usage.primary.is_none());
    assert!(result.usage.secondary.is_none());
    assert_eq!(result.credits.as_ref().unwrap().remaining, 14.5);
    assert_eq!(result.source_label, "oauth");
}

// upstream: CodexAdditionalRateLimitsTests.swift:6 maps additional spark limit into a named extra rate window
#[test]
fn maps_additional_spark_limit_into_a_named_extra_rate_window() {
    let raw = br##"{
  "plan_type": "pro",
  "rate_limit": {
    "primary_window": {
      "used_percent": 22,
      "reset_at": 1766948068,
      "limit_window_seconds": 18000
    },
    "secondary_window": {
      "used_percent": 43,
      "reset_at": 1767407914,
      "limit_window_seconds": 604800
    }
  },
  "additional_rate_limits": [
    {
      "limit_name": "GPT-5.3-Codex-Spark",
      "metered_feature": "gpt_5_3_codex_spark",
      "rate_limit": {
        "allowed": true,
        "limit_reached": false,
        "primary_window": {
          "used_percent": 30,
          "reset_at": 1766948068,
          "limit_window_seconds": 18000,
          "reset_after_seconds": 12345
        },
        "secondary_window": {
          "used_percent": 100,
          "reset_at": 1767407914,
          "limit_window_seconds": 604800,
          "reset_after_seconds": 56789
        }
      }
    }
  ]
}"##;
    let snapshot =
        CodexOAuthFetchStrategy::map_usage_for_testing(raw, &credentials(), Timestamp::now())
            .unwrap();
    let extras = snapshot
        .as_ref()
        .unwrap()
        .extra_rate_windows
        .as_ref()
        .unwrap();
    let spark = extras.first().unwrap();
    let weekly = extras.last().unwrap();
    assert_eq!(
        snapshot
            .as_ref()
            .unwrap()
            .primary
            .as_ref()
            .unwrap()
            .used_percent,
        22.0
    );
    assert_eq!(
        snapshot
            .as_ref()
            .unwrap()
            .secondary
            .as_ref()
            .unwrap()
            .used_percent,
        43.0
    );
    assert_eq!(extras.len(), 2);
    assert_eq!(spark.id, "codex-spark");
    assert_eq!(spark.title, "Codex Spark 5-hour");
    assert_eq!(spark.window.used_percent, 30.0);
    assert_eq!(spark.window.window_minutes, Some(300));
    assert!(!spark.window.resets_at.is_none());
    assert_eq!(weekly.id, "codex-spark-weekly");
    assert_eq!(weekly.title, "Codex Spark Weekly");
    assert_eq!(weekly.window.used_percent, 100.0);
    assert_eq!(weekly.window.window_minutes, Some(10080));
    assert!(!weekly.window.resets_at.is_none());
}

// upstream: CodexAdditionalRateLimitsTests.swift:75 keeps valid spark window when an additional limit sibling is malformed
#[test]
fn keeps_valid_spark_window_when_an_additional_limit_sibling_is_malformed() {
    let raw=br##"{
  "rate_limit": {
    "primary_window": { "used_percent": 22, "reset_at": 1766948068, "limit_window_seconds": 18000 },
    "secondary_window": { "used_percent": 43, "reset_at": 1767407914, "limit_window_seconds": 604800 }
  },
  "additional_rate_limits": [
    "garbage-not-an-object",
    {
      "limit_name": "GPT-5.3-Codex-Spark",
      "metered_feature": "gpt_5_3_codex_spark",
      "rate_limit": {
        "primary_window": { "used_percent": 30, "reset_at": 1766948068, "limit_window_seconds": 18000 }
      }
    },
    42
  ]
}"##;
    let snapshot =
        CodexOAuthFetchStrategy::map_usage_for_testing(raw, &credentials(), Timestamp::now())
            .unwrap();
    let extras = snapshot
        .as_ref()
        .unwrap()
        .extra_rate_windows
        .as_ref()
        .unwrap();
    assert_eq!(
        snapshot
            .as_ref()
            .unwrap()
            .primary
            .as_ref()
            .unwrap()
            .used_percent,
        22.0
    );
    assert_eq!(
        snapshot
            .as_ref()
            .unwrap()
            .secondary
            .as_ref()
            .unwrap()
            .used_percent,
        43.0
    );
    assert_eq!(extras.len(), 1);
    assert_eq!(extras.first().unwrap().id, "codex-spark");
    assert_eq!(extras.first().unwrap().window.used_percent, 30.0);
}

// upstream: CodexAdditionalRateLimitsTests.swift:114 keeps primary usage when every additional limit element is malformed
#[test]
fn keeps_primary_usage_when_every_additional_limit_element_is_malformed() {
    let raw = br##"{
  "rate_limit": {
    "primary_window": { "used_percent": 22, "reset_at": 1766948068, "limit_window_seconds": 18000 }
  },
  "additional_rate_limits": ["garbage", 1, true]
}"##;
    let snapshot =
        CodexOAuthFetchStrategy::map_usage_for_testing(raw, &credentials(), Timestamp::now())
            .unwrap();
    assert_eq!(
        snapshot
            .as_ref()
            .unwrap()
            .primary
            .as_ref()
            .unwrap()
            .used_percent,
        22.0
    );
    assert!(snapshot.as_ref().unwrap().extra_rate_windows.is_none());
}

// upstream: CodexAdditionalRateLimitsTests.swift:135 omits extra rate windows when additional limits are absent
#[test]
fn omits_extra_rate_windows_when_additional_limits_are_absent() {
    let raw = br##"{
  "rate_limit": {
    "primary_window": {
      "used_percent": 22,
      "reset_at": 1766948068,
      "limit_window_seconds": 18000
    }
  }
}"##;
    let snapshot =
        CodexOAuthFetchStrategy::map_usage_for_testing(raw, &credentials(), Timestamp::now())
            .unwrap();
    assert_eq!(
        snapshot
            .as_ref()
            .unwrap()
            .primary
            .as_ref()
            .unwrap()
            .used_percent,
        22.0
    );
    assert!(snapshot.as_ref().unwrap().extra_rate_windows.is_none());
}

// upstream: CodexAdditionalRateLimitsTests.swift:159 tolerates malformed additional limits while keeping primary window
#[test]
fn tolerates_malformed_additional_limits_while_keeping_primary_window() {
    let raw = br##"{
  "rate_limit": {
    "primary_window": {
      "used_percent": 22,
      "reset_at": 1766948068,
      "limit_window_seconds": 18000
    }
  },
  "additional_rate_limits": "unexpected"
}"##;
    let snapshot =
        CodexOAuthFetchStrategy::map_usage_for_testing(raw, &credentials(), Timestamp::now())
            .unwrap();
    assert_eq!(
        snapshot
            .as_ref()
            .unwrap()
            .primary
            .as_ref()
            .unwrap()
            .used_percent,
        22.0
    );
    assert!(snapshot.as_ref().unwrap().extra_rate_windows.is_none());
}

// upstream: CodexAdditionalRateLimitsTests.swift:184 skips additional limits without a usable window
#[test]
fn skips_additional_limits_without_a_usable_window() {
    let raw = br##"{
  "rate_limit": {
    "primary_window": {
      "used_percent": 22,
      "reset_at": 1766948068,
      "limit_window_seconds": 18000
    }
  },
  "additional_rate_limits": [
    {
      "limit_name": "GPT-5.3-Codex-Spark",
      "metered_feature": "gpt_5_3_codex_spark",
      "rate_limit": null
    }
  ]
}"##;
    let snapshot =
        CodexOAuthFetchStrategy::map_usage_for_testing(raw, &credentials(), Timestamp::now())
            .unwrap();
    assert_eq!(
        snapshot
            .as_ref()
            .unwrap()
            .primary
            .as_ref()
            .unwrap()
            .used_percent,
        22.0
    );
    assert!(snapshot.as_ref().unwrap().extra_rate_windows.is_none());
}

// upstream: CodexAdditionalRateLimitsTests.swift:215 maps non spark additional limit using a slugged id and api label
#[test]
fn maps_non_spark_additional_limit_using_a_slugged_id_and_api_label() {
    let raw = br##"[
  {
    "limit_name": "GPT-5.3-Codex-Mini",
    "metered_feature": "gpt_5_3_codex_mini",
    "rate_limit": {
      "allowed": true,
      "limit_reached": false,
      "primary_window": {
        "used_percent": 12,
        "reset_at": 1766948068,
        "limit_window_seconds": 18000
      }
    }
  },
  {
    "limit_name": "GPT-5.3-Codex-Mini",
    "metered_feature": "gpt_5_3_codex_mini",
    "rate_limit": {
      "allowed": true,
      "limit_reached": false,
      "primary_window": {
        "used_percent": 99,
        "reset_at": 1766948068,
        "limit_window_seconds": 18000
      }
    }
  }
]"##;
    let entries: Vec<AdditionalRateLimit> = serde_json::from_slice(raw).unwrap();
    let windows = CodexAdditionalRateLimitMapper::extra_rate_windows(
        Some(&entries),
        Timestamp::from_second(1_800_000_000).unwrap(),
    );
    let window = windows.first().unwrap();
    assert_eq!(windows.len(), 1);
    assert_eq!(window.id, "codex-gpt-5-3-codex-mini");
    assert_eq!(window.title, "GPT-5.3-Codex-Mini");
    assert_eq!(window.window.used_percent, 12.0);
}

// upstream: CodexAdditionalRateLimitsTests.swift:258 dedupes split spark entries by window kind
#[test]
fn dedupes_split_spark_entries_by_window_kind() {
    let raw = br##"[
  {
    "limit_name": "GPT-5.3-Codex-Spark",
    "metered_feature": "gpt_5_3_codex_spark",
    "rate_limit": {
      "allowed": true,
      "limit_reached": false,
      "primary_window": {
        "used_percent": 20,
        "reset_at": 1766948068,
        "limit_window_seconds": 18000
      }
    }
  },
  {
    "limit_name": "GPT-5.3-Codex-Spark Weekly",
    "metered_feature": "gpt_5_3_codex_spark",
    "rate_limit": {
      "allowed": true,
      "limit_reached": false,
      "primary_window": {
        "used_percent": 80,
        "reset_at": 1767407914,
        "limit_window_seconds": 604800
      }
    }
  },
  {
    "limit_name": "GPT-5.3-Codex-Spark Duplicate",
    "metered_feature": "gpt_5_3_codex_spark",
    "rate_limit": {
      "allowed": true,
      "limit_reached": false,
      "primary_window": {
        "used_percent": 99,
        "reset_at": 1766948068,
        "limit_window_seconds": 18000
      }
    }
  }
]"##;
    let entries: Vec<AdditionalRateLimit> = serde_json::from_slice(raw).unwrap();
    let windows = CodexAdditionalRateLimitMapper::extra_rate_windows(
        Some(&entries),
        Timestamp::from_second(1_800_000_000).unwrap(),
    );
    assert_eq!(
        windows.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(),
        ["codex-spark", "codex-spark-weekly"]
    );
    assert_eq!(windows.first().unwrap().window.used_percent, 20.0);
    assert_eq!(windows.last().unwrap().window.used_percent, 80.0);
}
