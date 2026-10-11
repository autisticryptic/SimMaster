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
- 共享 lease 模型按协商寿命安排主动续期（通用调度为寿命的十二分之十一，即 `11/12`）。
  以实际 lease/scheduler 为准；主动续期截止不等于注册到期。
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

### 临时拒绝与 Retry-After

- `RegisterFailure::metadata()` 区分初始/认证阶段、端点临时拒绝、传输故障、身份/认证拒绝和本地错误；不得从 `detail` 文本或上一次保存的 401 猜当前发送结果。
- 初始完整 408/500/502/503/504 可尝试同承载内另一合格 P-CSCF；普通身份拒绝、认证后失败不借静态头型/profile 更换重开当前鉴权。
- Retry-After 严格解析秒数、注释/参数和折行。重复、溢出、非法值不是零秒；合法零秒也有最小非零间隔。
- 端点等待以逻辑 P-CSCF 的地址/端口为键，保存在本线路同 SIM 的 live handle 外层，普通清理、profile/worker/同卡 MM owner 更换不重置；新 SIM 不继承旧卡等待。
- 最多保存 32 个端点，有限 deadline 最长 24 小时；更长/非法等待锁存停止端点，容量溢出停止该 scope，不驱逐未到期项或截短服务器等待。当前是进程内状态，不声称重启后持久保存。
- 所有可用端点失败后，在下一批 bearer/profile 分配前检查 not-before；恢复调度使用结构化剩余时间，而不是用换 profile/重建 PDN 绕过等待。
- 临时刷新拒绝先回滚暂存 SA/socket/auth，再保留仍有效的原保护绑定。等待越过旧 expiry 时只保留到原截止；到期唤醒做失效处理，不提前 REGISTER，也不把请求中的新 Expires 当成新租期。
- 已死亡 outbound flow、SIM/owner 失效和身份/认证拒绝不据此保留。注销仍检查绑定与端点等待，不向可能换卡后的订阅发送旧身份。

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
- `PcscfEndpoint` 保留 socket、UDP transport、source、原主机名和可用的 TTL/SRV priority/weight；当前不缓存 TTL，不由 metadata 推断可复用承载。
- IPv4、裸 IPv6、`[IPv6]:port`、`host:port` 和 `sip:host:port;transport=udp` 保端口。蜂窝尚无 TCP/TLS 通道，profile transport 或显式 URI 请求非 UDP 时明确失败，`sips:` 不再悄悄变成 UDP/5060。
- 只经 UE worker/当前 bearer DNS 查询；直接 A/AAAA 的完整有界集合优先，其次 UDP SRV，保留端口/优先级和加权顺序。没有全局 resolver、NAPTR 或 TCP DNS 兜底。
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
