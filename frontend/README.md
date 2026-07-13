# SimAdmin 前端管理平台

基于 React + TypeScript + MUI 的现代化设备管理界面。

## 🚀 快速开始

### 1. 安装依赖

```bash
# 使用 pnpm (推荐)
pnpm add @mui/icons-material @mui/x-data-grid @mui/x-charts react-router-dom swr

# 或使用 npm
npm install @mui/icons-material @mui/x-data-grid @mui/x-charts react-router-dom swr
```

### 2. 启动开发服务器

```bash
pnpm dev
# 或
npm run dev
```

访问：`http://localhost:5173`

**注意**：需要同时运行后端服务 (Rust)，请在另一个终端中运行：

```bash
cd ..
cargo run
```

### 3. 生产构建

```bash
pnpm build
# 或
npm run build
```

构建产物输出到 `../www` 目录。

## 📁 项目结构

```text
src/
├── api/              # API 接口层
├── components/       # 可复用组件
├── hooks/            # 自定义 Hooks
├── pages/            # 页面组件
├── theme.ts          # MUI 主题
└── App.tsx           # 路由配置
```

## 🎨 功能特性

- ✅ 实时设备监控仪表盘与设备状态集成卡片
- ✅ SIM 卡基本信息展示与实体 eSIM 写卡及 Profiles 管理
- ✅ 蜂窝网络服务小区及邻区注册状态诊断
- ✅ WLAN 客户端连接管理与原生 DDNS 解析
- ✅ 对话式短信发送、接收与批量清理
- ✅ 自动化事件配置与多通道通知中心路由
- ✅ 极速 OTA 系统更新与安全凭证管理
- ✅ 响应式布局自适应与夜间暗色模式支持

## 🔧 技术栈

- React 19
- TypeScript
- MUI v7
- React Router v6
- Vite

查看 `SETUP.md` 获取详细配置说明。
