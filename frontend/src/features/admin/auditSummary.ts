import type { AuditLog } from "../../api/types";

interface AuditChange {
  field: string;
  old_value?: unknown;
  new_value?: unknown;
  redacted?: boolean;
  resource_mode?: { old_value?: unknown; new_value?: unknown };
}

interface AuditDetails {
  changes?: unknown;
  changed_fields?: unknown;
}

export interface AuditSummary {
  actionLabel: string;
  summary: string;
  changes: string[];
  allChanges: string[];
  hiddenCount: number;
}

const FIELD_METADATA: Record<string, { label: string; unit?: string }> = {
  provider_api_key: { label: "API 密钥" },
  allow_registration: { label: "允许注册" },
  registration_requires_invite: { label: "邀请码注册" },
  session_ttl_seconds: { label: "会话有效期", unit: "秒" },
  register_ip_limit_per_hour: { label: "单 IP 每小时注册上限", unit: "次" },
  login_ip_limit_per_minute: { label: "每分钟登录失败上限", unit: "次" },
  login_username_failure_limit_per_5_minutes: {
    label: "单用户名五分钟失败上限",
    unit: "次",
  },
  argon2_concurrency: { label: "Argon2 并发计算数", unit: "个" },
  issue_inactive_days: { label: "Issue 闲置清理天数", unit: "天" },
  cleanup_exempt_usernames: { label: "自动清理白名单" },
  issue_max_content_size: { label: "Issue 内容上限", unit: "bytes" },
  archive_max_working_size: { label: "归档解压工作区上限", unit: "bytes" },
  upload_concurrent_processing_tasks: { label: "上传并发处理数", unit: "个" },
  upload_concurrent_receive_tasks: { label: "上传接收并发数", unit: "个" },
  upload_max_tmp_bytes: { label: "上传临时空间上限", unit: "bytes" },
  indexing_max_indexed_line_size: { label: "索引单行最大大小", unit: "bytes" },
  search_tantivy_writer_heap_size: { label: "Tantivy writer 堆预算", unit: "bytes" },
  api_file_preview_size: { label: "文件预览大小", unit: "bytes" },
  api_max_preview_line_size: { label: "预览单行最大大小", unit: "bytes" },
  api_default_line_page_size: { label: "行读取默认页大小", unit: "行" },
  api_max_line_page_size: { label: "行读取最大页大小", unit: "行" },
  api_max_line_page_bytes: { label: "行读取响应最大大小", unit: "bytes" },
  api_concurrent_line_reads: { label: "行读取并发数", unit: "个" },
  api_concurrent_line_reads_per_client: {
    label: "单客户端行读取并发数",
    unit: "个",
  },
  api_default_search_results: { label: "搜索默认结果数", unit: "个" },
  api_max_search_results: { label: "搜索最大结果数", unit: "个" },
  api_max_search_window: { label: "搜索窗口最大结果数", unit: "个" },
  temp_results_max_result_size: { label: "单次搜索结果容量", unit: "bytes" },
  temp_results_max_total_size: { label: "临时结果总空间", unit: "bytes" },
  temp_results_max_records: { label: "临时结果最多保留份数", unit: "个" },
  temp_results_concurrent_materializations: {
    label: "临时结果物化并发数",
    unit: "个",
  },
  temp_results_max_scan_duration_seconds: { label: "搜索超时时长", unit: "秒" },
};

const RESOURCE_MODE_LABELS: Record<string, string> = {
  upload_concurrent_processing_tasks: "上传并发模式",
  upload_concurrent_receive_tasks: "上传接收模式",
  search_tantivy_writer_heap_size: "Tantivy writer 堆预算模式",
  api_concurrent_line_reads: "行读取并发模式",
  temp_results_concurrent_materializations: "临时结果物化模式",
};

const ACTION_LABELS: Record<string, string> = {
  ADMIN_BOOTSTRAPPED: "初始化管理员",
  INVITATION_CREATED: "生成邀请码",
  INVITATION_REVOKED: "撤销邀请码",
  INVITATION_REDEEMED: "使用邀请码注册",
  AUTH_SETTINGS_UPDATED: "认证设置变更",
  USER_STATUS_CHANGED: "变更用户状态",
  USER_SESSIONS_REVOKED: "注销用户 Session",
  SETTINGS_UPDATED: "系统配置变更",
  SYSTEM_SETTINGS_INITIALIZED: "系统配置初始化",
};

function parseJsonObject(raw: string | null | undefined): Record<string, unknown> | null {
  if (!raw) return null;
  try {
    const parsed: unknown = JSON.parse(raw);
    return parsed !== null && typeof parsed === "object" && !Array.isArray(parsed)
      ? (parsed as Record<string, unknown>)
      : null;
  } catch {
    return null;
  }
}

function stringArray(value: unknown): string[] {
  return Array.isArray(value) && value.every((item) => typeof item === "string")
    ? value
    : [];
}

function formatBytes(value: number): string {
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let amount = value;
  let unitIndex = 0;
  while (amount >= 1024 && unitIndex < units.length - 1) {
    amount /= 1024;
    unitIndex += 1;
  }
  const formatted = Number.isInteger(amount)
    ? String(amount)
    : amount.toFixed(2).replace(/0+$/, "").replace(/\.$/, "");
  return `${formatted} ${units[unitIndex]}`;
}

