# IMS 注册、接入协调与兼容契约

本文是蜂窝 IMS / VoWiFi 注册策略的现行手册；资源归属和维护见
[MM 租约与恢复](IMS_MM_EXACT_FAMILY_LEASE_DESIGN.md)，采证见
[IMS 诊断](IMS_DIAGNOSTICS.md)。实时交接以 [HANDOFF](HANDOFF.md) 为准，
旧部署、实验和决策过程统一由 [档案索引](archive/README.md) 定位。

## 1. 范围与当前边界

- 蜂窝 IMS 是语音、短信、补充业务共享的接入；VoLTE/VoNR 是语音能力，不是注册开关。
- 已实现接入协调、协商门禁、受保护续期和按来源解析 profile；不承诺所有网络互通。
- `0502395` 已完整撤回 `7bd` 的 CMCC 专用 flag 补丁及测试；不能声称 CMCC 已修复。
- `c174551` 已完成全局蜂窝 REGISTER 的身份/安全要求继承及候选去重，Actions 与 410 原配置初始注册已验证。
  不新增运营商专用分支或测试；中国移动同卡、自然续期和业务验收仍独立，见 [HANDOFF](HANDOFF.md)。
- `5aaf3ea` 补全共享 ISIM 身份、蜂窝 P-CSCF 端点和临时拒绝恢复；Actions 成功但未部署，详细验证边界见 [HANDOFF](HANDOFF.md)。
- `3509e15` 已验证双接入 REGISTER rspauth 的请求关联与状态提交门禁；`344308c` 已验证 AUTS/stale/423 补强。reg-event 已在后继 `3d54aa8` 验证，当前新代码均未部署。
- 本文描述既有契约，不授权执行注册、呼叫、短信、换卡、重启或恢复写操作。

## 2. 启用意图与自动选择

只有 VoWiFi 与蜂窝 IMS 各自的启用开关，不提供额外的注册模式选择。

| 启用状态 | 自动行为 |
|---|---|
| 都关闭 | 不主动注册 |
| 仅 VoWiFi | 只尝试 VoWiFi，不开启蜂窝 IMS |
| 仅蜂窝 IMS | 只尝试蜂窝 IMS，不开启 VoWiFi |
| 都开启 | 请求多流能力；协商和流验证通过才追加第二注册 |
| 双注册不成立 | 在已启用且可注册的接入中优先 VoWiFi，再回退蜂窝 IMS |

- 兼容存储值统一为 `concurrent`，含义是自动协调，不代表实际双注册。
- 旧 `wlan_preferred` / `cellular_preferred` 可读；启动仅规范化过期模式字段，
  不改变启用、资费、线路设置。不能把旧模式当作仍受支持的选择器。
- 旧模式 API 读取返回自动值；设置已移除模式返回
  `ims_registration_mode_automatic_only`，不能假装保存成功。
- 蜂窝可用性需要驻网和安全的恢复预算，不是“检测到 modem”即可。
- WLAN 恢复有界；耗尽后才让合格蜂窝回退。显式重试不绕过注册准入。
- 单路均不可用时，仅被业务策略允许的短信/电话才考虑 CS。
  CS 不是第三路 IMS；SIP Trunk 没有可假装存在的 CS 音频后端。

## 3. 多流标准与准入

依据 3GPP TS 24.229 §5.1.1.2.1(f)、RFC 5626 §4.2、§4.4、§6：

1. 同一私有身份可以具有多个 IMS 流；共享稳定 `+sip.instance`，各流使用不同稳定 `reg-id`。
2. 首次请求声明 `Supported: outbound`，不在第一条请求强迫网络必须支持。
3. 成功响应的 `Require: outbound` 才确认网络采用 outbound；单纯回显 Contact、
   `reg-id`、`Supported` 或存在 `ob` URI 参数都不充分。
4. 首跳/Path、匹配绑定、未过期租期和传输健康是独立条件。
5. 成功响应未确认时，不继续为同一私有身份追加 IMS 流。
   新 Contact 可能替换旧绑定，但不能据此声称已证明特定网络删除了旧 SA。
6. 追加第二流需要 outbound 机制；明确不支持时走单注册兼容路径。
   超时不是明确不支持，不建立永久运营商黑名单。

实现要点：

- 无协商时采用先停旧注册、再建新注册的选择方式；协商成功则保留两路独立绑定和续期 owner。
- 新增第二流必须有有效、归属明确且传输已验证的流作为证据。
- keepalive 证明短暂过期不抹掉已有绑定的协商能力，也不授权拆掉已建立的双流；
  但不能用过期证明准入新的第二流。
- UDP outbound 使用 STUN keepalive；未证明首跳支持前不能盲发 STUN。
  TCP 使用 CRLF。明确无 NAT 的 UDP 路径可按标准省略 keepalive，但仍遵守 `Flow-Timer`。
- 合并 `Supported` 和 `Require` 的逗号列表并保留 security/GRUU 选项，
  避免只读取首行的对端漏掉能力；Digest 鉴权头不能这样合并。
- 不实现无缝 IR.51 切换，不跨接入迁移已建立通话。

## 4. 生命周期与并发

- 每线路 LTE/WLAN 注册转换串行化，先取得转换锁，再取得 bearer/access 锁。
- 锁内重新计算准入，排空已准入 REGISTER，仅释放不再准入的路径，随后发布 applied 决策。
- 低层 REGISTER 入口也失败关闭；诊断、短信或语音调用者不能绕过协调器。
- 不同 SIM 线路有独立锁和决策；失败续期不授权销毁另一接入。
- 调度器每轮及设置变化后协调已连接路径。
- dialing、ringing、incoming、active、held 及未知非终态通话都延后策略拆除；
  排空事务、取得 access 锁后再次检查。
