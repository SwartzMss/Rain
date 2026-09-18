# 前端暂时屏蔽 Skill 功能设计

## 目标

暂时停止用户从前端使用 AI Skill 相关功能，但保留后端实现、接口、数据库数据和后台任务，方便后续恢复。普通 Issue、日志搜索、文件查看和管理员其他设置不受影响。

## 范围

本次只调整前端展示入口：

1. 账户页不再展示“我的 Skills”标签和 Skill 管理页面。
2. Issue 文件查看页不再展示 Skill 诊断面板，也不再展示未登录用户的 Skill 诊断提示。
3. 管理员设置页不再展示 AI Provider 配置面板。

以下内容明确保留：

- `/api/me/skills`、Skill Review、Skill Run 和 AI Provider 后端接口；
- Skill 相关数据库表、历史数据和清理/恢复任务；
- 前端 API client 和类型定义，避免恢复时重新建立契约；
- 普通文件、日志和 Issue 浏览能力。

## 实现方案

采用最小前端改动：移除三个页面入口的渲染和仅服务于这些入口的 import、状态及回调，不删除 Skill feature 目录和 API 实现。这样用户界面不会触发相关请求，后台能力保持原样。

具体改动点：

- `AccountPage` 移除 SkillsPage import、tab 状态和 Skill tab，只保留账户安全表单。
- `FilesView` 移除 IssueSkillRunner、证据跳转回调和 SkillEvidence import。
- `AdminSettingsPage` 移除 AiProviderSettingsPanel import 及挂载。

## 测试策略

- 更新账户页行为测试，确认“我的 Skills”入口不显示。
- 更新 Issue 相关前端行为测试，确认 Skill 诊断入口不显示或不被挂载。
- 更新管理员设置行为测试，确认 AI Provider 面板不显示。
- 运行前端完整测试和类型/构建检查，确认移除入口后没有未使用依赖或回归。

## 恢复方式

后续恢复时重新挂载三个组件，并恢复对应 import、状态和回调即可；后端和数据库无需迁移。
