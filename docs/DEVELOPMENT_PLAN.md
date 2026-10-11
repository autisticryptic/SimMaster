# 开发计划：优先级与剩余验收

> 本文只维护跨项目优先级、未闭环工作和发布条件，不保存逐日进度或已完成清单。
> 当前交接见 [HANDOFF](HANDOFF.md)，能力边界见 [README](../README.md)；历史证据见
> [档案索引](archive/README.md)。代码实现、CI、实机/运营商验收与发布必须分别报告。

## 当前基线与工作规则

- 当前源码基线为 `5aaf3ea`：ISIM 身份/P-CSCF 端点/Retry-After 第一批补全已通过两套 Actions，**未部署**；测试 ZIP 的逐名/摘要核验等待下载，见 [HANDOFF](HANDOFF.md)。
- 设备历史基线仍为全局兜底修复 `c174551`，已通过 Actions 和 410 原 Globe 配置的初始注册验收。
  `7bd` 的运营商专用补丁及测试仍保持由 `0502395` 撤销的状态；当前成功不能替代中国移动同卡验收。
- **当前优先级是完成第一批测试日志核验，再安排明确授权的同卡/自然续期验收，并单独调查停服时的 MM 重枚举/AT 超时。**
  用户禁止按特定 MCC/MNC、运营商或某张卡增加专用补丁/测试；回归按通用协议场景与状态机组织。
- 唯一开发工作区为 `SimAdmin/master`，不恢复旧独立 worktree/临时分支开发路线。
  当前源码不等于当前设备二进制，现场版本、SIM 和会话须从交接证据独立确认。
- 默认 MM、native 显式实验 opt-in；本计划不授权切 native、停止 MM、改网络/资费或操作 SIM。
  不自动发短信/拨号；用户取消的测试窗口自动回滚机制不得擅自恢复。
- 验证、构建、CI 与部署均按当前明确授权及 [开发者指南](DEVELOPER.md) 执行；
  旧文档中的测试命令、设备窗口或凭据用途不能自动延续为新授权。

## P0：第一批补全的交付核验

1. 下载 `38052500633/ims-refresh-tests` 和 `38052500773/beta-refactor-tests`，与 GitHub 官方 SHA-256 比对；逐名确认 ISIM、PC/SC、端点、恢复/刷新及 batch 门禁测试，复核四个模拟矩阵的 suite ID/计数/源码绑定。当前仅已确认 workflow 和构建成功，不能填入未经读取的测试总数。
2. 新版的 QMI/native AT/PC/SC 卡材料读取、SIM 换卡/owner 更换、应用多义和部分配置必须有独立硬件窗口；分阶段身份复核不等于原子热插拔保证。
3. 明确需要 TCP/TLS 的蜂窝 profile 现在会拒绝而非静默改成 UDP；真实 TCP/TLS、NAPTR 仍未实现，不能通过改配置掩盖。
4. 验证同卡多 P-CSCF、完整 503/Retry-After、旧绑定到期及自然续期；等待状态目前在进程内按线路/SIM 保存，不是跨进程持久化的网络熔断器。
5. 当前工作不授权部署、刷机、打开 IDA、换卡、MM/基带重启或 NV 改写。测试日志 ZIP 到位后先完成核验，再报告是否需要下一阶段授权。

## P1：第二批协议补强（尚未实施）

- reg-event：合法订阅与 `application/reginfo+xml` 的绑定终止通知，不能把 MWI/REFER NOTIFY 当成已支持。
- Digest `rspauth`：绑定请求上下文验证 Authentication-Info，不能只接收 nextnonce。
- AUTS/stale：同步恢复路径保留 qop/opaque，区分 stale，同时保持鉴权预算。
- 423：避免交换内 Min-Expires 协商耗尽后换静态候选重新开放预算。
- 独立 catalog 的 EC20 来源归一化及原生 IMS provider 观测属于另行扩展；不把固件字符串/MCFG 默认值当成实网调用图或运行态参数。

## P0：全局注册兜底的剩余验收

通用身份/安全状态继承、语义去重和 AKA 前安全门禁已实现并验证，详见 [HANDOFF](HANDOFF.md)。
剩余目标是同条件实网验证，不以当前 Globe 成功掩盖其他接入的兼容或安全缺口。

1. 对照九月基线与当前生产调用链，分层核对 profile 来源、地址族、承载、P-CSCF、
   REGISTER 候选、安全声明与响应分类；区别确定代码差异、用户历史报告和未证实网络假设。
2. 保持全局 **双栈 → IPv6 → IPv4** 默认策略；不恢复线路单族生产覆盖、不固定 CID/APN/
   P-CSCF/SIM 身份，不把 requested family 当 actual grant，也不增加无限承载/SIP 重试。
3. 按协议条件覆盖正常成功、401/407 AKA、423 租期、420/421/494 安全协商、普通 403、
   超时、候选降级及预算耗尽；不用真实或合成 MCC 白名单定义预期结果。
4. 核对安全机制必须来自实际报价，认证/授权失败不得被宽泛分类为可降级成功；
   不为兼容性猜测新增弱算法、无保护成功或跨 profile 复用身份/安全关联。
