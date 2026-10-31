# 当前交接

本页是唯一当前交接入口。历史过程见[CHANGELOG](CHANGELOG.md)和[档案索引](archive/README.md)；旧 PID、SIM、地址、脚本或维护窗口不是实时状态，不能直接重放。

## 用户要求与操作边界

1. 本轮恢复原无忧行目标：**基础接电话（应答、挂机、双向音频、Trunk）＋接短信（落库、查询、通知、原回执）**。外呼、发短信仅留接口，计费属于独立无忧行账号，不借 SIM/IMS/CS。
2. 唯一开发目录 `SimAdmin`，只维护 `master`，普通 push `simmaster`；不强推、不清理未知用户文件。编译、Rust/前端测试、编译后的协议 peer 和注册模拟**仅在 GitHub Actions**执行。
3. 本机只做编辑、静态/mock、Git 与只读源码/制品分析。用户已明确授权410程序/前端及相关数据库升级，并要求**不做回滚备份**；保留SIM/IMS配置、历史、unit及辅助服务。已验部署为`5250123`，本次新版本尚未部署；真实无忧行登录/业务、付费外发与NV未授权。
4. IDA 仅只读使用用户手动打开的原样本：通话协议为 `libmtc.so`，本轮账号登录为 `libidss-cn.so`；调用前核对 metadata。不自行启动/加载、修改或保存 IDB。新 JADX/其他工具须先说明版本/用途，由用户手动安装。
5. 保持原 UI 布局；用户已追加**登录已绑定号码的App账号、自动取得通话凭据并开启托管**，此前登录UI暂缓不再限制本任务。现有手工导入保留兼容，不新增号码绑定或付费外发。公共文档维护原五手册及用户授权的逆向总结；报告、APK/固件、JSONL、`.local`、私有交接和凭据不提交。

## 当前：endpoint TLS与Router控制已验证，完整App自动启动仍未完成

