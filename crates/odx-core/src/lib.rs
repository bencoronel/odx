// Unit tests include tests/support (an integration-test module), which names this crate odx_core.
#[cfg(test)]
extern crate self as odx_core;

pub(crate) mod activity;
pub(crate) mod approval;
pub mod audit;
pub(crate) mod clients;
pub(crate) mod config;
pub mod context;
pub(crate) mod environments;
pub mod error;
pub(crate) mod execution;
pub mod favicon;
pub mod headers;
pub mod limits;
pub(crate) mod odoo;
pub mod output;
pub mod policy;
pub mod proxy;
pub mod security;
pub(crate) mod shell;
pub mod tools;
pub mod transports;
pub mod vault;

pub use activity::{ActivityCursor, ActivityEvent, ActivityFilter, ActivityPage};
pub use approval::{ApprovalNotice, PendingApproval};
pub use clients::{
    AgentProcess, Capability, ClientIdentity, ClientScope, ClientView, ConnectionGuard,
    IssuedToken, McpConnectionInfo,
};
pub use config::{Environment, EnvironmentDraft, Protocol, Settings, WriteMode};
pub use environments::{EnvironmentView, HealthReport};
pub use execution::{Core, SettingsView};
pub use output::{Format, Outcome, ToolError, ToolOutput};
pub use policy::{Authorization, Rule, RuleDraft, RuleKind, RuleView};
pub use shell::{HistoryEntry, ShellCall, ShellResult};
pub use tools::catalog::{ToolDefinition, tool_definitions};
pub use tools::terminal::DesktopRequest;

mod database;
