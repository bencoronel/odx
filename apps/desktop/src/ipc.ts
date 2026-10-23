import { invoke } from "@tauri-apps/api/core";
export type WriteMode = "read_only" | "approval" | "temporary" | "permanent";
export type Environment = { id:string;name:string;url:string;database:string;login:string;protocol:string;detected_protocol?:string|null;context:Record<string,unknown>;write_mode:WriteMode;write_until?:string|null;has_secret:boolean;tags:string[] };
export type Pending = { id:string;client_id?:string|null;client_label?:string|null;environment_id:string;environment_name:string;model:string;method:string;args:unknown;kwargs:unknown;context:Record<string,unknown>;created_at:string;expires_at:string };
export type EnvironmentDraft = {name:string;url:string;database:string;login:string;protocol:"auto"|"legacy_jsonrpc"|"xmlrpc"|"json2";context:Record<string,unknown>;secret:string};
export type Health = { environment_id:string;status:"ok"|"unreachable"|"auth_failed";server_version?:string|null;protocol?:string|null;latency_ms:number;error?:string|null;error_code?:string|null;error_params?:Record<string,string|number>|null;http_status?:number|null;network_error?:string|null };
export type Capability = "odoo:read"|"odoo:write-request";
export type Client = { local_stdio?:boolean;id:string;label:string;environment_ids:string[];all_environments:boolean;last_used_at?:string|null;capabilities:Capability[];revoked:boolean;created_at:string;token_issued_at:string };
export type Service = { state:"listening"|"stopped"|"error";address?:string|null;error?:string|null };
export type ServiceStatus = { api:Service;mcp:Service;connector_command:string;connector_args:string[];api_url:string;connections:{client_id:string;client_label?:string|null;since:string}[] };
export type ActivityFilter = { environment_id?:string;client_id?:string;q?:string;writes_only?:boolean;errors_only?:boolean };
export type AuditPayload = { text:string;truncated:boolean };
export type Authorization = { kind:"individual" } | { kind:"rule";rule_id:string;rule_revision:number } | { kind:"free" };
export type AuditDetails = { request?:AuditPayload|null;response?:AuditPayload|null;error?:AuditPayload|null;error_code?:string|null;error_params?:Record<string,string|number>|null;duration_ms?:number|null;authorization?:Authorization|null };
export type RuleKind = "method" | "fields" | "values";
export type RuleDraft = { id?:string;environment_id:string;model:string;method:string;kind:RuleKind;fields?:string[];values?:Record<string,unknown>;context:Record<string,unknown>;max_records:number;expires_at?:string|null };
export type Rule = { id:string;environment_id:string;model:string;method:string;kind:RuleKind;fields?:string[];values?:Record<string,unknown>;context:Record<string,unknown>;max_records:number;expires_at:string|null;revision:number;created_at:string;expired:boolean };
export type ActivityEvent = { request_id?:string|null;details?:AuditDetails|null;at:string;environment_id:string;environment_name?:string|null;client_id?:string|null;client_label?:string|null;operation:string;outcome:string;write:boolean };
/** Last row position; the next page starts after it. */
export type ActivityCursor = { at:number;key:string };
export type ActivityPage = { events:ActivityEvent[];next_cursor?:ActivityCursor|null;complete:boolean };
/** `client_id`: an MCP client ran it through terminal_run; the core checks its grant and audits it under that client. */
export type ShellCall = { environment_id:string;model:string;method:string;args?:unknown[];kwargs?:Record<string,unknown>;context?:Record<string,unknown>;confirmed?:boolean;client_id?:string };
/** What an MCP agent asked of the window: run a Terminal command, or show a tab. */
export type AgentRequest =
  | { id:number;kind:"run";terminal?:string|null;envId:string;command:string;clientId:string;clientLabel:string;allowWrites:boolean }
  | { id:number;kind:"show";terminal:string };
