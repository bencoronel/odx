pub mod activity;
pub mod approval;
pub mod audit;
pub mod clients;
pub mod config;
pub mod context;
pub mod error;
pub mod execution;
pub mod favicon;
pub mod headers;
pub mod limits;
pub mod odoo;
pub mod output;
pub mod policy;
pub mod proxy;
pub mod security;
pub mod shell;
pub mod tools;
pub mod transports;
pub mod vault;

pub use activity::{ActivityCursor, ActivityEvent, ActivityFilter, ActivityPage};
pub use clients::{Capability, ClientIdentity, ClientScope, ClientView, IssuedToken};
pub use config::{Environment, EnvironmentDraft, Protocol, Settings, WriteMode};
pub use execution::{
    ApprovalNotice, ConnectionGuard, Core, EnvironmentView, HealthReport, McpConnectionInfo,
    PendingApproval, SettingsView,
};
pub use output::{Format, Outcome, ToolError, ToolOutput};
pub use policy::{Authorization, Rule, RuleDraft, RuleKind, RuleView};
pub use shell::{HistoryEntry, ShellCall, ShellResult};
pub use tools::catalog::{ToolDefinition, tool_definitions};
pub use tools::terminal::DesktopRequest;

mod database;