- 延后期间保留旧 applied 决策，让旧路续期而不是让新路抢先注册。
  这是重复状态检查，不是原子来电屏障。
- 暂停未选中的接入不算 profile 失败，不消耗恢复预算，不改无线、数据或其他 SIM 配置。
- 清理必须保留已耗尽主路的 retry marker，防止其立即抢回准入。
- 安全关联轮换先验证并发布新自有 flow，再退休前任，避免临时能力空窗。
  分阶段续期保留原 outbound 要求，不能经挑战后的 200 静默降级。

## 5. 注册租期与受保护续期

### 通用租期

- 响应 Contact 的 `expires` 优先，其次响应 `Expires`，最后 profile 默认值。
- 共享 lease 模型按协商寿命安排主动续期：租期大于 1200 秒时提前 600 秒，否则在半租期处，最小延迟为 1 秒（`RegistrationLease::from_expires`）。
  以实际 lease/scheduler 为准；主动续期截止不等于注册到期，旧 `11/12` 文档描述已过时。
- 在完整租期仍有效时，受保护重试仍可使用原绑定；OPTIONS 超时仅为建议性证据。
- 423/Min-Expires 由共享 REGISTER 核心做有界重试。
- VoWiFi 续期失败使 IMS readiness 失效，不立即拆 ePDG/IKE/Child-SA/ESP/TUN；
  达到配置的连续失败阈值才进入接入重建。
- 蜂窝续期在 live loop 执行，不依赖另一接入是否启用。

### 注册身份与业务身份

- `registration_identity` 在初始成功时固定，所有续期、挑战鉴权及注销都用原 AoR。
- `identity` 可以随 P-Associated-URI 更新，用于呼叫、短信、订阅和 OPTIONS。
- 默认业务身份变化不改变注册 Contact、Call-ID/CSeq、安全上下文生命周期。

### 蜂窝受保护事务

依据 TS 33.203 §7.4、§7.4.1a、§7.4.2a：

1. SM1 使用当前受保护通道及 Security-Verify；Security-Client 已提出新的 client 端口/SPI，
   UE server 端口保持，预留 socket 保证 offer 中端口可用。
2. 直接 200 仅延长租期并丢弃未使用 offer，不更换 SA。
3. 可用 AKA/Security-Server 挑战必须让 SM7 使用原 SM1 的精确 offer；
   零 SPI、重叠绑定及未报价算法在安装前拒绝。
4. 暂存新通道时保留旧 sockets，允许旧 SA 的失败响应及无关 SIP 帧继续被接收。
5. 失败一起回滚 sockets、route、Security-Verify、原认证上下文；
   CSeq 和已经消耗的原 nonce-count 继续增长，新挑战状态不提交。
6. 最终成功才提交新 SA/凭据；保留一个前任受保护关联接收延迟报文，
   下一注册过程前释放它，完整会话 teardown 清理两套 plan。
7. 租期有效时超时继续用原受保护通道，不按超时次数改成明文 UDP。
   真正到期后的初始注册是恢复，不能计作续期成功。

`hmac-sha1-96` 与 `hmac-sha-1-96` 可按同一完整性算法比较，但保留原始 Security-Verify；
不因此接受 MD5、未报价加密或放宽 AES-only 限制。XFRM 清理仅限自有 UE worker，禁止全局 flush。

R7（d373ffa已软件验证）将XFRM安装/部分失败回滚/旧SA退役/重选/会话清理的API限定为原`UeWorkerBinding`。不能“检查旧binding后再从handle重新bind”；原代次在enqueue的lifecycle锁内以及回包后均核验，旧操作不会落入重启后的进程。稳定代次的部分失败仍仅反向删除同一plan，并保留安装的原始失败原因，不以清理错误覆盖。

### REGISTER 鉴权、响应认证与重试边界

- R5（6203511已软件验证）在现有只读QMI/native AT/PCSC接口上捕获应用列表与IMPI/DOMAIN/IMPU/P-CSCF的完整快照，两次读一致且前中后card/owner一致才接受。注册、AKA/stale、423、续期及注销边界复核；变化/不稳定/读取错误不当缺省、旧凭据或旧租期仍有效，成功响应在发布前再验。已收到临时失败的网络等待仍记到原订阅，不以同卡文件更新清空。不订阅或确认UICC REFRESH事件、不声称跨APDU硬件原子性；检测时点是注册过程边界，业务长期空闲间隔另需硬件事件支持。
- AUTS 保留选定 qop/opaque，但不是有效鉴权证明；非法/重复 AUTS 或仅 AUTS 后直接 2xx 不能形成注册或确认注销。
- stale 必须关联上一条已发送的同 scope 请求；同 RAND/AUTN 才复用绑定凭据，独立 `aka_origin` 不被 nextnonce 覆盖。已保护通道不因 stale 重装同 SPI/key 或退明文；需新安全上下文时明确恢复。
- 蜂窝刷新三候选共用一个 `RegisterNegotiationBudget`，包括 423→400/415/420/421/494 后不再换头型。门禁必须使用 `RegisterFailureMetadata.permits_variant_fallback()`，不能只看最后 SIP 状态；新鉴权后的 400 也不得重新开启 AKA/头型预算。返回错误保留 budget 的 Min-Expires 来源，正常交换首包格式兼容及 423→2xx 仍保留。R1 已由 `b6a7888` 软件验证，实机互通仍独立。
- 423协商一旦开始，就关闭本次静态头型/profile/端点兼容扩张；无效/过大/不增长/耗尽floor不能重开预算，零租期注销不协商正租期。R3（31d09ff已软件验证）将WiFi初次认证后的423交回同一共享驱动，原protected channel/接收配对不重建，使用实际凭据重算nc/proof；初始与鉴权共用两轮lease预算且不重置两轮AKA预算。AUTS不当有效凭据，成功先验rspauth再提交租期/nextnonce；注册后刷新旧逻辑保持。
- `3509e15` 已接通并验证 rspauth：仅对实际生成的非空 Digest 证明捕获不可变请求上下文，发送前检查最终 URI/Authorization/opaque，2xx 发布前验证响应对应的 qop/nc/cnonce 和 rspauth。Authentication-Info/Proxy-Authentication-Info 不串用，重复/畸形/不匹配明确失败；缺少可选 rspauth 仍兼容，不把 nextnonce-only 冒充服务器证明已验。当前支持范围为既有 REGISTER 算法、qop=auth/无 qop 和单一鉴权 scope。
- 响应认证失败不是传输失败或可换头型的协商提示；不能提交新的 nextnonce、租期、业务身份或安全上下文。失败保留已发送的原 nonce-count、回滚暂存状态；不放宽 AKA/ESP 算法门禁。本轮范围为 REGISTER，非 REGISTER 业务不自动宣称已支持。

