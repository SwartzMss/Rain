import { useCallback, useEffect, useState } from "react";
import type { ReactNode } from "react";
import { Navigate, NavLink } from "react-router-dom";
import { normalizeApiError, rainApi } from "../../api/client";
import type {
  AdminUser,
  AuditLog,
  UserStatus,
  AuthRateLimitEntry,
} from "../../api/types";
import { useAuth } from "../../auth/AuthContext";
import { isAdmin } from "../../auth/permissions";
import {
  advanceCursor,
  currentCursor,
  retreatCursor,
  runAdminAction,
  type CursorHistory,
} from "./adminFlow";

function parseAdminDate(value: string): Date {
  const normalized = value.trim().replace(/ UTC$/i, "Z").replace(" ", "T");
  const iso = /(?:Z|[+-]\d{2}:?\d{2})$/i.test(normalized)
    ? normalized
    : `${normalized}Z`;
  return new Date(iso);
}

function formatAdminDate(value: string): string {
  const date = parseAdminDate(value);
  if (Number.isNaN(date.getTime())) return value;
  const timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone;
  return new Intl.DateTimeFormat("zh-CN", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
    timeZone,
  }).format(date);
}

function AdminShell({ children }: { children: ReactNode }) {
  const navClass = ({ isActive }: { isActive: boolean }) =>
    `flex h-12 items-center border-b-2 px-7 text-sm font-medium transition ${
      isActive
        ? "border-cyan-500 text-cyan-700"
        : "border-transparent text-slate-600 hover:border-slate-300 hover:text-slate-950"
    }`;
  return (
    <div className="mx-auto max-w-7xl space-y-4">
      <nav className="flex overflow-x-auto rounded-xl border border-slate-200/90 bg-white/90 px-3 shadow-sm backdrop-blur">
        <NavLink className={navClass} to="/admin/users">
          用户管理
        </NavLink>
        <NavLink className={navClass} to="/admin/audit-logs">
          审计日志
        </NavLink>
        <NavLink className={navClass} to="/admin/auth-rate-limits">
          认证限流
        </NavLink>
        <NavLink className={navClass} to="/admin/settings">
          系统设置
        </NavLink>
      </nav>
      {children}
    </div>
  );
}

type AdminIconName =
  | "settings"
  | "registration"
  | "shield"
  | "clock"
  | "users"
  | "audit"
  | "rate-limits";

function AdminIcon({
  name,
  subtle = false,
}: {
  name: AdminIconName;
  subtle?: boolean;
}) {
  const toneClass = subtle
    ? "bg-slate-100 text-slate-700"
    : name === "audit"
      ? "bg-indigo-50 text-indigo-600"
      : "bg-cyan-50 text-cyan-600";
  const paths = {
    settings: (
      <>
        <circle cx="12" cy="12" r="3" />
        <path d="M19.4 15a1.7 1.7 0 0 0 .34 1.88l.06.06-2.12 2.12-.06-.06a1.7 1.7 0 0 0-1.88-.34 1.7 1.7 0 0 0-1.04 1.56V20.3h-3v-.08a1.7 1.7 0 0 0-1.04-1.56 1.7 1.7 0 0 0-1.88.34l-.06.06-2.12-2.12.06-.06A1.7 1.7 0 0 0 7 15a1.7 1.7 0 0 0-1.56-1.04H5.3v-3h.14A1.7 1.7 0 0 0 7 9.92a1.7 1.7 0 0 0-.34-1.88L6.6 7.98l2.12-2.12.06.06a1.7 1.7 0 0 0 1.88.34A1.7 1.7 0 0 0 11.7 4.7v-.1h3v.1a1.7 1.7 0 0 0 1.04 1.56 1.7 1.7 0 0 0 1.88-.34l.06-.06 2.12 2.12-.06.06a1.7 1.7 0 0 0-.34 1.88 1.7 1.7 0 0 0 1.56 1.04h.14v3h-.14A1.7 1.7 0 0 0 19.4 15Z" />
      </>
    ),
    registration: (
      <>
        <circle cx="10" cy="8" r="3" />
        <path d="M4.5 19v-1.4A4.6 4.6 0 0 1 9.1 13h1.8a4.6 4.6 0 0 1 4.6 4.6V19M18 8v6M15 11h6" />
      </>
    ),
    shield: (
      <>
        <path d="M12 3 5.5 5.8v5.1c0 4.1 2.8 7.8 6.5 9.1 3.7-1.3 6.5-5 6.5-9.1V5.8L12 3Z" />
        <path d="M12 7v9M9.5 10.5H12" />
      </>
    ),
    clock: (
      <>
        <circle cx="12" cy="12" r="8" />
        <path d="M12 7.5V12l3 1.8" />
      </>
    ),
    users: (
      <>
        <circle cx="9" cy="8" r="3" />
        <path d="M3.8 19v-1.3A4.7 4.7 0 0 1 8.5 13h1A4.7 4.7 0 0 1 14.2 17.7V19M16 6.7a3 3 0 0 1 0 5.6M17 14a4.7 4.7 0 0 1 3.2 4.5V19" />
      </>
    ),
    audit: (
      <>
        <path d="M7 3.8h10a2 2 0 0 1 2 2V20H5V5.8a2 2 0 0 1 2-2Z" />
        <path d="M9 3.8V2.5h6v1.3M8.5 9h7M8.5 13h7M8.5 17h4" />
      </>
    ),
    "rate-limits": (
      <>
        <path d="M12 3 5.5 5.8v5.1c0 4.1 2.8 7.8 6.5 9.1 3.7-1.3 6.5-5 6.5-9.1V5.8L12 3Z" />
        <path d="M8.5 12h7M12 8.5V12l2.2 2" />
      </>
    ),
  };
  return (
    <span
      className={`flex h-12 w-12 shrink-0 items-center justify-center rounded-full ${toneClass}`}
    >
      <svg
        aria-hidden="true"
        className="h-6 w-6"
        fill="none"
        stroke="currentColor"
        strokeLinecap="round"
        strokeLinejoin="round"
        strokeWidth="1.8"
        viewBox="0 0 24 24"
      >
        {paths[name]}
      </svg>
    </span>
  );
}

