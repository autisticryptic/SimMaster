# 当前交接

本页是唯一当前交接入口。旧过程见[历史摘要](archive/README.md)；文档中的 PID、SIM 和脚本不是实时状态，不能直接重放维护操作。

## 用户要求与操作边界

1. 修复**全局蜂窝 IMS 注册兜底**，不增加运营商/MCC/MNC 分支或特例测试。
2. 日常只维护 `master`；**编译、Rust 测试和注册模拟仅在 GitHub Actions 执行**。
3. 保留原线路列表、详情和标签布局；文档只更新固定入口，不新增日期流水账。
4. 本轮已获授权部署到 SSH 410，并检查原卡、原配置的 IMS 注册；不重启 MM/基带，不盲删资源。
5. 私有 eSIM 报告、诊断日志、JSONL、凭据和 `.local` 不提交，不清理未知用户数据。

## 已完成的全局修复

源码提交 **`c174551705bc6c0699cd2d7775e56c803e09b92b`**，远端 `autisticryptic/SimMaster`（本地 `simmaster`）。

- 通用候选保留初始空 AKA 身份与 `Supported: sec-agree`，只允许撤回本地主动的非强制声明。
- 本 P-CSCF 已明确要求的安全条件跨静态候选继承；超时探测不冒充服务器的明确要求。
- 裸 421 的 Security-Server 仅作未认证提示，不强制单机制重报价，也不能作为 Security-Verify/SA。
- 已确认需要安全协商时，401/407 缺少可用安全参数且没有现存受保护通道，在 AKA 前停止；无保护 200 也拒绝。
- 继承后按请求语义去重，不因候选名称不同重复消耗 24 次预算；已有身份、算法与保护约束参与判断。
- 首包配置、算法白名单/strict 校验、双栈→IPv6→IPv4、配置来源顺序、认证后止损及原 UI 不改。
- `7bdc59f` 的运营商专用尝试早已由 `0502395` 撤销，未部署；本轮没有恢复它。

实现及边界见[IMS 注册协议](IMS_REGISTRATION_POLICY.md)。

## Actions 与产物证据

- [Build-Release 37561979219](https://github.com/autisticryptic/SimMaster/actions/runs/37561979219)：success，Publish Release skipped。
- [Validate Beta Refactor 37561979212](https://github.com/autisticryptic/SimMaster/actions/runs/37561979212)：success。
- 两套日志均逐项核验 **29 项相关 Rust 回归＋86 个场景**：32 个全局兜底、12 个安全提示、24 个标准、18 个既有历史场景。
- 32 个新增全局场景为 15 成功、17 预期拒绝；全部四矩阵为 45 成功、41 预期拒绝。不是运营商实网成功计数。
- 本机仅运行 303 项 Python 静态/mock 检查、编辑和校验下载；没有本机 Rust/前端编译。
- ARM64/AMD64 的官方 artifact digest、包 SHA、ELF、commit 和各 30 个包内文件均核验。
- ARM64 包 SHA-256：`4a2c66cef0695bfb9146fad131aec571d77691e44615af2910a8b6c1771d50d0`。
- 证据：`.local/evidence/global-register-fallback/verified.json`。文档收尾提交不代表另一个二进制。

## 410 部署与原配置验收

**2026-10-07 03:18:11 UTC 独立收尾，03:25:30 UTC 再次只读复检**，已 pin 的 WLAN SSH `192.168.100.13`：

- 正式程序 **c174551 / 1.1.5 / PID481703**；原 Globe `51502`、`derived_3gpp_lte_51502`。
- 自 **03:13:56 UTC** 保持同一次 **derived / IPsec / IPv4** 注册，最终复检已约 12 分钟。
- `last_error=null`、`reconnect_count=1`；新会话自然续期 **0**，没有缩短租期或冒称续期通过。
- 与首次维护前相比，四个配置表、配置文件、运行 catalog 和 SIM 身份摘要一致；20 个前端文件磁盘/HTTP 摘要通过。
- MM PID538、辅助进程 PID1409、boot 未变；MM 内部 modem 对象最终为 `/Modem/5`，不能说对象编号未变。
- 恢复定时器 active、设备侧守卫 accepted，无新增内核故障；原界面保持。

### 部署期间的失败与恢复，不能省略

首次停旧版 `4bc3f77` 时，清理超时，MM 对象消失，AT1 连续超时后对象被标记 invalid。安装预检在替换前停止，**当时新二进制尚未安装**。
恢复原服务并对现有 MM 单次 ScanDevices 后，原卡重新 IPsec 注册；保持超过 3 分钟，原配置/程序一致，定时器恢复。
随后只重试一次部署：停服后有界等待/单次扫描，重新核验同一 MM 进程、SIM、控制口及空闲资源，才安装新包。
旧同 boot/已停止 owner 的账本保留，由正常启动恢复流程处理，**未手工 Delete profile、删账本或重启 MM/基带**。
这证明升级前的关闭/重枚举路径有独立缺口；部署助手的有界处理不是该生产缺口已被修复。

- 首次失败/原服务恢复：`.local/evidence/global-register-fallback/deployment/`。
- 成功重试/备份路径：`.local/evidence/global-register-fallback/deployment-retry1/`，远端 `/opt/simadmin-staging/global-fallback-c174551705bc-retry1/backup`。
- 汇总：`.local/evidence/global-register-fallback/deployment-final.json`，最终复检 `final-live-check.json`。备份用于明确授权的回滚，不自动恢复旧数据库或账本。

## 剩余验收与历史基线

- **当前原配置的初始注册未发现回归，但不能代替新会话自然续期、通话/音频或中国移动同卡验收。**
- 中国移动实际 421 的完整安全参数仍缺失；不能据此认定缺算法、放宽 strict 或新增 MD5。
- 用户给出的九月成功时间附近为 `7c6cf86 / 09edc03`；Cloudflare 文档明确成功样本是 46011，不能冒认中国移动。
- 已确认 `39b387b` 改全局首包 Require/Proxy-Require，`bdffdef` 区分主动声明与 server-required；旧 generic 丢身份的问题因可达路径变化暴露。
- `98d0e09` 的完整报价/strict 变化是独立线索，本轮没有猜测回退。历史原文检索见[历史摘要](archive/README.md)。
- 停服导致 MM 重枚举/AT 超时需单独调查；当前正常新租约不是旧残留，不得重放旧清理命令。

## 固定入口

- [开发与 Actions](DEVELOPER.md)、[未完成计划](DEVELOPMENT_PLAN.md)、[诊断](IMS_DIAGNOSTICS.md)。
- [MM 生命周期](IMS_MM_EXACT_FAMILY_LEASE_DESIGN.md)、[运营商配置](CARRIER_PROFILES.md)、[文档导航](README.md)。
- 独立 catalog `v0.3.1-catalog-v7` 已发布；本轮未重建、裁剪或替换设备库，未发布新的程序 Release。
- 文档保持 15 份受控主题入口；旧 72 份全文保留在 Git 历史及 `.local/evidence/docs-consolidation-20261006/docs-before.zip`。