### 运行中PCO端点复核（R6，6672a64已软件验证）

- 无活动对话的蜂窝续期边界查询原retained provider，前后保持SIM/owner/worker与承载绑定；不新增CID/PDN，不走modem-wide AT fallback。只有已选来源为BearerPco才启用，环境/profile/ISIM/DHCP/标准DNS与WiFi IKE来源不混用。
- 完整且非空的当前族观察在原承载DNS内重解析；不跨来源降级。空结果、未实现、查询不可用或不完整IPv6不是端点撤销；DNS无新可用结果也不凭空废弃仍有效绑定。原逻辑socket/transport/TLS authority仍在列表时保留原通道与SA，顺序/TTL变化不触发重选。
- 确认需要替换且无活动对话时，原PDN、worker、地址、SIM、profile和实例不变；用临时本地端口、新REGISTER/AKA/SA替换SIP绑定，继承旧IPsec-required/TLS身份约束，不复制旧Digest到新P-CSCF。成功移交原provider/profile句柄后只清旧SA/socket；不执行bearer网络清理，不把这次新注册计作自然续期。
- 已开始鉴权/423或安全失败不重新展开端点；临时失败及候选等待保留到原租期截止，不能通过再发现清预算。活动对话维持原流并延后本项观察/重选，reg-event真实撤销仍独立立即生效。该项不是动态DNS缓存系统或基带PCO事件订阅。
- 新注册成功后reg-event/MWI按新dialog重新建立，旧OPTIONS事务作废；临时重选失败时runtime仍显示实际保留的旧端点，失败候选只入执行诊断，不能在Registered状态展示未成功的目标。
- 共享最终成功响应携带接收时刻；材料/provider复核后的新租期和调度仍从该时刻计算，已在验证过程中到期不能发布ready。网络给出的原始expires数值不被伪造。

### reg-event 观察与注册撤销

- 只从已验证成功的最终 REGISTER 绑定构造观察器，不从业务默认身份或 profile 猜 AoR/Contact；同接入 owner/通道订阅 `Event: reg`。
- 严格关联 Call-ID/tags/CSeq、当前 AoR/Contact/instance/reg-id 及 reginfo XML namespace/full/partial/version；重复通知只 ACK 一次效果，缺 baseline/版本跳变有界请求 full，不回滚快照或恢复 readiness。
- 只有相关绑定终止才使该接入 readiness 和 flow 证明失效。订阅 4xx/超时/单纯 Subscription-State termination 不等同于 REGISTER 被撤销。
- 撤销等待绑定同 SIM，profile/worker/控制端口重编号不清空；超过支持的 24h 等待 fail-closed。活动对话允许排空，禁止原 session 重新刷新或新业务冒充已恢复；新接入成功才建立新观察器。
- 现有 MWI/REFER 不合并进 reg-event；未知或不可表示的自定义绑定报告观察不可用，不为了该可选观察打断原 REGISTER。

### 临时拒绝与 Retry-After

以下等待策略在 `core/register_retry.rs` 统一；蜂窝与 WiFi 的生产接线由 `c3fb036` 及既有代码完成软件验证（见 HANDOFF），不是由 reg-event 或诊断字段推导支持。