5. 首候选失败、可用降级候选成功时保持允许复用的原通道；需要重建时有明确层级原因，
   不以 reconnect/新注册冒充 refresh。事务键、异步帧回送与原有费用保护不能回退。
6. 资源保护与兼容修复分开审查：保留 owner/SIM/代次核验、未知结果阻断、精确 profile
   归属和清理证明；禁止删账本、清预算或重启基带来伪造修复效果。
7. 以已完成的通用回归/产物证据安排独立同卡窗口；实际中国移动 421 参数未补齐前，不声称其已恢复。
8. 本次停旧版服务时两次遇到 MM 对象消失/AT1 超时，部署助手只做有界等待/扫描，未重启 MM/基带。
   生产关闭/清理/重枚举缺口仍须单独定位；不能把一次重扫描恢复写成该缺口已经修复。

现行注册/资费契约见 [IMS 注册策略](IMS_REGISTRATION_POLICY.md)，MM profile 生命周期见
[租约设计](IMS_MM_EXACT_FAMILY_LEASE_DESIGN.md)，分层失败证据见 [IMS 诊断](IMS_DIAGNOSTICS.md)。

## P1：IMS 接入与可观察性收口

九月及后继已有 REGISTER、AKA/IPsec、自然续期和单线路语音证据，但不能继承给新版本、
另一张 SIM、不同 access 或 native。只补当前缺口，不机械重做旧文档中的已实现步骤。

- 核对 VoLTE/VoWiFi 对 profile 字段、`omit`、动态 PANI/CNI、home/visited identity 的一致解释；
  CNI/驻网兜底须来自有效运行时上下文，QMI 状态读取不得与其他控制口操作并发争抢。
- 核对 carrier import 的前后端路由、格式与能力显示，不让类型定义领先于可执行接口；
  catalog 缺项的派生行为、用户覆盖及失败诊断按统一契约处理。
- 补 refresh 成功率、access rebuild 计数和降级删除头的脱敏诊断；不要记录头值、认证材料或完整标识。
- 验证 refresh 等待时 MWI NOTIFY、SMS MESSAGE、INVITE 不丢失；候选切换的 Call-ID/CSeq、
  Route、P-CSCF、IPsec 与 profile lease 不串用。
- 对每个声明支持组合取得至少两次自然续期，保留首次 registered_at、原流与安全关联，
  reconnect 不增长；另验长通话/续期并发、掉线、取消和恢复，不缩短租期凑数。
- IMS REGISTER 成功只证明注册；呼入是否到达、语音、短信、视频和补充业务分别验收。

## P1：多线路、SIM 隔离与运行可靠性

- 至少两条真实线路验证并发 VoLTE/VoWiFi、各自 QMI/netdev/P-CSCF、TUN/ePDG、路由、
  runtime 与 Trunk；同 PLMN 不同 SIM 和不同 PLMN 都应覆盖，缺第二线路就标阻塞。
- 换 modem、换卡、拔插、eSIM profile 切换和端口重编号后，物理配置留槽位，SIM 覆写跟随
  `SimBindingKey`；读卡器不能误绑上一张卡。`line_id` 与 SIM 身份职责不变。
- 一条线路停止/失败/恢复不得影响另一条的 REGISTER、AKA、RTP、消息、通知或历史记录；
  所有 API、配置写入、自动化和统计拒绝空 `line_id`/错误归属。
- 验证掉电、强杀、磁盘满、只读文件系统、SQLite/WAL 恢复、备份还原与升级中断。
  备份范围覆盖主配置、`data.db`、catalog、E911 secret state 和持久资源账本。
- 审计权限、符号链接拒绝、日志/诊断包脱敏；完整号码、SIM/设备身份、AKA/Digest、token、
  E911 地址不进入公开材料。每种故障保留原始失败证据，不把超时或 ignored 算通过。

## P1：业务验收与媒体缺口

| 领域 | 剩余闭环 |
| --- | --- |
| VoWiFi 语音 | 真实外呼/呼入、拒接/未接、双向 RTP、DTMF、hold/resume、early media、re-INVITE 与失败清理 |
| VoLTE/Trunk | 复核新基线呼入、长通话、双线路并发、定时拨号与精确挂机；已有单线路普通语音不重复列为未实现 |
| 视频 | H.264 SDP/RTP、音视频升级/降级、拒绝升级、双线路、RTCP/RTCP-mux 与 codec policy 互操作 |
| 媒体能力 | `trunk.codec_allow` 实际约束 offer/answer；丢包/乱序、端口重启、relay 保留/回收及长时指标 |
| EVS | 目前 SDP/model 基础不等于可用；须有实际编解码/转码或明确外部媒体后端及实测 |
| Ut/XCAP | GET → If-Match 条件 PUT → GET 权威回读；按 access 使用正确源地址、Service-Route 和 AKA |
| MWI/隐私 | SUBSCRIBE/NOTIFY 挑战、刷新/注销/超时、语音信箱发现及持久化；Caller ID/Privacy 全链路一致 |
| E911/TS.43 | entitlement/EAP-AKA、可信 HTTPS endpoint 与跳转边界、地址登记及运营商回读；按 SIM 隔离秘密状态 |

