# Rain
# Log Viewer Web Application

## 简介

这是一个日志查看工具的 Web 应用，允许用户通过浏览器上传日志文件（支持多种格式），并查看其内容。支持压缩包文件上传，并能够解压并检索其中的内容。

### 核心功能（MVP）

- **文件上传**：支持上传 `.txt`, `.log`, `.zip`, `.tar.gz` 等格式的文件。
- **日志展示**：上传后直接展示文件内容。
- **压缩包文件处理**：支持上传并解压 `.zip`, `.tar.gz` 等压缩包文件，能够查看其中的文件内容。
- **视图展示**：
  - **Files View**：查看文件列表（包括压缩包内容），并支持嵌套压缩包的逐级解析。
  - **Logs View**：搜索并查看所有日志文件的内容，快速定位感兴趣的日志信息。

## 技术栈

- **前端**：
  - React
  - Vite
  - TailwindCSS

- **后端**：
  - Rust (Actix / Rocket)
  - WebSocket（用于实时推送，未来版本）
  - `zip` / `flate2` 库（处理压缩包）

- **部署**：
  - Docker

## 安装与运行

### 1. 克隆代码库

```bash
git clone https://github.com/your-username/log-viewer.git
cd log-viewer

