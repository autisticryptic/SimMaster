# SIM-04 / 1.1.5 续接验收（2026-09-24）

> 本记录区分代码、CI、制品、部署、MM IMS 与 native 验收。
> 代码 worktree：`SimAdmin-1.1.5`；分支：`dev/1.1.5-modem-backends`。
> 本文不包含入口凭据、SIP/AKA 原文、订阅者身份或原始设备日志。

## 1. 本轮完成

| 项目 | 提交与验证 |
|---|---|
| 命名迁移 | `afa7112` / `a4108f3` / `71513ea` 完成精确码表、错误码、JSON/配置、持久化迁移及 Bruno；本轮补齐记录 |
| 原生后端审计与被动发现 | `7a15a7f`；12 项 fake-sysfs Rust 回归在 CI 执行；两架构通过 |
| 命名迁移收尾回归 | `2129282`；数据库旧名迁移测试从“仅编译”补为实际执行；SMS-only profile 不误宣告 MMTEL |
| 数据库项目联动 | `carrier_Bundles:558a505`；LTE/NR IMS readiness 与 VoLTE/VoNR 语音标志解耦；29 单测及 CI 通过 |
| 本地检查 | 100 项 Python guards、8 项前端单测、cargo fmt 与 diff 检查通过；没有本地 Rust 编译/测试 |

`2129282` 的 [Validate](https://github.com/autisticryptic/SimMaster/actions/runs/35947763715)
和 [Build](https://github.com/autisticryptic/SimMaster/actions/runs/35947763717) 均 success；
arm64/amd64 成功，`Publish Release` 为 skipped。没有合并 master、升版或发布应用 Release。

## 2. 纠正“零自然续期”的旧判断

重新解析 T05 原始私密 journal：

- `27542f0` 初始 REGISTER 成功：2026-09-22 **19:18:17 UTC**。
- 成功记录共 8 条，其中 **7 条** `register_phase="refresh"`；对应 7 条成功后的重新调度。
- 首次续期 20:08:18 UTC，最后一次 2026-09-23 01:08:29 UTC。
- 网络租期 3600 秒，正常 `refresh_after_seconds=3000`，没有缩短测试租期。
- 01:12:17 UTC 收到退出信号，旧回滚结束了窗口；不是“此前 6 小时没有 refresh”。

此前按错误日志文案/未兼容引号的字段匹配得出的零计数无效。没有证据支持修改注册循环
去修复一个“续期完全不触发”的故障；本轮未为此改动注册状态机。

## 3. 既有正式服务实机证据（不是新构建验收）

2026-09-24 上午重新通过只读 API、systemd 和 journal 核对：

- `/opt/simadmin/simadmin` 正在运行 **`71513ea`**，不是早期原始版本或 T05 临时候选。
- 二进制 SHA256：`8f30958684439824831e99f4795f5ed36b1b50e9aae904846885bcad358fd6ed`。
- 同一服务自北京时间 **02:29:53** 启动；02:30:42 完成初始注册。
- 派生首槽 `derived_3gpp_lte_45507`，IPv6 / `wwan0`，`registered=true`，
  `reconnect_count=1`、`last_error=null`。
- API `register_refresh_count=9`；journal 从 03:20:44 至 **10:01:01** 记录 9 次
  成功后的续期调度，约每 3000 秒一次。
- 旧 `simadmin-sim04-*` 候选/rollback/sampler units 均未列出，测试 owner marker 不存在。

这证明 `71513ea` 的 **MM 路径**注册与自然续期通过；不证明 native owner 或新构建通过。

## 4. 最新构建部署与验收

用户已明确 SIM-04 是专用测试设备：以后直接原位运行最新通过 CI 的构建，不再使用
临时候选服务、克隆数据库或自动回滚定时器。运行路径是 `simadmin.service` / `/opt/simadmin`。

下载曾被 nightly.link HTTP 500 短暂阻塞；镜像恢复后已按独立 GitHub digest 验证并部署：

| 层次 | `2129282` 证据 |
|---|---|
| Build run | `35947763717` |
| arm64 artifact | `10787910283` |
| ZIP SHA256 | `7901607305ddfe83f7e8dd70362b282a52bea3d2ec96ca87f10ece1f8db658dd` |
| tar.gz SHA256 | `74624c064079d0ed0ab5dda03f3fc21b4fc06277a7731aca203845a4ddbac266` |
| binary SHA256 | `afcc9ecbe4331dd3cfa31b392920bad1cf096fb0f10f35f864490804790d6588` |
| 正式服务启动 | 北京时间 10:57:57；PID `248663`；健康检查通过 |
| 默认后端 | 新二进制 `modem-backend-mode --require-mm` 返回 `modemmanager` |
| 注册 | 10:58:47 初始认证 REGISTER 成功；derived 首槽、IPv6、UDP、`wwan0`、reconnect=1 |
| 网络租期 | 3600 秒，正常 refresh_after=3000 秒；预计约 11:48:47 自然续期 |
| P-CSCF/Contact | 初始成功记录 service_route_count=1、contact_binding_count=1 |
| 回滚 | 无旧候选/rollback units、无测试 owner marker；管理默认接口 `wlan0` |

本地部署工具支持缓存 ZIP 续接，但每次仍先核对 GitHub digest，再校验包内 commit 和
二进制哈希；未绕过制品验证。

### 4.1 只读发现实机结果

`2129282 discover-native` 在 MM 运行时成功，运行前后应用 PID 均为 `248663`：

- 一个 SoC WWAN 物理设备、一个 QMI 控制口、两个 AT 口；宿主可见 7 个网口。
- IMS `wwan0` 已位于 UE namespace，因此不在宿主网口清单；没有把其缺失判成无硬件。
- 报告 `multiple_at_ports` / `at_port_requires_probe` / 物理键及 slot 待确认；
  没有擅自选择 `wwan0at0` 或填入 IMS/data endpoint。
- 候选 AT/IMS/data 均为 null，短信消费为 false；MM 仍 active，未切换 native。
- sysfs 建议键确实不同于 MM `qcom-soc` 物理 UID，印证不能承诺 line ID 自动一致。

### 4.2 自然续期观察（按用户要求暂停主动轮询）

本地只读观察器已取得 11:04:30、11:09:43 两次同会话样本，随后按用户要求停止，
以继续 native 接口补强和分支整理；**没有停止或回滚设备服务**。租期由设备正常维护。
之后可一次性回看 journal 与运行态，不需要为了取得验收而占用开发进度持续轮询。
验收要求原进程/invocation、UE namespace/UDP socket 指纹、session_started_at、registered_at、
P-CSCF 指纹、重连计数和传输端口不变，同时 API refresh 增长、journal 明确 refresh 成功、
CSeq 相对初始采样基线前进和新租期调度。解析器兼容带引号的 `register_phase`，
四项离线正/负回归通过。未用初始注册或旧版本的续期记录冒充本项通过。

## 5. 尚未完成的边界

- `2129282` 自己的自然续期待回看日志验收（下载、部署、只读发现、初始注册已通过）。
- native 端到端硬件验收；`efe6135` 已补 AT/URC 基础分流及短信事件提示调度，
  完整电话/注册事件层、Quectel 专用控制、DJI 维护写操作、APDU 通道账本仍未完成。
- 新 SIM 更换测试、真实短信和通话，均未在本轮擅自执行。

详见 [原生审计](NATIVE_BACKEND_AUDIT_2026-09-24.md)、
[被动发现说明](NATIVE_MODEM_DISCOVERY.md) 和 [命名迁移计划](IMS_NAMING_PHASE2_PLAN.md)。
