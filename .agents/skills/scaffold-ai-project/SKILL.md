---
name: scaffold-ai-project
description: Scaffold high-performance Rust AI gateway / inference mesh projects and Admin UI consoles inheriting the ParaTensor / xrouter architecture, Rust 2024 edition stack, and strict design system. Use when bootstrapping new projects like cortex or creating new AI ecosystem subsystems.
---

# Scaffold AI Project — 架构与控制台脚手架规范

本 Skill 用于从 0 到 1 规范化初始化一个继承自 **ParaTensor / xrouter** 工业级标准的全新 AI 网关、推理网格（如 Cortex）或集群算力编排项目。

---

## 核心原则

1. **架构分层清晰**：
   - 后端：`server` / `main`（启动装配） → 领域模块（`mesh`, `kv`, `semantic`, `dispatcher`, `pool`） → `admin`（控制面） → `db`（数据存储与嵌入式迁移）。
   - 前端：`api-types`（类型契约） → `resources`（API Client） → `domain-hooks`（状态与轮询） → `pages`（布局与交互）。
2. **规范文件必须齐备**：
   - 项目根目录必须生成标准的 `AGENTS.md`（见 [templates/AGENTS.md.template](templates/AGENTS.md.template)）；
   - `docs/design.md` 必须作为前端唯一设计基准（见 [templates/design.md.template](templates/design.md.template)）；
   - `admin/src/index.css` 必须导入标准的 HSL 语义 Token（见 [templates/index.css.template](templates/index.css.template)）。
3. **零幻觉代码与验证闭环**：有声明必有调用，所有接口与核心算法必须包含单元测试与构建校验。

---

## 步骤清单 (Step-by-Step Execution)

### 步骤 1：生成项目核心规范文件
1. 将 `templates/AGENTS.md.template` 复制为新项目根目录的 `AGENTS.md`，替换项目名称与定位。
2. 将 `templates/design.md.template` 复制为新项目的 `docs/design.md`。
3. 创建 `docs/architecture.md` 阐述系统的数据面/控制面分层与核心调度流程。

### 步骤 2：初始化 Rust 后端技术栈 (`Cargo.toml`)
确保使用 Rust `2024` edition，并引入以下核心依赖：
```toml
[package]
name = "<project-name>"
version = "0.1.0"
edition = "2024"

[dependencies]
tokio = { version = "1.43", features = ["full"] }
axum = { version = "0.8", features = ["json", "ws", "macros"] }
tower = { version = "0.5", features = ["util"] }
tower-http = { version = "0.6", features = ["cors", "trace"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
sqlx = { version = "0.8", features = ["runtime-tokio", "tls-rustls", "postgres", "sqlite", "chrono", "uuid", "migrate"] }
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "stream", "json"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1.12", features = ["v4", "serde"] }
bcrypt = "0.16"
sha2 = "0.10"
anyhow = "1.0"
```

### 步骤 3：初始化前端 Admin UI (`admin/`)
1. 使用 Vite + React 19 + TypeScript + Tailwind CSS v4 初始化 `admin/` 目录。
2. 将 `templates/index.css.template` 写入 `admin/src/index.css`。
3. 安装基础组件依赖：
   - `@radix-ui/react-dialog`, `@radix-ui/react-dropdown-menu`, `@radix-ui/react-select`, `@radix-ui/react-tabs`, `@radix-ui/react-tooltip`
   - `lucide-react`, `sonner`, `zustand`, `react-router-dom`
4. 搭建经典四层布局：
   - `AppSidebar.tsx`（左侧折叠导航栏，静默选中 `bg-sidebar-accent`）；
   - `SiteHeader.tsx`（顶部栏，含 Breadcrumb, OrgSwitcher, UserMenu）；
   - `DashboardLayout.tsx`（主布局包装器）；
   - `DefaultPasswordBanner.tsx`（默认密码安全提示横幅）。

### 步骤 4：国际化与领域抽象落地
1. 在 `admin/src/lib/i18n/` 中初始化 `locales/zh.ts` 与 `locales/en.ts`，确保所有键值 100% 对称；
2. 按照 `api-types -> resources -> domain-hooks -> page` 四层抽象编写第一个业务模块。

### 步骤 5：全量自动化校验闭环
在交付前运行以下命令确保无编译警告与报错：
- 后端：`cargo check --tests && cargo test --lib`
- 前端：`cd admin && npm run lint && npm run build && npm test -- --run`
