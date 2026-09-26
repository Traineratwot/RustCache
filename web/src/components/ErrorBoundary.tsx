/**
 * React error boundary. Prevents a render crash from blanking the whole SPA.
 */
import { Component, type ErrorInfo, type ReactNode } from "react";

interface Props {
  children: ReactNode;
  /** Optional custom fallback. Receives the error and a reset callback. */
  fallback?: (error: Error, reset: () => void) => ReactNode;
}

interface State {
  error: Error | null;
}

/**
 * Catches render/lifecycle errors in its subtree and shows a recoverable
 * fallback instead of a white screen.
 */
export class ErrorBoundary extends Component<Props, State> {
  override state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  override componentDidCatch(error: Error, info: ErrorInfo): void {
    // Keep a breadcrumb in the console; no telemetry pipeline is wired.
    console.error("UI error boundary caught:", error, info.componentStack);
  }

  reset = (): void => {
    this.setState({ error: null });
  };

  override render(): ReactNode {
    const { error } = this.state;
    if (!error) return this.props.children;
    if (this.props.fallback) return this.props.fallback(error, this.reset);
    return (
      <div style={{ padding: "1.5rem", maxWidth: "40rem", margin: "2rem auto" }}>
        <h2>UI error</h2>
        <p style={{ whiteSpace: "pre-wrap" }}>{error.message}</p>
        <button type="button" onClick={this.reset}>
          Retry
        </button>
      </div>
    );
  }
}