export type ShellResult = { data:unknown;write:boolean;upstream_ms:number;upstream_bytes:number };
export type HistoryEntry = { at:string;environment?:string|null;command:string };
export type HeaderEntry = { name:string;value:string;enabled:boolean };
export type HeaderSettings = { headers:HeaderEntry[];inherited:HeaderEntry[] };
/** Same shape read and saved. */
export type ProxyInput = { name:string;url:string;username:string;password:string };
export type ProxyView = ProxyInput & { id:string;environments:number };
/** `default` is the id of the proxy environments use by default; null means direct. */
export type ProxiesView = { proxies:ProxyView[];default:string|null };
/** A proxy test that reached the server; `server_version` is null when it answered without one. */
export type ProxyTest = { latency_ms:number;server_version:string|null };
export type ProxyMode = "inherit"|"profile"|"direct";
export type EnvironmentProxyView = { mode:ProxyMode;id?:string|null };
export type EnvironmentProxyInput = { mode:"inherit" } | { mode:"direct" } | { mode:"profile";id:string };
export type ContextEntry = { name:string;value:unknown;enabled:boolean };
export type ContextSettings = { entries:ContextEntry[];inherited:Record<string,unknown> };
/** Settings, editable only in the desktop window. */
export type Settings = { global_concurrency:number;environment_concurrency:number;upstream_timeout_secs:number;approval_minutes:number;activity_days:number;activity_mib:number;output_hours:number;output_mib:number };
export type SettingsView = { settings:Settings;defaults:Settings;bounds:Record<keyof Settings,{min:number;max:number}> };
/** An agent CLI's registration of Odx: not installed, absent, current, or pointing at another executable path. */
export type AgentSetup = "no_cli" | "missing" | "added" | "outdated";
export type Agent = "claude" | "codex";
export const ipc = {
 openTableWindow:(table:unknown,title:string)=>invoke<void>("table_window_open",{table,title}),
 agentTake:()=>invoke<AgentRequest[]>("agent_take"), agentReply:(id:number,result?:unknown,error?:string)=>invoke<void>("agent_reply",{id,result:result ?? null,error:error ?? null}),
 agentSetup:(agent:Agent)=>invoke<AgentSetup>("agent_setup",{agent}), agentInstall:(agent:Agent)=>invoke<AgentSetup>("agent_install",{agent}),
 restart:()=>invoke<void>("desktop_restart"),
 settings:()=>invoke<SettingsView>("settings_get"), saveSettings:(settings:Settings)=>invoke<SettingsView>("settings_save",{settings}),
 faviconPage:(id:string)=>invoke<{url:string;html:string}>("environment_favicon_page",{id}),
 faviconImage:(id:string,url:string)=>invoke<{content_type:string;data:number[]}>("environment_favicon_image",{id,url}),
 context:(environmentId?:string)=>invoke<ContextSettings>("context_get",{environmentId:environmentId ?? null}),
 saveContext:(entries:ContextEntry[],environmentId?:string)=>invoke<void>("context_set",{environmentId:environmentId ?? null,entries}),
 headers:(environmentId?:string)=>invoke<HeaderSettings>("headers_get",{environmentId:environmentId ?? null}),
 saveHeaders:(headers:HeaderEntry[],environmentId?:string)=>invoke<void>("headers_set",{environmentId:environmentId ?? null,headers}),
 proxies:()=>invoke<ProxiesView>("proxies_get"), saveProxy:(id:string|null,proxy:ProxyInput)=>invoke<string>("proxy_save",{id,proxy}),
 deleteProxy:(id:string)=>invoke<void>("proxy_delete",{id}), testProxy:(id:string|null,proxy:ProxyInput,url:string)=>invoke<ProxyTest>("proxy_test",{id,proxy,url}),
 tagColors:()=>invoke<Record<string,string>>("tag_colors_get"), setTagColor:(tag:string,color:string|null)=>invoke<void>("tag_color_set",{tag,color}), setDefaultProxy:(id:string|null)=>invoke<void>("proxy_default_set",{id}),
 environmentProxy:(id:string)=>invoke<EnvironmentProxyView>("environment_proxy_get",{id}), saveEnvironmentProxy:(id:string,proxy:EnvironmentProxyInput)=>invoke<void>("environment_proxy_set",{id,proxy}),
 status:()=>invoke<string>("vault_status"), create:(password:string)=>invoke<void>("vault_create",{password}), unlock:(password:string)=>invoke<void>("vault_unlock",{password}), changePassword:(currentPassword:string,newPassword:string)=>invoke<void>("vault_change_password",{currentPassword,newPassword}), lock:()=>invoke<void>("vault_lock"), reset:()=>invoke<void>("vault_reset"),
 environments:()=>invoke<Environment[]>("environments_list"), saveEnvironment:(draft:EnvironmentDraft)=>invoke<Environment>("environment_save",{draft}), updateEnvironment:(id:string,draft:EnvironmentDraft)=>invoke<Environment>("environment_update",{id,draft}), cloneEnvironment:(id:string,newName:string)=>invoke<Environment>("environment_clone",{id,newName}), deleteEnvironment:(id:string,confirmed:boolean)=>invoke<void>("environment_delete",{id,confirmed}), openUrl:(id:string,record?:{model:string;id:number})=>invoke<void>("environment_open_url",{id,record:record ?? null}), check:(id:string)=>invoke<Health>("environment_check",{id}), checkDraft:(draft:EnvironmentDraft,proxy?:EnvironmentProxyInput)=>invoke<Health>("environment_check_draft",{draft,proxy:proxy ?? null}), databases:(url:string)=>invoke<string[]>("environment_databases",{url}), setTags:(id:string,tags:string[])=>invoke<Environment>("environment_tags_set",{id,tags}), mode:(id:string,mode:WriteMode,minutes?:number)=>invoke<void>("environment_write_mode",{id,mode,minutes}),
 serviceStatus:()=>invoke<ServiceStatus>("service_status"), activity:(cursor:ActivityCursor|null,filter:ActivityFilter)=>invoke<ActivityPage>("activity_list",{cursor,filter}), shell:(call:ShellCall)=>invoke<ShellResult>("shell_call",{call}), shellHistory:()=>invoke<HistoryEntry[]>("shell_history_list"), shellHistoryAdd:(command:string,environment?:string)=>invoke<void>("shell_history_add",{command,environment}), shellHistoryClear:()=>invoke<void>("shell_history_clear"), shellExport:(name:string,content:string)=>invoke<string>("shell_export",{name,content}), shellSessions:()=>invoke<string|null>("shell_sessions_load"), saveShellSessions:(data:string)=>invoke<void>("shell_sessions_save",{data}), clients:()=>invoke<Client[]>("clients_list"), updateClient:(id:string,label:string,environmentIds:string[]|null,capabilities:Capability[])=>invoke<Client>("client_update",{id,label,environmentIds,capabilities}),
 approvals:()=>invoke<Pending[]>("approvals_list"), decide:(id:string,approve:boolean,reason?:string)=>invoke<unknown>("approval_decide",{id,approve,reason:reason || null}),
 rules:(environmentId?:string)=>invoke<Rule[]>("rules_list",{environmentId:environmentId ?? null}), saveRule:(draft:RuleDraft)=>invoke<Rule>("rule_save",{draft}), deleteRule:(id:string)=>invoke<void>("rule_delete",{id})
};
