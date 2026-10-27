import type { ComponentProps } from "react";

/** Shared primitives as Tailwind utilities. Nothing merges conflicting utilities, so a variation is a prop: callers add only
 * classes the primitive does not set, or a longhand of one it sets (`pt-*` over `py-*`, `w-*` over `size-*`), which Tailwind
 * always sorts later. A bare old class name kept in a string (`status-dot`, `chip`...) is a hook other code selects. */
export const cx = (...parts: (string | false | null | undefined)[]) => parts.filter(Boolean).join(" ");

/** Literal names, so Tailwind also generates the tone-* utilities that callers build as `tone-${x}`. */
export const tones = { ok: "tone-ok", info: "tone-info", warn: "tone-warn", danger: "tone-danger", neutral: "tone-neutral", accent: "tone-accent" } as const;
export type Tone = keyof typeof tones;

const buttonVariants = {
  default: "border-line-strong bg-surface text-fg shadow-xs enabled:hover:bg-hover",
  primary: "border-accent bg-accent text-accent-fg shadow-xs enabled:hover:bg-accent-hover enabled:hover:border-accent-hover",
  danger: "border-danger bg-danger text-white shadow-xs enabled:hover:brightness-108",
  dangerGhost: "border-line-strong bg-surface text-danger shadow-xs enabled:hover:bg-danger-soft enabled:hover:border-danger",
  ghost: "border-transparent bg-transparent text-fg-2 enabled:hover:bg-hover enabled:hover:text-fg",
  /** A toggle whose state is on without being pressed, such as a filter button with filters applied. */
  accent: "border-accent bg-surface text-accent shadow-xs enabled:hover:bg-hover",
};
const buttonSizes = { sm: "h-6 px-2 text-xs [&_.icon]:size-[13px]", md: "h-7 px-3 text-sm", lg: "h-8 px-3 text-sm" };
const iconOnlySizes = { sm: "size-6 text-xs [&_.icon]:size-[13px]", md: "size-7 text-sm", lg: "size-8 text-sm" };
export type ButtonStyle = { variant?: keyof typeof buttonVariants; size?: keyof typeof buttonSizes; iconOnly?: boolean; full?: boolean };
export const button = ({ variant = "default", size = "md", iconOnly, full }: ButtonStyle = {}) => cx(
  "inline-flex items-center justify-center gap-1.5 border rounded-md font-medium whitespace-nowrap cursor-pointer transition-[background-color,border-color,color] duration-120 disabled:opacity-45 disabled:cursor-not-allowed aria-pressed:border-accent aria-pressed:text-accent-text aria-pressed:bg-accent-soft",
  buttonVariants[variant], (iconOnly ? iconOnlySizes : buttonSizes)[size], full && "w-full");
/** Defaults to type="button": a submit button says so. */
export function Button({ variant, size, iconOnly, full, className, type = "button", ...props }: ButtonStyle & ComponentProps<"button">) {
  return <button type={type} className={cx(button({ variant, size, iconOnly, full }), className)} {...props} />;
}

export const iconButton = (size: "sm" | "md" = "md") => cx("inline-grid place-items-center rounded-md text-fg-2 cursor-pointer enabled:hover:bg-hover enabled:hover:text-fg disabled:opacity-45 disabled:cursor-not-allowed",
  size === "sm" ? "size-5 [&_.icon]:size-[13px]" : "size-7");
export function IconButton({ size, className, type = "button", ...props }: { size?: "sm" | "md" } & ComponentProps<"button">) {
  return <button type={type} className={cx(iconButton(size), className)} {...props} />;
}

/** A row of toggles; each Segment shows its state through aria-pressed. */
export function Segmented({ className, ...props }: ComponentProps<"div">) {
  return <div className={cx("inline-flex gap-0.5 p-0.5 border border-line rounded-md bg-surface-2", className)} {...props} />;
}
export function Segment({ className, type = "button", ...props }: ComponentProps<"button">) {
  return <button type={type} className={cx("inline-flex items-center gap-1.5 h-[22px] px-2.5 rounded-sm text-fg-2 text-sm cursor-pointer enabled:hover:text-fg disabled:opacity-55 disabled:cursor-default aria-pressed:bg-surface aria-pressed:text-fg aria-pressed:shadow-[var(--shadow-sm),0_0_0_1px_var(--border)] [&_.icon]:size-[13px]", className)} {...props} />;
}

