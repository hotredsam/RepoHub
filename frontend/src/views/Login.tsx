import { useCallback, useEffect, useState } from "react";
import { getAuthStatus } from "../lib/api";
import type { AuthStatus } from "../lib/types";

// Login.tsx (P18) — full-screen sign-in card for remote (tailnet) access.
//
// When the app is reached over the tailnet and auth is required, the gate
// redirects unauthenticated requests here. The single action kicks off the
// Google OAuth auth-code flow by navigating the browser to the backend's
// login endpoint (a real navigation, not a fetch — the backend issues a 302
// to Google and sets the session cookie on the callback).
//
// We surface the email allowlist as a hint so the user knows which Google
// account to pick, and degrade gracefully: if Google OAuth is not configured
// (no client id), there is nothing to sign in with, so we explain that the
// app is simply running locally/unauthenticated.
//
// Reads GET /api/auth/status via getAuthStatus(); status.configured is the
// Google-OAuth-configured flag.

export default function Login() {
  const [status, setStatus] = useState<AuthStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(() => {
    setLoading(true);
    setError(null);
    getAuthStatus()
      .then(setStatus)
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  // A full navigation — the backend redirects to Google and, on callback,
  // sets the session cookie and bounces back to the app.
  const signIn = () => {
    window.location.href = "/api/auth/google/login";
  };

  const configured = status?.configured ?? false;
  const allowed = status?.allowed_emails ?? [];

  return (
    <div className="flex min-h-screen items-center justify-center bg-slate-950 px-4">
      <div className="w-full max-w-md rounded-2xl border border-edge bg-panel p-8 shadow-xl">
        <div className="text-center">
          <h1 className="text-2xl font-semibold tracking-tight text-slate-100">
            RepoHub
          </h1>
          <p className="mt-1 text-sm text-slate-400">
            Sign in to access the dashboard remotely.
          </p>
        </div>

        {loading ? (
          <p className="mt-8 text-center text-sm text-slate-400">
            Checking sign-in…
          </p>
        ) : error ? (
          <div className="mt-8 flex flex-col items-center gap-3">
            <p className="rounded-md bg-rose-500/10 px-3 py-2 text-center text-sm text-rose-400">
              {error}
            </p>
            <button
              onClick={load}
              className="rounded border border-edge px-3 py-1 text-xs text-slate-300 transition hover:bg-edge"
            >
              Retry
            </button>
          </div>
        ) : configured ? (
          <div className="mt-8 space-y-4">
            <button
              onClick={signIn}
              className="flex w-full items-center justify-center gap-2 rounded-md bg-accent/20 px-4 py-2.5 text-sm font-medium text-accent transition hover:bg-accent/30"
            >
              {/* Google "G" mark */}
              <svg
                viewBox="0 0 48 48"
                className="h-5 w-5"
                aria-hidden="true"
                focusable="false"
              >
                <path
                  fill="#EA4335"
                  d="M24 9.5c3.54 0 6.71 1.22 9.21 3.6l6.85-6.85C35.9 2.38 30.47 0 24 0 14.62 0 6.51 5.38 2.56 13.22l7.98 6.19C12.43 13.72 17.74 9.5 24 9.5z"
                />
                <path
                  fill="#4285F4"
                  d="M46.98 24.55c0-1.57-.15-3.09-.38-4.55H24v9.02h12.94c-.58 2.96-2.26 5.48-4.78 7.18l7.73 6c4.51-4.18 7.09-10.36 7.09-17.65z"
                />
                <path
                  fill="#FBBC05"
                  d="M10.53 28.59c-.48-1.45-.76-2.99-.76-4.59s.27-3.14.76-4.59l-7.98-6.19C.92 16.46 0 20.12 0 24c0 3.88.92 7.54 2.56 10.78l7.97-6.19z"
                />
                <path
                  fill="#34A853"
                  d="M24 48c6.48 0 11.93-2.13 15.89-5.81l-7.73-6c-2.15 1.45-4.92 2.3-8.16 2.3-6.26 0-11.57-4.22-13.47-9.91l-7.98 6.19C6.51 42.62 14.62 48 24 48z"
                />
              </svg>
              Sign in with Google
            </button>

            {allowed.length > 0 ? (
              <p className="text-center text-xs text-slate-500">
                Allowed account{allowed.length === 1 ? "" : "s"}:{" "}
                <span className="text-slate-400">{allowed.join(", ")}</span>
              </p>
            ) : (
              <p className="text-center text-xs text-amber-400">
                No email allowlist is configured — sign-in will be denied until
                an administrator adds an allowed address in Settings.
              </p>
            )}
          </div>
        ) : (
          <p className="mt-8 rounded-md border border-edge bg-slate-900/60 px-4 py-3 text-center text-sm text-slate-400">
            Remote access not configured; running locally.
          </p>
        )}
      </div>
    </div>
  );
}
