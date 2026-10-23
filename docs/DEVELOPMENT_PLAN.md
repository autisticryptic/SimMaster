# 开发计划：优先级与剩余验收

> 本文只维护跨项目优先级、未闭环工作和发布条件，不保存逐日进度或已完成清单。
> 当前交接见 [HANDOFF](HANDOFF.md)，能力边界见 [README](../README.md)；历史证据见
> [档案索引](archive/README.md)。代码实现、CI、实机/运营商验收与发布必须分别报告。

## 当前基线与工作规则

- 本轮续接PacketSender/Router offline扩展：SHA绑定ELF/FDE/ARM64静态证实下层BE32原packet长度，并新增有界stream decoder、Router8-byte/opaque无扩展routing/有序平衡两片及一次原ARC transport；902f4f5已由Build39912321845/Beta39912321855/双架构与官方SHA ZIP精确各208模块回归/86矩阵核验，373本机静态/mock通过，Release skipped。本步ELF足够，只有后续确需才请求用户手动开IDA。IDA MCP拒绝连接，不自行启动/加载；数据仍是opaque TLS，不能直接交ARC或授ready。未知route/控制/扩展/顺序wrap拒绝；既有borrowed/copy snapshot不可由close撤销，live writer/TLS取消/消费排他仍未实现。下一重点是可信TLS与record-aware plaintext边界、真实socket/NetStream/ENC provisioning，继而SMS分类落库/provider/API/trunk/call-media/费用续期并发，不用缺配置伪造默认值或仅剩实机。证据`.local/jegotrip/resume-network-framing-evidence.md`。

- 用户暂无实网配置，最新要求继续离线/理论完善无忧行剩余内容。当前已还原并实现有界六词/ACK/32history/45s/2048dedup scheduler、exact configured recvOnlineMessage与offline组合；8efcd49已由Build39834770425/Beta39834770520/ARM64+AMD64及官方SHA ZIP逐名各172模块回归/四矩阵86场景核验，370静态/mock通过。不猜端点/trust/receiver-kind或计费，不授provider/外发；剩余下层网络/完整boundary/ENC部署、SMS分类/落库/provider/API/trunk/call-media/续期/并发仍是真实实现缺口，不能改写成只剩实机验收。

- 本轮继续用户已开启的 `libmtc.so` 只读IDA：5d4bbb0有两套Actions/双架构/官方SHA测试ZIP/95模块回归/四矩阵86场景中间证据；其swap第二串已发现应为原选SDK session ID而非返回ARC token，后者属于metadata `token`，phone token属于`pwd`、SipPwd另用。当前新增getAppConfigs、owned ARC login/swap、Common分片/FF FF 06 carrier和原owner/scope nested hosting接线已由684ef73 Build39812859460/Beta39812859585/双架构、官方SHA测试ZIP逐名139模块回归/四矩阵86场景核验，368本机静态/mock通过；不把offline协议子阶段/CI当完整客户端。剩余重点为Router/NetStream/NetSsl socket/packet边界与可信TLS握手、ENC provisioning/端点、batch ACK/dedup调度、网络入站对象和真实短信/通话、provider/runtime/persistence/API/trunk/计费/续期/并发；暂不部署、不访问设备或服务。

- 应用provider本轮迁到 `connectivity/app_voice/providers/{jegotrip,google_voice,telegram,imessage}`；无忧行controller接SDK两阶段/注销/失败时序由31d3739双架构及两套Actions各8项controller回归验证，旧IMS import兼容。DEX/JNI参数已确认但Linux wire帧未还原；继续native `Mtc_CliLogin`/`Mtc_SmgwUpdateStatus`等分析，必要时用户手开IDA。不把接口调度当作已能收短信/接打电话；暂不部署。

