/** Plain text, or JSON to preserve numbers, booleans, arrays and objects. */
export function parseContextValue(text: string): unknown {
  const value = text.trim();
  if (/^[\[{\"]/.test(value) || /^(true|false|null)$/.test(value) || /^-?(0|[1-9]\d*)(\.\d+)?([eE][+-]?\d+)?$/.test(value)) {
    try { return JSON.parse(value); }
    catch { throw { code: "validation.context.json_invalid" }; }
  }
  return text;
}
export function formatContextValue(value: unknown): string {
  if (typeof value === "string") {
    try { if (parseContextValue(value) === value) return value; } catch { /* Preserve JSON-like text as a string. */ }
  }
  return JSON.stringify(value) ?? "";
}

// Odoo 19 context keys; modules may add others. Sources: odoo/orm/models.py, odoo/addons/base/models/ir_actions.py, addons/mail/models/mail_thread.py and Odoo view_architectures documentation.
export const contextKeys = [
  "active_id", "active_ids", "active_model", "active_test", "allowed_company_ids",
  "bin_size", "bin_size_", "default_", "group_by", "lang",
  "mail_create_nolog", "mail_create_nosubscribe", "mail_notrack", "mail_notify_author",
  "mail_notify_force_send", "search_default_", "tracking_disable", "tz",
];
