// Ported from CodexBar Tests/CodexBarTests/AntigravityQuotaSummaryTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

pub fn quota_summary(
    gemini_session: f64,
    gemini_weekly: f64,
    claude_session: f64,
    claude_weekly: f64,
) -> String {
    r#"{
  "response": {
    "description": "Within each group, models share a weekly limit and a 5-hour limit.",
    "groups": [
      {
        "displayName": "Gemini Models",
        "description": "Models within this group: Gemini Flash, Gemini Pro",
        "buckets": [
          {
            "bucketId": "gemini-weekly",
            "displayName": "Weekly Limit",
            "remaining": { "remainingFraction": \(geminiWeekly) },
            "description": "You have used some of your weekly limit, it will fully refresh in 5 days, 11 hours.",
            "resetTime": "2026-06-19T08:45:39Z"
          },
          {
            "bucketId": "gemini-5h",
            "displayName": "Five Hour Limit",
            "remaining": { "remainingFraction": \(geminiSession) },
            "description": "You have used some of your 5-hour limit, it will fully refresh in 4 hours.",
            "resetTime": "2026-06-15T11:39:34Z"
          }
        ]
      },
      {
        "displayName": "Claude and GPT models",
        "description": "Models within this group: Claude Opus, Claude Sonnet, GPT-OSS",
        "buckets": [
          {
            "bucketId": "3p-weekly",
            "displayName": "Weekly Limit",
            "remaining": { "remainingFraction": \(claudeWeekly) },
            "description": "You have used some of your weekly limit, it will fully refresh in 6 days, 22 hours.",
            "resetTime": "2026-06-20T00:39:54Z"
          },
          {
            "bucketId": "3p-5h",
            "displayName": "Five Hour Limit",
            "remaining": { "remainingFraction": \(claudeSession) },
            "description": "You have used some of your 5-hour limit, it will fully refresh in 3 hours, 38 minutes.",
            "resetTime": "2026-06-15T12:52:10Z"
          }
        ]
      }
    ]
  }
}"#.to_owned().replace(r"\(geminiSession)",&gemini_session.to_string()).replace(r"\(geminiWeekly)",&gemini_weekly.to_string()).replace(r"\(claudeSession)",&claude_session.to_string()).replace(r"\(claudeWeekly)",&claude_weekly.to_string())
}

pub fn unknown_summary() -> String {
    r#"{
  "response": {
    "groups": [
      {
        "displayName": "Gemini Models",
        "buckets": [
          {
            "bucketId": "gemini-weekly",
            "displayName": "Weekly Limit",
            "description": "Refreshes later."
          },
          {
            "bucketId": "gemini-5h",
            "displayName": "Five Hour Limit",
            "disabled": true,
            "remaining": { "remainingFraction": 0.5 }
          }
        ]
      }
    ]
  }
}"#
    .to_owned()
}

pub fn user_status() -> String {
    r#"{
  "code": 0,
  "userStatus": {
    "email": "test@example.com",
    "planStatus": {
      "planInfo": {
        "planName": "Pro"
      }
    },
    "cascadeModelConfigData": {
      "clientModelConfigs": [
        {
          "label": "Gemini 3 Pro Low",
          "modelOrAlias": { "model": "gemini-3-pro-low" },
          "quotaInfo": { "remainingFraction": 0.9, "resetTime": "2025-12-24T10:00:00Z" }
        }
      ]
    }
  }
}"#
    .to_owned()
}

pub fn model_config() -> String {
    r#"{
  "clientModelConfigs": [
    {
      "label": "Gemini 3 Pro Low",
      "modelOrAlias": { "model": "gemini-3-pro-low" },
      "quotaInfo": { "remainingFraction": 0.9, "resetTime": "2025-12-24T10:00:00Z" }
    }
  ]
}"#
    .to_owned()
}
