# 开发者指南

本文档是前端、后端和整包构建的唯一开发入口。子目录不再分别维护容易失真的 README。

## 项目结构

```text
.
├── backend/
│   ├── src/             Rust 后端源码
│   ├── Cargo.toml       crate 配置与依赖
│   └── build.rs         VERSION、Git 分支和提交号注入
├── frontend/
│   ├── src/             React + TypeScript 前端
│   ├── package.json     脚本和依赖
│   └── vite.config.ts   开发代理、版本注入和生产分包
├── bruno-api/           Bruno API 调试集合
├── deploy/              udev 规则、辅助 systemd 单元及待重构安装资源
├── scripts/             构建、真机测试及待重构部署/OTA 脚本
├── docs/                项目文档
├── VERSION              单一版本号来源
├── install_latest.sh    待重构，当前不作为安装入口
└── uninstall.sh         待重构，当前不作为卸载入口
```

## 后端架构

`backend/src/main.rs` 负责 CLI、初始化、路由装配和 HTTP 服务；`state.rs` 保存共享的
`AppState`。其余代码按依赖方向分为五个领域：

```text
backend/src/
├── api/                       HTTP handler、DTO、密码与会话认证
├── connectivity/
│   ├── core/                  共享 IMS/SIP/AKA/短信/语音核心
│   └── modems/softstack/
│       ├── volte/             IMS bearer、ip xfrm、SIP、RTP 与语音
│       └── vowifi/            IKEv2/ESP、ePDG、TUN、SIP 与运营商 Profile
├── hardware/
│   ├── cellular/              ModemManager、QMI、AT、数据代理和线路控制
│   └── sim/                   lpac/eUICC 操作
├── services/
│   ├── line_registry.rs       每条物理线路及其运行时注册表
│   ├── orchestrator/          SMS/语音多路径选路、接收腿选举和去重
│   ├── trunk/                 每线路 SIP Trunk 与桥接
│   ├── messaging/             短信监听和验证码提取
│   ├── notify/                通知发送及失败队列
│   ├── automation/            调度器与自动化动作
│   ├── network/               WLAN、DDNS 和网络诊断
│   └── system/                状态、系统事件和 OTA
└── platform/                  持久化配置、SQLite 和通用系统工具
```

架构约束：

- `connectivity/core` 不依赖具体接入腿；VoLTE 和 VoWiFi 复用相同的 SIP、AKA 和业务模型。
- `hardware` 只封装设备与固件操作，跨接入的策略放在 `services/orchestrator`。
- 所有会影响线路状态的 API 都应显式解析 `line_id`，不要重新引入“取第一个 modem”的全局接口。
- 同一物理 modem 的 D-Bus、QMI 和 AT 修改操作使用
  `hardware::cellular::serial::with_serial_for(modem_path, ...)` 串行化；不同 modem 可并行。
- carrier catalog 是只读基线，本地数据库只保存用户覆盖；运行时不应直接改写 catalog 文件。

## 前端架构

```text
frontend/src/
├── api/
│   ├── contracts.ts          与后端 DTO 对齐的 TypeScript 类型
│   └── current.ts            当前 REST API 封装
├── components/               布局和跨页面组件
├── contexts/                 主题等 React Context
├── hooks/                    通用 hooks
├── lib/                      Query Client 等基础设施
├── pages/                    路由页面及页面内组件
└── App.tsx                   登录保护、懒加载和路由表
```

前端采用 React 19、TypeScript 5、MUI 7、React Router 7、TanStack Query 和 Vite 7。
生产构建输出到 `frontend/dist/`，部署时复制到 `/opt/simadmin/www/`。

## 本地开发

### 前端

```bash
cd frontend
pnpm install --frozen-lockfile
pnpm dev
```

开发服务器监听 `http://127.0.0.1:5173`，并将 `/api` 代理到
`VITE_API_PROXY_TARGET`；未设置时目标为 `http://192.168.100.13:3000`：

```bash
VITE_API_PROXY_TARGET=http://127.0.0.1:3000 pnpm dev
```

常用检查：

```bash
pnpm lint
pnpm type-check
pnpm build
```

不要使用 `pnpm add` 代替依赖安装；依赖已经由 `package.json` 和 `pnpm-lock.yaml` 固定。

### 后端

```bash
cd backend
cargo check
cargo test
cargo clippy --all-targets
```

启动服务需要 system D-Bus，以及一个由 `carrier_Bundles` 生成并封存的 schema v7 SQLite
catalog。可以显式指定路径：