- `RegisterFailure::metadata()` 区分初始/认证阶段、端点临时拒绝、传输故障、身份/认证拒绝和本地错误；不得从 `detail` 文本或上一次保存的 401 猜当前发送结果。
- 初始完整 408/500/502/503/504 可尝试同承载内另一合格 P-CSCF；普通身份拒绝、认证后失败不借静态头型/profile 更换重开当前鉴权。
- Retry-After 严格解析秒数、注释/参数和折行。重复、溢出、非法值不是零秒；合法零秒也有最小非零间隔。
- 端点等待以逻辑 P-CSCF 的地址/端口为键，保存在本线路同 SIM 的 live handle 外层，普通清理、profile/worker/同卡 MM owner 更换不重置；新 SIM 不继承旧卡等待。
- 最多保存 32 个端点，有限 deadline 最长 24 小时；更长/非法等待锁存停止端点，容量溢出停止该 scope，不驱逐未到期项或截短服务器等待。当前是进程内状态，不声称重启后持久保存。
- 所有可用端点失败后，在下一批 bearer/profile 分配前检查 not-before；恢复调度使用结构化剩余时间，而不是用换 profile/重建 PDN 绕过等待。
- 临时刷新拒绝先回滚暂存 SA/socket/auth，再保留仍有效的原保护绑定。等待越过旧 expiry 时只保留到原截止；到期唤醒做失效处理，不提前 REGISTER，也不把请求中的新 Expires 当成新租期。
- 已死亡 outbound flow、SIM/owner 失效和身份/认证拒绝不据此保留。注销仍检查绑定与端点等待，不向可能换卡后的订阅发送旧身份。
- WiFi 初次等待键使用发现时的逻辑 P-CSCF 地址/端口，不使用后协商的 IPsec 端口；临时响应立即停止同端点头型阶梯，未开始新AKA时可选合格其他端点，认证/423后的止损保持。完整失败轮次之后，重启/profile/ESP重建先查同卡门禁。
- WiFi operator 通过不可序列化的 `RegisterRecovery`/`RegisteredRefreshFailure` 传递真实 metadata、not-before、原 lease deadline 与是否实际发送；这些控制值独立于被动 diagnostics。等待只恢复本来已有的服务flags，到旧截止降readiness，不补发早到REGISTER；保留活动对话原排空逻辑。独立撤销serial在最终stage发布处重验，网络reg-event撤销后，旧刷新不能恢复绑定。
- API 只计实际失败轮次，不把等待轮询计成新失败；耗尽后的重建动作也等门禁，清理只消费 pending 动作位，不清失败次数。新注册成功或正向确认换卡才清旧预算；有限scope/端点容量耗尽fail-closed。
- R4（0351006已软件验证）还将注销本次新收到的失败反馈给下一批注册：`observe_unregister_failure`用独立等待槽合并原门禁，不因清理、较短其他端点或晚到success清除。蜂窝使用session原Arc，WiFi准入时返回transaction-owned observer，网络返回后不重新按line寻找新卡。诊断只投影metadata/admission，不承担控制副作用；正常200不新增等待，零租期423不转正租期，临时/非法/长等待沿用原策略。
- WiFi 注册捕获原 worker binding，刷新/后续鉴权/注销核验原实例与代次；替换后的新worker不能被旧channel借用。TCP EOF/确定的连接错误按死流恢复，不能用旧租期包装成可重试UDP收包缺口。

## 6. 资费保护与业务路由

注册可用性不等于业务许可；双注册可接收蜂窝短信，但不能绕过费用门禁。

- 常规呼出/发送优先 VoWiFi，再选允许的备用路径。
- 短信页“仅通过 VoWiFi 发送”是硬限制，API、自动化、Trunk 均遵守；失败不落到蜂窝 IMS/CS。
- Trunk `vowifi_only` 约束网关呼出、接通前回退及入向接听；
  本地 API/自动化不能从同一网关失败绕到原生 CS 呼出或接听。挂断已有通话仍可执行。
- 收短信不受发送限制；两路均可入库/转发，保留跨通道去重。
- 不能命令运营商将蜂窝来电改送 WLAN；严格模式在接通前拒绝被禁蜂窝来电，不能先发 200。
- 每次尚未执行的回退重新检查限制，不能沿用设置变化前的候选缓存。
- 已发送短信不可撤回，设置变化不迁移已有通话，也不保证 VoWiFi 按本地资费计费。

### 可选归属地蜂窝语音准入

`allow_home_cellular_calls` 默认 `false`，只影响语音，不改变短信限制。

| `trunk.vowifi_only` | 新选项 | 语音行为 |
|---|---|---|
| 关闭 | 任意 | 原无限制行为，仍检查线路/无线/能力；可主动允许漫游 |
| 开启 | 关闭或缺失 | 原严格仅 VoWiFi，升级不改变旧含义 |
| 开启 | 开启 | 已注册 VoWiFi，或明确驻网非漫游蜂窝；未知、读取失败、仅 SMS 驻网不准入蜂窝 |

- 初始拨号、未接通备用路径、接听前重验；已注册 VoWiFi 不等待蜂窝状态读取。
- MM 证据绑定 unique owner、ICCID/SIM、slot、控制端口，并在读取前后核对；不按 APN/号码猜漫游。
- 非 MM provider 无等价证据则条件模式失败关闭；不宣称 native 已验收。
- 条件模式下 `BoundImmediate` 等待显式/网关接听命令再次核验，不自动先发 200。
- 定时拨号须唯一自有 IMS call ID，不能借同号码通话或复用 AT/CLCC index，故不走 CS/直接 AT 兜底。
- 定时长度从启动计时，不等于接通时长；取消/超时保留精确挂机任务。
- 真实初始 180 后的远端未应答，或确认的对端忙/拒接等可形成任务 success，保留 SIP/Q.850 和未接听事实。
  仅 100/183、裸 408 或无送达证据的观察结束不够；本端/媒体/费用错误、re-INVITE 失败、
  观察丢失、取消和清理失败仍 failed。任务 success 不等于接通、音频或计费成功。

## 7. 地址族与 profile 来源

### 全局地址族策略

- 蜂窝生产固定 `IPv4v6 → IPv6 → IPv4`，唯一来源 `ImsConnectionPlan::default()`。
- 不提供单族配置、自定义顺序或自动/手动开关；catalog `ip_stack` 不得缩减计划。
- 已移除 `cellular_ims_ip_families`、`cellular_ims_ip_families_auto` 及对应 `volte_*` 字段。
- 完整配置校验后以事务删除所有线路中的四个旧 JSON 字段；其他字段/表和未涉及时间戳保持。
  校验/写入失败回滚；导入旧配置也不能重新启用。
- 新旧 `/api/cellular-ims/lines/{line_id}/ip-families`、`/api/volte/lines/{line_id}/ip-families` 写入口均移除。
- 普通数据 APN 与 VoWiFi/ePDG 协议栈设置是不同功能。
- bearer 建立失败才继续下个族；结构化网络强制族允许有界去重重试。
  双栈请求实际只授予一族仍合法，不为凑双栈并发创建 PDN。