function formatValue(field: string, value: unknown): string {
  if (value == null) return "—";
  if (typeof value === "boolean") {
    return field === "allow_registration" ? (value ? "开启" : "关闭") : value ? "是" : "否";
  }
  if (Array.isArray(value)) return value.length ? value.map(String).join("、") : "无";
  const metadata = FIELD_METADATA[field];
  if (metadata?.unit === "bytes" && typeof value === "number") return formatBytes(value);
  if (metadata?.unit) return `${String(value)} ${metadata.unit}`;
  return String(value);
}

function formatResourceMode(value: unknown): string {
  if (value === "auto") return "自动";
  if (value === "manual") return "手动";
  return formatValue("", value);
}

function auditFieldLabel(field: string): string {
  const knownLabel = FIELD_METADATA[field]?.label;
  if (knownLabel) return knownLabel;
  const normalized = field.toLowerCase().replace(/[^a-z0-9]/g, "");
  if (normalized.includes("apikey")) return "API 密钥";
  if (normalized.includes("password") || normalized.includes("passwd")) return "密码";
  if (normalized.includes("token")) return "访问令牌";
  if (normalized.includes("secret") || normalized.includes("credential")) return "敏感配置项";
  return field;
}

function formatChange(change: AuditChange): string | null {
  if (!change || typeof change.field !== "string") return null;
  const label = auditFieldLabel(change.field);
  if (change.redacted) return `${label}：已修改`;

  const parts: string[] = [];
  if (JSON.stringify(change.old_value) !== JSON.stringify(change.new_value)) {
    parts.push(
      `${label}：${formatValue(change.field, change.old_value)} → ${formatValue(change.field, change.new_value)}`,
    );
  }
  const resourceMode = change.resource_mode;
  if (resourceMode && resourceMode.old_value !== resourceMode.new_value) {
    const modeLabel = RESOURCE_MODE_LABELS[change.field] ?? `${label}模式`;
    parts.push(
      `${modeLabel}：${formatResourceMode(resourceMode.old_value)} → ${formatResourceMode(resourceMode.new_value)}`,
    );
  }
  return parts.length ? parts.join("；") : null;
}

function structuredChanges(details: AuditDetails | null): AuditChange[] | null {
  if (!Array.isArray(details?.changes)) return null;
  return details.changes
    .filter(
      (change): change is AuditChange =>
        change !== null && typeof change === "object" && !Array.isArray(change),
    )
    .filter((change) => typeof change.field === "string");
}

function legacyChanges(
  details: AuditDetails | null,
  oldValue: string | null,
  newValue: string | null,
): AuditChange[] {
  const oldObject = parseJsonObject(oldValue);
  const newObject = parseJsonObject(newValue);
  if (!oldObject || !newObject) return [];
  const changedFields = stringArray(details?.changed_fields);
  const isRedacted = (value: unknown) =>
    value !== null &&
    typeof value === "object" &&
    !Array.isArray(value) &&
    (value as Record<string, unknown>).redacted === true;
  const fields = changedFields.length
    ? changedFields
    : [...new Set([...Object.keys(oldObject), ...Object.keys(newObject)])].filter(
        (field) => JSON.stringify(oldObject[field]) !== JSON.stringify(newObject[field]),
      );
  return fields
    .filter(
      (field) =>
        isRedacted(oldObject[field]) ||
        isRedacted(newObject[field]) ||
        JSON.stringify(oldObject[field]) !== JSON.stringify(newObject[field]),
    )
    .map((field) =>
      isRedacted(oldObject[field]) || isRedacted(newObject[field])
        ? { field, redacted: true }
        : { field, old_value: oldObject[field], new_value: newObject[field] },
    );
}

export function auditActionLabel(action: string): string {
  return ACTION_LABELS[action] ?? action;
}

export function formatAuditSummary(log: AuditLog): AuditSummary {
  const actionLabel = auditActionLabel(log.action);
  if (log.action === "SYSTEM_SETTINGS_INITIALIZED") {
    return { actionLabel, summary: "系统配置已初始化", changes: [], allChanges: [], hiddenCount: 0 };
  }

  if (log.action === "SETTINGS_UPDATED") {
    const details = parseJsonObject(log.details_json) as AuditDetails | null;
    const structured = structuredChanges(details);
    const allChanges = (structured ?? legacyChanges(details, log.old_value, log.new_value))
      .map(formatChange)
      .filter((change): change is string => change !== null);
    if (allChanges.length) {
      return {
        actionLabel,
        summary: "",
        changes: allChanges.slice(0, 3),
        allChanges,
        hiddenCount: Math.max(0, allChanges.length - 3),
      };
    }
    if (structured) {
      return { actionLabel, summary: "—", changes: [], allChanges: [], hiddenCount: 0 };
    }
    if (parseJsonObject(log.old_value) && parseJsonObject(log.new_value)) {
      return { actionLabel, summary: "—", changes: [], allChanges: [], hiddenCount: 0 };
    }
  }

  const summary = log.old_value || log.new_value
    ? `${log.old_value ?? "—"} → ${log.new_value ?? "—"}`
    : "—";
  return { actionLabel, summary, changes: [], allChanges: [], hiddenCount: 0 };
}
