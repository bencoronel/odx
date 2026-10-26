import { currentLanguage } from "./index";

export function currentLocale(): string {
  return currentLanguage() === "es" ? "es-ES" : "en-US";
}

// Building a formatter costs far more than using one, and tables format every cell: keep one per locale and options.
const formatters = new Map<string, Intl.NumberFormat | Intl.DateTimeFormat>();
function cached<T extends Intl.NumberFormat | Intl.DateTimeFormat>(kind: string, options: object | undefined, make: (locale: string) => T): T {
  const locale = currentLocale();
  const key = `${kind}|${locale}|${JSON.stringify(options ?? {})}`;
  let formatter = formatters.get(key) as T | undefined;
  if (!formatter) formatters.set(key, formatter = make(locale));
  return formatter;
}

export function formatNumber(value: number, options?: Intl.NumberFormatOptions): string {
  return cached("n", options, (locale) => new Intl.NumberFormat(locale, options)).format(value);
}

export function formatDateTime(value: Date | number, options?: Intl.DateTimeFormatOptions): string {
  return cached("d", options, (locale) => new Intl.DateTimeFormat(locale, options)).format(value);
}

export function formatRelativeTime(value: number, unit: Intl.RelativeTimeFormatUnit, options?: Intl.RelativeTimeFormatOptions): string {
  return new Intl.RelativeTimeFormat(currentLocale(), { numeric: "auto", ...options }).format(value, unit);
}