/** A label above its control; every direct <span> child gets the label's look. */
export const field = "grid gap-[5px] content-start min-w-0 [&>span]:text-sm [&>span]:font-medium [&>span]:text-fg-2";
export const searchIcon = "absolute left-2 top-1/2 -translate-y-1/2 size-[13px] text-fg-3 pointer-events-none";
export const searchInput = "pl-[27px] h-6 text-sm";
/** Masked secrets; on an <input> only the spacing and cursor showed before, since the input's own color won. */
export const secretMask = "text-fg-2 tracking-[.12em] cursor-default";
/** Inputs inside a property list while editing; `!` beats the input and Select styles they sit on. */
export const inlineInput = (mono?: boolean) => cx("h-[26px]! -ml-[9px] w-[calc(100%+9px)]!", mono ? "font-mono text-sm!" : "text-base!");

const pill = "inline-flex items-center h-5 rounded-sm text-xs whitespace-nowrap flex-none";
export const chip = ({ tone, token }: { tone?: Tone; token?: boolean } = {}) => cx("chip", pill, "font-medium border",
  tone ? cx(tones[tone], "text-tone bg-tone-soft border-transparent") : "text-fg-2 bg-surface-2 border-line",
  token ? "gap-[3px] pl-1.5 pr-0.5 cursor-default [&_.icon]:static [&_.icon]:transform-none [&_.icon]:size-[11px] [&_.icon]:text-inherit [&_button]:inline-grid [&_button]:place-items-center [&_button]:size-4 [&_button]:rounded-sm [&_button]:opacity-70 [&_button]:cursor-pointer [&_button:hover]:opacity-100 [&_button:hover]:bg-hover"
    : "gap-[5px] px-[7px] [&_.icon]:size-3");
export function Chip({ tone, token, className, ...props }: { tone?: Tone; token?: boolean } & ComponentProps<"span">) {
  return <span className={cx(chip({ tone, token }), className)} {...props} />;
}
export const count = (tone?: Tone) => cx(pill, "justify-center gap-[5px] min-w-5 px-[5px] font-medium tabular-nums", tone ? cx(tones[tone], "text-tone bg-tone-soft") : "text-fg-3 bg-hover");
/** Without a tone of its own it takes the nearest tone-* ancestor's, or neutral colors. */
export const countdown = (tone?: Tone) => cx(pill, tone && tones[tone], "gap-[5px] px-[7px] font-medium font-mono tabular-nums text-[color:var(--tone,var(--text-2))] bg-[color:var(--tone-soft,var(--surface-2))] [&_.icon]:size-3");
export const method = (tone?: Tone) => cx(pill, tone && tones[tone], "gap-[5px] px-[7px] font-mono font-semibold text-tone bg-tone-soft");

const dotKinds = {
  halo: "size-[7px] shadow-[0_0_0_3px_var(--tone-soft)]",
  small: "size-1.5 shadow-none",
  /** On an avatar's corner, ringed with the list's background. */
  badge: "absolute -right-[3px] -bottom-[3px] size-[9px] shadow-[0_0_0_2px_var(--bg)]",
};
export function StatusDot({ tone, kind = "halo", className, ...props }: { tone?: Tone; kind?: keyof typeof dotKinds } & ComponentProps<"span">) {
  return <span className={cx("status-dot rounded-full bg-tone flex-none", dotKinds[kind], tone && tones[tone], className)} {...props} />;
}
/** An environment's favicon with a `badge` StatusDot. */
export const envAvatar = (small?: boolean) => cx("relative grid place-items-center flex-none border border-line rounded-md bg-surface-2 text-fg-3 [&_.icon]:size-3.5",
  small ? "size-[22px] [&_.favicon]:size-3.5" : "size-[26px] [&_.favicon]:size-4");
export const tile = (tone?: Tone) => cx("grid place-items-center size-8 flex-none border rounded-lg [&_.icon]:size-4 [&_.favicon]:size-[18px]",
  tone ? cx(tones[tone], "text-tone bg-tone-soft border-transparent") : "border-line bg-surface-2 text-accent-text");