```bash
cargo run -- serve \
  --host :: \
  --port 3000 \
  --carrier-catalog /path/to/carrier-bundles.sqlite3
```

等价环境变量为 `HOST`、`PORT` 和 `SIMADMIN_CARRIER_CATALOG`。生产配置数据库可通过
`SIMADMIN_CONFIG_DB` 覆盖；该路径必须指向 SQLite 数据库，不支持旧 JSON 配置导入。
日志级别通过 `RUST_LOG` 控制。

普通开发机没有 ModemManager、真实 modem、QMI 端点或 `/dev/net/tun` 时，纯逻辑测试仍可
运行，但服务启动和硬件接口可能失败。真机网络、P-CSCF、IMS 注册、RTP、eSIM 和 Trunk
互通必须在目标设备验证。

## CLI

```text
simadmin [--host HOST] [--port PORT]                  # 启动服务，兼容旧调用
simadmin serve [选项]                                 # 显式启动服务
simadmin auth reset-password                          # 交互式重置管理员密码
simadmin auth clear                                   # 清除密码，恢复首次设置
simadmin inspect-modems                               # 脱敏输出 modem/SIM 线路清单
simadmin secondary-qmi-init [--dry-run]               # 准备每基带独立 IMS QMI 端点
simadmin extract-zip <archive> <target>               # 安装脚本使用的 ZIP 解压器
```

## 前后端契约与 API

后端路由集中在 `backend/src/main.rs`，处理函数和模型分别位于
`backend/src/api/handlers.rs` 与 `backend/src/api/models.rs`；前端对应
`frontend/src/api/current.ts` 和 `frontend/src/api/contracts.ts`。

新增或修改接口时至少同步检查：

1. 后端请求/响应 DTO。
2. handler、路由方法与认证边界。
3. 前端类型、API 封装和调用页面。
4. `bruno-api/` 中对应 `.bru` 请求及其环境变量。
5. 线路级接口是否始终携带并验证 `line_id`。
6. 兼容性、错误码和真机验收项是否需要写入文档。

除 `/api/health`、`/api/auth/status`、`/api/auth/setup`、`/api/auth/login` 和
`/api/auth/logout` 外，业务 API 默认受会话认证保护。会话使用 `simadmin_session`
HttpOnly Cookie；修改或清除管理员密码会让已有会话失效。

## 手动构建可部署产物

当前不要用 `scripts/build.sh` 生成发布包。前端和本机后端可以直接构建：

```bash
cd frontend
pnpm install --frozen-lockfile
pnpm build
cd ../backend
cargo build --release
cd ..
```

交叉构建 aarch64 musl 后端：

```bash
rustup target add aarch64-unknown-linux-musl
cargo install cargo-zigbuild
cd backend
SQLITE3_STATIC=1 LIBSQLITE3_SYS_USE_PKG_CONFIG=0 \
  cargo zigbuild --release --target aarch64-unknown-linux-musl
cd ..
```

可部署集合至少包含：

- 与设备架构匹配的 `simadmin` 二进制。
- `frontend/dist/`。
- 经过审核、已封存且契约兼容的 `carrier-bundles.sqlite3`。
- `scripts/simadmin.service`；需要 VoLTE 副 QMI 时再加入 `deploy/system/` 对应资源。

文件传输、目标路径和 systemd 安装命令见[手动安装指南](./INSTALL.md)。

## 自动安装与 OTA 状态

以下实现仍留在源码中供后续重构，但当前不构成受支持的开发或发布流程：

- `install_latest.sh`、`uninstall.sh`、`deploy/install.sh`。
- `scripts/build.sh`、`scripts/deploy.sh`、`scripts/pack-ota.sh`。
- Web OTA 页面以及 `/api/ota/*` 上传、下载和应用接口。

恢复这些入口前，需要先确定新仓库地址、制品命名、catalog 分发、完整性/签名校验、版本
兼容、原子替换和回滚契约。届时应重新编写脚本和文档，而不是只替换旧 URL。

## ModemManager 调试

```bash
mmcli -L
mmcli -m any
mmcli -m any --simple-status
mmcli -m any --location-get
mmcli -m any --signal-get
mmcli -m any --command='AT+CGSN'

dbus-monitor --system "sender='org.freedesktop.ModemManager1'"
busctl introspect org.freedesktop.ModemManager1 \
  /org/freedesktop/ModemManager1/Modem/0
```

排查多线路问题时同时记录 `line_id`、ModemManager object path、主/副 QMI 设备、netdev、
bearer path、P-CSCF 和 trace ID。发布前按[真机测试清单](../真机测试清单.md)执行回归。
