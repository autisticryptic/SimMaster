# 版本更新记录

> 本页合并版本发布说明。历史“当前/未发布”措辞只属于当时阶段，不代表设备现状或现有Release指向。
> 当前代码/未完成项见[HANDOFF](HANDOFF.md)，实测时间线见[历史摘要](archive/README.md)。

## 1.1.5 主线

- 无忧行IO actor（cd0684e已验证）：只消费caller-provided非阻塞AsyncIO，不dial/connect/installprovider。原TLS+ARC单actor、共享close/write-pollgate/Notify、command/events字节有界、IO/RPC/TLS期限、partial/cancel关闭不重放/丢IO与JoinHandle确认；原metadata/ticketconsumer接线，不从write成功授SDK/hosting。Build39958776382/Beta39958776314/ARM64+AMD64 success，官方SHA ZIP各251回归＋4sharedretainedSIP/86矩阵核验，378Python/staticmock通过，Release skipped；14新actor实际执行，超时reviewpartial指出两test编译缺口已修但不是全审查。仍缺原全Router/NetStream控制、ENC部署/端点/provider持久化APItrunk/SMSclass/通话media/费用续期并发，不设备服务部署，本机Rust仍禁。

- 无忧行可信TLS/明文流（784cd3c已验证）：rustls TLS1.2/1.3显式importedroot/原authority+exactSAN，无earlydata/resumption/额外trust兜底；只有真握手成功才加密login。Common原envelope长度和native mode3 canonical16382分片定义明文边界，不加猜测BE32，不把SSL/rustls read当packet。原ARC一次TLS或rawtransport、private verified packets/固定handshake与cipher/carrier/plaintext partial deadline/EOF隔离；共享exactSAN保持原SIP算法/测试路径。Build39937771208/Beta39937771426/ARM64+AMD64 success，官方SHA ZIP各237 Jegotrip回归＋2sharedSAN/2retainedSIP及原四矩阵86场景核验，376静态/mock通过，Release skipped；两辅助review超时没有成功确认，main独立审查，原静态fixture命中guard失败留证修复。不启provider、不设备服务部署/本机编译，live socket/NetStream/关闭消费排他/ENC部署/短信分类落库与通话媒体/费用续期并发仍缺。

- 无忧行下层离线传输（902f4f5已验证）：静态还原PacketSender四字节BE32原packet长度、缓存分流与Router独立8-byte头/opaque routing/有序两半。新增packet_stream/router_packets/packet_transport，绑定一次原ARC及exact独立TX/RX；有界长度/帧数/EOF、错scope先拒绝、错误/关闭共享终止，carrier不交RPC/不授ready。Build39912321845/Beta39912321855及ARM64/AMD64成功，官方SHA ZIP逐名精确各208模块回归/四矩阵86场景核验，373 Python静态/mock通过，Release skipped。只读review发现close不能撤销旧borrowed bytes，已更正checked snapshot并实际回归，未来live writer/TLS消费/取消排他未实现。IDA MCP拒绝连接但ELF静态材料够用，确需后续IDA才告知用户；辅助TLS研究超时不算完成review。socket/runtime/可信TLS与plaintext边界/provisioning/SMS业务/media/provider仍未实现，无部署/设备/服务端/本机编译。

- 无忧行离线调度/入站扩展（8efcd49已验证）：用户暂无实网参数，继续只读native还原六词local/peer incarnation+exclusive next+bitmap、32批/45s history、2048去重/原server响应ticket。新增owned scheduler、配置式recvOnlineMessage codec/dispatcher与OfflineBackend组合，未提交/取消snapshot不伪ACK、queued未submitted不被poll结果完成，票据借用回压重试/一次消费、original owner/scope和无ready副作用。Build39834770425/Beta39834770520实际各172模块回归/ARM64+AMD64成功，官方SHA测试ZIP逐名/四矩阵86场景核验，370本机静态/mock通过，Release skipped。type0后BE16两字语义补正为计数1/唯一方法版本0而非major/minor，字节不变。未接socket/可信TLS/完整boundary/provisioning/短信分类落库/通话媒体/API/provider/计费续期并发，provider关闭；不是实网互通或完整理论完成。

- 无忧行ARC会话/packet子阶段（684ef73已验证）：只读IDA纠正login/swap第二串为原选SDK session ID；phone token写metadata `pwd`、返回ARC token写`token`、SipPwd另用，不混四份材料。新增getAppConfigs、owned login/auth/swap及Common有界分片/FF FF 06 carrier、offline协议组合；一次绑定原ARC tracker/私有owned batch回包/terminal scope，错连接同序号不重绑，登录退休/失败清partial，ForceFlag保持0/ENC拒绝。Build39812859460/Beta39812859585实际各139模块回归与ARM64/AMD64成功；官方SHA测试ZIP精确逐名/四矩阵86场景核验，368本机静态/mock通过，Release skipped。Router/NetStream/NetSsl socket/完整packet边界、可信TLS/部署解密/调度/网络入站/真实短信通话仍未实现，provider不启用，无设备/服务端/部署；不把子阶段或CI当整个任务完成。

- 无忧行RPC封装/批次（5d4bbb0已验证）：继续只读IDA定位type0流标记，修正1d8d042漏字节夹具；补Common request/reply、signed int32索引swapCalls请求/响应批次、loginSession入参/返回及terminal错误footer。原提交owner/operation与连接scope追踪精确reply，旧/重复/注销回包不授新hosting；消息与binary __magic原字节保留，解析长度/count/重复/trailing有界。95项模块回归由Build39767535250/Beta39767535255及双架构通过，官方SHA测试ZIP逐名/四矩阵86场景核验；366静态/mock通过。不能用通用TLS socket直接替代外层NetStream/FF-FF/DataKeyTls封装，ARC认证/续期与真实业务仍未实现，provider不启用，无部署。

- 无忧行原生适配（1d8d042已验证，stream前缀随后校正）：只读IDA确认托管参数的二进制版本/字节串/map排序及本地完成通知/cookie；新增RPC argument编码、字节保真的收消息适配，注册/注销同名通知按提交时operation隔离，重建同名线路用独立incarnation，交付边界检查过期/超时且过期优先。55项模块回归由Build39747922739/Beta39747922813两套Actions及双架构通过，官方摘要ZIP逐名核验、四矩阵86场景通过；365静态/mock通过；参数不是完整ARC/TLS帧，不启用provider、不声称真实托管/短信/通话可用；本轮未部署/访问设备或服务端。

