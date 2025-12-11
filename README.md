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
2. 前端安装
cd frontend
npm install
npm run dev

3. 后端安装
cd backend
cargo build --release
cargo run

4. 访问 Web 应用

打开浏览器，访问 http://localhost:3000 以查看应用。

功能列表
第一阶段（MVP）

文件上传：

支持上传 .txt, .log, .zip, .tar.gz 等文件格式。

上传文件后，直接展示其内容。

压缩包处理：

支持上传并解压 .zip, .tar.gz 文件。

在浏览器中展示压缩包内部的文件列表和内容，支持递归解析压缩包。

视图展示：

Files View：展示所有上传的文件以及压缩包内的文件信息。如果文件是压缩包，展示其内的文件列表，支持逐级解析压缩包。

支持 递归解析 压缩包，逐步查看压缩包内部的文件。

支持 文本文件预览，对于 .txt 和 .log 等文件，能够直接显示其内容。

Logs View：展示所有日志文件的内容，支持全文搜索，不关注具体哪个文件。

搜索功能：允许用户搜索所有日志文件中的关键字，迅速定位感兴趣的日志信息。

后续功能

实时日志推送（WebSocket）：

支持未来版本通过 WebSocket 推送日志更新。

日志过滤与搜索：

按时间、级别等条件过滤日志文件。

用户认证与权限管理：

支持多用户环境，只有授权用户才能上传和管理文件。

开发与贡献

欢迎贡献代码！如果你有任何建议或遇到问题，请提 Issue
 或提交 Pull Request。

License

MIT License
