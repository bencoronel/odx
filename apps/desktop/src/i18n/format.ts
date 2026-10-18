import { currentLanguage } from "./index";

export function currentLocale(): string {
  return currentLanguage() === "es" ? "es-ES" : "en-US";
}

export function formatNumber(value: number, options?: Intl.NumberFormatOptions): string {
  return new Intl.NumberFormat(currentLocale(), options).format(value);
}

export function formatDateTime(value: Date | number, options?: Intl.DateTimeFormatOptions): string {
  return new Intl.DateTimeFormat(currentLocale(), options).format(value);
}

export function formatRelativeTime(value: number, unit: Intl.RelativeTimeFormatUnit, options?: Intl.RelativeTimeFormatOptions): string {
  return new Intl.RelativeTimeFormat(currentLocale(), { numeric: "auto", ...options }).format(value, unit);
}