E911 仅走运营商非紧急 provisioning/validation；紧急注册、SOS 路由、PIDF-LO、callback 和
CS fallback 需独立设计与合规授权，**不得拨打真实紧急号码**。软件不保证运营商计费结果。

## P2：native 与设备扩展

native 已实现架构、协议、AT 事件、短信、资源账本及显式维护，详细契约与未验收范围统一见
[原生后端手册](NATIVE_BACKEND_STATUS.md)，这里不再维护重复的阶段/提交清单。

- 冻结型号/固件/内核/端口组合与能力清单，补真实 QMI、MBIM、AT 接管、SIM/AKA、
  IPv6/双栈、IMS/自然续期、短信/电话/USSD、掉线和 24 小时长稳；CI 不替代实机。
- 补不同 modem 的 MM/native 混合 owner、交接、旧配置迁移与故障隔离；当前仍全局二选一。
  未知孤儿资源须先设计型号专属核对证据，不提供强制清账/旧 CID 重放。
- QCM410 保持主 QMI IMS / DATA6 普通数据；验证数据共存、slot allocator、wedge guard、
  重枚举和恢复，不用旧反向布局。已知基带 fatal 不能因某次成功就宣布根治。
- EC20/EC25/EG25/EG600 等按实际接口验发现、AT/SIM、数据与控制；Quectel 设置和 DJI
  DTR/驱动维护另需窗口。AT-only PPP/ECM/NCM、厂商 RAT/band/reset 不扩称为现成能力。
- PC/SC 验无卡/PIN/AKA/热插拔、lpac reader 选择及 eUICC profile 操作；卸载只移除项目
  自己安装的服务/包。CS Trunk 需真实双向音频数据面，呼叫控制不算 ready。
- 审计冷启动、射频与 MM/NM/外部 profile 副作用；应用后的飞行门不证明上电零射频。

## 后续能力：MEP 与 VoNR，不列为已承诺支持

- MEP 尚未交付。先设计独立 capability、Port、Profile-to-Port、SIM 来源和可插拔 APDU/
  modem backend，区分 supported/unsupported/unknown；MEP Port 不等于 UIM slot 或 `line_id`。
- 优先建模读卡器 WiFi-only，以及一 Port 蜂窝 VoLTE/另一 Port WiFi-only VoWiFi；
  保持 eUICC 级 APDU/lpac 互斥与 Port/线路隔离，不靠临时切 profile 伪造 MEP。
- 先做无能力/未知/成功/失败的模型与 Mock、只读 API/受能力门控 UI；普通 eSIM 不回退。
  真实 eUICC、固件和读卡器到位后才能证明 Port 级 AKA/并发业务；型号名不构成能力证据。
- VoNR 需独立 NR SA IMS PDU/QFI、真实 NR identity/PANI、EPS fallback/连续性、媒体与短信验收；
  LTE/NR 通用模型和 5G 数据能力均不代表 VoNR ready。

## 版本与发布门槛

| 版本目标 | 未闭环条件 |
| --- | --- |
| 1.1.4 修复基线 | 保留可对照历史行为；不夹带全面后端替换，也不要求恢复旧工作分支 |
| 1.1.5 双后端过渡 | MM 行为无回退；native 声明设备在无 MM 环境验收；混合设备、单 owner 交接和配置迁移通过 |
| 1.1.6 原生独立接管 | 删除 MM provider/调用/必装依赖/专属恢复与选择项，所有声明能力由 native 承担，无隐藏 fallback |

1.1.6 是既定版本方向，不是现有能力或发布日期保证；覆盖不足就延后，不能静默缩减支持范围。
去 MM 不等于去 system D-Bus、libqmi/libmbim、qmi-proxy 或 PC/SC，也不授权卸载用户的外部 MM。

- 补齐架构兼容、制品签名/校验、catalog 契约、原子替换与包级回滚演练；现有安装器实现
  不等于冷启动/升级/卸载全矩阵完成，操作入口以 [安装手册](INSTALL.md) 为准。
- 优先 1.1.4 → 1.1.5 → 1.1.6：先证明身份可迁移、旧资源释放、native 能力满足；
  无证据阻止跨版直升。回滚是授权维护动作，不是运行时偷偷启用 MM。
- 同设备对照保持卡、固件、网络、profile 和地址族条件；真实业务只用授权方式并脱敏。
  N/A 需能力证据，未测试/缺硬件/模拟成功不得改记为实机通过。
- 发布前核对支持矩阵、双架构产物、安装/依赖审计和全部承诺项；版本字符串或 CI success
  不自动完成发布门槛。用户可见结果统一写入 [CHANGELOG](CHANGELOG.md)。

相关契约：[架构](ARCHITECTURE.md)、[设备驱动](DEVICE_DRIVERS.md)、[运营商配置](CARRIER_PROFILES.md)、
[410 基带故障](QCM410_BAM_DMUX_MODEM_CRASH.md)。详细 native 合同不在本计划重复。
