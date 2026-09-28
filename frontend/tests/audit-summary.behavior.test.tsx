import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it } from "vitest";
import type { AuditLog } from "../src/api/types";
import { AuditSummaryCell } from "../src/features/admin/AdminPage";
import { auditActionLabel, formatAuditSummary } from "../src/features/admin/auditSummary";

function auditLog(overrides: Partial<AuditLog>): AuditLog {
  return {
    id: "audit-1",
    actor_type: "USER",
    actor_user_id: "admin-1",
    target_user_id: null,
    target_username: null,
    action: "SETTINGS_UPDATED",
    old_value: null,
    new_value: null,
    details_json: null,
    client_ip: null,
    created_at: "2026-09-28T00:00:00Z",
    ...overrides,
  };
}

it("labels settings audit actions and hides the initialization snapshot", () => {
  expect(auditActionLabel("SETTINGS_UPDATED")).toBe("系统配置变更");
  expect(auditActionLabel("SYSTEM_SETTINGS_INITIALIZED")).toBe("系统配置初始化");

  const summary = formatAuditSummary(
    auditLog({
      action: "SYSTEM_SETTINGS_INITIALIZED",
      new_value: '{"provider_api_key":"must-not-display"}',
    }),
  );
  expect(summary.summary).toBe("系统配置已初始化");
  expect(summary.changes).toEqual([]);
  expect(summary.summary).not.toContain("must-not-display");
});

it("formats structured settings changes, units, booleans, resource modes and redaction", () => {
  const log = auditLog({
    old_value: '{"temp_results_max_scan_duration_seconds":999}',
    new_value: '{"temp_results_max_scan_duration_seconds":1000}',
    details_json: JSON.stringify({
      changes: [
        {
          field: "temp_results_max_scan_duration_seconds",
          old_value: 30,
          new_value: 300,
          apply_mode: "hot",
        },
        {
          field: "temp_results_max_total_size",
          old_value: 1024 ** 3,
          new_value: 2 * 1024 ** 3,
          apply_mode: "hot",
        },
        {
          field: "allow_registration",
          old_value: false,
          new_value: true,
          apply_mode: "hot",
        },
        {
          field: "upload_concurrent_processing_tasks",
          old_value: 4,
          new_value: 4,
          apply_mode: "restart_required",
          resource_mode: { old_value: "manual", new_value: "auto" },
        },
        { field: "provider_api_key", redacted: true, apply_mode: "hot" },
      ],
    }),
  });
  const summary = formatAuditSummary(log);

  expect(summary.actionLabel).toBe("系统配置变更");
  expect(summary.allChanges).toEqual([
    "搜索超时时长：30 秒 → 300 秒",
    "临时结果总空间：1 GiB → 2 GiB",
    "允许注册：关闭 → 开启",
    "上传并发模式：手动 → 自动",
    "API 密钥：已修改",
  ]);
  expect(summary.changes).toEqual(summary.allChanges.slice(0, 3));
  expect(summary.hiddenCount).toBe(2);
  expect(summary.allChanges.join("\n")).not.toContain("999");
});

it("reads legacy changed_fields from full old/new JSON snapshots", () => {
  const summary = formatAuditSummary(
    auditLog({
      old_value: JSON.stringify({ issue_inactive_days: 7, allow_registration: true }),
      new_value: JSON.stringify({ issue_inactive_days: 30, allow_registration: true }),
      details_json: JSON.stringify({ changed_fields: ["issue_inactive_days"] }),
    }),
  );

  expect(summary.changes).toEqual(["Issue 闲置清理天数：7 天 → 30 天"]);
  expect(summary.hiddenCount).toBe(0);
});

it("shows historical sensitive changes without exposing their values", () => {
  const summary = formatAuditSummary(
    auditLog({
      old_value: JSON.stringify({ provider_api_key: { redacted: true } }),
      new_value: JSON.stringify({ provider_api_key: { redacted: true } }),
      details_json: JSON.stringify({ changed_fields: ["provider_api_key"] }),
    }),
  );
  expect(summary.changes).toEqual(["API 密钥：已修改"]);
  expect(summary.changes.join(" ")).not.toContain("secret");
});

it("does not fall back to full snapshots for a legacy mode-only update", () => {
  const snapshot = JSON.stringify({ upload_concurrent_processing_tasks: 4 });
  const summary = formatAuditSummary(
    auditLog({
      old_value: snapshot,
      new_value: snapshot,
      details_json: JSON.stringify({
        changed_fields: ["upload_concurrent_processing_tasks"],
      }),
    }),
  );
  expect(summary.summary).toBe("—");
  expect(summary.summary).not.toContain("upload_concurrent_processing_tasks");
});

it("falls back to the generic old-to-new summary for unparseable historical values", () => {
  const summary = formatAuditSummary(
    auditLog({ old_value: "old raw value", new_value: "new raw value" }),
  );
  expect(summary.summary).toBe("old raw value → new raw value");
});

it("expands a folded multi-field change list on demand", async () => {
  const user = userEvent.setup();
  const log = auditLog({
      details_json: JSON.stringify({
      changes: [
        { field: "issue_inactive_days", old_value: 7, new_value: 8 },
        { field: "temp_results_max_records", old_value: 100, new_value: 200 },
        { field: "temp_results_max_result_size", old_value: 1024, new_value: 2048 },
        { field: "temp_results_max_total_size", old_value: 4096, new_value: 8192 },
      ],
    }),
  });

  render(<AuditSummaryCell log={log} />);
  expect(screen.getByText("Issue 闲置清理天数：7 天 → 8 天")).toBeInTheDocument();
  expect(screen.queryByText("临时结果总空间：4 KiB → 8 KiB")).not.toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "查看全部（+1 项）" }));
  expect(screen.getByText("临时结果总空间：4 KiB → 8 KiB")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "收起变更" })).toBeInTheDocument();
});
