import { Component } from "react";
import type { ErrorInfo, ReactNode } from "react";

export class ErrorBoundary extends Component<
  { children: ReactNode },
  { error: Error | null }
> {
  state = { error: null as Error | null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("FixTrace UI boundary", error, info.componentStack);
  }

  render() {
    if (this.state.error) {
      return (
        <main className="fatal-error" role="alert">
          <div className="empty-mark">F</div>
          <h1>FixTrace could not render this view</h1>
          <p>{this.state.error.message}</p>
          <button className="accent-button" onClick={() => location.reload()}>
            Reload from Rust state
          </button>
        </main>
      );
    }
    return this.props.children;
  }
}
