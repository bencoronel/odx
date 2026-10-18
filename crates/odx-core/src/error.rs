use serde_json::{Value, json};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum OdxError {
    #[error("vault locked")]
    Locked,
    #[error("incorrect password or damaged vault")]
    UnlockFailed,
    #[error("invalid input: {0}")]
    Invalid(String),
    /// Validation with a stable UI code and a message for the external API/MCP envelope.
    #[error("{message}")]
    Validation {
        code: &'static str,
        params: Value,
        message: String,
    },
    #[error("invalid or revoked client token")]
    Unauthorized,
    #[error("client not found")]
    ClientNotFound,
    #[error("environment not found")]
    EnvironmentNotFound,
    #[error("denied by environment policy")]
    Denied,
    #[error("approval missing, expired or already decided")]
    ApprovalUnavailable,
    #[error("uncertain result: Odoo may have applied the write")]
    Uncertain,
    #[error("vault or audit failure")]
    Persistence,
    #[error("upstream failure: {0}")]
    Upstream(String),
    #[error("Odoo rejected the environment credentials")]
    AuthenticationRejected,
    /// HTTP error from Odoo or an upstream proxy.
    #[error("Odoo returned HTTP {status}")]
    HttpStatus { status: u16 },
    /// Network failure before a response; `kind` is a stable short label.
    #[error("network failure ({kind}): {message}")]
    Network { kind: &'static str, message: String },
    /// Logical Odoo error (JSON-RPC `error` response).
    #[error("Odoo: {0}")]
    Remote(String),
    #[error("capacity limit reached")]
    Capacity,
    /// Odoo response exceeds this call's limit in MiB.
    #[error("Odoo response exceeds {limit_mib} MiB")]
    ResponseTooLarge { limit_mib: usize },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T, E = OdxError> = std::result::Result<T, E>;

impl OdxError {
    pub fn validation(code: &'static str, message: impl Into<String>) -> Self {
        Self::Validation {
            code,
            params: json!({}),
            message: message.into(),
        }
    }

    pub fn validation_with_params(
        code: &'static str,
        params: Value,
        message: impl Into<String>,
    ) -> Self {
        Self::Validation {
            code,
            params,
            message: message.into(),
        }
    }

    /// Stable identifier for desktop error localization. Never derive this from Display text.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Locked => "app.locked",
            Self::UnlockFailed => "app.unlock_failed",
            Self::Invalid(_) => "app.invalid_input",
            Self::Validation { code, .. } => code,
            Self::Unauthorized => "app.unauthorized",
            Self::ClientNotFound => "app.client_not_found",
            Self::EnvironmentNotFound => "app.environment_not_found",
            Self::Denied => "app.denied",
            Self::ApprovalUnavailable => "app.approval_unavailable",
            Self::Uncertain => "upstream.uncertain_result",
            Self::Persistence | Self::Io(_) | Self::Json(_) => "app.persistence",
            Self::Upstream(_) => "upstream.failure",
            Self::AuthenticationRejected => "upstream.auth_rejected",
            Self::HttpStatus { .. } => "upstream.http_status",
            Self::Network { .. } => "upstream.network",
            Self::Remote(_) => "upstream.remote",
            Self::Capacity => "app.capacity",
            Self::ResponseTooLarge { .. } => "upstream.response_too_large",
        }
    }

    /// Only low-risk scalar values needed by localized messages are exposed.
    pub fn ui_params(&self) -> Value {
        match self {
            Self::HttpStatus { status } => json!({ "status": status }),
            Self::Network { kind, .. } => json!({ "kind": kind }),
            Self::ResponseTooLarge { limit_mib } => json!({ "limitMiB": limit_mib }),
            Self::Validation { code, params, .. } => {
                let allowed: &[&str] = match *code {
                    "validation.settings.out_of_range" => &["setting", "min", "max"],
                    "validation.rule.limit_reached" => &["limit"],
                    _ => &[],
                };
                let mut safe = serde_json::Map::new();
                for name in allowed {
                    if let Some(value) = params
                        .get(*name)
                        .filter(|value| value.is_string() || value.is_number())
                    {
                        safe.insert((*name).to_owned(), value.clone());
                    }
                }
                Value::Object(safe)
            }
            _ => json!({}),
        }
    }

    /// Detail is restricted to Odoo/network errors and has already passed through scrubbers.
    pub fn safe_detail(&self) -> Option<&str> {
        match self {
            Self::Remote(detail)
            | Self::Network {
                message: detail, ..
            } => Some(detail),
            _ => None,
        }
    }

    /// Copy an error for another recipient, such as a waiting client.
    pub(crate) fn duplicate(&self) -> OdxError {
        match self {
            OdxError::Locked => OdxError::Locked,
            OdxError::UnlockFailed => OdxError::UnlockFailed,
            OdxError::Invalid(m) => OdxError::Invalid(m.clone()),
            OdxError::Validation {
                code,
                params,
                message,
            } => OdxError::Validation {
                code,
                params: params.clone(),
                message: message.clone(),
            },
            OdxError::Unauthorized => OdxError::Unauthorized,
            OdxError::ClientNotFound => OdxError::ClientNotFound,
            OdxError::EnvironmentNotFound => OdxError::EnvironmentNotFound,
            OdxError::Denied => OdxError::Denied,
            OdxError::ApprovalUnavailable => OdxError::ApprovalUnavailable,
            OdxError::Uncertain => OdxError::Uncertain,
            OdxError::Persistence | OdxError::Io(_) | OdxError::Json(_) => OdxError::Persistence,
            OdxError::Upstream(m) => OdxError::Upstream(m.clone()),
            OdxError::AuthenticationRejected => OdxError::AuthenticationRejected,
            OdxError::HttpStatus { status } => OdxError::HttpStatus { status: *status },
            OdxError::Network { kind, message } => OdxError::Network {
                kind,
                message: message.clone(),
            },
            OdxError::Remote(m) => OdxError::Remote(m.clone()),
            OdxError::Capacity => OdxError::Capacity,
            OdxError::ResponseTooLarge { limit_mib } => OdxError::ResponseTooLarge {
                limit_mib: *limit_mib,
            },
        }
    }
}

// SQLite errors may contain SQL values; never forward them to clients.
impl From<rusqlite::Error> for OdxError {
    fn from(_: rusqlite::Error) -> Self {
        Self::Persistence
    }
}

#[cfg(test)]
mod tests {
    use super::OdxError;

    #[test]
    fn ui_error_contract_uses_codes_and_safe_parameters() {
        let status = OdxError::HttpStatus { status: 503 };
        assert_eq!(status.code(), "upstream.http_status");
        assert_eq!(status.ui_params(), serde_json::json!({ "status": 503 }));
        assert_eq!(status.safe_detail(), None);

        let invalid = OdxError::Invalid("secret-like user input".into());
        assert_eq!(invalid.code(), "app.invalid_input");
        assert_eq!(invalid.safe_detail(), None);

        let uncertain = OdxError::Uncertain;
        assert_eq!(uncertain.code(), "upstream.uncertain_result");

        let validation = OdxError::validation_with_params(
            "validation.rule.limit_reached",
            serde_json::json!({ "limit": 128, "secret": "never-forward" }),
            "legacy text",
        );
        assert_eq!(validation.code(), "validation.rule.limit_reached");
        assert_eq!(validation.ui_params(), serde_json::json!({ "limit": 128 }));
        assert_eq!(validation.safe_detail(), None);
    }
}