export const titleWarn = "title-warn tone-warn relative inline-grid place-items-center size-[18px] rounded-full text-tone cursor-help hover:bg-tone-soft focus-visible:bg-tone-soft [&>.icon]:size-[13px] [&_.tooltip]:-left-1.5 [&_.tooltip]:normal-case [&_.tooltip]:tracking-[normal]";

const bannerTones = {
  danger: "text-danger bg-danger-soft border-danger/35",
  ok: "text-ok bg-ok-soft border-ok/35",
  warn: "text-warn bg-warn-soft border-warn/35",
  info: "text-fg-2 bg-surface-2 border-line [&>.icon]:text-info",
};
export type BannerStyle = { tone: keyof typeof bannerTones; compact?: boolean; page?: boolean };
/** `page`: a notice across the top of the window. */
export const banner = ({ tone, compact, page }: BannerStyle) => cx("flex gap-2 border rounded-md wrap-anywhere [&>.icon]:mt-px", bannerTones[tone],
  compact ? "px-[9px] py-1.5 text-xs" : "px-2.5 py-2 text-sm", page ? "items-center mx-5 mt-2.5" : "items-start has-[>button]:items-center");
export function Banner({ tone, compact, page, className, ...props }: BannerStyle & ComponentProps<"p">) {
  return <p className={cx(banner({ tone, compact, page }), className)} {...props} />;
}

/** Cards. Popups inside (mode picker, warnings, header suggestions) need the card not to clip them. */
export const panel = "border border-line rounded-lg bg-surface overflow-hidden has-[.mode-select,.title-warn,.header-input]:overflow-visible";
export function PanelHeader({ className, ...props }: ComponentProps<"header">) {
  return <header className={cx("flex items-center justify-between gap-4 min-h-[44px] px-3.5 py-1.5 border-b border-line last:border-b-0 [&_h3]:text-fg [&_h3]:text-base [&_h3]:tracking-normal [&_h3]:normal-case", className)} {...props} />;
}
export const panelBody = "panel-body px-3.5 py-3 border-line [.setting-row+&]:border-t";
export const panelSummary = "flex-[0_1_auto] self-center min-w-0 ml-auto truncate text-fg-3 text-sm";
/** A label and description on the left, its control on the right; rows after another row or a body get a divider. */
export const settingRow = "setting-row flex items-center justify-between gap-6 min-h-[52px] px-3.5 py-2.5 border-line [.setting-row+&,.panel-body+&]:border-t [footer&]:justify-end max-sm:flex-wrap [&_strong]:text-base [&_strong]:font-medium [&>div:not(:last-child)]:flex-1 [&>div:not(:last-child)]:grid [&>div:not(:last-child)]:gap-0.5 [&>div:not(:last-child)]:min-w-0 [&>:last-child:not(:first-child)]:flex-none";
const propsBase = "grid [&>div]:grid [&>div]:items-center [&>div]:gap-3 [&>div]:border-t [&>div]:border-line [&>div:first-child]:border-t-0 [&_dt]:flex [&_dt]:items-center [&_dt]:gap-[7px] [&_dt]:text-fg-3 [&_dt]:text-sm [&_dt_.icon]:size-[13px] [&_dd]:text-base [&_dd]:wrap-anywhere [&_dd.mono]:text-sm";
/** A <dl> of <div><dt/><dd/></div> rows. */
export function Props({ compact, editing, className, ...props }: { compact?: boolean; editing?: boolean } & ComponentProps<"dl">) {
  return <dl className={cx(propsBase, compact ? "border border-line rounded-md [&>div]:grid-cols-[100px_minmax(0,1fr)] [&>div]:min-h-[30px] [&>div]:px-2.5 [&>div]:py-1"
    : cx("[&>div]:grid-cols-[140px_minmax(0,1fr)] max-sm:[&>div]:grid-cols-[110px_minmax(0,1fr)] [&>div]:min-h-[34px] [&>div]:px-3", editing ? "[&>div]:py-[3px]" : "[&>div]:py-1.5"), className)} {...props} />;
}
export const valuesTables = "grid gap-2 px-3 py-2.5 border-t border-line";
export const valuesTable = "values-table w-full text-sm [&_caption]:text-left [&_caption]:pb-1 [&_caption]:text-fg-3 [&_caption]:text-xs [&_:is(th,td)]:px-2 [&_:is(th,td)]:py-[5px] [&_:is(th,td)]:border-b [&_:is(th,td)]:border-line [&_:is(th,td)]:text-left [&_:is(th,td)]:align-top [&_thead_th]:text-fg-3 [&_thead_th]:text-xs [&_thead_th]:font-medium [&_tbody_th]:w-[30%] [&_tbody_th]:font-medium [&_tbody_th]:text-fg-2 [&_tbody_th]:wrap-anywhere [&_code]:text-fg [&_code]:whitespace-pre-wrap [&_code]:wrap-anywhere [&_tr:last-child>*]:border-b-0";
export const empty = "empty flex-1 flex flex-col items-center justify-center gap-2 px-6 py-10 text-center [&_p]:max-w-[380px] [&_p]:text-fg-2 [&_p]:text-sm";
export const emptyIcon = (tone?: Tone) => cx("grid place-items-center size-11 mb-1 rounded-xl [&_.icon]:size-5", tone ? cx(tones[tone], "bg-tone-soft text-tone") : "bg-accent-soft text-accent-text");