- 应用通话目录隔离（31d3739 已验证）：无忧行迁至 `connectivity/app_voice/providers/jegotrip/`，Google Voice/Telegram/iMessage 各自独立目录；共享 registry 不共享账号/协议，旧 IMS import 用 re-export 兼容。无忧行新增 controller 调用 SDK 登录→托管注册→注销/退出，有限超时/回调归属/停止后换凭据；没有Linux wire backend不授ready。iMessage仅消息声明，所有未实现provider仍unsupported。本轮静态分析未访问设备/服务端；Build39626186439/Beta39626186611双架构成功，官方摘要测试ZIP各14原凭据+8controller+6registry测试逐名通过，363静态/mock成功。

- 无忧行静态复核（d6381b3 已验证）：撤销“无 SIP 字符串即私有协议”和 `libmsc.so` 媒体栈推断；嵌套 DEX 证实 JusTalk SDK 会话使用 phone token，托管网关另用 `SipNumber`/`SipPwd` 等五字段。新增 `jegotrip` 外部凭据与两阶段 SDK 适配契约、归属/撤销/到期测试，元数据 API 明确 `transport_implemented=false`；登录 UI 暂缓，Linux backend/真实收短信和接打电话仍未实现。本轮无设备/服务端操作；两套 Actions/ARM64/AMD64 成功，官方摘要测试 ZIP 各 14 项新契约回归逐名成功，361 静态/mock 通过。见 [勘察](DEVELOPER.md#无忧行托管静态证据凭据与实现边界)。

- R13（无模组线路 + 预留应用语音接口，417bfcb 已验证）：`LineRuntimeRegistry::refresh` 为每条已安装五码线路注入 `line_kind="virtual"` 的 `LineRuntime`（无 modem/port/reader），`virtual_line_binding` 只填身份字段；虚拟线路因此复用既有 VoWiFi 自动恢复、访问决策与诊断链路，且 `/api/cellular-ims/lines` 过滤掉它们避免重复展示；`non_3gpp_voice::PLANNED_PROVIDERS` 预留 `app_voice`/`google_voice`/`telegram_calls` 三个 id 与标签，接口返回 `unsupported`，不注册、不可选。
- R12（五码虚拟 SIM 接线，426c2a7 已验证）：DB 表 `virtual_sim_key`/`virtual_sim_profile`；`virtual_sim/db.rs` 主密钥自举、密封读写、删除、单事务全量重加密、启动 `install_all`；接口 `GET/POST /api/ims/virtual-sim`、`DELETE /api/ims/virtual-sim/{line_id}`、`POST /api/ims/virtual-sim/rotate`、`GET /api/non-3gpp/providers`（响应永不含 Ki/OPc）；前端线路卡片 `+` 新建 WLAN 线路、五码弹窗（接入类型下拉、OP/OPc 二选一、保存后清空密钥）、虚拟线路与模组线路同列表展示、主密钥轮换按钮。两工作流/双架构 + 前端 Playwright 全绿。
- R11（virtual SIM 存储/接口，34100f7已验证）：`virtual_sim/sqn.rs`（48 位 SQN、严格新值校验、AUTS 用 next）、`virtual_sim/store.rs`（五码严格解析、OP/OPc 二选一并校验一致、AES-256-GCM 落库加密、AAD 绑定 profile 防止记录挪用、密钥轮换入口）、`non_3gpp_voice.rs`（非 3GPP 应用语音的**契约占位**，默认零注册）。两工作流/双架构、各 566 项相关回归/86 场景通过。
- R10（Milenage 核心，cec2aa9已验证）：新增 `virtual_sim/milenage.rs`（TS 35.206：f1/f1*、f2..f5、f5*、OP→OPc 推导、AUTN 校验、AUTS 生成），用 TS 35.207/35.208 Test Set 1 全部 8 个输出 + Rijndael 内核向量验证；两工作流/双架构、各 550 项相关回归/86 场景通过。密钥材料 Debug 全程脱敏，模块只用于主机侧 VoWiFi。
- R9 `9f1625d`已验证：新增每线路只读 QMI UIM refresh 订阅（Register 0x002A / Complete 0x002C / indication 0x0033），忙时投 deny 最多 20 秒；完成仅作提示，重读 R5 快照后置 identity_stale 并等通话排空再按门禁重建；不支持时退回边界检查。同轮 Actions 验证通过（首轮仅测试断言缺 Debug，生产未改）。
- R8 `9f1625d`已验证：`profile.ims.identity_source` 变为可执行策略：`auto`（默认，含旧标签）保持历史行为，`isim_required` 缺 ISIM 时失败关闭，`usim_derived` 忽略 ISIM 并走 USIM AKA；AID 随策略一致，旧 catalog 字符串不改变行为。两工作流/双架构、各 542 项相关回归/86 场景通过。
- R7 `d373ffa`已验证：修复XFRM操作检查旧代次后又从可变worker handle捕获新代次的竞态，全部生产install/uninstall及失败回滚传递原UeWorkerBinding，原子enqueue和回包门禁拒绝跨代命令。两套Actions（Build-Release 38999487086、Validate Beta 38999487107）/双架构、各522相关回归/86场景含5项真实worker消息/代次回归，342静态/mock通过；双架构官方包SHA/meta/ELF/嵌入提交/全文件清单已只读核验，未解包执行。不改算法、精确选择器或设备；软件侧已确认缺口闭环，按用户要求停在实机验收前。
- R6 `6672a64`已验证：原retained provider PCO复核、固定来源重解析及空闲同PDN/worker注册级重选；不从空/partial结果推断撤销，不重激活承载，不借旧Digest或降级旧IPsec/TLS，临时失败保留旧到期/等待及实际旧端点，切换后重建观察器。两工作流/双架构、各517相关回归/86场景含8重选＋2时钟新回归、339静态通过；首轮2个结构守卫定位已精确修正，后续观察器/旧端点审查补强重新跑全套。响应时刻固定租期；新离线汇编进一步定位列表比较并校正ISIM状态标签证据等级，未设备/IDA/部署。
- R5 `6203511`已验证：共享同卡应用/ISIM材料快照及双接入注册/AKA/stale/423/续期/注销边界复核，两次稳定读取与owner前中后核验；变化/读失败不复用旧身份或提交租期，原订阅已收到等待保留。两工作流/双架构、各507相关回归/86场景含19新测试和335静态通过；首轮晚到材料变化丢失已发送nc/CSeq的真实缺口已修正，原断言未放宽、失败制品保留。不声明实时UICC REFRESH事件或实机验收，R6继续。
- R4 `0351006`已验证：补双接入注销新失败的恢复反馈，同订阅捕获状态/独立注销等待跨清理保留，控制与诊断分离，不重试注销、不延长租期、不影响新SIM。两套Actions/双架构、各488相关回归/86场景含16新测试及官方摘要核验通过，332静态/mock通过；首轮5项Contact独立expires夹具误判已修复留证，生产协议未放宽。原报告六项有界对照已落表，固件包/分析ELF/4MBN与506记录/373EFS复校；未知动态更新/完整运行分支和实机仍未确认，未部署。
- R3 `31d09ff`已验证：补WiFi初次鉴权后的423重试，保留原protected channel/接收对与ESP状态，将floor交回共享预算并重算Digest nc/proof。两套Actions/双架构、各472相关回归/86场景含10项新真实authenticator/worker/gateway-policy测试及官方摘要通过，328静态/mock通过；前端未改，复核旧49单测/12browser制品而非重跑。首轮缺Security-Server的夹具失败已修复留证，不放宽安全要求，未设备/部署。
- 用户校正范围：本轮只按EC20证据补齐SimAdmin IMS注册逻辑及最终查漏；独立carrier_Bundles未修改且不需修改，移出必做/阻塞。X2额外诊断不替代注册兜底，停止扩展；此次仅校正文档/任务，不擅自回滚功能。
- X2 `ca38391`已验证：EC20单参数只读NativeFleet观测、same-SIM/owner/host排他、取消drain保护、零IO缓存、认证API和原工作台显式按钮；末项UT、零有valid-bit歧义，不变成主机ready。三工作流/双架构、两套各462相关回归/86场景、49单测/12browser、官方ZIP/19源码/三张截图及325静态通过。首轮3处fixture非空断言lint失败已修复留证，未设备/NV/部署，X1范围及最终复审继续。
- N8 `22646c8` 已验证：显式承载需求/driver前置门禁、Native QMI/MBIM认证、原WDS CID P-CSCF地址/域名及保留原IPv6观察、worker opt-in DHCP信息请求、API/原UI脱敏和旧保存兼容。三工作流/双架构、各417项相关回归/86场景、46单测/4browser、官方ZIP/源码/截图和321静态/mock通过。首轮browser断言误命中说明文字已修复留证；通用PCO写入、MM认证变更与DHCP地址分配明确unsupported，未部署/设备/IDA，相关扩展与固件复审继续。
- `c3fb036` 完成 R2 WiFi 普通 Retry-After 恢复：统一端点策略、同卡跨profile/worker等待与已耗计数、有效原租期/typed operator-API控制、注销门禁；TCP死流、原worker与reg-event撤销竞态有独立回归。两套Actions/双架构、各304项相关回归/四矩阵及官方摘要通过，316静态/mock通过。首轮误关旧认证前404兼容已恢复，旧测试不改且最终通过，失败证据保留。未部署，N8/相关扩展/固件复审继续。

- `b6a7888` 完成 R1 刷新预算补漏：三候选共用423协商预算，按结构化失败止损并保留 Min-Expires 依据；423→格式拒绝/503、floor边界、新鉴权后400、正常兼容与TCP死流有7项新生产回归。两套Actions/双架构、各257项相关回归/四矩阵及官方摘要核验通过，311静态/mock通过，未部署。前端源码与N7相同，R2/N8/固件复审仍继续。

- `ee9ee28` 完成 N7 同源执行诊断：shared schema、cellular/WiFi 实际观察、epoch/serial/弱owner、统一 API 与原工作台只读折叠区；三套 Actions/双架构、各250项相关后端回归/四矩阵、43单测/3browser与官方摘要/截图核验通过。声明模板、报价/协商、收到等待/真实门禁、历史成功/当前 readiness 分开。首轮编译lint和第二轮browser fixture问题修复留证，未部署。刷新423连续预算与WiFi普通等待仍独立待修，不以页面或绿色CI算整体完成。
- `6c0c140` 完成 worker 内 SIP TCP/TLS、四向 TCP/IPsec、原 authority 精确 TLS DNS/IP SAN、有界 DNS TC→TCP/NAPTR D2T，保留 UDP 和防降级；两套 Actions、双架构、各 219 项相关回归及四矩阵/官方测试摘要通过，未部署。TLS 不绕过 required-IPsec，私有 CA/mTLS/新的反向 TLS-IPsec 未冒称支持；首次 E0451 已修复并留证。

- `3d54aa8` 接入同 owner 的 reg-event 订阅与注册状态通知，严格匹配实际 REGISTER 绑定和版本/dialog；无关或坏通知不影响 readiness。合法撤销保留同卡恢复等待与活动对话排空，MWI/REFER 不变；两套 Actions、双架构与相关制品证据通过，未部署。
- `0c1fff8` 增加 bearer/worker 内 DNS TC→TCP 的同问题有界重试及 SIP NAPTR→UDP SRV 发现，保留旧 direct/SRV 优先路径与显式来源；两套日志各 186 项相关回归及四矩阵通过。**这不是蜂窝 SIP TCP/TLS 实现**，完整兜底目标仍在推进。

- `3509e15` 完成双接入 REGISTER 响应认证：最终 Request-URI/Authorization/opaque 与 rspauth、qop/nc/cnonce 关联验证，先验后提交 nonce、租期和安全状态；无效 2xx 不借 profile/头型降级，可选 rspauth 缺失保留兼容。
  两套 Actions、双架构构建和前端通过；测试 ZIP 官方摘要、各 76 项相关回归及 86 模拟场景已核验。两轮新 fixture 失败与修正留在[交接](HANDOFF.md)，**未部署、未发布新 Release**，reg-event 等剩余项未混称完成。
- `344308c` 已补强 AUTS 的 qop/opaque 保留、stale/nextnonce 的独立 AKA 来源与原 ESP 通道复用，以及跨候选 423 预算；WiFi 初次认证后 423 仍明确 unsupported，不假装支持或重发旧 nonce-count。

- `5aaf3ea` 增加共享 ISIM 材料/身份与认证应用绑定，保留无配置时的 USIM 派生，VoWiFi EAP 仍用 USIM。
  接入 QMI、native AT 和 PC/SC 读取，保留明确读取错误与卡/owner 复核；不是热插拔实机验收声明。
- 蜂窝 P-CSCF 保留端口、UDP 传输和来源，承载内 DNS 返回有界多候选；显式 TCP/TLS/sips 不再被静默降级。
- 蜂窝 REGISTER 临时拒绝和 Retry-After 贯通端点/跨 profile 等待；刷新保留仍有效的旧保护绑定但不延长租期。
  两套 Actions 及双架构构建成功，测试产物官方摘要、各 87 项相关回归及四矩阵均已核验；**未部署、未发布新 Release**，见[交接](HANDOFF.md)。

- 统一设备接口；MM仍为默认，native为显式实验性选择，不能混用同一物理owner。
- 线路/SIM身份分离、namespace隔离、SMS持久化、资源/逻辑通道账本及设备专项维护。
- 离线同版本完整发布包与校验安装器；普通push不自动发布，不覆盖旧Release。
- IMS完整报价、续期、安全恢复事务、原线路工作台及只读显示逐步补强；验证范围须看具体commit。
- 原生端到端业务、全设备/多流、未知资源自动恢复、全部SIM兼容并未由版本号保证。
- `7bdc59f`运营商专属尝试仍由`0502395`保持撤销；`c174551`以全局协议状态修复，不恢复特例。
- 全局蜂窝 REGISTER 兜底保留空 AKA/Supported、继承已确认的安全要求、区分超时探测，并在计费前去重等价候选。
  真实安全要求下，缺少可用安全参数的挑战在 AKA 前停止；不放宽算法白名单、普通 403 或认证后止损。
- `c174551` 已通过两套 Actions 的 86 场景及相关 Rust 回归，并部署到 410；原 Globe 配置恢复 IPsec 注册，
  配置/运行库保持。自然续期、中国移动同卡与业务未由本次初始注册验收保证；停服重枚举的处理及证据见[交接](HANDOFF.md)。

## beta 发布说明摘要

| 版本 | 已包含的重点 | 验证边界 |
|---|---|---|
| 1.1.4-beta1 | 注册/续期事务、Supported/Require规范化、单/多流准入、旧安全上下文回滚 | 原通道自然续期和单注册回退有设备证据；网络未接受多流，不声称双注册成功 |
| 1.1.4-beta2 | cellular_ims命名兼容、纯Rust DNS、UserProvidedOrder、配置子树补全、资费门禁 | 应用DNS和受保护自然续期验证；专用运营商DNS/全部业务未统一实测 |
| 1.1.4-beta3 | 紧凑AT双栈解析、实际WDS族、空AKA身份、初始化等待 | 初始/UDP同会话续期；四库测试可实际落到同一derived，不等于四库专属配置都通过 |
| 1.1.5 | 统一设备接口与上述主线功能 | CI/发布/设备验收独立，不能把不同commit的同版本包混用 |

早期GitHub上错误产生的1.1.7/1.1.8数字不是这条代码线比1.1.5更新的证据；始终核对commit及包摘要。
详尽旧发布说明保留在精简前Git历史及本地文档备份，不继续维护多个互相冲突的状态页。

## v1.1.4-beta3（预发布）

- 修复紧凑 IPv4v6 AT 输出的本地地址／P-CSCF 解析，并交叉核对同一 CID 的 CGPADDR。
- 原生 WDS 与 SIP 使用同一实际启动地址族；端点未就绪不再耗尽运营商配置槽。
- 标准派生蜂窝 IMS 首条 REGISTER 按 IMS-AKA 规范携带空挑战身份信息，
  避免在进入 AKA 前因缺少私有身份被拒绝；保留认证失败后的终止边界。
- 新实机完成初始注册、原 UDP 通道自然续期及四库派生兜底复测。
  具体范围和未验证项目见本页发布说明摘要及[历史验证](archive/README.md)。

## 未发布

> 以下保留早期 v1.1.4 开发阶段的归纳；后续 beta 版本的独立验收范围以
> 本页发布说明摘要与历史证据为准。

- **实验性 native 专项维护与通道账本。** 增加 Quectel EC2x/EG25 诊断和需精确线路/revision
  确认的维护 API、默认只输出计划的 DJI `dji-prepare`；未知写入/关闭结果保留 receipt，不自动
  停 MM、切 owner、写 NV 或回滚。SIM/APDU 账本覆盖 AT/QMI 与 lpac 外部操作范围；
  通话/驻网事件可唤醒权威核对，保留低频兜底。已通过双架构 CI，**未做对应 native 硬件验收**；
  直接短信 PDU/送达报告完整消费仍待完成。详见 `docs/NATIVE_BACKEND_STATUS.md`。

- **原生 AT 事务与 URC 分流。** 增加有界、无敏感正文的事件提示；修复异步呼叫结束
  误伤无关查询、USSD 文本内 `>` 被误判短信提示等情况。启用原生短信接收后，事件可触发
  存储扫描，仍保留 15 秒轮询及既有持久化/删除保护。默认 MM 不变，native 实机业务验收
  仍待完成，详见 [原生后端事件契约](NATIVE_BACKEND_STATUS.md)。

- **新增只读原生设备发现 `simadmin discover-native`。** 按 USB 驱动与 WWAN/MHI 拓扑
  列出物理锚点、端口和配置缺口；多控制口不任意选择，AT 角色只作提示，IMS/data 端点留空。
  不发硬件命令、不改变 MM 默认后端、不自动接管。参见 [原生设备发现](NATIVE_BACKEND_STATUS.md)；
  native 注册/短信/通话的实机验收仍独立待办。

- **IMS 注册相关命名从 `volte` 统一为 `cellular_ims`。** VoLTE 只是 IMS 之上的语音
  业务，而这些名称描述的是蜂窝接入的 IMS 注册本身（同时承载短信、语音和补充业务）。
  错误码改为 `cellular_ims_*`，前端按完整 token 精确匹配，不再做子串匹配；
  接口 JSON 与线路配置写出新字段名，旧字段名仍可读入；启动时把数据库中的
  `volte_refresh_stats` 表、短信/事件的 `volte_ims` 传输标记和 `volte.*` 事件类型迁移为
  新名称，界面对旧值仍能正确显示。`/api/volte/*` 旧路由继续保留为别名；
  环境变量改为 `SIMADMIN_CELLULAR_IMS_*`，旧名仍生效。界面上的“4G/5G”“VoLTE”文案不变。
  详见 [IMS命名与兼容契约](IMS_REGISTRATION_POLICY.md)。相邻 `carrier_Bundles` 保留真实语音能力
  `services.volte`，但不再用它否决 LTE IMS 的静态 readiness；新增 SMS-only IMS
  profile 不宣告 MMTEL 的契约回归。既有 sealed catalog 不就地修改。

- **Telegram 通知支持自定义反代地址。** 新增 `api_base` 字段：留空仍直连官方
  `https://api.telegram.org`，不改变现有部署；填入自建反代（如 CF-Workers-TGbot
  这类保持 `/bot<token>/<method>` 路径的代理）即可在直连受限的网络下发送。
  只接受 HTTPS，拒绝携带账号密码、查询串、片段和 `..` 的地址，IP 字面量必须是
  公网地址；Bot Token 限定为 Telegram 实际签发的字符集，并从报错和响应中脱敏，
  避免路径中的 Token 随错误信息外泄。不内置任何第三方域名。
- **运营商数据库下载改为读取 Release 中的全部数据库。** 之前是写死的三个 URL，其中
  iPhone 那个因上游文件改名（`26.6` → `26.6.1`）已经 404，小米那个数据库则一直选不到。
  现在打开对话框时枚举最新 Release 的所有 `.sqlite3` 并显示大小。
- **VoLTE IMS 注册修复。** 派生 profile 的候选阶梯缺少带空 AKA Authorization 的形态，
  导致运营商始终以 400/421 终止、从未发起 AKA 挑战。补上该候选并保证 sec-agree 头与
  空 AKA 一起发送。
- **VoWiFi 自定义 ePDG DNS 支持端口。** 之前 `1.1.1.1:5353` 这类配置会被静默改回 53。
- **carrier profile 写入接口拒绝残缺请求。** PUT 时如果漏掉 REGISTER 三态开关，
  以前会让默认值替运营商做决定（其中四个默认为"发送"），现在直接报错并指出缺哪个字段。
- VoLTE IMS 地址族回退顺序调整，部分双栈承载失败后会尝试首选族。
- 线路卡片文案精简：移除 VoLTE 下与 VoWiFi/Trunk 不一致的静态说明。

### 📌 v1.1.4

- 暂停旧仓库的一键安装、在线升级和 OTA 发布入口；当前仅提供手动构建与部署文档，相关
  脚本和 OTA 契约将在重构完成后重新设计。
- 将 modem/SIM 建模为独立 `line_id`，蜂窝控制、数据代理、流量、eSIM、VoLTE、VoWiFi、
  通话和短信 API 全面改为线路级寻址与持久化。
- 重构后端为 `connectivity`、`hardware`、`services`、`platform`、`api` 五个领域，并抽取
  VoLTE/VoWiFi 共用的 IMS、SIP、Digest-AKA、短信与语音核心。
- 新增每基带独立 IMS QMI 端点、VoLTE bearer/注册恢复、VoWiFi 多运行时与每线路代理能力。
- 新增 VoWiFi/VoLTE IMS 语音选路、VoWiFi/VoLTE/CS 短信路径策略、接收腿选举、跨传输
  去重和每线路 SIP Trunk。
- eSIM 管理由全局工作模式下沉到每条基带/读卡器线路，支持自动探测与手动启停。
- 引入只读、已封存的 schema v7 carrier catalog，并支持本地覆盖及 AOSP/IPCC 配置事实导入。
- 扩展自动化任务、系统事件、通知队列、真机诊断和 Bruno 线路级 API 集合。
- 重写项目总览、开发与运行环境文档，归并过时的前后端子目录 README。

### 📌 v1.1.3

#### ✨ 新增功能

- 正式上线 WiFi Calling（VoWiFi） 功能，支持 WiFi 环境下的短信收发。
- 自动适配主流运营商配置，无需手动设置，降低使用门槛，原生支持标准运营商，免去繁琐手动配置。
- 提供可视化诊断面板，实时监控连接状态与运行参数。
- 支持配置、状态、日志持久化存储，兼容自动化任务调度。

#### 🏗️ UI 重构

- 侧边栏重新布局，高频功能置顶显示，与系统配置项明确分隔。
- 新增统一的 SIM 卡管理页面，整合 SIM 卡基本信息和 eSIM 管理功能，优化界面布局。
- 取消独立的设备信息页面，相关内容合并至仪表盘。
- 拆分系统配置为多个独立页面，新增单独的安全性设置入口，页面层级更清晰。

#### 💫 体验优化

- 优化 eSIM Profile 详情页布局与交互细节，提升操作便利性与界面纯净度。
- 优化全局 UI 视觉样式，移除冗余元素，调整页面间距与文字样式，界面更清爽。
- 优化 VoWiFi 错误诊断与故障排查建议，提供更加友好的故障引导。

#### 📚 接口与文档更新

- 新增 12 个 VoWiFi (WiFi Calling) 接口的 Bruno 调试文件，同步补充接口端点与参数说明至 Bruno 接口文档。
- 规范化重构项目文档体系，精简主 README.md 并将其模块化拆分为安装部署、环境管理、开发指南及版本更新记录。

### 📌 v1.1.2

#### ✨ 新增功能

- 当芯片未返回总空间时，支持手动设置 eUICC 芯片容量，自动计算空间占用更新进度条展示。
- 短信发送成功后，延迟 5s 自动清理 SIM 卡临时短信，防止 SIM 卡空间占满无法收发短信.
- 下载安装 eSIM Profile 时，新增 SM-DP+ 服务器、激活标识码（Matching ID）的数据库缓存与详情展示。

#### 💫 体验优化

- 优化 eSIM 详情底层策略排版，将“允许删除”和“允许禁用”紧密并列合并为单列展示。
- 优化 eSIM 写卡交互，写卡成功后保留完整终端日志，移除页面遮罩，取消按钮动态切换为返回。
- 将 GSMA 标准函数标识、网络类错误转换为通俗易懂的中文提示。
- 重构版本更新检测流程，直接读取 GitHub Release 元数据，检测阶段无需下载安装包，降低流量损耗。
- 移除通知中心 MD5 相关字段与渲染逻辑，精简通知模板，自动适配旧模板。

#### 🐞 bug 修复

- 全链路规范化清洗 ICCID，修复状态展示、缓存异常问题。
- 修复 lpac 相关参数缓存失效问题，增加重试、智能扫描逻辑补齐参数。

### 📌 v1.1.1

#### ✨ 新增功能

- 优化通知中心模板，在版本更新、自动化事件通知中新增 `{{本机号码}}` 变量，多机部署场景下可精准定位设备来源。
- 上线自动化中心，支持`重启基带`、`重启设备`、`发送短信`三类自动化任务，提供基于定时及间隔周期的双轨定时任务后台调度引擎。
- 通知中心新增自动化事件转发规则与事件通知能力，实现业务联动推送。
- 新增自动化运行日志系统，支持对自动化日志分页展示、关键词检索、日期筛选、日志清理，默认开启自动清理策略。

#### 💫 体验优化

- 优化通知中心转发日志自动清理策略，默认开启并统一动态开关状态。
- 日期选择器全面公共化，移除本地实现，复用系统公共组件。
- 统一转发日志模块列表字号，优化视觉展示效果。
- 优化转发规则列表滚动体验，固定顶部标题栏，仅内容区域支持滚动。

#### 🐞 bug 修复

- 修正日期选择器组件引用旧版属性引起的编译错误。

#### 📚 接口与文档更新

- 新增自动化配置、日志查询、清理与单次测试执行全套 API 接口。
- 完善说明文档，补充自动化中心使用场景与接口调用规范。

### 📌 v1.1.0

#### ✨ 新增功能

- **实体 eSIM 全量写卡与元数据缓存**：新增 eSIM 资料下载安装能力，写卡完成后自动解析 ICCID、IMSI、运营商编码等卡片信息并本地持久缓存，补齐写卡后无法读取卡片元数据的短板。
- **二维码本地解析**：内置离线解码能力，支持点击 / 拖拽图片解析 LPA 激活码，无需调用设备摄像头、不依赖外部网络接口。
- **LPA 地址智能解析**：粘贴 LPA 格式链接（如 `LPA:1$...`）时自动拆分服务器地址、配对 ID、校验码等信息，一键自动填充表单。

### 📌 v1.0.9

#### ✨ 新增功能

- 短信通知模板新增 `{{验证码}}` 变量，可按需嵌入转发规则使用，原有默认模板保持不变。
- 后端新增专属验证码识别提取器，结合数字位数、位置权重、关键词关联、格式特征及干扰词剔除等规则进行加权识别智能解析验证码。
- 适配中英文主流验证码短信格式，可识别带短横线分隔的验证码，并自动规整为纯数字格式。
- 通知中心新增设备状态定时报表，支持定点或间隔周期推送，兼容现有通知规则、推送通道、转发日志及免打扰机制。
- 报表支持各项状态指标单独启停监控，涵盖网络连接、蜂窝状态、系统运行、流量使用、短信转发统计、OTA 更新等设备核心信息。
- 短信转发统计支持今日、最近 24 小时、最近 7 天和累计统计周期，优先展示接收、成功、失败关键数据，自动隐藏无数据空值项。
- 新增消息队列与保护机制，转发通道支持独立频率配置并预置默认规则。

#### 💫 体验优化

- 设备状态数据采用懒加载采集策略，仅读取已开启指标数据，减少无效系统调用，节约系统资源。
- 后端统一处理温度传感器名称映射，报表与仪表盘统一展示中文友好名称。
- 设备状态报表模板支持分类段落排版，规整内容层级结构，条理更清晰，方便查阅各类设备指标信息。

### 📌 v1.0.8

#### ✨ 新增功能

- 短信管理加入常驻搜索框，支持按号码、短信内容检索会话；匹配项可自动高亮检索关键词。
- 短信统计新增推送数据统计，直观展示推送成功、尝试推送条数。
- 新增系统配置「安全性」页面，支持启用密码保护、修改管理员密码、密码策略、会话有效期和空闲超时设置。
- 重构通知中心架构，解耦为转发日志、转发规则、转发通道三大模块，明确分工：通道负责发送、规则负责筛选分配、日志负责监控排错。
- 升级转发通道，支持多实例配置，可创建多个同类型通道，支持独立命名、启停、配置及测试发送。
- 升级转发规则为事件路由层，支持多类型事件匹配、多目标通道分发、纯文本模板配置，新增星期 + 时间段免打扰。
- 转发日志新增分页、多维度筛选及清空功能，细化状态分类，提升转发链路问题排查效率。
- 转发日志增强时间范围筛选、高级清理和自动清理配置，支持按类型、状态、时间段及保留策略管理日志。
- DDNS 转发规则新增连续失败推送阈值，支持达到指定失败次数后再推送，减少短时网络波动造成的通知干扰。
- 通知中心新增系统事件通知，覆盖基带、蜂窝网络、设备网络、系统/服务、安全审计、SIM/eSIM 和资源告警，支持按事件独立启停、阈值/恢复推送及系统事件模板。

#### 💫 体验优化

- 精简短信统计面板，隐藏总计数据，保留收发统计，新增独立推送统计卡片，支持悬停查看数据说明。
- 持久化存储短信转发状态，过滤无效异常数据，保证推送统计结果准确可靠。
- 优化登录页和首次设置密码页视觉效果，补充忘记密码提示。
- 支持按已保存的密码策略校验新密码，并提供密码强度提示。
- 统一通知模板为纯文本格式，前端支持中文变量名，简化模板配置。
- 优化短信监听转发策略，区分历史短信补录与新短信转发，避免批量转发与消息漏推。

### 📌 v1.0.7

#### ✨ 新增功能

- 新增管理员密码登录能力，支持初始配置、会话校验，防护页面与接口访问访问保护。
- 支持 SSH 本地命令交互式重置密码、清空认证配置后重新设置。
- 增加密码复杂度校验，过滤非法格式字符。

#### 💫 体验优化

- 优化本机号码变量展示，国内号码精简为 11 位格式，境外号码保留或补全国际前缀。
- 版本检测改为北京时间每天 09:00 和 18:00 执行，并新增超时限制、多节点降级重试，提升更新通知获取成功率。

### 📌 v1.0.6

#### ✨ 新增功能

- 数据连接管控从 ModemManager D-Bus 迁移至 NetworkManager，通过nmcli实现联网启停。
- 交由 NetworkManager 自动完成接口启用、IP、DNS、IPv6 及路由全流程配置。
- 新增双轨流量统计机制，精准获取蜂窝网口真实上下行流量。
- 新增统一网络地址选择能力，仪表盘与 DDNS 共用同一套 IPv4 / IPv6 网口判断逻辑。
- 在未自动识别到 SIM 卡的本机号码或短信中心（SMSC）时，允许用户在界面手动输入并持久化缓存。
- 手动输入的本机号码立即生效，自动支持并映射通知中心的短信转发变量。

#### 💫 体验优化

- 职责拆分，ModemManager 仅负责信号、注册状态等只读查询。
- 程序启动自动清理旧 `unmanaged` 配置，自动生成蜂窝网络配置。
- 移除 `wwan0` 等网口名称硬编码，由 `NetworkManager` 自动适配识别。
- APN 配置界面默认选中并展示当前已连接的 APN 槽位。
- 优化 DDNS 公网 IP 获取逻辑，非蜂窝网卡跳过冗余 D-Bus 请求，减少资源开销。
- 完善蜂窝接口流量展示，支持展示真实上下行流量字节与数据包数量。
- 优化仪表盘与 DDNS 的 IP展示和获取优先级，有无线网络时优先使用无线网口，无无线网络时自动回落到蜂窝数据网口。
- 自动识别无线网口、蜂窝网口和默认路由，不再依赖 `wlan0`、`wwan0` 等固定网口名称。
- 在仪表盘和设备信息页增加 SIM 号码空值识别，无缝集成在线编辑组件及输入格式校验。

#### 🐞 bug 修复

- 彻底修复数据连接假开关问题，解决前台显示已联网、实际接口未启用无 IP 无法上网的故障。
- 修复正常网口因内核限制被误判为 `UNKNOWN` 的问题，优化状态自动纠正逻辑。
- 彻底解决高通平台蜂窝网口流量统计恒为 0 问题，同时兼容联发科、展锐等多类模组设备。
- 解决内核网口统计数据不准，造成前端无法展示真实蜂窝流量的问题。
- 修复仪表盘和 DDNS 在部分场景下误选本地回环、管理桥或链路本地地址，导致显示或解析 IP 不符合实际上网出口的问题。

### 📌 v1.0.5

#### ✨ 新增功能

- 新增短信中心号码、本机号码多源读取 + 缓存能力，多通道静默提取、命中即止。
- 新增缓存表，接收短信时自动学习并持久化存储短信中心号码、本机号码。
- 新增 SIM / eSIM 工作模式功能开关，默认保持普通 SIM 模式。
- 依托 `lpac` 实现实体 eUICC 卡轻量 eSIM 管理，配套管理页面支持卡信息查看、配置文件切换、重命名与删除。
- 通知模板新增本机号码变量，适配短信转发场景。

#### 💫 体验优化

- 短信中心号码、本机号码读取失败均静默处理，不影响功能使用与页面展示。
- 优化短信接收可靠性，服务启动、Profile 切换和基带恢复后会主动检查未同步短信。
- 优化短信去重策略，减少设备重启或短信编号复用导致的误判漏收。
- 采用命中即停策略，减少冗余调制解调器指令调用，降低日志干扰。
- 优化系统配置页面 UI，调整布局并新增工作模式卡片。
- 侧边栏随工作模式动态隐藏 eSIM 相关入口，普通 SIM 模式不加载对应页面资源。
- 优化 eSIM 调用逻辑，仅使用对应功能时调用 `lpac`，无需常驻后台进程。
- 优化设备开机及 SIM 搜网注册阶段前端界面频繁报错的问题，同时将用户操作触发的错误提示改为常显，解决此前错误信息自动消失过快导致用户无法阅读的问题。

#### 🐞 bug 修复

- 修复 PushPlus 部分配置场景报错问题，提升通知渠道兼容性。
- 修复短信中心号码读取时，ModemManager 非调试模式的未授权告警问题。
- 修复实体 eSIM 卡在 Profile 切换或基带恢复后，短信已到设备但 Web 页面未显示的问题。

#### 📚 接口与文档更新

- 数据库新增号码缓存表，持久化存储 SMSC 与本机号码。
- 新增工作模式、eSIM 管理全套 API 接口。
- eSIM 接口适配 `lpac` 标准 JSON 输出，适配各类 eSIM 管理操作。
- 完善文档，补充相关使用说明。

### 📌 v1.0.4

#### ✨ 新增功能

- 通知中心新增 PushPlus 通知渠道，支持配置 Token、标题模板、群组编码、消息模板、发送渠道、渠道参数和回调地址，并接入短信、DDNS 事件转发及测试发送。
- 通知中心新增版本更新提醒：后台每天北京时间 09:00 和 18:00 检测，发现新版本时按渠道首次提醒一次，不对检测失败或无更新场景推送通知。

#### 💫 体验优化

- 抽象封装前端通用 IPv6 公网地址筛选工具，仪表盘连接展示、设备网络 DDNS 接口统一复用同一套判定规则。
- 数据连接看门狗保留蜂窝状态巡检与 Modem 自愈能力，不再自动清空宿主机 iptables/ip6tables 规则，避免影响 Docker、VPN 及容器端口映射等业务。
- 切换蜂窝数据连接时取消全局防火墙规则刷新，减少对宿主机网络栈、容器转发链路的副作用。
- 通知中心统一在模板渲染层将短信、DDNS 的时间变量格式化为北京时间，避免各渠道收到 UTC 原始时间。

#### 🐞 bug 修复

- 修正 DDNS 模块 IPv6 公网地址判断逻辑，前后端统一以公网域 IPv6 作为 AAAA 记录候选地址，避免误选链路本地地址、内网 ULA 地址。
- IPv6 地址候选按 `/128` 优先排序，提升多 IPv6 地址场景下 DDNS 自动选择地址的准确性。
- 修复宿主机 `FORWARD` 默认策略为 `DROP` 时，清空 filter 表规则引发 Docker 网桥转发失效、外部无法访问容器映射端口的问题。
- 修复企业微信应用消息可能报错的问题。

#### 📚 接口与文档更新

- 明确 NetworkManager `wwan*` 非托管配置仅用于防止蜂窝接口被双重管理，不会直接修改 iptables 规则。
- 同步更新 README 及 Bruno 接口示例，标注 `iptables/ip6tables` 仅作只读诊断使用，程序不会自动清空宿主机防火墙规则。

### 📌 v1.0.3

#### ✨ 新增功能

- 新增“设备网络”模块，用于配置设备侧 WLAN 客户端联网、 DDNS 动态解析，以及后期其他高级功能的扩充。
- 新增设备侧 WLAN 管理能力：
  - 开启 / 关闭 WLAN。
  - 扫描附近无线网络。
  - 连接、断开和忘记已保存 WLAN。
  - 保存自动加入和 IPv4 地址配置。
- 新增原生 DDNS 同步能力：
  - 支持腾讯云 DNSPod、阿里云 AliDNS 和 Cloudflare。
  - 支持 IPv4 / IPv6 独立解析配置。
  - 支持通过网卡或多接口 API 获取公网 IP。
  - 支持立即同步、后台周期同步、同步状态和最近 50 条运行日志。
- DDNS 变更和失败事件接入通知中心，可按通知渠道独立开启“DDNS 变更”转发。

#### 💫 体验优化

- 侧边栏和页面分为“蜂窝网络”和“设备网络”，职责更清晰。
- OTA 更新页面优化待安装更新和验证结果展示，在线更新支持 GitHub Release 检查、下载加速节点和下载进度。
- 仪表盘温度展示改为冷到热连续热力色，并同时标识最低和最高温度探头。

#### 🐞 bug 修复

- 运营商扫描和手动 / 自动注册增加超时处理，避免 ModemManager D-Bus 调用长时间阻塞。
- 数据连接 watchdog 增强长时间 `searching` 状态恢复逻辑，可自动尝试重新注册或循环射频状态。
- 注册遇到 QMI network selection internal 类错误时统一走射频恢复流程，减少注册异常后的卡死概率。

#### 📚 接口与文档更新

- 新增设备网络 API 文档和 Bruno 示例，覆盖 DDNS 配置、状态、同步、日志以及 WLAN 状态、扫描、连接、断开、忘记和配置保存。
- API 契约新增 DDNS、WLAN 和 DDNS 同步日志相关类型。
- 配置持久化新增 `device_network.ddns`，通知渠道配置新增 `forward_ddns`。

### 📌 v1.0.2

#### ✨ 新增功能

- 频段锁定状态新增后端报告的真实支持频段：
  - `supported_lte_fdd_bands`
  - `supported_lte_tdd_bands`
  - `supported_nr_fdd_bands`
  - `supported_nr_tdd_bands`
- 小区与信号数据增加 `qmicli` 兜底解析，提升 ModemManager `GetCellInfo` 不可用场景下的可观测性。

#### 💫 体验优化

- 优化移动网络注册问题的诊断路径，重点区分 SIM 归属运营商、当前注册网络、漫游状态、射频模式、频段限制和小区数据。
- 前端自定义频段选项改为基于后端 `SupportedBands` 动态显示，不再依赖固定写死的 LTE/NR 频段列表。
- 从“未锁定”切换到“自定义”时，默认勾选当前设备真实支持的全部频段，更符合使用预期。
- 移除网络页面中不必要的“验证说明”文案，简化频段锁定 UI。

#### 🐞 bug 修复

- SIM 信息优先从 IMSI 推导 MCC/MNC，避免注册到漫游网络时把当前网络运营商误当成 SIM 归属运营商。
- 后端设置自定义频段时会校验用户选择是否被当前 modem 支持；发现不支持的频段会返回明确错误，不再静默忽略。
- 自动注册遇到 QMI Internal 类错误时增强恢复处理，降低调制解调器处于异常注册状态时的卡死概率。

#### 📚 接口与文档更新

- 修正 README、Bruno API 示例 and 前端开发代理中的默认访问地址为 `http://192.168.68.1:3000/`。

### 📌 v1.0.1

#### ✨ 新增功能

- 新增“通知中心”多渠道通知配置能力。
  - 通知配置从单一 Webhook 扩展为多渠道：
    - Webhook
    - Bark
    - 企业微信应用消息
    - 企业微信群机器人
    - 钉钉群自定义机器人
    - 钉钉企业内机器人
    - 飞书机器人
    - Telegram 机器人
  - 新增通知中心后端接口：
    - `GET /api/notifications/config`
    - `POST /api/notifications/config`
    - `POST /api/notifications/test/{channel}`
  - 新增各通知渠道的测试发送能力，可按渠道发送模拟短信测试。
- 新增短信批量管理能力。
  - 删除短信：
    - 删除单条短信
    - 删除整个短信对话
    - 批量删除短信或多个对话
  - 新增短信删除相关后端接口：
    - `DELETE /api/sms/message/{id}`
    - `DELETE /api/sms/conversation/{phone_number}`
    - `POST /api/sms/batch-delete`
- 新增设备侧一键卸载脚本 `uninstall.sh`，支持彻底卸载和保留用户数据卸载。

#### 💫 体验优化

- Dashboard 仪表盘加载性能优化。
- 通知中心页面重构为左右分栏布局：
  - 左侧展示通知渠道列表和启用统计
  - 右侧展示当前渠道配置表单
- 通知中心支持按渠道独立配置启用状态、短信转发、模板和渠道专属参数。
- 通知模板编辑支持中文变量标签，前端显示中文变量，保存时映射为后端变量。
- 通知渠道测试按钮会先保存当前配置，再执行测试，减少“配置未保存导致测试不生效”的误操作。
- 短信页面优化为更完整的对话管理体验：
  - 对话列表支持删除按钮
  - 聊天消息支持单条删除
  - 支持批量管理，有清晰的选择数量提示
  - 删除这种不可逆的危险操作增加二次确认
- 短信列表加载数量提升，减少历史短信遗漏。

#### 📚 接口与文档更新

- README 中 Webhook 章节改为“通知中心”，补充多渠道通知能力说明。
- Bruno 文档同步更新通知中心接口说明。
- README 新增卸载脚本使用说明、参数说明和清理范围说明。
- API 类型定义新增完整通知中心配置类型，包括各渠道配置结构。
