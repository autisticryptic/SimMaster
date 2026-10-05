# 文档导航

先按用途选入口，不必逐一阅读历史报告。

## 开始与接手

- **[当前接手与项目状态](HANDOFF.md)**：当前版本/主线、已完成与未验收边界、设备离线状态、本地资料位置和新对话提示。
- [单主分支整合与维护](BRANCH_CONSOLIDATION_2026-10-05.md)：master统一代码、临时分支清理门槛、历史备份与私有工作区保留。
- [开发总计划](DEVELOPMENT_PLAN.md)：长期功能、硬件与发布验收；旧条目按最新代码证据核对。
- [设备后端路线图](MODEM_BACKEND_ROADMAP_1.1.5_1.1.6.md)：1.1.5 过渡与 1.1.6 原生接管目标，不等于发布验收通过。
- [安装、eSIM 与数据库变体本轮交付](INSTALL_ESIM_CATALOG_2026-10-02.md)：2026-10-02 本地改动、12份产物和当时验证边界。
- [跨MM/SIM资源恢复补强](IMS_CROSS_OWNER_RECOVERY_2026-10-04.md)：自动absence收尾、显式plan恢复、持久化防重放及71c970d正式部署/注册验收。
- [换卡屏障与历史卡注册回归](IMS_SWITCH_REGRESSION_2026-10-04.md)：Actions验证、42场景、双架构产物及Globe受控维护后的实网恢复。
- [数据库精简产物Actions验证](CATALOG_ACTIONS_2026-10-04.md)：四源12库减55.70%、v0.3.1公开发布及20文件独立下载核验。
- [410重启恢复与EID修复](IMS_REBOOT_RECOVERY_2026-10-03.md)：此前部署、注册/续期、真实维护中断及收尾状态。
- [Minimal精简差距审查](CATALOG_PRUNING_AUDIT_2026-10-03.md)：旧版实际减幅、保留门槛与报告口径修复。
- [运行时精简与小米VoWiFi调查](CATALOG_RUNTIME_MINIMAL_2026-10-03.md)：此前体积减半及小米提取缺陷调查。
- [小米完整OTA VoWiFi修复](XIAOMI_VOWIFI_FULL_OTA_2026-10-03.md)：最新完整固件重提取、380条静态配置、测试后推送及实网验收边界。
- [版本更新记录](CHANGELOG.md)；[历史档案索引](archive/README.md)。

## 使用、运行与开发

| 文档 | 用途 |
|---|---|
| [安装](INSTALL.md) / [环境](ENVIRONMENT.md) | 已校验发布包安装、维护激活、依赖、路径、systemd 与数据管理 |
| [架构](ARCHITECTURE.md) / [开发者指南](DEVELOPER.md) | 模块职责、开发和测试；Rust 编译只在 Actions |
| [Bruno API](../bruno-api/README.md) | 可执行 API 请求及环境配置 |
| [设备驱动](DEVICE_DRIVERS.md) / [UE namespace](ue-network-namespaces.md) | 驱动边界、线路/网络隔离 |
| [运营商 Profile](CARRIER_PROFILES.md) | catalog 来源与匹配边界 |
| [Hickory DNS](DNS_HICKORY.md) | 已完成的系统解析迁移及专用 DNS 非目标 |
| [IMS 命名与兼容](IMS_NAMING_MIGRATION.md) | 规范路由/字段、旧值兼容和持久化迁移 |

## IMS 协议与诊断

- [恢复受阻／无SIM时的物理线路展示](PASSIVE_LINE_INVENTORY_2026-10-05.md)：db54abc已部署，线路可见但操作门禁保留；当前SIM failure及注册尚未成功的边界。
- [CMCC 421→403定向修复候选](CMCC_421_CANDIDATE_2026-10-05.md)：实际派生回退确认、单次安全提示重报价、54场景/68相关回归及未实网验证边界。
- [蜂窝IMS实际协商补强](CELLULAR_IMS_SECURITY_NEGOTIATION_2026-10-03.md)：此前多机制报价、空加密安装修复及410第二机制注册证据。
- [派生协商前一轮补强](DERIVED_REGISTRATION_HARDENING_2026-10-03.md)：此前VoWiFi提案/IKE检查及LTE验证边界。

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
