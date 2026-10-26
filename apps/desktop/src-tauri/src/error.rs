//! The desktop IPC error the window localizes; external API/MCP envelopes are separate.
use serde::Serialize;
use serde_json::{Value, json};

/// Private desktop IPC error. External API/MCP error envelopes remain unchanged.
#[derive(Debug, Clone, Serialize)]
pub struct AppError {
    pub code: String,
    #[serde(skip_serializing_if = "serde_json::Value::is_null")]
    pub params: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<&'static str>,
}

impl AppError {
    pub(crate) fn new(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            params: json!({}),
            detail: None,
            outcome: None,
        }
    }

    pub(crate) fn tool(error: odx_core::ToolError) -> Self {
        let ui_code = error.ui_code;
        let detail = matches!(
            error.code,
            "upstream_error" | "upstream_network" | "upstream_timeout"
        )
        .then_some(error.message);
        Self {
            code: ui_code.unwrap_or(error.code).into(),
            params: if ui_code.is_some() {
                error.ui_params
            } else {
                json!({})
            },
            detail,
            outcome: Some(match error.outcome {
                odx_core::output::Outcome::NotSent => "not_sent",
                odx_core::output::Outcome::Failed => "failed",
                odx_core::output::Outcome::Uncertain => "uncertain",
            }),
        }
    }
}

impl From<odx_core::error::OdxError> for AppError {
    fn from(error: odx_core::error::OdxError) -> Self {
        let outcome = match &error {
            odx_core::error::OdxError::Uncertain => Some("uncertain"),
            odx_core::error::OdxError::Remote(_)
            | odx_core::error::OdxError::Network { .. }
            | odx_core::error::OdxError::HttpStatus { .. } => Some("failed"),
            _ => Some("not_sent"),
        };
        Self {
            code: error.code().into(),
            params: error.ui_params(),
            detail: error.safe_detail().map(str::to_owned),
            outcome,
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(_: std::io::Error) -> Self {
        Self::new("system.operation_failed")
    }
}
impl From<String> for AppError {
    fn from(_: String) -> Self {
        Self::new("app.operation_failed")
    }
}
impl From<&str> for AppError {
    fn from(_: &str) -> Self {
        Self::new("app.operation_failed")
    }
}

#[cfg(test)]
mod error_contract_tests {
    use super::*;
    use odx_core::error::OdxError;

    #[test]
    fn invalid_input_does_not_leak_the_original_value() {
        let error = AppError::from(OdxError::Invalid("api_key=secret-value".into()));
        assert_eq!(error.code, "app.invalid_input");
        assert!(error.detail.is_none());
        assert!(
            !serde_json::to_string(&error)
                .unwrap()
                .contains("secret-value")
        );
    }

    #[test]
    fn validation_errors_keep_stable_codes_and_safe_params_for_the_desktop() {
        let error = AppError::from(OdxError::validation_with_params(
            "validation.settings.out_of_range",
            json!({ "setting": "global_concurrency", "min": 1, "max": 64, "api_key": "secret" }),
            "legacy validation message containing api_key=secret",
        ));
        assert_eq!(error.code, "validation.settings.out_of_range");
        assert_eq!(
            error.params,
            json!({ "setting": "global_concurrency", "min": 1, "max": 64 })
        );
        assert!(error.detail.is_none());
        let serialized = serde_json::to_string(&error).unwrap();
        assert!(!serialized.contains("legacy validation message"));
        assert!(!serialized.contains("secret"));
    }

    #[test]
    fn upstream_details_and_uncertain_outcomes_keep_their_contract() {
        let remote = AppError::from(OdxError::Remote("access denied; key=[REDACTED]".into()));
        assert_eq!(remote.code, "upstream.remote");
        assert_eq!(
            remote.detail.as_deref(),
            Some("access denied; key=[REDACTED]")
        );

        let uncertain = AppError::from(OdxError::Uncertain);
        assert_eq!(uncertain.code, "upstream.uncertain_result");
        assert_eq!(uncertain.outcome, Some("uncertain"));
        assert!(uncertain.detail.is_none());
    }

    #[test]
    fn tool_uncertain_result_does_not_include_backend_text() {
        let error = AppError::tool(odx_core::ToolError::new(
            "uncertain_result",
            "localized by the UI",
            odx_core::Outcome::Uncertain,
        ));
        assert_eq!(error.code, "uncertain_result");
        assert_eq!(error.outcome, Some("uncertain"));
        assert!(error.detail.is_none());
    }
}