function AdminPageHeader({
  icon,
  title,
  description,
  actions,
  embedded = false,
}: {
  icon: AdminIconName;
  title: string;
  description: string;
  actions?: ReactNode;
  embedded?: boolean;
}) {
  const content = (
    <>
      <div className="flex items-center gap-5">
        <AdminIcon name={icon} subtle={icon === "settings"} />
        <div>
          <h1 className="text-2xl font-semibold tracking-tight text-slate-950">
            {title}
          </h1>
          <p className="mt-1 text-sm leading-6 text-slate-500">{description}</p>
        </div>
      </div>
      {actions ? (
        <div className="w-full lg:w-auto lg:shrink-0" data-testid="admin-page-actions">
          {actions}
        </div>
      ) : null}
    </>
  );
  if (embedded) {
    return (
      <div
        className="flex flex-col gap-4 border-b border-slate-100 px-5 py-5 sm:px-6 lg:flex-row lg:items-center lg:justify-between"
        data-testid="admin-page-header"
      >
        {content}
      </div>
    );
  }
  return (
    <section
      className="flex flex-col gap-4 rounded-2xl border border-slate-200/90 bg-white/95 p-5 shadow-[0_12px_32px_rgba(15,23,42,0.06)] backdrop-blur sm:p-6 lg:flex-row lg:items-center lg:justify-between"
      data-testid="admin-page-header"
    >
      {content}
    </section>
  );
}

function AdminContentCard({
  children,
  className = "",
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <section
      className={`overflow-hidden rounded-2xl border border-slate-200/90 bg-white/95 shadow-[0_12px_32px_rgba(15,23,42,0.06)] backdrop-blur ${className}`}
    >
      {children}
    </section>
  );
}

function EmptyState({
  title,
  description,
}: {
  title: string;
  description?: string;
}) {
  return (
    <div className="flex min-h-52 flex-col items-center justify-center px-5 py-10 text-center">
      <svg
        aria-hidden="true"
        className="h-20 w-28 text-slate-200"
        fill="none"
        viewBox="0 0 112 80"
      >
        <ellipse
          cx="56"
          cy="67"
          fill="currentColor"
          opacity=".45"
          rx="35"
          ry="6"
        />
        <path
          d="M35 38h42l7 14v13H28V52l7-14Z"
          fill="currentColor"
          opacity=".55"
        />
        <path d="M28 52h18l5 7h10l5-7h18" stroke="white" strokeWidth="3" />
        <path
          d="M22 32h18M71 25h21M83 34h13"
          stroke="currentColor"
          strokeLinecap="round"
          strokeWidth="7"
          opacity=".35"
        />
      </svg>
      <p className="mt-2 text-base font-semibold text-slate-800">{title}</p>
      {description ? (
        <p className="mt-1 text-sm text-slate-500">{description}</p>
      ) : null}
    </div>
  );
}

function RefreshIcon() {
  return (
    <svg
      aria-hidden="true"
      className="h-4 w-4"
      fill="none"
      stroke="currentColor"
      strokeLinecap="round"
      strokeWidth="2"
      viewBox="0 0 24 24"
    >
      <path d="M20 7v5h-5M4 17v-5h5" />
      <path d="M6.1 9A7 7 0 0 1 18 6.5L20 9M4 15l2 2.5A7 7 0 0 0 17.9 15" />
    </svg>
  );
}

function SettingsSection({
  icon,
  title,
  description,
  children,
}: {
  icon: AdminIconName;
  title: string;
  description: string;
  children: ReactNode;
}) {
  return (
    <section className="rounded-2xl border border-slate-200/90 bg-white/95 p-5 shadow-[0_12px_32px_rgba(15,23,42,0.06)] backdrop-blur sm:p-6">
      <div className="flex items-start gap-4">
        <AdminIcon name={icon} />
        <div className="min-w-0 flex-1">
          <h2 className="text-lg font-semibold text-slate-950">{title}</h2>
          <p className="mt-1 text-sm leading-6 text-slate-500">{description}</p>
          {children}
        </div>
      </div>
    </section>
  );
}

function AdminGuard({ children }: { children: ReactNode }) {
  const auth = useAuth();
  if (auth.state.status === "LOADING") return <p>正在确认身份…</p>;
  if (auth.state.status !== "AUTHENTICATED")
    return <Navigate to="/login" replace state={{ from: "/admin/users" }} />;
  if (!isAdmin(auth.state.user))
    return (
      <section className="panel">
        <h2 className="text-xl font-semibold">403</h2>
        <p>此页面需要管理员权限。</p>
      </section>
    );
  return <AdminShell>{children}</AdminShell>;
}

export function AdminPage() {
  return <Navigate to="/admin/users" replace />;
}

