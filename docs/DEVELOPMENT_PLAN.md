# 开发计划：优先级与剩余验收

> 本文只维护跨项目优先级、未闭环工作和发布条件，不保存逐日进度或已完成清单。
> 当前交接见 [HANDOFF](HANDOFF.md)，能力边界见 [README](../README.md)；历史证据见
> [档案索引](archive/README.md)。代码实现、CI、实机/运营商验收与发布必须分别报告。

## 当前基线与工作规则

- **当前原生组件增量`8bd0439`已核验**：Build41165670060/Beta41165670082、双架构、官方digest/CRC/source-run各821 Jegotrip（14新增）＋21HTTP及原回归/86；477static/mock、前端检查成功、67首编译warnings。独立endpoint TLS/PacketSender与原TLS品牌Router连接控制已实现，旧Common路径不改；**仍未接到生产TCP/完整Router path/动态account与接收器**，主目标不能标完成或以仅认证替代。
- **前专用数据库登录增量`ff87712`已核验**：Build41157401567/Beta41157401553、双架构、官方digest/CRC/source-run各807 Jegotrip（42登录）＋21HTTP及原Trunk/通知/SIP/DNS/86矩阵，前端检查成功、469本机static/mock单次全过，66warnings。`jegotrip_*`独立表/迁移、CAS/清空墓碑、默认enable_hosting/start_receiver=true已接线；前三轮语法/类型/Drop/HTTP错误分类失败已修，原失败证据保留。**原App实际TLS/Router启动与共享参数自动装配仍是当前必做缺口**，不是仅待实网；新版本未部署，410日志已验安装为5250123。
- **前账号登录增量`d449163`已核验**：Build41002428358/Beta41002428390、双架构、官方digest/CRC/source-run各780 Jegotrip＋19HTTP及原回归，前端检查成功。原App账号密码→绑定号码/电话凭据→正常业务开通/读回确认→显式原接收器接线已验证；安全挑战不伪造通过，原不确定写请求不重放。该检查点的文件配置已由新数据库优先/整份旧文件兼容取代，不宣称默认SDK全自动配置。
- **前静态续查/压缩SDP增量 `2986a3d` 已核验**：Build40585189198/Beta40585189753、ARM64/AMD64成功，官方digest ZIP/CRC/逐名各757 Jegotrip＋7registry＋17HTTP＋46Trunk＋31通知＋4SIP＋7DNS/86，67warnings保留，Release skipped。原邀请Base64检测→解码→zlib inflate、回复同格式zlib deflate→标准Base64已实现，解码后plain-media门禁不放宽；5项新回归含实际账号/Trunk/native Accept。APK静态确认csb/token活跃获取链，但refreshToken helper无调用/响应语义；SRTP/DirectConnect被当前APK显式关闭，server显式，NetStream/Locator无已证call path。
- **前退出进程回收增量 `1401be0` 已核验**：Build40525320619/Beta40525320712、ARM64/AMD64成功，官方digest ZIP/CRC/逐名各752 Jegotrip＋7registry＋17HTTP＋46Trunk＋31通知＋4SIP＋7DNS/86，446static/mock，67warnings保留，Release skipped。flock证明退出、仅回收本机制标记的租约，旧版无标记租约与存活锁持有者不动。
- **前自动重连增量 `e5ef46d` 已核验**：Build40517095074/Beta40517094859、ARM64/AMD64成功，官方digest ZIP/CRC/逐名各749 Jegotrip＋7registry＋17HTTP＋46Trunk＋31通知＋4SIP＋7DNS/86，445static/mock，67warnings保留，Release skipped。opt-in、cleanup完成后同revision全新start、错误分类与次数有界、stop/start/replace/关机取消；不是凭据续期或会话恢复。
- **前长期收件增量 `c6f0ba1` 已核验**：Build40499505183/Beta40499505215、ARM64/AMD64成功，officialdigest ZIP/CRC/逐名各744 Jegotrip＋7registry＋17HTTP＋46Trunk＋31通知＋4SIP＋7DNS/86，444static/mock、67warnings。DIRECT原双180秒窗口及实际响应write后frontier；batch原result-bearing epoch精确ACK→FIFO私有retirement/sparse身份与before-base水位，不过pending/unACKed result。各保最近128完成envelope与全部SMS、未知/pending不清，稀疏2048/account+8192global仅按proof退休；GC失败不重发原ACK/result。新14项含同run600batch、2300ring滚动、partialACK/低gap/删除不复活和129postACK GC失败；DIRECT600及前回归保持，详见DEVELOPER。
- **前断线托管增量 `b13223a` 已核验**：Build40458126579/Beta40458126681、ARM64/AMD64成功，officialdigest ZIP/CRC/逐名各711 Jegotrip＋7registry＋17HTTP＋46Trunk＋31通知＋4SIP＋7DNS/86，438static/mock。全call watcher补振铃断线/过期/版本变化；可选cleanup_on_disconnect defaultfalse只清本manager退休原run、20秒有界/同owner最多重等一次，不自动start/reconnect/renew/清unknownorphan。原Trunk FIFO先处理/发Ended/CANCEL/BYE再停，history SQL失败保lease，terminal原因和cleanup_error分开。首fixture在release后查询当前call的错误已修、原失败ZIP保留；详见DEVELOPER。
- **前无忧行基础范围：接电话/接短信，外呼/发短信只留接口。** `d0a2b67` 已推送并核验：Build40445960947/Beta40445960949、ARM64/AMD64成功，官方digest ZIP各699 Jegotrip＋7registry＋16HTTP＋42Trunk＋31通知＋4SIP＋7DNS/86矩阵逐名/source-run匹配，435static/mock通过，Release skipped。多任务Join取消重试、batch回执queued与Inform准入、应答/挂机入队前取消三项缺口已由新6项及双Join回归验证；不以旧4375a53成绩替代。
- 基础入站已接原 receiver/control、实际 Endpoint TCP-Mpath↔UDP 音频、账号/API/Trunk、durable SMS与通知。当前已加入账号登录UI/API，但仍不是默认完整SDK或付费外发实现；真实服务部署与资费仍未验收。实施/配置说明见[开发指南](DEVELOPER.md#当前基础托管配置与操作)。

### 历史实施检查点（不覆盖当前范围与能力）

以下为各旧提交当时的证据/缺口；其中“未实现来电/媒体/provider”与较宽的付费外发/全部SDK计划不代表当前状态，不能据此重做已接通组件或扩大本轮目标。

- **前bootstrap代码 `7cc8b8f` 已验证**：Build40202373634/Beta40202373625/ARM64 AMD64 success，官方SHA测试ZIP各525 Jegotrip＋31通知service/模板＋12真实HTTP/auth＋4SIP＋7DNS/原86矩阵，410static/mock、58warnings/17节选、Release skipped，首轮全过。原matched getAppConfigs字段/引用→单独导入account trust和destination scope→原source actor Join后新connection/TLS/login已接真实AccountManager；raw DomainName与Client Domain/App/CC、原pwd/selector/device、双root/SNI/Scope/总deadline/取消和旧version1配置兼容均实际验证，不启provider/Channels/paidoutbound。完整locator/default选路/NetStream、call ingress/media/trunk及独立policy/principal并发/renewal、orphan与共享历史仍open，不能把此支持子集改称全backend或只剩实机。

- **前原run/通知代码 `c55f773` 已验证**：Build40135247656/Beta40135247579/ARM64 AMD64 success，官方SHA测试ZIP各499 Jegotrip＋31通知service/旧模板＋10真实HTTP/auth＋4SIP＋7DNS/86矩阵，406static/mock、58warnings/19节选，Release skipped。原run持久fence、actor及consumer双Join后已投影envelope回收、保留SMS/unknown raw；正常run结束后不再终身占用128配额，但原direct协议128活跃history不改。已渲染通知计划与SMS同事务入队，目标指纹/CAS claims/有界工作池/到期和不确定结果、原上下文HTTP实测、main关机与pending start取消已接通；首main Duration编译缺口已修，失败ZIP保留。未启UI/globalprovider/Channels/paidoutbound。完整native和进程/业务后续仍open，详见本页P0及`app-notification-implementation-evidence.md`。

- **前Smgw文本代码 `0390692` 已验证**：Build40121560400/Beta40121560403/双架构success，官方SHA测试ZIP各471 Jegotrip＋8真实HTTP/auth＋4共享/原SIP＋7DNS/原86矩阵，403static/mock、56warnings/16节选，Release skipped。已证实原Sip.Request Smgw文本可进入app_voice独立线路的共享SMS history：严格UTF8/sender/可选millis，raw＋SMS＋双脱敏event同事务commit后原consumer回执，sender/received时间源明确；其他kind/非法编码时间不猜。SMS删除清逻辑raw副本且原票据retry不复活，existing SMS读/删与新增认证messages API接通，三路外发在IMS/CS前拒绝。global provider/Channels仍false，通知外发/历史清理、默认native路径、call/media/Trunk、独立费用续期并发仍open。原IDA已实时metadata确认同SHA/base0并只读核实JSON/DEX文本路径，无用户手动操作需求；没有本机Rust/设备服务/部署或IDB写。

- **前独立账号/耐久入站代码 `d666f10` 已验证**：Build40114399262/Beta40114399090/双架构success，官方SHA测试ZIP各445 Jegotrip＋6真实HTTP/auth＋4共享/原SIP＋7DNS/原86矩阵，401static/mock；Release skipped，56warnings/16节选保留。独立app-account私有配置/即时事务cap64/CAS、脱敏CRUD/start/stop及原scope/Join清理接通；generic body/sender/time与原receiver binding、revision/run nonce/协议序号私有入站，raw＋脱敏事件原子commit后回复原ticket，限额/去重/回压与marker失败不replay。旧receiver缺失保持unknown，不猜SMS/PDU，不授provider/channel/外发。前423账号阶段及442inbox核心有独立证据，两轮失败夹具/provider-id修复ZIP保留；REST403以官方网页digest/job状态和原REST成功元数据严格核验，非只看绿灯。证据`.local/jegotrip/verified-native-d666f10.json`及`app-inbox-implementation-evidence.md`。

- **前显式驱动代码 `d593551` 已验证**（实现9809ae7，首E0618 callback fixture shadow修复）：Build40057801164/Beta40057801104、ARM64/AMD64成功，官方SHA测试ZIP各精确394 Jegotrip＋4共享/原SIP＋7原DNS/原86矩阵通过；391static/mock，57warnings/17Jegotrip excerpts保留，Release skipped。真实RuntimeConnector/PreparedRuntime/NativeHostingBackend原owner/physical scope接线、shared Hickory prepared DNS与逐poll gate、owned callback/poll/semantic unregister/SDKlogout/Join cleanup已实现，仅显式 supplied DirectTlsCarrier profile。不是默认SDK自动provision/locator、全Router/NetStream/媒体或已安装provider；Channels/外发仍false。状态及首失败见`.local/jegotrip/runtime-backend-implementation-evidence.md`。

- **历史注销代码 `8e9a570` 已验证**：Build40045409934/Beta40045409943、ARM64/AMD64成功；两套官方SHA测试ZIP各逐名355 Jegotrip＋4共享/原SIP、原86矩阵通过，387本机static/mock，Release skipped，60warnings/20Jegotrip excerpts保留。原STATUS3内层结果与外层ACK/SDK logout/IO release分层；独立deadline覆盖所有后续IO及晚到结果，完整aggregate/queue admission在cancel guard前，同响应fallible发送/事件交付完成后才发布成功。两个readonly source reviews完成，main修复三具体缺口；不把source inspection或offline CI当实网/完整backend。Controller原backend_owner、commands-only clone、Secret Dropwipe与失败cleanup保留均保持，无设备/服务/部署/IDB写/target执行或本机Rust。

- `60c6c64`的341＋4/86已由Build40026210652/Beta40026210599及双架构/官方SHA核验；`65fad23`的315＋4/86由Build40017848477/Beta40017848512核验。Router/Channel kind2/3、2048-bit统计不drop duplicate、16-bit邻片、8/10header/LSB ACK/gap31/BE16/FE-FF为数据边界；缓存expiry/owned Dropwipe问题已修。尚未接完整liveNetStream/时序/路由/可信connector，不授provider/channel/outbound。先复用23日志中用户已开IDA后的139次只读调用与原SHA ELF，仅具体缺证据才请求同文件MCP，不能把旧gate或缺实网配置当停止理由。

- 当前部署/config/reference/ownedbootstrap子阶段`ed6fb33`已由Build39983918744/Beta39983918718/ARM64+AMD64验证，官方SHA ZIP各281＋4old/sharedSIP及86矩阵逐项核验，381静态mock通过、Release skip；57warnings/17Jegotrip excerpts保留。salt8DES-CBC-PKCS5/1000MD5仅配置unwrap，explicitClient/Reference contextkey，无SDKdefault/auth；facet/id/adapter-locator/explicit -h/-p是候选数据，私有originalowner/scopebootstrap与actor配置event不改TLStrust/授ready。partialreview3issues main修、dropwipe/确定padding负例，非fullreview；仍fullRouterNetStream/provision/可信endpointconnector/providerpersistAPI/SMSmedia费用续期并发未接通。无本机Rust/设备服务部署，该已验证解码子阶段未使用IDA，unsupported/outboundfalse。

- 原会话transport_actor已由cd0684e Build39958776382/Beta39958776314/双架构、官方SHA ZIP各251模块回归＋4sharedSIP/86矩阵验证，378静态mock通过，Release skipped：caller-provided AsyncIO单actor/共享pollgate+Notify、取消/部分write关闭丢IO、JoinHandle确认drain，精确原metadata/私有borrowed票据/真实RPCpendingdeadline，无dial/provider。251实际离线回归不是实网，不报真实托管；下一仍fullRouter/NetStream/ENC端点/persistenceproviderAPItrunk/SMSclasscallmedia/费用续期并发。

- 当前可信TLS/明文流扩展已由784cd3c Build39937771208/Beta39937771426/双架构验证，官方SHA ZIP各237模块回归＋2sharedSAN/2原SIP及86矩阵核验，376静态/mock通过，Release skipped：显式importedroot/originalauthority/exactSAN、TLS1.2/1.3内存引擎/验证前无login，Common按self-delimiting envelope/canonical16382分片，不猜额外BE32或read边界；固定handshake/partial deadline/EOF、原ARC一次slot/private verified scope。已验证237 Jegotrip＋2sharedSAN及原SIP测试，不把绿灯替代实网。无socket/live actor/ENC provisioning/短信媒体/provider业务，尚不能标全部backend；这批不需要用户IDA。

- 本轮续接PacketSender/Router offline扩展：SHA绑定ELF/FDE/ARM64静态证实下层BE32原packet长度，并新增有界stream decoder、Router8-byte/opaque无扩展routing/有序平衡两片及一次原ARC transport；902f4f5已由Build39912321845/Beta39912321855/双架构与官方SHA ZIP精确各208模块回归/86矩阵核验，373本机静态/mock通过，Release skipped。本步ELF足够，只有后续确需才请求用户手动开IDA。IDA MCP拒绝连接，不自行启动/加载；数据仍是opaque TLS，不能直接交ARC或授ready。未知route/控制/扩展/顺序wrap拒绝；既有borrowed/copy snapshot不可由close撤销，live writer/TLS取消/消费排他仍未实现。下一重点是可信TLS与record-aware plaintext边界、真实socket/NetStream/ENC provisioning，继而SMS分类落库/provider/API/trunk/call-media/费用续期并发，不用缺配置伪造默认值或仅剩实机。证据`.local/jegotrip/resume-network-framing-evidence.md`。

- 用户暂无实网配置，最新要求继续离线/理论完善无忧行剩余内容。当前已还原并实现有界六词/ACK/32history/45s/2048dedup scheduler、exact configured recvOnlineMessage与offline组合；8efcd49已由Build39834770425/Beta39834770520/ARM64+AMD64及官方SHA ZIP逐名各172模块回归/四矩阵86场景核验，370静态/mock通过。不猜端点/trust/receiver-kind或计费，不授provider/外发；剩余下层网络/完整boundary/ENC部署、SMS分类/落库/provider/API/trunk/call-media/续期/并发仍是真实实现缺口，不能改写成只剩实机验收。

- 本轮继续用户已开启的 `libmtc.so` 只读IDA：5d4bbb0有两套Actions/双架构/官方SHA测试ZIP/95模块回归/四矩阵86场景中间证据；其swap第二串已发现应为原选SDK session ID而非返回ARC token，后者属于metadata `token`，phone token属于`pwd`、SipPwd另用。当前新增getAppConfigs、owned ARC login/swap、Common分片/FF FF 06 carrier和原owner/scope nested hosting接线已由684ef73 Build39812859460/Beta39812859585/双架构、官方SHA测试ZIP逐名139模块回归/四矩阵86场景核验，368本机静态/mock通过；不把offline协议子阶段/CI当完整客户端。剩余重点为Router/NetStream/NetSsl socket/packet边界与可信TLS握手、ENC provisioning/端点、batch ACK/dedup调度、网络入站对象和真实短信/通话、provider/runtime/persistence/API/trunk/计费/续期/并发；暂不部署、不访问设备或服务。

- 应用provider本轮迁到 `connectivity/app_voice/providers/{jegotrip,google_voice,telegram,imessage}`；无忧行controller接SDK两阶段/注销/失败时序由31d3739双架构及两套Actions各8项controller回归验证，旧IMS import兼容。DEX/JNI参数已确认但Linux wire帧未还原；继续native `Mtc_CliLogin`/`Mtc_SmgwUpdateStatus`等分析，必要时用户手开IDA。不把接口调度当作已能收短信/接打电话；暂不部署。

- **新增无忧行托管任务（静态分析）**：先走手动导入自有 root 手机凭据路线，不做登录/绑定 UI。SDK phone token 与 SIP 网关密码已定位；契约模块 d6381b3 已通过两套 Actions 各14项新测试/双架构；Linux backend、持久化/provider 接线、收短信/接打电话、计费/续期/设备并发仍未完成，不注册虚假 provider。详细参数与阶段见 [勘察](DEVELOPER.md#无忧行托管静态证据凭据与实现边界)；本轮不部署、不访问设备或无忧行服务。

### IMS原任务与通用工作规则

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

## P0：无忧行 backend 的实施顺序与原生证据关口

用户恢复的目标是基础接电话（接收/应答/挂机/双向音频、账号/API/Trunk）与接短信（durable历史、查询/删除、通知、原ACK）。既有路径允许从自有 root 手机导入合法原参数；用户现已追加App账号登录、自动取得通话凭据和开通托管的要求，登录UI进入本轮范围，不新增号码绑定；打电话/发短信只留接口，资费属于独立无忧行账号。继续完成这个完整基础目标，不以codec存在、hosting成功或仅安装descriptor替代业务链路。

**业务前提须单独说明：**高级导入以已有合法开通/参数为前提；新App账号登录入口可按用户授权提交正常业务开通并读回确认，不先强迫手机蜂窝一键认证，也不伪造`authentication_code`或忽略服务器挑战。普通账号`start/stop`仍只管理客户端，不取消业务，导入布尔值不当开通证明。安装侧原HTTP/native私有上下文、真实账户资格与支持模式仍是前提，不以账号字段猜SDK参数，详见[开发指南](DEVELOPER.md#app账号登录主入口与一次性部署配置)。

| 必须交付的行为 | 当前实现/验证关注点 |
| --- | --- |
| 原受信连接与托管 | 新endpoint TLS/原品牌Router控制已软件验证；仍必须接生产TCP/原IO Join、完整path时序和动态account/回调与共享装配。既有DirectTlsCarrier/matched bootstrap保持，不删ri/换TCP/编造profile，不授SDKready |
| 独立账号生命周期 | 私有配置、revision/run nonce持久排他、auth/no-store API、手动start/stop；不恢复cached ready，不从其他manager或未知孤儿接管 |
| 原来电应答/挂机 | CallClientEx/direct与batch原票据、Inform/Accept/Release及本地call history/event；本次修复queued批次回执不能被误当Inform已可提交 |
| 真实媒体/Trunk | 原Endpoint TCP-Mpath与实际UDP双向RTP、raw或zlib＋Base64 native SDP按邀请镜像，解码后匹配RTP/AVP/payload、同call的OperatorLink/SIP应答/BYE；只有原媒体活跃才connected |
| 耐久接短信/通知 | 原Sip.Request严格分类，raw＋SMS＋事件＋通知计划同事务commit后原ACK，查询/删除/tombstone/目标指纹/有限重试；其他编码不猜 |
| 取消、到期及能力边界 | queued Connected重核；任务入队前取消保持可重试；每个Join单独确认并移除；Voice/Messaging不互授，过期/换revision不移交旧grant |
| 外发接口与费用隔离 | 有效reserved请求501，Asterisk/interactive/automation SMS或call均不能借入站ready走SIM/IMS/CS或免费WLAN规则 |
| 证据与手册 | 新SHA两Actions实际逐名执行、官方digest ZIP/CRC/源码-run/86矩阵及双架构；原固定五手册、私有交接同步；不把静态/helper/绿灯当实网 |

**当前长期接收已分别验证**：c6f0ba1两Actions/双架构及officialdigest744/17HTTP/46Trunk等清单通过；cb33653的DIRECT730项也有独立证据。直接窗口、真实response write后frontier、batch精确result-epoch ACK与durable sparse/before-base证明解决及时完成的已分类收件终身128/512限制；不驱逐unknown/pending，不靠新nonce/重连掩盖容量，低gap/burst/未ACK与磁盘仍有限。原watch/cleanup/FIFO能力保留。后续工作保持：

1. APK静态已确认活跃phone token由认证`csb/token`的`body.token`获取并供下一次login使用；`refreshToken` helper无直接调用、TTL/响应/活动会话交接证据，因此自动续期仍需原服务授权证据，当前保持手工重新导入。自动重连只是在清理后全新start。带标记退出进程运行已由1401be0用OS锁证明回收；remote lease、无标记旧版owner接管与跨run exactly-once仍不提供。
2. zlib＋Base64 SDP已由2986a3d实现；当前APK显式关闭SRTP/DirectConnect且显式server。**当前App的TLS/Router不是未来可选扩展**：8bd0439已实现endpoint TLS/PacketSender与原品牌clientConnect1/3控制，但生产TCP/原IO Join、完整Router path时序/动态account/receiver和共享装配仍在主链前。RouterClient.AccessKey为逗号两项共享上下文，不从24字符SDK App key或个人token猜值；后续其他媒体/Locator/NetStream仍须独立证据。
3. 用户已授权410程序/前端及相关数据库升级且要求不做回滚备份，已验安装5250123；ff87712尚未部署。真实账号/服务/Trunk/通知/账单另需明确授权。后续源码保持新SHA Actions/official artifact核验，docs-only编译输入同ff87712。
4. 专用表与主流程默认已软件验证，但不能交付默认仅认证、空表或手工SDK凭据替代品。**优先完成当前原App完整自动启动链**：登录→绑定资料→phone token/SipPwd→正常业务确认→原接收器，保留原owner/Join/ACK与Voice/Messaging/media分层；之后按授权部署到410供用户测试。额外验证码/迁移/安全挑战、自动续期/原地替换仍独立，不重放结果不确定的业务请求。

**前基础软件交付已验证**：`d0a2b67`的Build40445960947/Beta40445960949、ARM64/AMD64及官方digest ZIP核验通过，新增6项admission/batch和双Join取消场景均实际执行；各699 Jegotrip与原HTTP/Trunk/通知/SIP/DNS/86矩阵精确匹配，435本机static/mock通过。固定五手册记录当前配置/API及支持界限；文档收尾不改变编译输入。后续源码改变仍须新SHA验证，不能复用当前绿灯；详细清单见 `verified-native-d0a2b67.json`。以下是明确的扩展与实网验收工作，不再把已接通的基础来电/媒体/SMS写成未实现。

**明确但不伪装已完成的扩展**：当前原生/媒体支持raw或zlib＋Base64 `RseEx.SDP`（按邀请格式回复）、解码后严格RTP/AVP sendrecv、RTCP-mux和编码passthrough。当前APK无条件关闭SRTP/DirectConnect并显式server，但这不证明未来版本；default Locator/Router/NetStream、SRTP/ICE/BUNDLE/P2P、转码、自动token续期、remote principal/device全局lease、无标记旧版owner恢复、额外SMS编码及无限单连接历史仍不是已支持能力。付费外发尤其不是本轮必做项。DIRECT/batch已按原回执和durable证明回收完成副本，但pending/unknown/burst/未ACK及磁盘保持有界；正常run原actor/consumer/媒体Join后继续回收已投影私有envelope且保留SMS，不承诺跨新run exactly-once。

**实网边界**：目前未实际登录无忧行服务或验收账单；410程序5250123曾在用户授权窗口部署验证，本次新版本未部署。静态导入字段、isolated peer和raw/压缩SDP分支不能证明服务实际参数或当前选择。以后经明确授权及自有有效账号验证接收、双向音频、Trunk/通知、有效期/掉线/绑定与独立计费；真实配置依赖未支持模式时继续据证实现，不能改状态或放松TLS/owner门禁冒称兼容。

**报告与工具**：第三方DOCX全段落/表格已只读核对，报告APK与本地APK字节不同；已有Androguard 4.1.4已完成7 DEX的token/refresh/mode xref扫描，当前IDA仍为原libmtc/base0/相同SHA并闭合zlib＋Base64 SDP算法。报告HTTP加密/签名/Android信任策略不是native wire或授权证明。现阶段无需JADX；需要其他新工具时先告知版本和用途，由用户手装。

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