- **新增无忧行托管任务（静态分析）**：先走手动导入自有 root 手机凭据路线，不做登录/绑定 UI。SDK phone token 与 SIP 网关密码已定位；契约模块 d6381b3 已通过两套 Actions 各14项新测试/双架构；Linux backend、持久化/provider 接线、收短信/接打电话、计费/续期/设备并发仍未完成，不注册虚假 provider。详细参数与阶段见 [勘察](DEVELOPER.md#无忧行托管静态证据凭据与实现边界)；本轮不部署、不访问设备或无忧行服务。

- **用户已校正本轮范围：基于EC20固件分析补齐SimAdmin IMS注册逻辑并复核查漏，不修改独立carrier_Bundles。** catalog/原生诊断不是必做项或阻塞；X2已做的额外诊断不作为注册兜底成果继续扩展。最新已验证代码d373ffa保留，注册相关基线/证据见[HANDOFF](HANDOFF.md)，未部署。
- 设备历史基线仍为全局兜底修复 `c174551`，已通过 Actions 和 410 原 Globe 配置的初始注册验收。
  `7bd` 的运营商专用补丁及测试仍保持由 `0502395` 撤销的状态；当前成功不能替代中国移动同卡验收。
- **用户允许在仅剩实机验收时停止；当前只继续已确认的注册软件缺口。** 最后核查发现R7 XFRM代次竞态，修复/CI闭环后不再扩大可选能力；同卡/自然续期验收与停服MM重枚举/AT超时调查留给设备窗口。
  用户禁止按特定 MCC/MNC、运营商或某张卡增加专用补丁/测试；回归按通用协议场景与状态机组织。
- 唯一开发工作区为 `SimAdmin/master`，不恢复旧独立 worktree/临时分支开发路线。
  当前源码不等于当前设备二进制，现场版本、SIM 和会话须从交接证据独立确认。
- 默认 MM、native 显式实验 opt-in；本计划不授权切 native、停止 MM、改网络/资费或操作 SIM。
  不自动发短信/拨号；用户取消的测试窗口自动回滚机制不得擅自恢复。
- 验证、构建、CI 与部署均按当前明确授权及 [开发者指南](DEVELOPER.md) 执行；
  旧文档中的测试命令、设备窗口或凭据用途不能自动延续为新授权。

## P0：第一批补全的交付核验

1. 第一批代码和日志核验已闭环，用户无需补交测试 ZIP。若后续授权部署，再独立下载并核验同提交程序包的官方 digest、包 SHA、meta/ELF 和包内清单；测试 ZIP 不能替代程序包验收。
2. 新版的 QMI/native AT/PC/SC 卡材料读取、SIM 换卡/owner 更换、应用多义和部分配置必须有独立硬件窗口；分阶段身份复核不等于原子热插拔保证。
3. TCP/IPsec 与独立 TLS 已有 N6 软件验证；TLS 不替代 required-IPsec，私有 CA/mTLS/新反向 TLS-IPsec 未声明支持，互通仍需独立硬件窗口。
4. 验证同卡多 P-CSCF、完整 503/Retry-After、旧绑定到期及自然续期；等待状态目前在进程内按线路/SIM 保存，不是跨进程持久化的网络熔断器。
5. 当前工作不授权部署、刷机、打开 IDA、换卡、MM/基带重启或 NV 改写；下一阶段设备操作需要新的明确授权。

## P1：第二批协议补强（剩余项）

- reg-event 已实现并验证；仍需真实网络互通/设备验收。自定义或歧义 Contact 不猜绑定；不支持的订阅挑战、超时或 XML 缺口只停止/重同步观察，不伪造 REGISTER 丢失。
- REGISTER rspauth 已验证；后续互通须单独验证真实服务器响应。现有范围仅 qop=auth/无 qop 和单一 Authorization scope，不宣称 auth-int、多重 origin/proxy 鉴权或非 REGISTER 业务已经支持。
- **R9 ISIM刷新订阅已由9f1625d软件验证。** 独立只读 QMI UIM client 订阅 refresh（0x002A/0x002C/0x0033），忙时 deny≤20s；完成后重读 R5 快照，变化置 identity_stale，通话排空后重建；不支持则退回边界检查；仅蜂窝接入。同轮 Actions 验证通过。
- **R8身份来源策略已由9f1625d软件验证。** `identity_source` 从声明标签变为行为开关：`auto`（默认，兼容旧标签）/`isim_required`（缺 ISIM 失败关闭）/`usim_derived`（忽略 ISIM，身份与 AKA 走 USIM）；AID 与策略一致，诊断如实。用于“ISIM 有内容但运营商不期望使用”的场景；两工作流/双架构、各 540 相关回归/86 场景通过。
- **R7 XFRM原worker代次门禁已由d373ffa软件验证。** 安装、部分失败回滚、旧SA退役/重选/清理使用原捕获binding，不在检查后从handle重新bind；旧回包不能确认新进程成功或触发跨代删除。两套Actions/双架构、各522相关回归/86场景含5项实际worker队列/回包回归通过，342静态/mock通过；不改变算法/选择器或既有错误优先级。
- **R6原承载PCO动态复核已由6672a64软件验证。** 无活动对话的续期边界查询原provider；仅完整非空同族候选可触发重选，未知/部分IPv6不冒认撤销。原端点仍在则保留；确认替换在同PDN/worker新建REGISTER/SA并继承原IPsec/TLS，不借旧Digest，临时失败保留原截止/等待和旧端点显示，切换后重建观察器。两工作流/双架构、各517相关回归/86场景含8重选＋2时钟新回归通过；非PCO、WiFi IKE来源和活动对话不自动套用该策略。
- **R5同卡材料新鲜度已由6203511软件验证。** 现有只读QMI/native AT/PCSC在REGISTER/AKA/续期/注销边界做两次稳定快照，旧材料变化/读取失败不借USIM缺省或旧租期混过；同卡重建不清等待，已发送计数不回退。两工作流/双架构、各507相关回归/86场景含19新回归通过；R6已有后继验证。实机/REFRESH事件订阅与特殊mode不由本项推断。
- **R4注销反馈已由0351006软件验证。** 双接入注销新收到临时拒绝/Retry-After写入发送前捕获的同订阅等待；清理和晚到成功不清等待，旧卡响应不影响新卡，诊断hook无控制副作用。两套Actions/双架构、各488相关回归/86场景含16项新回归通过；首轮可选Contact零租期夹具错误已修复留证。不新增注销重试或恢复旧租期。
- **R3首次认证后423已由31d09ff软件验证。** WiFi直接Digest和protected AKA将423交回共享budget，重算nc/request-proof，不重装ESP、不重开候选；两套Actions/双架构、各472相关回归/86场景含10项新真实路径测试通过。需要新安全上下文的受保护挑战仍明确交恢复层，非REGISTER业务恢复不混入本项。
- 蜂窝刷新 423→400/新鉴权后400的预算补漏已由 `b6a7888` 验证；后续需保持结构化门禁、Min-Expires来源和旧通道/租期/计数，不回到末次状态猜测。
- R2 普通 WiFi 等待已由 `c3fb036` 软件验证，包含初次/刷新/注销/API门禁、同卡预算连续性、原租期保留和reg-event撤销竞态；实机仍未验收，不能由CI推断实际运营商接受情况。
- N1～N8、R1～R7已有软件CI/制品闭环；原报告六项有界对照及动态材料/端点代码见IMS协议。R5/R6使用现有只读接口在程序边界检查，不冒称实时UICC/PCO指示、所有来源动态DNS或全部特殊mode恢复图；不重开可选扩展。

## P1：原报告的传输与功能扩展（部分基础已具备）

- **P-CSCF 传输已由 N6 验证。** worker 内 TCP/TLS、TCP/IPsec 四向选择器、原 authority 精确 TLS 身份、有界 DNS TCP/NAPTR D2T 接通；旧 UDP、来源/预算/跨 profile 防降级保持。TLS/IPsec 组合边界和实机验收仍须明确，不能把字段存在当作全组合支持。
- **来源诊断已由 N7 软件验证。** 实际 profile/身份/domain/realm/端点、请求/授予族、报价/协商/响应认证、失败作用域和真实门禁已进入同源 API/原 GUI；epoch/serial 与弱 runtime 拒绝晚到写入。声明模板、收到的 Retry-After 与最近成功都不授予当前 readiness；实机未知值仍如实未知。
- **N8承载门禁/有证据适配已由22646c8验证。** catalog/override要求在profile准备及每次family dispatch前验证；Native QMI支持PAP/CHAP/BOTH/none、MBIM支持PAP/CHAP/none，原WDS client只读IPv4/FQDN并保留原IPv6 exact-address观察，opt-in DHCP仅信息请求。API/原UI只投影能力和凭据存在标志，旧保存不清空认证。通用PCO策略写入、旧MM认证变更和DHCP地址分配仍unsupported；这些组合与实机验收不得由现有绿灯推断支持。
- **EC20离线catalog来源不属于本轮委托。** 原报告将它列为独立可选扩展；用户只要求本项目IMS注册逻辑，故不修改carrier_Bundles、不构建新库，也不把该项或跨项目许可列为阻塞。此前独立仓库未修改。
- **EC20原生IMS只读观测是已做的额外诊断，不再扩展。** ca38391已有三工作流/双架构、462相关回归、49单测/12browser证据，但不等于注册兜底，也不能充当本轮IMS注册任务完成依据。本次不擅自回滚既有提交；后续回到注册执行逻辑。
- **本轮固件/源码对照继续闭环，不冒称完整运行分支。** 已补R4～R6并逐项验证；EC20材料摘要/CRC和4MBN/506记录/373EFS已复校。新汇编定位当前已注册/正在尝试P-CSCF与新列表的比较、状态门禁；ISIM Refresh锚点实际是状态文本匹配，不能据此编造事件处理。普通priority与条件non-IPSec证据保留；未知packet/特殊mode/当前NV选择及实机不猜结论。

本轮以用户明确的IMS注册逻辑补全为准，不将原报告的可选catalog/provider或项目MEP/VoNR/E911愿景扩大成必做；QoS/precondition/UT等既有业务也不重报成注册缺口。

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
