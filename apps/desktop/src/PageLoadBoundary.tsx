import { Component, Suspense, type ErrorInfo, type ReactNode } from "react";
import { useTranslation } from "react-i18next";

class Boundary extends Component<{ children: ReactNode; reloadLabel: string; failedMessage: string }, { failed: boolean }> {
  state = { failed: false };

  static getDerivedStateFromError() { return { failed: true }; }
  componentDidCatch(_error: Error, _info: ErrorInfo) { /* Offer reload without exposing loader details. */ }

  render() {
    if (this.state.failed) return <div className="banner danger" role="alert">
      <span className="grow">{this.props.failedMessage}</span>
      <button type="button" className="btn sm" onClick={() => window.location.reload()}>{this.props.reloadLabel}</button>
    </div>;
    return this.props.children;
  }
}

export function PageLoadBoundary({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  return <Boundary reloadLabel={t("common.retry")} failedMessage={t("app.errors.pageLoadFailed")}><Suspense fallback={null}>{children}</Suspense></Boundary>;
}
