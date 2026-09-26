# 文档导航

先按用途选入口，不必逐一阅读历史报告。

## 开始与接手

- **[当前接手与项目状态](HANDOFF.md)**：当前版本/主线、已完成与未验收边界、设备离线状态、本地资料位置和新对话提示。
- [开发总计划](DEVELOPMENT_PLAN.md)：长期功能、硬件与发布验收；旧条目按最新代码证据核对。
- [设备后端路线图](MODEM_BACKEND_ROADMAP_1.1.5_1.1.6.md)：1.1.5 过渡与 1.1.6 原生接管目标，不等于发布验收通过。
- [版本更新记录](CHANGELOG.md)；[历史档案索引](archive/README.md)。

## 使用、运行与开发

| 文档 | 用途 |
|---|---|
| [安装](INSTALL.md) / [环境](ENVIRONMENT.md) | 手动部署、依赖、路径、systemd 与数据管理 |
| [架构](ARCHITECTURE.md) / [开发者指南](DEVELOPER.md) | 模块职责、开发和测试；Rust 编译只在 Actions |
| [Bruno API](../bruno-api/README.md) | 可执行 API 请求及环境配置 |
| [设备驱动](DEVICE_DRIVERS.md) / [UE namespace](ue-network-namespaces.md) | 驱动边界、线路/网络隔离 |
| [运营商 Profile](CARRIER_PROFILES.md) | catalog 来源与匹配边界 |
| [Hickory DNS](DNS_HICKORY.md) | 已完成的系统解析迁移及专用 DNS 非目标 |
| [IMS 命名与兼容](IMS_NAMING_MIGRATION.md) | 规范路由/字段、旧值兼容和持久化迁移 |

## IMS 协议与诊断

- [IMS 只读诊断](IMS_DIAGNOSTICS.md)：初始 REGISTER、AKA、终止响应、脱敏和运行程序核对。
- [注册策略](IMS_REGISTRATION_POLICY.md)、[接入共存](IMS_ACCESS_COEXISTENCE.md)、[REGISTER 三态字段](IMS_REGISTER_TRISTATE_SCHEMA.md)。
- [自然续期](VOLTE_REFRESH.md)、[VoWiFi 回退审计](VOWIFI_REGISTRATION_FALLBACK_AUDIT.md)。
- [MM exact-family lease 设计](IMS_MM_EXACT_FAMILY_LEASE_DESIGN.md)：保留其具体设计/历史边界，状态以当前源码为准。
- [QCM410 崩溃调查](QCM410_BAM_DMUX_MODEM_CRASH.md)：设备故障证据与保护要求，不跨型号套用。

## Native 硬件接口

**[当前状态与未完成项](NATIVE_BACKEND_STATUS.md)** 是 native 的总入口。

- [被动发现](NATIVE_MODEM_DISCOVERY.md)
- [AT / URC 事件](NATIVE_AT_EVENTS.md)
- [持久化短信与送达报告](NATIVE_SMS_INBOX.md)
- [SIM/APDU 通道账本](NATIVE_SIM_CHANNEL_LEDGER.md)
- [资源账本与显式恢复](NATIVE_RESOURCE_RECOVERY.md)
- [Quectel / DJI 维护](NATIVE_DEVICE_MAINTENANCE.md)
- [eSIM MEP 规划](ESIM_MEP_INTERFACE_PLAN.md)

## 归档与本地材料

- 日期化的排查、旧接手、旧分支和阶段计划统一放在 `docs/archive/`，保留证据但不充当当前操作步骤。
- 原根目录会话、临时脚本、下载、设备证据和本地检查点集中在 `.local/`，不随 Git 分发。
- `.local/` 可能含历史凭据、数据库与私钥；不能当作普通发布附件，也不能整体删除。
- 唯一开发工作区为 `SimAdmin/master`；`SimAdmin-1.1.5` 已完成合入确认并移除，不再维护第二份代码树。