/** List and detail panes (Environments, Approvals, Activity). */
export const split = "flex-1 min-h-0 grid grid-cols-[var(--list-w,300px)_minmax(0,1fr)] max-md:grid-cols-[min(var(--list-w,300px),240px)_minmax(0,1fr)] max-sm:grid-cols-1 max-sm:grid-rows-[auto_1fr]";
export const listPane = "relative flex flex-col min-h-0 border-r border-line bg-bg max-sm:border-r-0 max-sm:border-b max-sm:max-h-[180px]";
export const listToolbar = "relative grid gap-1.5 p-2 border-b border-line";
/** Row rules have zero specificity (`:where`), so a row's own utilities win as they did over the old CSS; the dot rings do not. */
export const envList = "flex-1 overflow-auto p-1 [:where(&_li)]:flex [:where(&_li)]:items-center [:where(&_li)]:gap-2.5 [:where(&_li)]:px-2 [:where(&_li)]:py-[7px] [:where(&_li)]:rounded-md [:where(&_li)]:cursor-pointer [:where(&_li:hover:not([aria-selected=true]))]:bg-hover [:where(&_li[aria-selected=true])]:bg-active [&_li[aria-selected=true]_strong]:text-fg [&_li[aria-selected=true]_.status-dot]:shadow-[0_0_0_2px_var(--active)] [&_li:hover:not([aria-selected=true])_.status-dot]:shadow-[0_0_0_2px_var(--hover)]";
export const envListText = "grid min-w-0 [&_strong]:text-base [&_strong]:font-medium [&_strong]:text-fg [&_strong]:truncate [&_small]:text-xs [&_small]:text-fg-3 [&_small]:font-mono [&_small]:truncate";
export const listEmpty = "grid justify-items-center gap-1 px-4 py-7 text-fg-3 text-sm leading-[1.45] text-center cursor-default hover:bg-transparent [&_strong]:text-fg-2 [&_strong]:text-base [&_strong]:font-medium";
export const listEmptyIcon = "grid place-items-center size-9 mb-1.5 rounded-[10px] bg-surface-2 text-fg-3 [&_.icon]:size-4";
/** Hidden tooltips near the edge must not add a horizontal scrollbar. */
export const detailPane = "detail-pane min-w-0 overflow-x-hidden overflow-y-auto";
export const detail = "detail @container grid grid-cols-[minmax(0,1fr)] gap-3 min-w-0 max-w-[760px] px-5 pt-4 pb-6 has-[.headers-table]:pb-[104px]";
export const detailHeader = "detail-header flex flex-wrap items-center gap-x-3 gap-y-2 pb-1";

export const filterBadge = "absolute -top-[5px] -right-[5px] grid place-items-center min-w-3.5 h-3.5 px-[3px] rounded-[7px] bg-accent text-accent-fg text-3xs font-semibold";
export const filterPanel = "absolute z-20 top-[calc(100%-4px)] left-2 right-2 grid border border-line-strong rounded-md bg-surface shadow-pop";
export const filterSection = "filter-section grid gap-1 p-2 border-line [.filter-section+&]:border-t";
export const filterHeading = "px-1 text-2xs font-semibold tracking-[.06em] uppercase text-fg-3";
export const filterOptions = "max-h-[220px] overflow-auto";
export const filterOption = "flex items-center gap-2 min-w-0 p-1 rounded-sm text-sm text-fg-2 cursor-pointer hover:bg-hover hover:text-fg";
export const filterCount = "ml-auto text-xs text-fg-3 tabular-nums";

