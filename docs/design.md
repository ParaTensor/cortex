# Cortex Admin UI Design Specification (控制台设计规范)

`docs/design.md` 是前端 UI 设计的**唯一基准入口**。

## 一、设计基调与美学理念

本项目 Admin UI 是一个**高密度的 GPU 推理集群与真 KV-Cache 运维控制台**，而非营销展示页。界面风格必须保持**克制、技术感、沉稳、高信号密度**：
- **表面底色**：纯白与近中性灰底色，主内容区采用 `bg-muted/40`；
- **主强调色**：单一、识别度高的品牌蓝/靛青色（`--primary: 226 62% 40%`），严禁紫色暗黑风；
- **状态色彩**：明确、语义化的状态色（Success 绿色、Warning 橙黄色、Destructive 红色）；
- **响应与双语**：在中英文切换下均不发生文本截断或布局跳动。

---

## 二、十六条硬性纪律 (Hard Rules)

1. **严禁硬编码颜色**：禁止在业务组件内硬编码 HEX 颜色值或 Ad-hoc 颜色类（如 `text-red-500`、`bg-violet-600`），必须使用语义 Token（`text-destructive`、`bg-primary`、`text-muted-foreground`）。
2. **严禁原生 Select**：一律使用统一封装的 Radix `Select` / `SearchableSelect` 组件。
3. **严禁第二套主按钮**：全局保持单一主操作按钮样式体系。
4. **弹窗结构规范**：
   - 弹窗必须具备标准的 `DialogHeader`、`DialogContent` 与 `DialogFooter`；
   - 实体创建/编辑统一使用 Dialog/Drawer 浮层，严禁破坏列表上下文进行整页跳转替换。
5. **浮层关闭交互**：所有 Dialog / Sheet / AlertDialog 点击遮罩层（Overlay）或按下 `Escape` 键必须能够正常关闭。
6. **严禁浏览器原生弹窗**：严禁使用 `window.confirm()`、`alert()` 或 `prompt()`；危险删除操作必须使用 `AlertDialog`，轻量提示使用 `toast`。
7. **静默选中 (Quiet Selection)**：实体列表和侧边栏选中使用浅主色填充（`bg-primary/10` 或 `bg-sidebar-accent`）和字重加粗，**禁止使用左侧竖向彩色指示条或荧光边框**。
8. **主从布局 (Master-Detail)**：宽屏下列表与详情面板并列分栏展示；窄屏下自适应降级为 Sheet 抽屉，严禁直接覆盖列表。
9. **下拉选项纯净**：Select / Combobox 的选项中只展示实体名称，严禁将协议、状态、计数等元数据粗暴拼接进选项文本中。
10. **防布局跳动 (Layout Stability)**：创建向导或复杂表单中，条件渲染的区块必须提前预留高度（`min-h`），切换时不得引起页面或弹窗纵向跳动。
11. **键盘与无障碍 (A11y)**：所有纯图标按钮必须配置 `tooltip` 与 `aria-label`；焦点状态（Focus Ring）清晰可见。
12. **表格横向滚动**：表格的横向滚动条必须被约束在表格容器内部，不得溢出导致整页横向滚动。
13. **国际化完全对称**：禁止组件内硬编码中文或英文作为唯一文案；每次新增文案必须同时补齐 `locales/zh.ts` 和 `en.ts`。
14. **搜索与排序一致性**：
    - 排序项必须显式标明方向（如：“显存（高→低）”）；
    - 搜索框 Placeholder 必须说明可检索字段（如：“搜索节点 ID、IP、模型...”）。
15. **层级卡片扁平化**：卡片嵌套不得超过 2 层，避免层层包裹导致信息密度下降。
16. **拒绝虚假渐变**：严禁在标题或关键词上使用 CSS 渐变文本特效（Gradient Text）。

---

## 三、PR / 发布审查清单 (PR Checklist)

- [ ] 所有颜色均来自 CSS 语义变量（`primary` / `muted` / `destructive` / `border` 等）；
- [ ] 中英双语 `t('...')` 键值在 `zh.ts` 和 `en.ts` 中完全对齐；
- [ ] 列表排序标明了方向，搜索框标注了可搜字段；
- [ ] 弹窗遮罩点击与 Escape 关闭正常；
- [ ] 运行 `npm run lint` 与 `npm run build` 0 错误通过；
- [ ] 运行 `npm test -- --run` 全量单元测试通过。
