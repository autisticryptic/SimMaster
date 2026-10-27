# 当前交接

本页是唯一当前交接入口。历史过程见[CHANGELOG](CHANGELOG.md)和[档案索引](archive/README.md)；旧 PID、SIM、地址、脚本或维护窗口不是实时状态，不能直接重放。

## 用户要求与操作边界

1. 本轮恢复原无忧行目标：**基础接电话（应答、挂机、双向音频、Trunk）＋接短信（落库、查询、通知、原回执）**。外呼、发短信仅留接口，计费属于独立无忧行账号，不借 SIM/IMS/CS。
2. 唯一开发目录 `SimAdmin`，只维护 `master`，普通 push `simmaster`；不强推、不清理未知用户文件。编译、Rust/前端测试、编译后的协议 peer 和注册模拟**仅在 GitHub Actions**执行。
3. 本机只做编辑、静态/mock、Git 与只读源码/制品分析。**未部署、未访问设备或真实无忧行服务、未改 NV**；旧设备授权不延续。
4. IDA 仅只读使用用户已打开的原 `libmtc.so`，调用前核对 metadata；不自行启动/加载、修改或保存 IDB。新 JADX/其他工具须先说明版本/用途，由用户手动安装。
5. 保持原 UI 布局；本轮是后端，不做登录/绑定 UI。公共文档只更新原五手册。报告、APK/固件、JSONL、`.local`、私有交接和凭据不提交。

## 当前：OS锁证明的退出进程运行回收已验证

- **最新源码 `1401be0f0add6f1bf89d17152a43c227b406f90f`（实现04c585d） 已推送并核验**：进程启动时（任何账号start与通知worker之前）对`data.db.app-voice-owner.lock`取非阻塞独占flock；之后注册的运行租约标记本进程实例。成功取得锁即证明此前打标记的进程都已退出（它们存活时都持有同一把锁），于是释放其遗留租约：未结束的应用来电写结束时间并发`call.ended`(recovered)事件，`sending`通知改为uncertain并清正文，按原规则回收已分类envelope，短信历史与未知raw保留，并发`app_voice.runtime_recovered`事件。不按PID/运行时长推断；锁被另一存活进程持有时不做任何回收；无标记的旧版本租约仍是`runtime_owner_unconfirmed`；恢复不启动任何会话。
- Build40525320619/Beta40525320712、ARM64/AMD64成功，官方digest ZIP/CRC/逐名各752 Jegotrip＋7registry＋17HTTP＋46Trunk＋31通知＋4SIP＋7DNS/86，446static/mock，67warnings保留，Release skipped；3项新native恢复测试与原回归实际通过。首轮04c585d的Build有一项旧DIRECT测试在GC失败后人工stop收到原生如实的`app_account_disconnected`（同提交Beta及此前三轮均通过）；按既有"同owner再等一次"约定只修夹具（batch同类夹具同步），失败官方ZIP保留，生产代码未改。

## 前检查点：可选有界自动重连已验证

- **最新源码 `e5ef46dab3254016ab1ec2ba2aba316f4d407b1e` 已推送并核验**：可选`config.reconnect`（须同时`cleanup_on_disconnect=true`；`max_attempts`1..16、`initial_delay_ms`1000..600000、`max_delay_ms`≤3600000，指数退避）：只有本manager原run完成实际Join与fence释放后，才对同一已存revision做一次全新start（新nonce/owner/登录/托管），不是token续期或会话恢复。仅disconnected/backend_unavailable/operation_timeout可重试；鉴权/托管拒绝、过期、revision/owner/存储错误不重试。自身HostingAck后重置计数；人工stop/start、replace/delete、关机取消或取代待执行重启；未启用supervisor时报`unsupervised`。`GET A`在策略存在时给出`reconnect_attempt`/`reconnect_state`/`reconnect_error`。跨新run的重投与原生一样是至少一次，不承诺exactly-once。
- Build40517095074/Beta40517094859、ARM64/AMD64成功，官方digest ZIP/CRC/逐名各749 Jegotrip＋7registry＋17HTTP＋46Trunk＋31通知＋4SIP＋7DNS/86，445static/mock，67warnings保留，Release skipped；新增4项真实native（断线后单次全新run、鉴权拒绝不重试、失败次数耗尽、延迟期间stop取消/无supervisor）与1项配置回归实际通过。凭据续期仍为手工重新导入。