- bearer 成功后仅在其实际授予地址上尝试 P-CSCF/SIP；没有新增 SIP 失败后换族重建的外层循环。
- SIM/owner 变化、清理未知或基带故障终止本轮，不是永久禁族。
- QCM410 IPv4 路径曾触发固件 fatal，双栈也可能触及；冷却不是固件修复或跨进程硬件熔断。
  新 fatal 后停止重复激活并保留证据，见 [QCM410 故障边界](QCM410_BAM_DMUX_MODEM_CRASH.md)。

### 解析顺序与标准派生

1. 对目标 access 合格的 source-bound 自定义数据库记录。
2. source-bound 只读 carrier catalog 投影。
3. 有效 HPLMN 的明确标记标准派生 profile。

- IMS-only 数据库记录可用于蜂窝；`voice.vowifi_enabled=false` 必须阻止 Wi-Fi resolver、
  live matcher 和显式 Wi-Fi pin 使用，数据库搜索仍展示其不具备 VoWiFi readiness。
- `ims_vowifi.profile_id` 兼容 pin 与蜂窝 pin 使用同一来源约束；数据库 pin 不得命中同名 catalog 行。
- 派生可提供标准 `ims` APN、ePDG FQDN、IMS domain、EAP-AKA realm、UDP 和保守 REGISTER envelope。
  MCC 999、畸形或有歧义 HPLMN 不送公共 DNS；MNC 标准域名使用三位标签。
- 不从 PLMN 猜真实 P-CSCF、私有 ePDG/DNS、私有身份模板、IPsec 端口或服务器 SA 元组。
- ePDG 候选按显式线路、UICC 选择/归属、访问国家、存储 profile、派生顺序处理；
  位置型名称需要明确 UICC 选择规则。缺失值按阶段报告，不伪装成数据库记录。
- Wi-Fi 成功进入 ePDG/IKE/ESP 后，选中 profile 的 IMS 参数进入共享 REGISTER 驱动。
- IKE 使用 UDP/500，协商 NAT-T 后 UDP/4500；超时、认证/提案失败或一般 Notify 不证明地址族要求。
- 保留完整 REGISTER，在外层 IP 分片；重组检查 offset、ID、连续性、重叠和非末片对齐，
  不用裁剪 REGISTER 掩盖 MTU 问题。

### UICC 身份与应用

- `uicc_ims.rs` 统一选用 IMPI、IMPU、DOMAIN 和认证 AID。完整合法 ISIM 身份优先；合法缺文件/FF/部分未配置保留 USIM 派生，坏 TLV/FCP、读失败或归属变化显式失败。
- IMPI/DOMAIN 为透明 EF，IMPU/P-CSCF 按 FCP 的记录布局读取；每 EF 最多 4096 字节、32 条记录，不猜记录长度，不输出材料到 Debug 日志。
- 显式 database/catalog/用户 domain/realm 保持；只有 derived 的未覆盖默认域可由 ISIM 替换，实际字段来源标为 `isim`。ISIM 身份不被 IMSI/号码格式候选覆盖。
- QMI 按 slot 获取完整 AID；native AT 在既有自有逻辑通道读取 MF/EF_DIR；PC/SC 的每个进程重选相同 AID/文件，并固定 reader 名称而非可重排索引。
- 读取前后、AKA 前后及蜂窝 REGISTER 发布/刷新/注销检查卡/slot/端口/owner；MM 缺失或零值 PrimarySimSlot 只规范为单卡槽 1，缺 IMSI 仍允许受归属约束的只读 AT/UIM 回退。
- VoWiFi IMS 使用捕获的身份/AID，EAP-AKA 仍使用 USIM，不把 IMS 私有域改写到 EAP。分阶段观察不是完整物理热插拔原子保证，实机竞态验收仍独立。

### 蜂窝 P-CSCF 端点

- 按来源存在性选择：显式环境值 → bearer PCO → 显式 profile → ISIM P-CSCF → 标准 DNS。显式值无效/解析失败/不支持时不静默跳到其他来源；不同族仍走原实际授予族计划。
- `PcscfEndpoint` 保留 socket、UDP/TCP/TLS transport、source、原 authority、可用 TTL/SRV priority/weight；当前不缓存 TTL，不由 metadata 推断可复用承载。
- IPv4、裸 IPv6、`[IPv6]:port`、`host:port` 和显式 SIP URI 保持端口/传输；`sips:` 使用 TLS，不降为 UDP。TCP/IPsec 使用四向 selector，protected 接收端口先 bind、SA 安装后 listen；原 UDP 两向实现保持。
- TLS1.2/1.3 使用内置 WebPKI roots，校验原逻辑 authority 的精确 DNS/IP SAN，不接受通配符、错名、不可信链或静默降级。显式 required-IPsec 与不支持的新 TLS/IPsec/客户端证书组合拒绝，不以 TLS 撤销 IMS 安全要求。
- 只经 UE worker/当前 bearer DNS 查询，没有全局 resolver。保留 direct/SRV 优先和现有加权策略；匹配 TC question/ID 才允许同 resolver 的一次 TCP 重试，原查询总 deadline 不延长。旧路径耗尽后有界 NAPTR D2U/D2T/SIPS D2T，TLS 原 authority 不被 SRV 目标改名；完整 N6 由 `6c0c140` 验证。
- 最多 16 个端点、16 次 DNS 查询、3 个同族 bearer resolver；每次查询最多 4 秒、整个发现最多 12 秒。重复查询与端点去重，DNS 长度/owner/CNAME/截断边界继续严格校验。

## 8. REGISTER 三态与写入契约