- 已验源码 **`8bd0439952c9dba66392bfd09f2b625ce24cedf0`** 已普通推送：实现独立`EndpointTlsEngine`（PacketSender在endpoint TLS内），端点数据品牌不能投为Common/ARC plaintext；原root/authority/SNI/Scope/固定deadline保持。原`clientConnect1/3`控制保留`id=Router`、版本、fresh路由、匹配逻辑ID/footer与原TLS scope，不授SDK/hosting/接收ready。
- 前专用数据库源码 **`ff877127e745fd2274ea7138a2307a2738b00a64`** 已验证：`data.db`中独立`jegotrip_login_settings`、`jegotrip_accounts`、`jegotrip_login_attempts`；原登录任务ID/不确定状态保留，配置revision/CAS与清空墓碑不重新启用旧文件，不保存密码/App token。主入口默认**登录→取电话凭据→确认业务→启动原接收器**，不是仅认证摘要。
- Build **41165670060** / Beta **41165670082**、ARM64/AMD64成功；官方digest ZIP/CRC/source-run逐名各 **821 Jegotrip（14新增端点/Router，42登录）＋21HTTP＋7registry＋46Trunk＋31通知＋4SIP＋7DNS/86矩阵**。前端检查成功；477本机static/mock单次全过，67首编译warnings保留，Release skipped。新内存TLS分层请求/回包及错root/旧scope/零ID/超时/后续坏包抑制通过；前轮语法/类型/nonce/Drop及PUT分类失败的原制品保留。
- 配置API为`/api/jegotrip/config`，登录为`/api/jegotrip/login/*`；数据库优先，只有无配置行才兼容`jegotrip-login.json`。HTTP-only兼容配置不能让主入口退回仅登录，模板存在也不是接收ready；详见[开发指南](DEVELOPER.md#app账号登录主入口与一次性部署配置)。
- **完整原App路径仍未接入账号运行时**：新组件不是默认SDK已启动；还需生产TCP选择、原IO/Join、完整Router path时序/动态account与回调、共享参数装配。现有Profile仍只准原显式DirectTlsCarrier，不能删`-ri`/换TCP或编造参数代替。RouterClient.AccessKey原值为逗号两项共享上下文，24字符SDK App key不能盲当它或个人电话凭据。
- 本轮未访问410或真实服务；日志已验安装仍`5250123`，配置/历史及辅助服务保持。新表、成功fixture或旧部署不证明一键实网可用，不交付仅认证替代品。证据`verified-native-8bd0439.json`及`resume-20261031-01a18672/NATIVE_COMPONENTS_8BD0439.md`。

## 前App账号登录检查点（d449163，实网未验收）

- **源码`d44916340b87e8562a046f782fc706d6711a6790`已推送并验证**（实现8dd159f，后一次只修前端effect生命周期）：支持账号/手机号＋密码→绑定号码核对→后台取phone token/SipPwd→正常认证开通请求与状态确认→创建独立账号并可启动原接收器。现有页面新增账号登录与应用账号区，不输入个人SDK token/SipPwd，不伪造SIM。
- 后台一次性读取`data.db`旁的私有`jegotrip-login.json`：App HTTP/密码预处理上下文与原显式native template分开；不是仅有账号就能猜出所有SDK配置。缺文件/无原OS锁则拒绝登录。APK共享HTTP默认值已另存私有partial context，但**尚无实际完整部署上下文或实网验收**；不把partial文件当可启用配置。
- 2并发、120秒固定尝试预算，4096条持久任务ID防重复提交；先记录`request_uncertain`才发送开通，接受后再次查询状态为2才创建账号。中断/超时不重放或自动关闭原业务；重启仅在原OS锁证明下标中断。口令/请求体不入任务表，原SDK/hosting/接收readiness仍独立。新入口不调用手机一键认证，也不伪造`authentication_code=1`；额外挑战/迁移/既有业务处置如实报错。
- Build41002428358/Beta41002428390、ARM64/AMD64成功，official digest ZIP/CRC/source-run逐名各**780 Jegotrip（23新）＋19HTTP＋7registry＋46Trunk＋31通知＋4SIP＋7DNS/86矩阵**通过；前端lint/type-check/unit成功含7新Node测试。453本机static/mock单次通过，67warnings保留，Release skipped；不是浏览器E2E、原SDK执行或真实服务验收。
- Build artifact12954725911 SHA`6534b64151874ac834611aa9f52792adacd4cfd9c183cb5649256fff049f1c3e`；Beta12954795258 SHA`bea6ba8f3a10a290eb587a3dc78428d1e063593667b84928f081b86563ec2939`。首轮8dd前端lint失败，Beta后端已过；仅改原scope清理对象后两整套通过，没有屏蔽lint/改断言。证据`verified-native-d449163.json`及`account-login-20261029/{implementation-verification.json,first-failure-and-recovery.md}`。

## 前次研究：托管业务与本机无卡边界

以下“未移植”描述的是当时阶段；新的正常账号登录/业务开关以上方为准，安全验证不自动完成。

- 2026-10-29 补查旧会话未答问题。实际APK的两个业务/SDK DEX（16628类、112251方法）只读扫描及SHA-1/Adler-32校验：业务开关`setVoipCsState`、号码/安全设备认证与`JCAccount.updateStatus(4)`在线注册是不同层；前两者**尚未移植到Rust**，当前采用已有合法凭据导入。
- 号码来自App用户对象，当前Rust客户端不依赖本机SIM/modem；但这不是服务端授权绕过。APK号码认证失败后会走安全设备后备查询，成功仍须服务响应`code=0`、`authentication_code=1`，不是无卡直接放行；该认证token也不是SDK phone-login token。
- `enrollment_confirmed`仅是用户声明，不开通服务；`start/stop`不代替号码托管/呼转业务的开通/取消。未访问真实服务、未安装新工具或改运行时代码；源码基线仍`2986a3d`，本轮文档补充不代表新的功能/实网验收。详见[逆向总结第3节](JEGOTRIP_REVERSE_ENGINEERING_SUMMARY.md#31-业务开通设备验证与在线注册是不同层)，私有证据`resume-20261029/hosting-findings.md`。

## 前检查点：APK/IDA续查与zlib＋Base64 SDP已验证

- **该检查点源码 `2986a3d8b1283d8aaa428d252ab83098a1f311bf`（实现7350f5f、两次仅测试契约修正）已推送并核验**：原 `0x613140→0x5fa164/0x251958→0x60ed60` 证明邀请若为完整Base64语法则先Base64解码、再zlib inflate；`0x5fa47c`按同一邀请格式以zlib deflate＋标准有padding Base64回复，raw邀请仍raw。后端现在有独立wire/解压上限，解码后仍严格只接受单音频RTP/AVP sendrecv/RTCP-mux及匹配payload；Trunk/UI看到raw SDP，只有原native Accept镜像邀请编码，crypto/ICE/BUNDLE等解码后仍拒绝。
- Build40585189198/Beta40585189753、ARM64/AMD64成功，官方digest ZIP/CRC/逐名各757 Jegotrip＋7registry＋17HTTP＋46Trunk＋31通知＋4SIP＋7DNS/86，67warnings保留，Release skipped；5项新增压缩SDP回归（固定zlib、URL-safe输入/标准输出、畸形/尾随/解压膨胀、解压后安全属性拒绝、实际账号邀请→Trunk raw→native压缩Accept）均实际通过。Build artifact12772387707 SHA`2e8e4150867c4b31ffc2e9da720a655d2a1a4efd92a68936a83eaccc238d8477`；Beta12771222636 SHA`8fa4eda1c46cf697ce3d5f39db44d46dd493bd9f3cb10d10384e5ffca588cafc`。
- 本地APK的Java静态链确认活跃phone token来自认证的`service/voip/v1/csb/token`：请求`accountId`，成功取`body.token`写原偏好，下一次`JCClient.login`使用；`service/voip/v1/refreshToken`只有URL初始化与`requestDelayTokenValid` helper，在应用DEX 8567类中无直接调用、无响应解析/TTL/活动会话交接。因此续期仍须手工重新导入，不能只凭方法名调用真实服务。
- 同一APK在成功login后无条件`closeAudioSrtp()`（crypto type 0/auth false），`MtcEngine.login`无条件关闭DirectConnect，且总是显式设置server address；SRTP配置getter无调用，NetStream无Java/已证call-path引用，Locator仍只有方法注册名。故当前不实现SRTP、ICE/P2P、default Locator或NetStream，而不是等待凭据后盲猜。首7350f5f为测试局部变量遮蔽＋旧契约断言导致编译/静态失败，第二9a0e1ca仅漏一个HTTP旧字符串；两轮官方失败制品保留，生产算法未因夹具修正改变。

## 前检查点：OS锁证明的退出进程运行回收已验证

- **该检查点源码 `1401be0f0add6f1bf89d17152a43c227b406f90f`（实现04c585d）已推送并核验**：进程启动时（任何账号start与通知worker之前）对`data.db.app-voice-owner.lock`取非阻塞独占flock；之后注册的运行租约标记本进程实例。成功取得锁即证明此前打标记的进程都已退出（它们存活时都持有同一把锁），于是释放其遗留租约：未结束的应用来电写结束时间并发`call.ended`(recovered)事件，`sending`通知改为uncertain并清正文，按原规则回收已分类envelope，短信历史与未知raw保留，并发`app_voice.runtime_recovered`事件。不按PID/运行时长推断；锁被另一存活进程持有时不做任何回收；无标记的旧版本租约仍是`runtime_owner_unconfirmed`；恢复不启动任何会话。
- Build40525320619/Beta40525320712、ARM64/AMD64成功，官方digest ZIP/CRC/逐名各752 Jegotrip＋7registry＋17HTTP＋46Trunk＋31通知＋4SIP＋7DNS/86，446static/mock，67warnings保留，Release skipped；3项新native恢复测试与原回归实际通过。首轮04c585d的Build有一项旧DIRECT测试在GC失败后人工stop收到原生如实的`app_account_disconnected`（同提交Beta及此前三轮均通过）；按既有"同owner再等一次"约定只修夹具（batch同类夹具同步），失败官方ZIP保留，生产代码未改。

## 前检查点：可选有界自动重连已验证

- **该检查点源码 `e5ef46dab3254016ab1ec2ba2aba316f4d407b1e` 已推送并核验**：可选`config.reconnect`（须同时`cleanup_on_disconnect=true`；`max_attempts`1..16、`initial_delay_ms`1000..600000、`max_delay_ms`≤3600000，指数退避）：只有本manager原run完成实际Join与fence释放后，才对同一已存revision做一次全新start（新nonce/owner/登录/托管），不是token续期或会话恢复。仅disconnected/backend_unavailable/operation_timeout可重试；鉴权/托管拒绝、过期、revision/owner/存储错误不重试。自身HostingAck后重置计数；人工stop/start、replace/delete、关机取消或取代待执行重启；未启用supervisor时报`unsupervised`。`GET A`在策略存在时给出`reconnect_attempt`/`reconnect_state`/`reconnect_error`。跨新run的重投与原生一样是至少一次，不承诺exactly-once。
- Build40517095074/Beta40517094859、ARM64/AMD64成功，官方digest ZIP/CRC/逐名各749 Jegotrip＋7registry＋17HTTP＋46Trunk＋31通知＋4SIP＋7DNS/86，445static/mock，67warnings保留，Release skipped；新增4项真实native（断线后单次全新run、鉴权拒绝不重试、失败次数耗尽、延迟期间stop取消/无supervisor）与1项配置回归实际通过。凭据续期仍为手工重新导入。

## 前检查点：DIRECT 与 batch 长期接收已验证

- **该检查点源码 `c6f0ba13bf2277910cc33834bcc1ea6fcd63f6b6` 已推送并核验**：DIRECT 使用原双180秒 magic 窗口、正序不复用RPC ID与独立pending/burst/字节限额；原响应实际write后mint replay frontier，当前line/revision/run lease下同事务清退已完成短信副本。batch仅在原受认证回包ACK承载结果的精确epoch后生成私有FIFO retirement，不凭`batch_queued`或外层成功回收。
- batch保留稀疏已ACK身份与before-base前缀；前缀不越过pending request或working/unACKed result，乱序未见gap仍可交付。稀疏身份最多2048/account、8192全局，只按证明退休，不任意evict；DIRECT/batch各保最近128项已完成短信envelope，完整SMS及原通知处理策略保留，unknown/pending不容量清除。失败回收退休原scope，不重发已提交DIRECT ACK或已ACK batch result；最终原Join仅清对应nonce。
- Build **40499505183** / Beta **40499505215**、ARM64/AMD64成功；官方digest ZIP/CRC/逐名/source-run各 **744 Jegotrip＋7registry＋17HTTP＋46Trunk＋31通知＋4SIP＋7DNS/86矩阵**，**444 static/mock通过**，67warnings保留，Publish Release skipped。新14项batch回归含同原manager/ARC/nonce600短信、2300序号ring滚动、低pending gap、partial ACK、删除不复活及129条后ACK/GC失败不重放。
- DIRECT前源码`cb33653`已独立核验Build40497185743/Beta40497185943各730项/双架构。首`9d0f8a1`两Actions原TLS夹具只有inbox fence、缺runtime lease，生产frontier正确拒绝；仅夹具改原runtime注册并增强SQL/原ACK断言，另补缺lease必须拒绝负例，失败官方ZIP保留。证据`.local/jegotrip/verified-native-c6f0ba1.json`及`continuation-20261027-resume/{direct-first-failure-repair.md,batch-retention-design.md,batch-verification-summary.json}`。

### 前断线来电收尾与可选原运行清理（仍有效）

- **前源码 `b13223ab8f8515a9e27b84a64c12f0a68809e90a` 已推送并核验**：CallService从Checking/Ringing起保留唯一watch，原ticket/account/媒体scope结束即按原operation收尾history/Ended/Join。`cleanup_on_disconnect`缺省false，显式true只在本manager原run退休后20秒有界清理，不自动start/reconnect/renew或清unknown orphan。
- 原Trunk的私有FIFO fence保证Ended/CANCEL/BYE本地处理后才停driver；普通observer或替换/drop/lag消费者不能假确认。不确定发送单独报错；call history SQL失败不释放lease，重等原owner不重放Release。
- Build **40458126579** / Beta **40458126681**、ARM64/AMD64成功；官方digest ZIP/CRC/逐名/source-run各 **711 Jegotrip＋7registry＋17HTTP＋46Trunk＋31通知＋4SIP＋7DNS/86矩阵**，**438 static/mock通过**，65warnings保留，Publish Release skipped。新振铃断线/expiry/revision、原watch不替换、失败清理/consumerJoin/人工取消/显式重启、unknownowner拒绝、actual SIP CANCEL和HTTP均实际执行。
- 首e78a7b1两Actions一个新fixture误在最终lease释放后unwrap当前callquery，现保留原对象/SQLite Ended并断言旧query必须拒绝；仅fixture修复，不放宽原owner/FIFO/Join，官方失败ZIP保留。证据`.local/jegotrip/verified-native-b13223a.json`和`continuation-20261027/{retirement-implementation.md,retirement-verification-summary.json,retirement-first-failure.md}`。

### 前基础整链检查点（仍有效，不代替最新源码验证）

- **前源码 `d0a2b675329489c72542bb3b3d382e02b6eda15a` 已推送**：修复批次邀请回执只 queued 时过早 Inform；只对明确未入队的 `CallNotReady` 按同 owner/ticket 和固定预算等待，不重放已提交 RPC。应答/挂机先取得任务入队位置再改变 queued 状态；取消前未入队保持可重试。每个已完成 Join 即时移除，取消停用后不再重复 poll 已消费句柄。
- 新增 6 项原生协议/任务入队回归，另将停用取消夹具扩展为“已完成第一个任务＋第二个持有 socket 的阻塞任务”。**435 本机 static/mock 通过**；Build **40445960947** / Beta **40445960949** 首轮成功，官方 digest ZIP/CRC/逐名/source-run 各精确匹配 **699 Jegotrip＋7 registry＋16 HTTP＋42 Trunk＋31通知＋4 SIP＋7 DNS /86矩阵**。新回归全部实际通过，ARM64/AMD64成功，Publish Release skipped；保留65条编译warning。
- **前整链提交 `4375a53e90ab88a73bba1a32c48288c37973ba86` 也已独立复核**：Build **40374831389** / Beta **40374831552** 的官方 digest ZIP、CRC、逐名清单和源码/run 指纹匹配，各 693 Jegotrip＋7 registry＋16 HTTP＋42 Trunk＋31通知＋4 SIP＋7 DNS /86矩阵；ARM64/AMD64成功，Publish Release skipped。其 432 static/mock 已再次运行通过，但不能替代新修复验证。
- 已有整链覆盖生产 Rust 驱动与隔离 native peer：认证 HTTP 应答、原 Endpoint TCP-Mpath 与实际 UDP 双向 RTP、通话期间 SMS 落库/原 ACK、Asterisk 侧 SIP 应答/双方 BYE、原端口释放。**不是原 Android SDK 执行、实网/PBX部署或资费验收**。
- 当前证据：`.local/jegotrip/resume-20261027-current/{basic-requirements-audit.md,call-admission-repair.md,report-native-review.md}`、`verified-native-d0a2b67.json`、`d0a2b67-verification-summary.json` 和 `source-state-basic-hosting-progress.json`。一份辅助审查 partial 的三项发现已主线程独立核实；另一审查超时无结果，不当 signoff 或停止理由。

## 已接通的基础行为与使用入口

| 项目 | 当前实现与不变量 |
| --- | --- |
| 独立账号 | `app_account` / `application_data`；App账号登录或高级私有参数导入、随机本地 line_id、revision CAS、显式 start/stop；不伪造 ICCID/UICC，不恢复 cached ready |
| 业务开通/安全设备认证 | 新登录入口可提交正常认证的开通请求并读回确认；安全挑战只报告、不自动绕过。原账号start/stop仍只管接收器，不取消服务端业务 |
| 原来电 | 原 receiver、64-bit native ID、owner/scope/run 关联的邀请、Inform/Accept/Release；SDK/hosting 接受不等于媒体就绪 |
| 音频与 Trunk | 原 Endpoint TCP/TLS＋独立 Mpath media、实际本地 UDP、原 payload/peer 校验；全call watcher、应答/挂机/history/event和Join；先原driver处理结束事件再停Trunk |
| 原运行清理/重连 | `cleanup_on_disconnect`可选、defaultfalse；只清本manager退休run并保留原因/cleanup_error。另可选`reconnect`在清理完成后同revision全新start（有界、仅断线类错误）；不renew token，不猜未知进程退出 |
| 接短信 | 精确 `Sip.Request` 严格文本分类；raw/SMS/事件/通知计划同事务 commit 后才回原 ticket；未知 kind/编码不猜 SMS/PDU |
| 查询与通知 | 既有 SMS 历史、删除/逻辑 raw 清除、通知目标指纹与有界重试；删除或改同名目的地不重放/改投旧短信 |
| 能力隔离 | Voice/Messaging 各自实时原 proof；汇总 available 不互授；排队 Connected 消费时重核原权限、版本/有效期和物理媒体 |
| 外发接口 | 有效外呼/发送请求仍 501 / `app_account_outbound_unsupported`，不生成 outgoing history，不回退 SIM/IMS/CS；Asterisk 外呼同样拒绝 |

配置形状、各字段与完整 API 见[开发指南](DEVELOPER.md#当前基础托管配置与操作)。账号入口 `/api/non-3gpp/accounts`；单账号 `.../{line_id}` 下有 `start`、`stop`、`messages`、`notifications`、`calls`、`trunk`，通话有 `answer`、`hangup`、`reject`。全部沿用现有认证；管理/通话响应 no-store，不导出凭据。

`credential-contract` 的 `transport_implemented=true` 只描述**显式入站实现**，不是当前账号授权。`hosted`、两项 incoming readiness、单通话 media_active 与 outbound_authorized 分开；最后一项本阶段始终 false。202 start 不是已经可以接听。

### 明确的支持与验收边界

- 新入口支持正常App密码登录及自动获取个人电话凭据，需先配置匹配的私有部署文件；本次创建新line，不原地替换旧运行或自动续期，旧历史仍归原line。高级参数导入和matched bootstrap保留。本地`data.db`/部署文件是明文私有存储，不是加密保险箱；管理界面应走HTTPS或可信本机，替换原参数仍须stop/Join，备份与目录权限需保护。
- 媒体限原 raw 或 zlib＋Base64 `RseEx.SDP`（回复镜像邀请格式）；解码后仍只接受RTP/AVP sendrecv、RTCP-mux、匹配编码 RTP passthrough。须 `allow_plain_media=true` 和明确本地 bind IP；信令 TLS 不降级。SRTP、ICE/BUNDLE/P2P、转码及其他原生选路模式不假装支持。
- `Client.Host/ArcPort` 可能是逻辑 Router 地址/端口，不是 OS UDP/RTP 目的地。压缩分支是邀请内容的有界wire包装，不证明服务当前选择哪一分支，也不授权其他媒体模式。
- 可选断线清理20秒有界，与人工stop/关机同原run串行。history失败保lease、consumer/actor/媒体/Trunk原Join后才finalize；自动清理后GET账号保留终端原因与cleanup_error。运行期calls接口不从已release租约重获owner，已结束数据库历史保留。Trunk FIFO只确认本地driver处理/发送，不是远端ACK；发送不确定不能冒称成功。
- 单连接/账号/队列仍有显式限额。DIRECT双180秒窗口/4096 burst与128 pending及原字节预算，batch32epoch/45秒未ACK与2048/account稀疏身份均有界；及时完成/ACK的已分类收件不再因终身累计128/512停止。低未完成gap、未知raw或过量burst仍可失败关闭；正常原run Join后回收已投影副本，保留SMS，不承诺无限磁盘、token续期或跨run exactly-once；自动重连只按可选策略全新start。未知prior-process owner不凭PID/年龄清除；只有持OS锁的新进程可回收打过本机制标记的退出进程运行。
- 默认 locator/全部 Router-NetStream、自动续期、remote principal/device 全局租约及其他媒体组合是后续据证扩展；当前APK静态路径显式server、关闭DirectConnect/SRTP，不据此推断未来版本。用户本轮没有要求实现付费外发。通知不能承诺远端 exactly-once，已发字节不可召回，删除不是 SQLite/WAL/备份物理擦除。
- 真实账号/服务/媒体部署、网络条件、双向音频、通知目的地、有效期/并发和账单仍无验收证据；以后须明确授权及自有有效参数。当前不要求用户贴出秘密或安装新工具。

### 第三方报告与原目标

已只读审阅用户 DOCX 全部 1610 段（含表格），报告文件未改；现有 OfficeCLI 禁用自动 resident。报告描述的 APK 与本地 APK SHA/大小不同，实际七个嵌套 DEX 已重新扫描并验证 SHA-1，不能照搬报告偏移/常量。复用已有Androguard 4.1.4只读扫描本地DEX（未安装JADX），并实时复核当前 IDA 原 `libmtc.so` 的 base 0、SHA-256 与实际 APK 内库相同；已闭合phone-token/未调用refresh helper、压缩SDP、SRTP/direct-connect显式关闭及server选路。HTTP签名/Android TLS安全结论仍不是native协议、授权或服务部署证明；没有运行报告网络示例、复制密钥、写IDB或访问服务。

## 本轮软件交付与后续验收

1. 基础接电话/应答/挂机/双向音频/Trunk、durable短信/查询删除/通知原ACK，以及新取消/批次缺口已有新SHA两Actions整链证据；不是仅codec或单个信令阶段。
2. 公共手册同步专用数据库入口、默认完整流程、新endpoint TLS/Router组件和仍未接入的主链；docs-only编译输入保持与已验 `8bd0439` 相同。文档不代表另一个二进制，后续源码仍须新SHA执行/产物核验，原失败证据不删。
3. DIRECT/batch长期接收、退出回收、重连与raw/压缩SDP已分别取得原protocol/durable/静态及Actions证明，不能互用ACK、清除pending/unknown或扩大媒体授权。凭据自动续期仍缺原服务响应/TTL/活动会话语义；default Locator/NetStream、未来版本SRTP/direct-connect与真实部署/账户/账单仍需独立证据和授权；不把静态helper称为续期或整个实网目标已经完成。

## IMS 注册主线：历史软件基线保持

用户原要求是全局注册兜底和 EC20/EM05-G 机制查漏，**禁止新增运营商/MCC/MNC 专属分支或特例测试**；不修改独立 carrier_Bundles，不把可选诊断扩展冒充注册修复。

| 已验证阶段 | 软件行为 |
| --- | --- |
| R8/R9 `9f1625d` | `auto` / `isim_required` / `usim_derived` 身份策略；只读 QMI UIM refresh，忙时 deny≤20s、变化置 identity_stale、通话排空后重建；不支持退回边界检查 |
| R5～R7 `d373ffa` | 同卡材料检查、同 PDN PCO 重选、XFRM 原 worker binding；Build38999487086/Beta38999487107、双架构和各522相关回归/86矩阵有独立证据 |
| R1～R4 | 原刷新423预算、同卡 Retry-After、初始受保护423重试、注销结果与取消反馈；不重开预算或把临时等待当新成功 |
| N5～N8 | reg-event、SIP TCP/TLS 与 DNS、同源执行诊断、显式承载能力；逐阶段官方产物见 CHANGELOG，不冒称所有固件组合等价 |
| 第一批与全局兜底 | ISIM/多 P-CSCF、响应认证、安全要求继承、身份/Supported 保留、候选去重；原算法/strict 与24候选预算保持 |

EM05-G 全球版只读交叉核查 26 MBN /2905记录/2097 EFS：未发现新的注册机制；差异主要是配置覆盖。非标准 ePDG 需要显式 catalog host，不能凭运营商名字推导；不扩展成设备/NV操作授权。

### 仍未覆盖的实机问题

- 当前源码不等于现场版本；首次注册、同会话自然续期、注销/故障恢复、双流和实际通话/短信/音频仍需设备窗口，缩短租期或重建不能冒称自然续期。
- 中国移动实际 421 的完整安全参数缺失，不能据此新增 MD5、放宽 strict 或复制某卡专属补丁；Cloudflare 的 46011 样本不能冒认中国移动。
- 停旧服务触发 MM 重枚举/AT 超时是独立现场缺口，部署助手的临时处理不是生产代码已经修复。
- 未承诺的固件模式、mTLS/反向 TLS-IPsec、动态参数写入等不升级为本轮必做项。

## 旧部署记录（不是本轮授权或实时观察）

2026-10-07 曾验收 `c174551 /1.1.5` 的 410 原 Globe 配置初始 derived/IPsec/IPv4 注册；保留原配置与程序/前端摘要，自然续期次数当时为 0。首次停旧版 `4bc3f77` 时 MM 对象消失、AT 超时，安装在替换前停止；恢复原服务并单次 ScanDevices 后才获准重试安装。未手工删原账本、重启 MM/基带或覆盖配置。失败/恢复/成功证据在 `.local/evidence/global-register-fallback/deployment*`，仅供复核，禁止自动重放。

独立 catalog `v0.3.1-catalog-v7` 历史发布保留，本轮未重建、裁剪或替换设备库，未发布新程序 Release。旧72份文档保存在 Git 历史及 `.local/evidence/docs-consolidation-20261006/docs-before.zip`。

## 固定入口

[开发与 Actions](DEVELOPER.md) · [未完成计划](DEVELOPMENT_PLAN.md) · [注册与应用边界](IMS_REGISTRATION_POLICY.md) · [历史变更](CHANGELOG.md) · [诊断](IMS_DIAGNOSTICS.md) · [MM生命周期](IMS_MM_EXACT_FAMILY_LEASE_DESIGN.md) · [运营商配置](CARRIER_PROFILES.md) · [文档导航](README.md)