export function AdminSettingsPage() {
  const [allowed, setAllowed] = useState(true);
  const [ipLimit, setIpLimit] = useState(20);
  const [usernameLimit, setUsernameLimit] = useState(10);
  const [issueInactiveDays, setIssueInactiveDays] = useState<number | "">(0);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [hasLoadedSettings, setHasLoadedSettings] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [feedbackSection, setFeedbackSection] = useState<
    "registration" | "rate-limits" | "issue-expiry" | null
  >(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const load = useCallback(async () => {
    setLoading(true);
    setLoadError(null);
    try {
      const value = await rainApi.fetchAdminSettings();
      setAllowed(value.allow_registration);
      setIpLimit(value.login_ip_limit_per_minute);
      setUsernameLimit(value.login_username_failure_limit_per_5_minutes);
      setIssueInactiveDays(value.issue_inactive_days);
      setHasLoadedSettings(true);
    } catch (e) {
      setHasLoadedSettings(false);
      setLoadError(normalizeApiError(e));
    } finally {
      setLoading(false);
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);
  const save = async (value?: boolean, thresholds = false) => {
    setFeedbackSection(thresholds ? "rate-limits" : "registration");
    setSaving(true);
    setMessage(null);
    setSaveError(null);
    try {
      const result = thresholds
        ? await rainApi.updateAdminSettings(undefined, ipLimit, usernameLimit)
        : await rainApi.updateAdminSettings(value);
      setAllowed(result.allow_registration);
      setIpLimit(result.login_ip_limit_per_minute);
      setUsernameLimit(result.login_username_failure_limit_per_5_minutes);
      setIssueInactiveDays(result.issue_inactive_days);
      setMessage("设置已保存");
    } catch (e) {
      setSaveError(normalizeApiError(e));
      await load();
    } finally {
      setSaving(false);
    }
  };
  const saveThresholds = () => void save(undefined, true);
  const saveIssueExpiry = async () => {
    setFeedbackSection("issue-expiry");
    if (
      issueInactiveDays === "" ||
      !Number.isInteger(issueInactiveDays) ||
      (issueInactiveDays !== 0 &&
        (issueInactiveDays < 7 || issueInactiveDays > 30))
    ) {
      setSaveError("Issue 非活跃天数必须为 0，或 7 到 30");
      return;
    }
    setSaving(true);
    setMessage(null);
    setSaveError(null);
    try {
      const result = await rainApi.updateAdminSettings(
        undefined,
        undefined,
        undefined,
        issueInactiveDays,
      );
      setIssueInactiveDays(result.issue_inactive_days);
      setMessage("Issue 过期配置已保存");
    } catch (e) {
      setSaveError(normalizeApiError(e));
      await load();
    } finally {
      setSaving(false);
    }
  };
  const sectionFeedback = (section: typeof feedbackSection) => {
    if (feedbackSection !== section) return null;
    if (message) {
      return (
        <p
          className="mt-4 flex items-center gap-2 rounded-lg border border-emerald-200 bg-emerald-50/80 px-3 py-2 text-sm text-emerald-700"
          role="status"
        >
          <span
            aria-hidden="true"
            className="flex h-4 w-4 items-center justify-center rounded-full bg-emerald-500 text-[10px] font-bold text-white"
          >
            ✓
          </span>
          {message}
        </p>
      );
    }
    if (saveError) {
      return (
        <p
          className="mt-4 rounded-lg border border-rose-200 bg-rose-50 px-3 py-2 text-sm text-rose-700"
          role="alert"
        >
          保存失败：{saveError}
        </p>
      );
    }
    return null;
  };
  const controlsDisabled =
    loading || saving || !hasLoadedSettings || Boolean(loadError);
  const primaryButtonClass =
    "rounded-lg bg-cyan-600 px-5 py-2.5 text-sm font-semibold text-white shadow-sm shadow-cyan-600/20 transition hover:bg-cyan-700 disabled:cursor-not-allowed disabled:opacity-50";
  return (
    <AdminGuard>
      <div className="space-y-3">
        <AdminPageHeader
          icon="settings"
          title="系统设置"
          description="配置系统的注册、认证与过期策略，保障系统安全与稳定运行。"
        />

        {loadError ? (
          <p
            className="rounded-xl border border-rose-200 bg-rose-50 px-4 py-3 text-sm text-rose-700"
            role="alert"
          >
            {loadError}
          </p>
        ) : null}

        <SettingsSection
          icon="registration"
          title="用户注册"
          description="控制新用户是否可以注册。关闭后，将无法注册新用户，但已有用户仍可正常登录和使用系统。"
        >
          <div className="mt-4 flex flex-wrap items-center justify-between gap-4 border-t border-slate-100 pt-4">
            <span className="text-sm font-medium text-slate-600">用户注册</span>
            <div className="flex items-center gap-3">
              <button
                type="button"
                aria-label="用户注册"
                aria-pressed={allowed}
                disabled={controlsDisabled}
                onClick={() => void save(!allowed)}
                className={`relative h-7 w-12 rounded-full transition-colors ${allowed ? "bg-cyan-600" : "bg-slate-300"} disabled:cursor-not-allowed disabled:opacity-50`}
              >
                <span
                  className={`absolute top-1 h-5 w-5 rounded-full bg-white shadow-sm transition-all ${allowed ? "left-6" : "left-1"}`}
                />
              </button>
              <span
                className={`min-w-10 text-sm font-medium ${allowed ? "text-slate-700" : "text-slate-500"}`}
              >
                {allowed ? "已启用" : "已关闭"}
              </span>
            </div>
          </div>
          {sectionFeedback("registration")}
        </SettingsSection>

        <SettingsSection
          icon="shield"
          title="认证限流"
          description="通过限制认证请求频率，降低暴力破解与滥用风险。"
        >
          <div className="mt-5 grid gap-5 md:grid-cols-2">
            <div>
              <label
                className="text-sm font-medium text-slate-700"
                htmlFor="login-ip-limit"
              >
                IP 每分钟阈值
              </label>
              <input
                id="login-ip-limit"
                aria-describedby="login-ip-limit-help"
                type="number"
                min="1"
                max="1000"
                value={ipLimit}
                disabled={loading || saving || !hasLoadedSettings}
                onChange={(e) => setIpLimit(Number(e.target.value))}
                className="mt-2 w-full rounded-lg border border-slate-200 bg-white px-3 py-2.5 text-slate-900 shadow-sm outline-none transition focus:border-cyan-500 focus:ring-2 focus:ring-cyan-100 disabled:bg-slate-50"
              />
              <p
                id="login-ip-limit-help"
                className="mt-1.5 text-xs font-normal leading-5 text-slate-500"
              >
                同一 IP 在每分钟内允许的最大认证请求次数。
              </p>
            </div>
            <div>
              <label
                className="text-sm font-medium text-slate-700"
                htmlFor="login-username-limit"
              >
                用户名失败 5 分钟阈值
              </label>
              <input
                id="login-username-limit"
                aria-describedby="login-username-limit-help"
                type="number"
                min="1"
                max="100"
                value={usernameLimit}
                disabled={loading || saving || !hasLoadedSettings}
                onChange={(e) => setUsernameLimit(Number(e.target.value))}
                className="mt-2 w-full rounded-lg border border-slate-200 bg-white px-3 py-2.5 text-slate-900 shadow-sm outline-none transition focus:border-cyan-500 focus:ring-2 focus:ring-cyan-100 disabled:bg-slate-50"
              />
              <p
                id="login-username-limit-help"
                className="mt-1.5 text-xs font-normal leading-5 text-slate-500"
              >
                同一用户名在 5 分钟内允许的最大失败次数。
              </p>
            </div>
          </div>
          <div className="mt-4 flex justify-end">
            <button
              type="button"
              disabled={controlsDisabled}
              onClick={saveThresholds}
              className={primaryButtonClass}
            >
              保存限流配置
            </button>
          </div>
          {sectionFeedback("rate-limits")}
        </SettingsSection>

        <SettingsSection
          icon="clock"
          title="Issue 非活跃自动过期"
          description="设置 Issue 在多少天未活跃后自动过期。"
        >
          <div className="mt-5 flex flex-col gap-4 md:flex-row md:items-end md:justify-between">
            <div className="w-full md:max-w-sm">
              <label
                className="text-sm font-medium text-slate-700"
                htmlFor="issue-inactive-days"
              >
                非活跃天数
              </label>
              <input
                id="issue-inactive-days"
                aria-describedby="issue-inactive-days-help"
                type="number"
                min="0"
                max="30"
                value={issueInactiveDays}
                disabled={loading || saving || !hasLoadedSettings}
                onChange={(e) =>
                  setIssueInactiveDays(
                    e.target.value === "" ? "" : Number(e.target.value),
                  )
                }
                className="mt-2 w-full rounded-lg border border-slate-200 bg-white px-3 py-2.5 text-slate-900 shadow-sm outline-none transition focus:border-cyan-500 focus:ring-2 focus:ring-cyan-100 disabled:bg-slate-50"
              />
              <p
                id="issue-inactive-days-help"
                className="mt-1.5 text-xs font-normal leading-5 text-slate-500"
              >
                0 表示关闭；启用时可设置为 7 到 30 天。
              </p>
            </div>
            <button
              type="button"
              disabled={
                controlsDisabled ||
                issueInactiveDays === "" ||
                (issueInactiveDays !== 0 &&
                  (issueInactiveDays < 7 || issueInactiveDays > 30)) ||
                !Number.isInteger(issueInactiveDays)
              }
              onClick={() => void saveIssueExpiry()}
              className={primaryButtonClass}
            >
              保存 Issue 过期配置
            </button>
          </div>
          <p className="mt-4 flex items-start gap-2 rounded-lg border border-sky-200 bg-sky-50/70 px-3 py-2 text-xs leading-5 text-sky-700">
            <span
              aria-hidden="true"
              className="mt-0.5 flex h-4 w-4 shrink-0 items-center justify-center rounded-full bg-sky-500 text-[10px] font-bold text-white"
            >
              i
            </span>
            配置将在下一次后台扫描任务执行时生效，扫描任务通常每隔一段时间自动运行。
          </p>
          {sectionFeedback("issue-expiry")}
        </SettingsSection>
      </div>
    </AdminGuard>
  );
}

export function AuthRateLimitsPage() {
  const [usernameFailures, setUsernameFailures] = useState<
    AuthRateLimitEntry[]
  >([]);
  const [loginIps, setLoginIps] = useState<AuthRateLimitEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [clearingKey, setClearingKey] = useState<string | null>(null);
  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const data = await rainApi.fetchAuthRateLimits();
      setUsernameFailures(data.username_failures);
      setLoginIps(data.login_ips);
    } catch (e) {
      setError(normalizeApiError(e));
    } finally {
      setLoading(false);
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);
  const clear = async (type: "usernames" | "ips", key?: string) => {
    if (!window.confirm(key ? "确认解除该限流？" : "确认清除全部该类限流？"))
      return;
    const operationKey = key ?? `all:${type}`;
    setClearingKey(operationKey);
    try {
      if (key) await rainApi.clearAuthRateLimit(type, key);
      else await rainApi.clearAllAuthRateLimits(type);
      setNotice("限流已清除");
      await load();
    } catch (e) {
      setError(normalizeApiError(e));
    } finally {
      setClearingKey(null);
    }
  };
  const table = (
    title: string,
    description: string,
    type: "usernames" | "ips",
    items: AuthRateLimitEntry[],
    label: (item: AuthRateLimitEntry) => string,
  ) => (
    <section className="overflow-hidden rounded-xl border border-slate-200">
      <div className="flex flex-col gap-3 border-b border-slate-100 px-5 py-4 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex flex-col gap-1 sm:flex-row sm:items-baseline sm:gap-6">
          <h2 className="text-lg font-semibold text-slate-950">{title}</h2>
          <p className="text-sm text-slate-500">{description}</p>
        </div>
        <button
          type="button"
          disabled={loading || Boolean(clearingKey) || !items.length}
          onClick={() => void clear(type)}
          className="rounded-lg border border-rose-200 bg-white px-3 py-2 text-sm font-medium text-rose-600 transition hover:bg-rose-50 disabled:cursor-not-allowed disabled:opacity-40"
        >
          {clearingKey === `all:${type}` ? "清除中…" : "清除全部"}
        </button>
      </div>
      {!items.length ? (
        <EmptyState title="当前没有认证限流记录" />
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full min-w-[760px] text-left text-sm">
            <thead className="bg-slate-50 text-xs font-semibold text-slate-500">
              <tr>
                <th className="px-5 py-3">标识</th>
                <th className="px-5 py-3">次数/阈值</th>
                <th className="px-5 py-3">最近事件</th>
                <th className="px-5 py-3">恢复倒计时</th>
                <th className="px-5 py-3">操作</th>
              </tr>
            </thead>
            <tbody>
              {items.map((item) => (
                <tr
                  key={item.key}
                  className="border-t border-slate-100 transition hover:bg-sky-50/40"
                >
                  <td className="px-5 py-3.5 font-medium text-slate-900">
                    {label(item)}
                  </td>
                  <td className="px-5 py-3.5 tabular-nums text-slate-700">
                    <span className="font-semibold">{item.current_count}</span>
                    <span className="text-slate-400"> / {item.limit}</span>{" "}
                    {item.limited ? (
                      <span className="ml-2 inline-flex rounded-full bg-rose-50 px-2 py-0.5 text-xs font-medium text-rose-700 ring-1 ring-inset ring-rose-200">
                        受限中
                      </span>
                    ) : null}
                  </td>
                  <td className="whitespace-nowrap px-5 py-3.5 text-slate-500">
                    {item.last_event_at
                      ? formatAdminDate(item.last_event_at)
                      : "—"}
                  </td>
                  <td className="px-5 py-3.5 tabular-nums text-slate-500">
                    {item.limited ? `${item.retry_after_seconds}s` : "—"}
                  </td>
                  <td className="px-5 py-3.5">
                    <button
                      type="button"
                      disabled={loading || Boolean(clearingKey)}
                      onClick={() => void clear(type, item.key)}
                      className="rounded-lg border border-rose-200 bg-white px-2.5 py-1.5 text-xs font-medium text-rose-600 transition hover:bg-rose-50 disabled:cursor-not-allowed disabled:opacity-40"
                    >
                      {clearingKey === item.key ? "清除中…" : "清除"}
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
  return (
    <AdminGuard>
      <AdminContentCard>
        <AdminPageHeader
          icon="rate-limits"
          title="认证限流"
          description="查看当前登录保护状态并解除异常限流；运行时记录会在服务重启后清空。"
          embedded
          actions={
            <button
              type="button"
              onClick={() => void load()}
              disabled={loading}
              className="inline-flex items-center gap-2 rounded-lg border border-cyan-400 bg-white px-4 py-2.5 text-sm font-medium text-cyan-700 shadow-sm transition hover:bg-cyan-50 disabled:cursor-not-allowed disabled:opacity-40"
            >
              <RefreshIcon />
              {loading ? "刷新中…" : "刷新"}
            </button>
          }
        />
        {notice ? (
          <p
            className="mx-5 mt-4 rounded-xl border border-emerald-200 bg-emerald-50 px-4 py-3 text-sm text-emerald-700"
            role="status"
          >
            {notice}
          </p>
        ) : null}
        {error ? (
          <p
            className="mx-5 mt-4 rounded-xl border border-rose-200 bg-rose-50 px-4 py-3 text-sm text-rose-700"
            role="alert"
          >
            {error}
          </p>
        ) : null}
        <div className="space-y-5 p-5">
          {table(
            "用户名失败限流",
            "按用户名统计失败登录次数，便于观察是否存在暴力破解行为。",
            "usernames",
            usernameFailures,
            (item) => item.username ?? item.key,
          )}
          {table(
            "登录 IP 限流",
            "按 IP 维度统计登录限流记录，便于排查异常来源。",
            "ips",
            loginIps,
            (item) => item.ip ?? item.key,
          )}
        </div>
      </AdminContentCard>
    </AdminGuard>
  );
}

export function AdminUsersPage() {
  const auth = useAuth();
  const [users, setUsers] = useState<AdminUser[]>([]);
  const [query, setQuery] = useState("");
  const [status, setStatus] = useState<UserStatus | "">("");
  const [history, setHistory] = useState<CursorHistory>([undefined]);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (auth.state.status !== "AUTHENTICATED" || !isAdmin(auth.state.user))
      return;
    setLoading(true);
    setError(null);
    try {
      const page = await rainApi.fetchAdminUsers({
        query: query || undefined,
        status: status || undefined,
        cursor: currentCursor(history),
      });
      setUsers(page.items);
      setNextCursor(page.next_cursor);
    } catch (loadError) {
      setError(normalizeApiError(loadError));
    } finally {
      setLoading(false);
    }
  }, [auth.state, history, query, status]);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <AdminGuard>
      <AdminContentCard>
        <AdminPageHeader
          icon="users"
          title="用户管理"
          description="管理普通用户状态及登录会话。"
          embedded
          actions={
            <div className="flex items-center gap-2 text-sm font-medium text-slate-500">
              <span className="h-2 w-2 rounded-full bg-emerald-500" />
              当前显示 {users.length} 位用户
            </div>
          }
        />
        <div className="border-b border-slate-100 bg-slate-50/70 px-5 py-4">
          <div className="grid gap-2 md:grid-cols-[minmax(0,1fr)_180px_auto]">
            <label className="relative block">
              <span className="sr-only">搜索用户名</span>
              <span className="pointer-events-none absolute inset-y-0 left-3 flex items-center text-slate-400">
                ⌕
              </span>
              <input
                className="w-full rounded-xl border border-slate-200 bg-white py-2.5 pl-9 pr-3 text-sm outline-none transition placeholder:text-slate-400 focus:border-sky-400 focus:ring-4 focus:ring-sky-100"
                placeholder="搜索用户名"
                value={query}
                onChange={(event) => {
                  setQuery(event.target.value);
                  setHistory([undefined]);
                }}
              />
            </label>
            <select
              className="rounded-xl border border-slate-200 bg-white px-3 py-2.5 text-sm text-slate-700 outline-none focus:border-sky-400 focus:ring-4 focus:ring-sky-100"
              value={status}
              onChange={(event) => {
                setStatus(event.target.value as UserStatus | "");
                setHistory([undefined]);
              }}
            >
              <option value="">全部状态</option>
              <option value="ACTIVE">启用</option>
              <option value="DISABLED">停用</option>
            </select>
            <button
              className="inline-flex items-center justify-center gap-2 rounded-xl border border-cyan-400 bg-white px-5 py-2.5 text-sm font-medium text-cyan-700 transition hover:bg-cyan-50"
              type="button"
              onClick={() => void load()}
            >
              <RefreshIcon />
              刷新
            </button>
          </div>
        </div>
        {error ? (
          <p className="mx-5 mt-4 rounded-xl border border-rose-200 bg-rose-50 px-4 py-3 text-sm text-rose-700">
            {error}
          </p>
        ) : null}
        {notice ? (
          <p className="mx-5 mt-4 rounded-xl border border-emerald-200 bg-emerald-50 px-4 py-3 text-sm text-emerald-700">
            {notice}
          </p>
        ) : null}
        <div className="mx-5 my-4 overflow-x-auto rounded-xl border border-slate-200">
          <table className="w-full min-w-[980px] text-left text-sm">
            <thead className="bg-slate-50 text-xs font-semibold text-slate-500">
              <tr className="border-b border-slate-200">
                <th className="px-4 py-3">用户名</th>
                <th className="px-4 py-3">状态</th>
                <th className="px-4 py-3">Issue 数</th>
                <th className="px-4 py-3">占用容量</th>
                <th className="px-4 py-3">活跃会话</th>
                <th className="px-4 py-3">创建时间</th>
                <th className="px-4 py-3">最近登录</th>
                <th className="px-4 py-3">操作</th>
              </tr>
            </thead>
            <tbody>
              {loading ? (
                <tr>
                  <td
                    className="py-12 text-center text-sm text-slate-500"
                    colSpan={8}
                  >
                    用户加载中…
                  </td>
                </tr>
              ) : users.length === 0 ? (
                <tr>
                  <td colSpan={8}>
                    <EmptyState
                      title="暂无匹配的普通用户"
                      description="请尝试调整搜索条件或状态筛选"
                    />
                  </td>
                </tr>
              ) : (
                users.map((user) => (
                  <UserRow
                    key={user.id}
                    user={user}
                    reload={load}
                    onError={setError}
                    onNotice={setNotice}
                  />
                ))
              )}
            </tbody>
          </table>
        </div>
        <div className="flex items-center justify-between border-t border-slate-100 px-5 py-3 text-xs text-slate-500">
          <span>
            {loading
              ? "正在同步…"
              : users.length
                ? `本页 ${users.length} 条`
                : "暂无数据"}
          </span>
          <div className="flex gap-2">
            <button
              className="rounded-lg border border-slate-200 bg-white px-3 py-1.5 text-sm text-slate-600 transition hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-40"
              disabled={history.length <= 1}
              onClick={() => setHistory((value) => retreatCursor(value))}
            >
              上一页
            </button>
            <button
              className="rounded-lg border border-slate-200 bg-white px-3 py-1.5 text-sm text-slate-600 transition hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-40"
              disabled={!nextCursor}
              onClick={() =>
                nextCursor &&
                setHistory((value) => advanceCursor(value, nextCursor))
              }
            >
              下一页
            </button>
          </div>
        </div>
      </AdminContentCard>
    </AdminGuard>
  );
}

function UserRow({
  user,
  reload,
  onError,
  onNotice,
}: {
  user: AdminUser;
  reload: () => Promise<void>;
  onError: (message: string) => void;
  onNotice: (message: string) => void;
}) {
  const auth = useAuth();
  const act = async (action: () => Promise<unknown>, message: string) => {
    if (!window.confirm("确认执行此管理操作？")) return;
    try {
      await runAdminAction({ action, reload, refreshAuth: auth.refresh });
      onNotice(message);
    } catch (error) {
      onError(normalizeApiError(error));
    }
  };
  return (
    <tr className="border-b border-slate-100 transition hover:bg-sky-50/40 last:border-0">
      <td className="px-4 py-3.5">
        <div className="font-semibold text-slate-900">{user.username}</div>
        <div className="mt-0.5 font-mono text-[11px] text-slate-400">
          {user.id.slice(0, 8)}
        </div>
      </td>
      <td className="px-4 py-3.5">
        <span
          className={`inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-xs font-medium ${user.status === "ACTIVE" ? "bg-emerald-50 text-emerald-700 ring-1 ring-inset ring-emerald-200" : "bg-slate-100 text-slate-600 ring-1 ring-inset ring-slate-200"}`}
        >
          <span
            className={`h-1.5 w-1.5 rounded-full ${user.status === "ACTIVE" ? "bg-emerald-500" : "bg-slate-400"}`}
          />
          {user.status === "ACTIVE" ? "启用" : "停用"}
        </span>
      </td>
      <td className="px-4 py-3.5 font-medium tabular-nums text-slate-700">
        {user.issue_count}
      </td>
      <td className="whitespace-nowrap px-4 py-3.5 font-medium tabular-nums text-slate-700">
        {formatBytes(user.storage_bytes)}
      </td>
      <td className="px-4 py-3.5 font-medium tabular-nums text-slate-700">
        {user.active_session_count}
      </td>
      <td className="whitespace-nowrap px-4 py-3.5 tabular-nums text-slate-600">
        {formatAdminDate(user.created_at)}
      </td>
      <td className="whitespace-nowrap px-4 py-3.5 tabular-nums text-slate-600">
        {user.last_login_at ? formatAdminDate(user.last_login_at) : "从未登录"}
      </td>
      <td className="px-4 py-3.5">
        <div className="flex flex-wrap gap-2">
          <button
            className="rounded-lg border border-slate-200 bg-white px-2.5 py-1.5 text-xs font-medium text-slate-700 transition hover:border-sky-300 hover:bg-sky-50 hover:text-sky-700"
            type="button"
            onClick={() =>
              void act(
                () =>
                  rainApi.changeUserStatus(
                    user.id,
                    user.status === "ACTIVE" ? "DISABLED" : "ACTIVE",
                  ),
                user.status === "ACTIVE" ? "用户已停用" : "用户已启用",
              )
            }
          >
            {user.status === "ACTIVE" ? "停用" : "启用"}
          </button>
          <button
            className="rounded-lg border border-rose-200 bg-white px-2.5 py-1.5 text-xs font-medium text-rose-600 transition hover:bg-rose-50 disabled:cursor-not-allowed disabled:opacity-40"
            type="button"
            disabled={user.active_session_count === 0}
            onClick={() =>
              void act(
                () => rainApi.revokeUserSessions(user.id),
                "活跃 Session 已注销",
              )
            }
          >
            注销会话
          </button>
        </div>
      </td>
    </tr>
  );
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KiB", "MiB", "GiB", "TiB"];
  let value = bytes;
  let unit = "B";
  for (const nextUnit of units) {
    value /= 1024;
    unit = nextUnit;
    if (value < 1024 || nextUnit === "TiB") break;
  }
  return `${value >= 10 ? value.toFixed(0) : value.toFixed(1)} ${unit}`;
}

export function AuditLogsPage() {
  const auth = useAuth();
  const [logs, setLogs] = useState<AuditLog[]>([]);
  const [history, setHistory] = useState<CursorHistory>([undefined]);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(async () => {
    if (auth.state.status !== "AUTHENTICATED" || !isAdmin(auth.state.user))
      return;
    setLoading(true);
    try {
      const page = await rainApi.fetchAuditLogs(currentCursor(history));
      setLogs(page.items);
      setNextCursor(page.next_cursor);
    } catch (loadError) {
      setError(normalizeApiError(loadError));
    } finally {
      setLoading(false);
    }
  }, [auth.state, history]);
  useEffect(() => {
    void load();
  }, [load]);
  const today = new Date().toDateString();
  const todayCount = logs.filter(
    (log) => parseAdminDate(log.created_at).toDateString() === today,
  ).length;
  const userChangeCount = logs.filter(
    (log) => log.action === "USER_STATUS_CHANGED",
  ).length;
  const sessionActionCount = logs.filter(
    (log) => log.action === "USER_SESSIONS_REVOKED",
  ).length;
  return (
    <AdminGuard>
      <AdminContentCard>
        <AdminPageHeader
          icon="audit"
          title="审计日志"
          description="查看管理员对用户、会话和系统配置执行的安全操作。"
          embedded
          actions={
            <div
              className="grid w-full grid-cols-2 gap-2 lg:w-auto lg:grid-cols-4"
              data-testid="audit-metrics"
            >
              <AuditMetric label="本页事件" value={logs.length} tone="indigo" />
              <AuditMetric
                label="本页今日操作"
                value={todayCount}
                tone="emerald"
              />
              <AuditMetric
                label="本页用户变更"
                value={userChangeCount}
                tone="amber"
              />
              <AuditMetric
                label="本页会话操作"
                value={sessionActionCount}
                tone="violet"
              />
            </div>
          }
        />
        <div className="border-b border-slate-100 bg-slate-50/70 px-5 py-4">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="text-sm font-medium text-slate-700">操作记录</div>
            <button
              className="inline-flex items-center gap-2 rounded-xl border border-cyan-400 bg-white px-4 py-2.5 text-sm font-medium text-cyan-700 transition hover:bg-cyan-50"
              type="button"
              onClick={() => void load()}
            >
              <RefreshIcon />
              刷新
            </button>
          </div>
        </div>
        {error ? (
          <p className="mx-5 mt-4 rounded-xl border border-rose-200 bg-rose-50 px-4 py-3 text-sm text-rose-700">
            {error}
          </p>
        ) : null}
        {loading ? (
          <p className="py-10 text-center text-sm text-slate-500">
            审计日志加载中…
          </p>
        ) : logs.length === 0 ? (
          <EmptyState
            title="暂无审计记录"
            description="管理员操作会记录在这里"
          />
        ) : (
          <div className="mx-5 my-4 overflow-x-auto rounded-xl border border-slate-200">
            <table className="w-full min-w-[760px] text-left text-sm">
              <thead className="bg-slate-50 text-xs font-semibold text-slate-500">
                <tr className="border-b border-slate-200">
                  <th className="px-4 py-3">时间</th>
                  <th className="px-4 py-3">操作类型</th>
                  <th className="px-4 py-3">目标用户</th>
                  <th className="px-4 py-3">变更摘要</th>
                  <th className="px-4 py-3">客户端 IP</th>
                </tr>
              </thead>
              <tbody>
                {logs.map((log) => (
                  <tr
                    className="border-b border-slate-100 transition hover:bg-sky-50/40 last:border-0"
                    key={log.id}
                  >
                    <td className="whitespace-nowrap px-4 py-3.5 tabular-nums text-slate-600">
                      {formatAdminDate(log.created_at)}
                    </td>
                    <td className="px-4 py-3.5">
                      <span className="inline-flex rounded-full bg-indigo-50 px-2.5 py-1 text-xs font-medium text-indigo-700 ring-1 ring-inset ring-indigo-200">
                        {log.action === "ADMIN_BOOTSTRAPPED"
                          ? "初始化管理员"
                          : log.action === "USER_STATUS_CHANGED"
                            ? "变更用户状态"
                            : log.action === "USER_SESSIONS_REVOKED"
                              ? "注销用户 Session"
                              : log.action}
                      </span>
                    </td>
                    <td className="px-4 py-3.5">
                      <div className="font-semibold text-slate-900">
                        {log.target_username || "用户已删除"}
                      </div>
                      {log.target_user_id ? (
                        <div className="mt-0.5 font-mono text-[11px] text-slate-400">
                          {log.target_user_id.slice(0, 8)}
                        </div>
                      ) : (
                        <div className="mt-0.5 text-xs text-slate-400">
                          系统操作
                        </div>
                      )}
                    </td>
                    <td className="px-4 py-3.5 text-slate-600">
                      {log.old_value || log.new_value
                        ? `${log.old_value ?? "—"} → ${log.new_value ?? "—"}`
                        : "—"}
                    </td>
                    <td className="px-4 py-3.5 font-mono text-xs text-slate-600">
                      {log.client_ip || "—"}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
        <div className="flex items-center justify-between border-t border-slate-100 px-5 py-3 text-xs text-slate-500">
          <span>
            {loading
              ? "正在同步…"
              : logs.length
                ? `本页 ${logs.length} 条`
                : "暂无数据"}
          </span>
          <div className="flex gap-2">
            <button
              className="rounded-lg border border-slate-200 bg-white px-3 py-1.5 text-sm text-slate-600 transition hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-40"
              disabled={history.length <= 1}
              onClick={() => setHistory((value) => retreatCursor(value))}
            >
              上一页
            </button>
            <button
              className="rounded-lg border border-slate-200 bg-white px-3 py-1.5 text-sm text-slate-600 transition hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-40"
              disabled={!nextCursor}
              onClick={() =>
                nextCursor &&
                setHistory((value) => advanceCursor(value, nextCursor))
              }
            >
              下一页
            </button>
          </div>
        </div>
      </AdminContentCard>
    </AdminGuard>
  );
}

function AuditMetric({
  label,
  value,
  tone,
}: {
  label: string;
  value: number;
  tone: "indigo" | "emerald" | "amber" | "violet";
}) {
  const toneClass = {
    indigo: "bg-indigo-50 text-indigo-700 ring-indigo-100",
    emerald: "bg-emerald-50 text-emerald-700 ring-emerald-100",
    amber: "bg-amber-50 text-amber-700 ring-amber-100",
    violet: "bg-violet-50 text-violet-700 ring-violet-100",
  }[tone];
  return (
    <div
      className={`rounded-2xl p-4 shadow-[0_8px_24px_rgba(15,23,42,0.04)] ring-1 ring-inset ${toneClass}`}
    >
      <div className="text-xs font-medium opacity-75">{label}</div>
      <div className="mt-1 text-2xl font-semibold tabular-nums">{value}</div>
    </div>
  );
}
