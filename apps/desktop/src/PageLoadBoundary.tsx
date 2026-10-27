import { Component, Suspense, type ErrorInfo, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Button, banner } from "./ui";

class Boundary extends Component<{ children: ReactNode; reloadLabel: string; failedMessage: string }, { failed: boolean }> {
  state = { failed: false };

  static getDerivedStateFromError() { return { failed: true }; }
  componentDidCatch(_error: Error, _info: ErrorInfo) { /* Offer reload without exposing loader details. */ }

  render() {
    if (this.state.failed) return <div className={banner({ tone: "danger" })} role="alert">
      <span className="grow">{this.props.failedMessage}</span>
      <Button size="sm" onClick={() => window.location.reload()}>{this.props.reloadLabel}</Button>
    </div>;
    return this.props.children;
  }
}

export function PageLoadBoundary({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  return <Boundary reloadLabel={t("common.retry")} failedMessage={t("app.errors.pageLoadFailed")}><Suspense fallback={null}>{children}</Suspense></Boundary>;
}