bundle → `RegisterPolicyRecord` → `CarrierProfile` → `RegisterRequestPolicy` → SIP 字节。
三态只存在于 bundle JSON，投影后为具体 bool：显式 false 必须压过 baseline。

| 输入 | 语义 |
|---|---|
| `true` / `"true"` | 开启 |
| `false` / `"false"` / `"omit"` | 关闭 |
| 缺失 / `null` | 无意见，采用 baseline |
| 其他值，包括 `0`、`1`、`"yes"`、`"no"` | 拒绝整行 profile |

字符串 trim 且大小写不敏感；非法值返回
`carrier_catalog_register_bool_invalid:<pointer>:<value>`。
AccessIdentityPolicy 允许 `omit/static/dynamic_if_known/required_dynamic`，连字符/下划线等价。

相对于 `sip.common.register`：

| 字段 | 缺省 |
|---|---|
| `include_pani_initial` / `include_pani_authenticated` | LTE/EPC true，Wi-Fi/ePDG 需显式开启 |
| `include_route_header` | false |
| `include_p_preferred_identity` / `always_add_sip_instance` | true |
| `enable_cellular_network_info` | false |
| `require_sec_agree_headers` / `proxy_require_sec_agree_headers` | false |
| `enable_initial_reject_fallback` | false |
| `include_mmtel_features` | 从服务声明推导，不是独立 omit 开关 |
| `include_visited_network` | 从 visited-network 值推导，不是独立 omit 开关 |

- `security_agreement=required` 首次发 Security-Client；`auto`/缺失由 421/494 或具体 offer 驱动；
  `omit/disabled` 永不发 Security-Client / Security-Verify。机制列表可保留用于往返，不代表允许发 offer。
- record 的四个 default-true 字段是 PANI 两项、P-Preferred-Identity、sip.instance；
  中间层丢字段可能重新开启 false，导出/patch 不得省略显式值。
- 数据库加载将原始 JSON 交给 legacy normalization，通过 presence 区分缺失与 authored false。
- `PUT /api/vowifi/carrier-profiles` 是整体替换；`from_api_value()` 要求
  `REQUIRED_REGISTER_SWITCHES` 中八项全部存在（PANI 两项、Route、P-Preferred-Identity、
  sip.instance、Cellular-Network-Info、Require sec-agree、Proxy-Require sec-agree），缺项返回 400
  `carrier_profile_register_switch_missing:<字段名>`，缺段返回 `carrier_profile_register_section_missing`。
- `custom_carrier_profiles` 不在 `CONFIG_TABLES` 导出范围；二进制 restore 为整表复制。
  完整 profile 契约见 [Carrier Profiles](CARRIER_PROFILES.md)。

### 全局蜂窝 REGISTER 候选与安全继承

`register_fallback.rs` 保存本次 P-CSCF 尝试的状态，不修改共享 profile，也不跨 P-CSCF/SIM 继承。

- 保留原 profile 首包。通用/格式候选不能丢掉已选择的空 AKA 身份和 Supported；空 AKA 不是计算后的鉴权响应。
- 本地主动 Require/Proxy-Require 可以在非强制通用候选中撤回；profile 的明确要求及服务器已确认要求不能随候选切换消失。
- 421 的 Require/Proxy-Require 必须只含已支持的 `sec-agree`，未知或空必需 token 停止；494 本身表示安全协商要求。
  既有受限 420/Warning 处理保留。超时探测仍可进入原动态兼容阶梯，但不记作服务器明确要求。
- 裸 421 的 Security-Server 只是未认证报价提示，不强制收窄算法；普通保留身份的候选仍使用原完整报价。
  明确安全要求触发的既有一次性 AES 重报价仍受严格参数/已报价机制限制，失败不再退回宽报价。
- 421/494 提示不生成密钥、不安装 SA，也不直接用作 Security-Verify；后续 401/407 的合法选择才绑定该值。
- 已确认必须协商安全时，挑战缺少可用 Security-Server 且不存在受保护绑定，必须在调用 SIM AKA 前停止；无保护 200 不算成功。
  “不可用”也可能是缺头、SPI/端口或格式错误，并不必然说明项目不支持算法。auto 且无明确要求的原路径保留。
- 继承后按身份类型、实际请求 policy、报价格式、机制限制及确认的保护约束去重；label/随机 Call-ID、SPI、端口不是新兼容候选。
  重复候选不消耗最多 24 次的发送预算；SIP 原报文重传另由共享驱动负责，不在这里删除。
- 普通 403 和认证后失败不进入静态候选；既有受严格条件约束的认证前动态身份提示不扩大。
  AKA 两轮、单交换时限、地址族/来源顺序及算法白名单/strict 均不因本修复放宽。

## 9. 命名、迁移与状态 API

- 规范 HTTP `/api/cellular-ims/*`，保留 `/api/volte/*` 别名并共享鉴权/响应；已删除的族接口除外。
- 规范字段为 `cellular_ims`、`cellular_ims_profiles`、`cellular_ims_ready`，线路为
  `cellular_ims_connection_enabled/auto_restore/profile_selection` 等；旧 serde alias 可读，新名写出，双写歧义拒绝。
- SIM 覆写 `ims_cellular` / `ims.cellular_ims` 兼容旧 `ims_volte` / `ims.volte`。
- `SIMADMIN_CELLULAR_IMS_PCSCF/CID` 优先，旧 `SIMADMIN_VOLTE_*` 仅回退。
- bundle `lte_ims_status/nr_ims_status` 与语音能力分离；SMS-only 不因 `services.volte=false` 拒绝。
- `volte_refresh_stats` 合并进 `cellular_ims_refresh_stats` 并删除旧表；降级后再升级可重新迁移。
- 短信/去重/事件 transport `volte_ims → cellular_ims`；事件 `volte.* → cellular_ims.*`；
  短信标记 `volte-mt: → cellular-ims-mt:`，保留内容指纹去重和旧值读取。
