import type { Pending } from "./ipc";
import { useTranslation } from "react-i18next";

export function methodTone(method: string) {
  return method === "unlink" ? "danger" : ["create", "write"].includes(method) ? "warn" : "info";
}

export function CallDetails({ item }: { item: Pick<Pending, "args" | "kwargs" | "context"> }) {
  const { t } = useTranslation();
  return <dl className="call">
    <dt>{t("approvals.arguments")}</dt><dd><code>{JSON.stringify(item.args)}</code></dd>
    <dt>{t("approvals.namedArguments")}</dt><dd><code>{JSON.stringify(item.kwargs)}</code></dd>
    <dt>{t("approvals.context")}</dt><dd><code>{JSON.stringify(item.context)}</code></dd>
  </dl>;
}