## 前检查点：DIRECT 与 batch 长期接收已验证

- **最新源码 `c6f0ba13bf2277910cc33834bcc1ea6fcd63f6b6` 已推送并核验**：DIRECT 使用原双180秒 magic 窗口、正序不复用RPC ID与独立pending/burst/字节限额；原响应实际write后mint replay frontier，当前line/revision/run lease下同事务清退已完成短信副本。batch仅在原受认证回包ACK承载结果的精确epoch后生成私有FIFO retirement，不凭`batch_queued`或外层成功回收。
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
| 独立账号 | `app_account` / `application_data`；私有参数导入、随机本地 line_id、revision CAS、显式 start/stop；不伪造 ICCID/UICC，不恢复 cached ready |
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

- 支持原参数显式导入和可选 matched bootstrap handoff；本地存储是明文私有 `data.db`，不是加密保险箱。参数替换前先完成原 stop/Join，备份与目录权限仍需保护。
- 媒体限原 raw `RseEx.SDP`、RTP/AVP sendrecv、RTCP-mux、匹配编码 RTP passthrough。须 `allow_plain_media=true` 和明确本地 bind IP；信令 TLS 不降级。压缩 SDP、SRTP、ICE/BUNDLE/P2P、转码及所有原生选路模式不假装支持。
- `Client.Host/ArcPort` 可能是逻辑 Router 地址/端口，不是 OS UDP/RTP 目的地。原 raw SDP 分支存在，**不证明服务当前选择了这个分支**。
- 可选断线清理20秒有界，与人工stop/关机同原run串行。history失败保lease、consumer/actor/媒体/Trunk原Join后才finalize；自动清理后GET账号保留终端原因与cleanup_error。运行期calls接口不从已release租约重获owner，已结束数据库历史保留。Trunk FIFO只确认本地driver处理/发送，不是远端ACK；发送不确定不能冒称成功。
- 单连接/账号/队列仍有显式限额。DIRECT双180秒窗口/4096 burst与128 pending及原字节预算，batch32epoch/45秒未ACK与2048/account稀疏身份均有界；及时完成/ACK的已分类收件不再因终身累计128/512停止。低未完成gap、未知raw或过量burst仍可失败关闭；正常原run Join后回收已投影副本，保留SMS，不承诺无限磁盘、token续期或跨run exactly-once；自动重连只按可选策略全新start。未知prior-process owner不凭PID/年龄清除；只有持OS锁的新进程可回收打过本机制标记的退出进程运行。
- 默认 locator/全部 Router-NetStream、自动续期、remote principal/device 全局租约及其他媒体组合是后续扩展；用户本轮没有要求实现付费外发。通知不能承诺远端 exactly-once，已发字节不可召回，删除不是 SQLite/WAL/备份物理擦除。
- 真实账号/服务/媒体部署、网络条件、双向音频、通知目的地、有效期/并发和账单仍无验收证据；以后须明确授权及自有有效参数。当前不要求用户贴出秘密或安装新工具。

### 第三方报告与原目标

已只读审阅用户 DOCX 全部 1610 段（含表格），报告文件未改；现有 OfficeCLI 禁用自动 resident。报告描述的 APK 与本地 APK SHA/大小不同，实际七个嵌套 DEX 已重新扫描并验证 SHA-1，不能照搬报告偏移/常量。当前 IDA 原 `libmtc.so` 的 base 0、SHA-256 与实际 APK 内库相同；只读复核 raw/变换 SDP 分支。HTTP 签名/Android TLS 安全结论不是 native 协议、授权或服务部署证明；没有安装 JADX、运行报告网络示例或复制其密钥。

## 本轮软件交付与后续验收

1. 基础接电话/应答/挂机/双向音频/Trunk、durable短信/查询删除/通知原ACK，以及新取消/批次缺口已有新SHA两Actions整链证据；不是仅codec或单个信令阶段。
2. 公共收尾只更新原五手册，构建输入保持与已验 `1401be0` 相同；文档不代表另一个二进制。后续改源码仍须新SHA执行/产物核验，原失败证据不能删去。
3. DIRECT/batch长期接收已分别取得原protocol与durable回收证明，不能互用ACK或清除pending/unknown。可选重连已由e5ef46d实现；凭据/token续期、其他native/media模式与真实部署/账户/账单仍需独立证据和授权；不把这些增量称为续期或整个实网目标已经完成，不复用失效Goal工具ID或旧设备授权。

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