- 错误码集中在 `cellular_ims/errors.rs::code`；前端生成表按完整 token 匹配。
  runtime `last_error` 不是同名持久化列，不得机械改写其他业务词表或第三方 VoLTE 字段。

`GET /api/modem/lines/{line}/ims/status` 的 `registration_policy`：

- `requested` 为兼容请求，`effective` 为 `single_registration/concurrent/none`；
  `desired/applied` 带每接入准入和稳定 reason，`switch_deferred_for_call` 报告延后。
- `concurrent_support`：`client_incomplete/not_negotiated/not_supported/negotiated`。
  `not_supported` 仅来自当前自有成功 flow 明确未接受 outbound，移除该 flow 后证据失效。
- 每路响应 `require_outbound/flow_timer_seconds` 与 `cellular_flow/wlan_flow` 分开报告
  lifetime、offered/negotiated、`transport_validated`；不能用两个缓存 registered 标志证明双流。
- 不输出 Contact、订户身份、Authorization 或 SA keys；日志同样仅记录能力/归属元数据。

### 执行事实 `diagnostics`（N7，`ee9ee28` 已软件验证）

- 统一状态的 `three_gpp.diagnostics` / `non_three_gpp.diagnostics`，cellular 线路的 `runtime.diagnostics` 与 WiFi 线路的 `diagnostics` 都来自各自 runtime 的同一投影，不重新读卡、建连接或修改恢复条件。
- `version=1`、`observed=false`/`unknown` 保留未观察状态。profile/identity/domain/realm/P-CSCF 只给来源类型和配置 ID；身份值、AID、原始 SIP、nonce、密钥和证书不属于这个 schema。
- `declared_identity_source` 是配置声明；`identity_source` 是实际选择，`declared_only` 明确模板规则未实现/未执行，不由标签伪造能力。请求/授予/信令族分开，合法单族授予不能被说成双栈。
- `pcscf_port` 是发现端口，`signaling_port` 是通道实际目的端口；`socket_ready` 仅记曾创建自有通道，不保证仍存活。实际 TLS 版本/套件和白名单 IPsec 报价、协商分开；`negotiated_from_success` 是最近成功历史，不是当前注册权威。缺可选 rspauth 不冒称验证成功。
- 失败带 scope、initial/refresh/unregister/reg-event、鉴权阶段、状态码和 423 依据；`failure.response_retry_after_seconds` 是收到的要求，`wait.not_before`/remaining 是实际控制门禁。长等待/停止不伪造零秒重试，查询不刷新 deadline。
- epoch/serial 令牌拒绝旧尝试更新；WiFi 弱 runtime 与执行阶段原 owner/epoch 上下文防止 reset 后才恢复的任务认领新状态。投影不重置任何协议预算、授予 readiness 或改变 UI 保存意图。
- 原工作台详情内增加可选只读折叠区；旧响应无字段仍可显示。Bruno `get_ims_status` 是被动查询；既有 `get_vowifi_status?live=true` 保留原显式探测语义，不能把所有 GET 都称为无副作用。
- R1 刷新423预算、R2普通WiFi等待均已软件验证；诊断字段绝不作为授权重试或保留租期的控制输入。真实网络与驱动硬件验收仍独立。

### 显式承载要求与驱动能力（N8，22646c8 已软件验证）

- `CarrierProfileRecord.ims.bearer`与SIM覆写`ims_cellular.bearer`保存完整原子要求：`authentication.method`为`unspecified/none/pap/chap/pap_or_chap`，PAP/CHAP需显式用户名和密码；显式空密码与缺失不同。不从AKA或普通data APN借凭据。默认`unspecified`保留旧行为，`none`必须由实际adapter执行，不能等同未知。
- `pcscf_via_pco=network_default/required/disabled`描述modem profile请求策略；只有自有profile、readback/回滚实现才可承诺。当前Native/MM均明确拒绝非default，不用reporting开关伪装实现。catalog的既有`pcscf_discovery`只是来源提示，不升级为强制写入。
- `pcscf_via_dhcp=disabled/information_request`默认关闭；显式启用只在已授予并已配置的原UE接口上发送DHCPINFORM/INFORMATION-REQUEST，不分配地址、不安装默认路由或替换DNS。DHCP地址配置、PPP等仍不得伪装静态授予。
- driver返回`supported/unsupported/unknown`；Native QMI支持PAP/CHAP/BOTH与明确none，MBIM支持PAP/CHAP/none但不猜BOTH。旧MM认证变更未接owned-profile凭据readback，明确unsupported。支持声明不是当前订阅开通、实际授予或readiness。
- 需求在profile创建/reporting前和每次family dispatch前验证；显式要求失败阻止继续family/profile，不丢字段退APN-only。APN改变时不能将catalog凭据隐式转给另一个APN，须同时明确整个bearer覆写。
- QMI P-CSCF只在原retained WDS CID上查询`0x002d`/mask`0x1800`，严格解析`0x23` IPv4列表和`0x24`域名；不分配/释放其他client、不把generic Domains或IPv6地址TLV冒认P-CSCF。声明该查询不覆盖数值IPv6，IPv6授予保留原exact-address AT补充观察，不因空/IPv4返回而丢失原IPv6来源；真正owner/协议查询错误不转AT。控制口/proxy归属和原worker前后核验，已移入UE的网卡不从宿主机重新认领。
- 域名在原承载DNS解析并保留TLS authority。默认来源顺序不变；DHCP信息位于原PCO/profile/ISIM后、标准DNS前，其域名优先于地址。DHCP有总时限/消息/长度/事务与客户端标识门禁；IPv4仅接受直接服务端和client-ID echo，未知relay不猜信任。
- API/原工作台只返回方法、凭据存在标志、来源和driver能力；Debug、profile详情、覆写和保存响应均不得回显用户名/密码。旧编辑器省略bearer字段时保留原存储；显式clear/replace与脱敏结果回写必须区分，不能把脱敏对象当有效凭据或混用别的SIM/profile。