export const selectOption = (active?: boolean) => cx("flex items-center min-h-7 px-2 py-1 rounded-sm text-sm leading-[1.35] wrap-anywhere cursor-pointer hover:bg-accent-soft hover:text-accent-text aria-selected:text-fg aria-selected:font-medium",
  active ? "bg-accent-soft text-accent-text" : "text-fg-2");
const termMenuPlaces = {
  prompt: "left-3.5 bottom-[42px] min-w-[260px] max-w-[calc(100%-28px)] max-h-[194px]",
  suggestions: "header-suggestions left-0 top-[calc(100%+3px)] min-w-full max-h-[168px]",
  table: "grid right-0 bottom-[calc(100%+4px)] min-w-[210px] max-w-[calc(100%-28px)] max-h-[194px]",
};
/** Items are <li>s; as in envList their own utilities win. */
export const termMenu = (place: keyof typeof termMenuPlaces) => cx("absolute z-5 overflow-auto p-[3px] border border-line-strong rounded-md bg-surface shadow-pop [:where(&_li)]:flex [:where(&_li)]:items-baseline [:where(&_li)]:gap-2.5 [:where(&_li)]:h-[21px] [:where(&_li)]:px-1.5 [:where(&_li)]:rounded-sm [:where(&_li)]:leading-[21px] [:where(&_li)]:cursor-pointer [:where(&_li[aria-selected=true])]:bg-accent-soft [&_li[aria-selected=true]_.term-menu-value]:text-accent-text", termMenuPlaces[place]);
export const termMenuValue = "term-menu-value text-fg whitespace-nowrap";
export const termMenuDetail = "min-w-0 truncate text-fg-3 font-sans text-xs";
/** Menus of <button>s (MenuItems); a button marked `text-danger` keeps that color. */
export const contextMenu = "fixed z-1000 grid min-w-[190px] p-[3px] border border-line-strong rounded-md bg-surface shadow-pop [&_button]:flex [&_button]:items-center [&_button]:justify-between [&_button]:gap-4 [&_button]:h-[26px] [&_button]:px-2 [&_button]:rounded-sm [&_button:not(.text-danger,:disabled)]:text-fg [&_button]:text-sm [&_button]:text-left [&_button]:cursor-pointer [&_button:hover:not(:disabled)]:bg-hover [&_button:focus-visible]:bg-hover [&_button:disabled:not(.text-danger)]:text-fg-3 [&_button:disabled]:cursor-default [&_kbd]:border-0 [&_kbd]:bg-transparent [&_kbd]:p-0 [&_kbd]:text-fg-3 [&_kbd]:text-xs [&_hr]:mx-1 [&_hr]:my-[3px] [&_hr]:border-line";
export const moreMenu = "fixed z-120 grid min-w-[200px] p-[3px] border border-line-strong rounded-md bg-surface shadow-pop [&_button]:flex [&_button]:items-center [&_button]:gap-2 [&_button]:h-[30px] [&_button]:px-2 [&_button]:rounded-sm [&_button:not(.text-danger)]:text-fg-2 [&_button]:text-sm [&_button]:text-left [&_button]:cursor-pointer [&_button:is(:hover,:focus-visible)]:bg-hover [&_button:not(.text-danger):is(:hover,:focus-visible)]:text-fg [&_.icon]:size-3.5";

export const appWindow = "h-dvh flex flex-col overflow-hidden rounded-[10px] bg-bg border border-line [:root[data-window-filled]_&]:rounded-none [:root[data-window-filled]_&]:border-transparent";
/** Sidebar and Settings navigation; the label <span> gets `flex-1 max-md:hidden` so narrow windows show icons only. */
export const navItem = "flex items-center gap-[9px] w-full h-7 px-2 rounded-md text-fg-2 text-base text-left cursor-pointer enabled:hover:bg-hover enabled:hover:text-fg aria-[current=page]:bg-active aria-[current=page]:text-fg aria-[current=page]:font-medium aria-[current=page]:[&_.icon]:text-accent-text disabled:opacity-55 disabled:cursor-default max-md:relative max-md:justify-center max-md:p-0";