### 基带原生IMS观察（X2，ca38391已软件验证）

独立认证`/api/modem/lines/{line_id}/native/ims/{status,probe}`与原工作台按钮只产生时间戳观测：GET零设备IO，POST显式一次只读；默认无自动probe。主机两接入关闭且无session/flow/cleanup才准入，try-lock不排队，取消必须保留原物理和host排他。driver资格与同卡/slot/owner前后校验，不清账本或改变连接意图。IMSA六字段含UT、零可能未报告；样本不改变软件SIP租期、业务readiness/计费，也不声称自主固件/host长期互斥。详见[原生后端](NATIVE_BACKEND_STATUS.md)。

### EC20原报告注册缺口对照（有界静态复核）

以原审查基线42057a6与当前6672a64为对照，原报告第4节六项均已有生产实现；复核新增R4～R6也已单独Actions验证。证据等级不因CI而升级为完整固件调用图或实网成功。

| 原报告项 | 生产入口 | 状态/保留边界 |
|---|---|---|
| P1-A ISIM材料 | `uicc_ims::resolve_ims_identity`、cellular `load_device_identity`、WiFi `live_identity` | 身份/AID同卡绑定，R5程序边界材料复核；首个配置IMPU，不声明任意多IMPU注册 |
| P1-B endpoint | `pcscf_endpoint`、`pcscf_dns_transport`、`bind_endpoint_in_worker` | UDP/TCP/TLS及有界DNS已接通；R6补空闲原PCO复核/同PDN重选，TTL不是所有来源的动态缓存 |
| P1-C 临时拒绝 | `EndpointRetryState`、cellular `connect_inner/refresh_live_registration`、WiFi `register_retry_live` | 初次/刷新已验证；本次发现注销新等待反馈缺口R4，0351006已修复验证 |
| P2-D reg-event | shared `reg_event`、cellular `live_reg_event`、WiFi operator | 仅相关绑定撤销readiness；订阅不支持不伪造注册失效 |
| P2-E AUTS/stale/rspauth | `digest_aka`、`DigestRequestProof`、双接入authenticator | 先验后提交；单scope、auth/无qop，非REGISTER不扩称 |
| P2-F 423预算 | `RegisterNegotiationBudget`、双接入候选与refresh | 初次/刷新共用原预算；R3初次protected AKA已补齐 |

固件普通priority的00/02/03/01及条件non-IPSec构造分支已有汇编证据，但不能推导所有mode、设备当前NV/MBN或任意失败降级。新离线分析定位`0xd051c950`的新列表成员比较及上下文/状态条件，支持保留仍在列表中的当前端点；`0xd06e573c`的ISIM Refresh标签属于文本到状态匹配，不是已还原的UICC事件处理。R5/R6已补程序边界的材料和PCO检查；实时指示、活动对话期间的主动迁移、其他来源动态DNS、未知packet/完整恢复图不自动宣称支持。实机首次/自然续期/注销/故障恢复及MM重枚举另需授权；独立catalog与额外原生诊断不在此次注册补全范围。

## 10. 验证清单与验收标准

构建/Rust/注册模拟仅走 GitHub Actions，流程见 [开发指南](DEVELOPER.md)。
手册不替代测试报告；当前已核验的 commit、运行记录与实机范围见 [HANDOFF](HANDOFF.md)。

- 全局候选：身份/Supported 继承、主动声明与真实要求区分、重复候选不计费、完整尾部格式可达、
  421 提示不变成 Security-Verify、401/407 缺安全参数在 AKA 前拒绝；使用无运营商分支的合成协议场景。
- 接入：启用组合、精确 Require 解析、独立流/keepalive、pending proof、呼叫延后、
  每线路串行化、低层 fail-closed、停放不耗预算、回退粘性、已有流轮换。
- 报文：initial/authenticated/refresh/remove 的最终能力字段、重传字节稳定、
  folded/compact/repeated headers、security 选项保留、Contact 回显不算协商。
- 续期：固定 offer、零 SPI 拒绝、分离 XFRM plan、完整回滚、旧/新 SA 接收、
  direct 200、多次真实 Timer E/F 超时不转明文、注册身份与业务别名独立。
- schema：omit、非法类型、合法拼写、JSON 往返、partial PUT 拒绝、所有 variant
  不重新开启已禁字段；最终字节检查要提供真实 P-CSCF，避免 Route 断言空转。
- 迁移：重复启动、降级再升级、事务回滚、旧族路由失效、旧字段清除；
  错误码声明/ALL/前端表一致，两个实际 CI 过滤器均执行相关测试。
- 业务：费用硬限制、候选缓存更新、来电先检查后 200、定时任务精确身份与结果分类。
- 实网：匹配制品 commit/哈希，观察自然续期，不将调度缩成 120 秒；
  完整 SM1→挑战→SM7→200、端口/SPI/CSeq、租期与计数同时核验。
- 双流验收须两路协商绑定和各自自然续期，互不破坏；到期重建不算续期，
  关闭蜂窝不算修复双注册。未协商时可以验收诚实报告的单路回退。
- mock 不模拟真实 MM/UIM/bearer/MTU；历史网络超时不能证明 header 或 SA 根因。
  通话、音频、漫游账单及 native 能力须独立验收。
