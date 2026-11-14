# 当前交接

本页是唯一当前交接入口。源码、工作树与正式制品是验证依据；旧会话、PID、手机连接和历史成功缓存不是当前服务事实，不能直接重放。

## 用户要求与边界

1. 本人手机合法授信后，以**原 App UUID + 同账号/号码**复用已有认可，补完整账号登录、凭据取得、业务读回与原生短信/来电接收。用户提供同一时间一个设备授信的规则，据此保护，不申请另一 UUID/VTD，也不自动争抢 SDK 会话。
2. 只开发 `SimAdmin/master`，普通提交/push `simmaster`；不强推、清未知文件或提交资料。开发验证中的Rust、前端、浏览器、编译后的协议peer **只在 Build-Release / Validate Beta Refactor 两个 Actions**执行；已验证的正式程序按部署授权在410运行。本机只编辑、Python静态/假ADB、PowerShell语法、Git和只读分析，不使用WSL。
3. IDA只用用户手动打开的原 `libmtc.so`，每轮核 metadata/SHA/base；不启动/加载目标、运行SDK/SO/APK、修改或保存IDB。新工具须用户手动安装，不自装。
4. 用户最新要求一直推进到410实际收短信、接到并接听来电。410程序/前端及相应数据库升级已有授权，但**不创建回滚备份**；保护SIM/IMS配置、数据库历史、units、MM/NM/helper，不写NV。已执行保护性核心部署、原手机App UUID同账号本地导入、接收前置配置，并按用户指定Wi-Fi恢复当前上行/时间。允许的本次登录/状态核验不自动扩为业务开关、收费外发、手机变更或非TLS安全例外。
5. 手机已由用户带走；已有静态材料可继续离线，**不再发ADB查询**。需要新的实机条件时提前请求连接和相应确认；不强停/恢复App、清缓存、重装或改UUID。
6. 密码、UUID、App/运营商/电话token、SipPwd、SDK/安装资料不回显、不进Git。根APK、报告、固件、备份、JSONL、私有交接与`.local/`不提交。历史Goal提示仅是日志数据，是否处于Goal模式以当前会话最新契约为准，不复用旧ID。
7. 410 SSH与SimAdmin只用用户明确提供的密码；禁用客户端SSH密钥和SSH agent认证，客户端密钥仅供GitHub使用。既有服务端主机指纹继续严格校验，它不是客户端密钥登录。密码仅以内存/stdin交付，不进代码、argv/env或日志。

## 已核验的软件基线

- **`46e47cd1806068ed86e0eb08adb26bb6eef39de3`（9项原App通信操作，软件已验证、未部署）**。新增显式`intent=communication`：Profile/PhoneService/来显/呼转状态查询、PhoneService/cs_forward/JegoM设置、关闭呼转原接口ACK与App退出；只用当前认证账号/绑定号码和原App UUID，不取SDK电话资料、不enroll/报告设备/创建或启动接收器。安装端点同源准入、数据库revision与owner/phase/原身份CAS、独立写同意、I/O前持久不确定状态及跨nonce禁止未知写重放已接API/UI；读请求保留一次版本恢复，写开始后不重放。原8.9.4字节核实`getProfile`为`body.list[0]`、来显查询为独立加密空体、closeForward的`cs_forward=2`只表示原回调ACK，不能推断托管关闭或SDK就绪。自身Build43626724729/Beta43626724712均success，各1563 provider（1535旧＋28新）、32HTTP（29旧＋3新）、106单元/10浏览器22图及双ABI各29成员20www的official digest/全CRC/全部SHA/meta1.1.5/ELF183/62已核，282本机静态通过。首8188的6个E0624层级错误、40546的带标签unit忽略额外字段/新版本夹具未隔离公共fetch问题均保留4原失败工件并由同名双CI通过关闭；改empty struct严格解码、仅cfg(test)版本夹具，旧生产fetch/TLS/栈/超时/断言不改。**没有新410、手机或Provider/SDK操作；当前真实接收、可信Router、Message/Invite延迟消费者、call/platform通知及其余通信/媒体仍未完成。** 权威`.local/jegotrip/goal-replication-20261114/communication-apis/SOFTWARE_VERIFIED_46e47cd.json`及`.local/jegotrip/verified-native-46e47cd.json`。该软件基线优先于下列旧基线；已装410仍只引用先前df2读回，不是本轮新实时状态。

- **`1d67e35b45dd4b2278657332792cf5ef55dde7d1`（独立AccountAlive运行时，软件已验证、未部署）**。可选`profile.account_alive`仅用本次原bootstrap单一stcp引用和精确批准host/port，缺省不安装、未批准不连接；UID/selector/native token/mainAccount来自同owner/scope实际SDK登录，独立agent/物理scope/RPC与主连接分开。匹配registerAlive后才允许双侧checkAlive正向、返回interval秒数并更新活动时间；原2×interval、6s..600s退避、同socket刷新、去重/过期/撤销/取消/Join及session错误不争抢已接线。自身Build43609509067/Beta43609508998各1535 provider（1504旧＋31新）、29HTTP、98前端单元/8浏览器18图及双ABI各29成员20www全官方digest/CRC/SHA/meta/ELF通过，270本机静态通过。首4aef语法、ac9新DIRECT漏magic/map期望失败均保留并关闭，原主门禁/length-first codec/栈预算不改；同scope拒绝、同socket/cipher刷新、异常/取消Join新回归通过。下载中断按同一official artifact续读副本后全校验，不重跑CI。**本轮未部署/操作410、手机或Provider，也未批准真实非TLS通道；不是整体接收验收。**

- **`5676a6a59e73b8ca51fa4c83519c91b056f91124`（原生内置音频生命周期，软件已验证、未部署）**。显式`call_media.native_lifecycle.session_timer_seconds`与独立Alive分开，仅允许native默认控制及受支持的明确媒体配置；新增内置单音频Accept的SDP/mediaType/keepAlive/SDK.IsNew、单pending KeepAlive原inform RPC、一次性耗时、零周期/漏tick合并/撤销/Join。自身Build43588312755/Beta43588312821各1504 provider（1485旧＋19新）、29HTTP、95前端单元/7浏览器16图及双ABI29成员20www的官方digest/全CRC/全部SHA/meta/ELF通过，262本机静态通过。3个新隔离端到端用例实际走Router/TLS/Common、独立TLS/Mpath、PCMA双向数据、负保活及挂断等待；不是实网。首510的7个新时序夹具用`_`提前Drop RPC，现仅改为`_rpc`持有到断言结束，原全部断言/标准栈/预算/生产守卫保持，双CI同名通过后关闭失败。当前410仍是上一部署df2，未改配置、身份/TLS或再启SDK；完整R01–R14仍未完成。

- **`df2ef9d50e1f395b6a6e65c74707f7c938d24459`（原生默认来电控制，已验证并部署核心）**。显式`profile.native_call_control=true`仅允许原Router/Common＋native接收器，与旧`call_control`数组互斥；旧id+domain保护不变。默认`#CallServerEx`、精确存在性Inform及随后原邀请per-call oid的Accept/Release已接线，不降级endpoint/facet/adapter引用。自身Build43580577473/Beta43580577469首轮均success，各1485 provider（1461旧＋24新）、29HTTP、95前端单元/7浏览器/16图及双ABI官方digest/全CRC/29成员20www/meta/ELF全核；254本机静态通过。已单次核心升级410到本SHA/PID127787，原配置/身份/历史/辅助服务保护通过，不备份、不启用新call_control或猜媒体参数。**完整通话、Accept附加参数/保活、媒体/Alive/通知与真实收件仍未完成。**

- **原生启动分层诊断（818d139软件已验证，失败已关闭）**。仅为原bootstrap/账号连接保留首个有限`phase/stage/code`；错误映射、TLS/精确SAN、Router选择、重试与ready门禁不变，整体超时不猜网络阶段。自身Build43577807064/Beta43577807069各1461 provider/29HTTP、95单元/7浏览器/16图、双ABI完整工件通过。cb6的导入/工厂名编译错误、919两个Actions原同名Common bootstrap栈溢出均保留原日志/ZIP/注解；移除新增async/Result大future嵌套恢复原单层状态机，原测试/标准栈/预算未改，同名回归通过。metadata403已按reset恢复并完成审计，私有`startup-diagnostics/VERIFIED_818d139.json`关闭该失败。诊断随df2部署后已实际定位到bootstrap Router TLS失败，详见下节，不冒称授信/SDK或收件成功。

- **`2b8555acc03319cacd1055f15e9a8ae7ed1ad448`（本地SDK接收准备，已验证并部署）**：实际8.9.4核心登录/注册使用当前电话token/SIP资料，不把App的`cs_forward`或某个UI回调的VoIP status当通用SDK门禁。新增非默认`prepare_local_receiver`，原UUID/同账号、当前已有设备认可、当前电话资料和再次认可后才准备本地账号，呼转只观察，不enroll/同步设备/修改业务；旧呼转复用和native ACK/撤销/Join保护保持。Build43563389936 / Beta43563389938均success，各1446 provider（1418旧+28新）、29HTTP（26旧+3新）、90前端单元/6浏览器/14图及双ABI官方digest/全CRC/29成员20www/meta/ELF已核，本机239静态通过。首a23仅新增UUID冲突测试误配普通认证的可选enroll前置，已仅修夹具、原断言/生产路径/预算不改并关闭失败留证。410当时运行本SHA/PID75130（现已升级df2）；实际SDK首启仍断连，详见下节，完整目标未完成。

- **`865991d1d986f25e060a54272f595b889e1b0c4b`（同次诊断状态保留，已验证并部署）**：`CheckDeviceTrust`保存原来已发出的那一次`getCallStatus`授权码及`localCsForward`，不增加请求/重试、不改变原UUID、HTTP wire或复用门禁。旧记录不回填；复用/诊断形状分别校验，原2KiB上限保持。Build43549614252 / Beta43549614209首轮success，各1418精确provider（1406旧+12新）、26HTTP与原registry/Trunk/通知/DNS/矩阵通过；各82前端单元、4浏览器、12图及双ABI各29成员/20前端文件的官方digest/全CRC/SHA/meta/ELF已核。本机231静态通过，旧断言/工作流/前端/API/依赖与预算保持。410已仅核心升级到本SHA/PID26706，历史/身份/配置/辅助服务保护通过，无回滚备份。证据`.local/jegotrip/diagnostic-status-20261114/`；完整托管仍未完成。

- **`e2279969654c3530f6214b80137b7211253b1acf`（通知可靠性修复）**：已识别但未完整承接的Message/Invite三键通知返回明确false，不因这种语义拒绝关闭整条连接、不产生成功事件；通知worker拥有者等待超时/取消保留uncertain，防止将可能已送达的工作自动重排。Build43389164094 / Beta43389164174均success，各1406精确provider回归（原1386+新20）、26HTTP、原registry/Trunk/通知/DNS/矩阵，以及各82前端单元/4浏览器/12图、自身SHA-run/官方digest/全CRC通过；双ABI各29校验成员/20前端全SHA、meta/ELF已核。225本机静态检查通过。证据`.local/jegotrip/notification-reliability-20261113/`及`.local/jegotrip/verified-native-e227996.json`。本组件不是完整SDK/OEM通知业务关联；目前组件已交付，当前核心版本与接收验收状态见下节。

- **`ed4bc650bb981cd02790c2fa8713b0a79c2e894d`（最新纯编解码前置）**：无网络AccountAlive参数/结果codec，既有运行时、TLS、配置和checkAlive行为逐字保持。Build43368280015 / Beta43368280032均success；各1386精确provider回归（原1370+新16）、26HTTP及原registry/Trunk/通知/DNS/矩阵通过，各82前端单元/4浏览器/12图SHA-run/官方digest/全CRC与两ABI完整包成员/meta/ELF已核。本机219静态检查通过。源码与核验在`.local/jegotrip/alive-codec-20261113/`及`.local/jegotrip/verified-native-ed4bc65.json`；未新增或启用任何通道，不能把这个前置组件称作Alive注册成功。

- **`a1c6d88061808c37b101899c8c9dbc5815b76308`（本次）**：原生未限定本地接收器、分离的DIRECT/batch域规则、NULL接收域与原HostingAck前的有界短信队列，以及会话撤销后的即时媒体停止。Build43358440268 / Beta43358440285均success；每workflow1370精确provider回归（完整保留原1339，新增28接收+3媒体）、26HTTP及原registry/Trunk/通知/DNS/矩阵通过。前端各82精确单元/4浏览器/12图、自身SHA/run、官方digest/全ZIP CRC通过；ARM64/AMD64正式包各29成员/20前端文件、全部SHA、meta完整commit/target/version1.1.5及ELF已核。仅软件子集交付，未执行制品或部署。
- 本次权威证据：`.local/jegotrip/native-hosting-20261113/`的源码读回、失败关闭、前端和包证明及最终交付，另有`.local/jegotrip/verified-native-a1c6d88.json`。文档收尾只改docs，构建输入保持a1c6d88，不冒称另一个新二进制。

- **`38ae8d785fcbe10b6416b1240ad4c2150d811664`**：显式原绑定ClientEx控制子集与会话撤销保护。Build43253503712 / Beta43253503724均success；每workflow1339精确provider回归（原1309+新30）、26HTTP、原registry/Trunk/通知/DNS/矩阵通过，前端82精确单元/4浏览器/12图、自身SHA/run/官方digest/全ZIP CRC已核。ARM64/AMD64正式包各29成员/20前端文件完整SHA、tar/ZIP摘要、meta完整commit/target/version及ELF已核，未执行或部署。本组件不包含独立Alive正向/默认动态接收/完整实网验收。
- 新权威证据：`.local/jegotrip/resume-20261112-1957/`中的交付、源码读回、失败关闭、UI及双架构包证明，以及`.local/jegotrip/verified-native-38ae8d7.json`。公开文档收尾只改docs，构建输入仍与38ae一致，不冒称另一个新二进制。

- **`0c4c1c45de2c92601f25ac6c2e7272d269efe7c7`**：原UUID复用软件、本地原始备份读取、快捷root导出与原生离线积压消费。Build43199766742 / Beta43199766709均success，双架构、正式digest/全ZIP CRC、每workflow1309精确provider回归、26HTTP、79前端单元、4浏览器用例/12图已核。
- 原 `a1ad162` 复用流程不enroll/VTD/旧enrollId/自动换身份。本次 `getCallStatus.authentication_code=1` 与业务读回后才取得凭据；校验原生模板后再次读回。设备sync仅用明确本人手机资料、原UUID和本次accountid；不确定写入不重放。
- `profile.offline_reception` 默认false，必须已有原receiver绑定。signed64初值-1/limit100、本次UID/selector/token取件；原包、支持SMS、通知/事件/cursor同事务提交才继续。未知type2隔离不推进，删除不复活；原行ID、自增水位、索引、容量与无进展保护保持。
- 快捷 `scripts/Export-JegotripTrust.ps1` / `jegotrip_root_trust.py` 已有19假ADB与既有普通ZIP离线验证，App/SDK双读、符号链接/混根/超限拒绝、私有DACL写前复核、不覆盖旧输出，不读取密码/token或写手机。新脚本尚无本轮手机在线实测。
- 权威证据：`.local/jegotrip/resume-20261112-1631/{DELIVERY_0c4c1c4.md,FINAL_STATUS_0c4c1c4.json,verification-readback-0c4c1c4.json}`及`.local/jegotrip/verified-native-0c4c1c4.json`。不能借这些旧绿结果验证下面的新源码。
- 410已从`6f85149`依次升级e227、865、2b，当前正式核心为**df2ef9d/PID127787**。均只更新核心、保护原历史/配置且无备份，不是接收验收。最新`.local/jegotrip/goal-replication-20261114/deploy-native-df2ef9d/`保留部署、保留账号的唯一分层SDK启动/按run_nonce守卫停止及独立读回；旧`deploy-local-2b8555a/`保持，不重放任何终态请求。

## 本次恢复与已验证的 ClientEx 子集

指定目录3份历史JSONL共1485+878+1174条逐行解析、0失败；原用户要求、完整摘要与最后任务已恢复。962个tracked文件冻结在原0c4基线。续接证据目录：`.local/jegotrip/resume-20261112-1957/`。

### 已恢复的原生事实

- 手机与原IDA库SHA相同：`57086cd624d8227dc1b1b64a63d7c8b2258f87b261779b681a9bfd306ad9d2e8`，24598088字节、base0。本次metadata/reloc与原ARM字节再核；31份本轮只读结果及上轮中断后39结果已保存。
- `MessageReceiver`所登记的同一CommonObject先分ClientEx，再分在线/离线消息。对象名、接收kind、DIRECT域策略、已认证batch adapter仍是不同层，不把出站domain抄成所有入站域。
- `checkAlive`请求只有原版本，结果bool+u32+Common footer；原state3来自独立`registerAlive`成功，不能用SDK登录或HostingAck代替。
- `notifyCalls`为原版本+完整64位标记+map。空map只触发原swap调度；非空map进入SDK/OEM通知hooks，当前不假称已实现。
- `notifyDisconnect`为原版本+reason，原实现核对本次ARC token并处理另一设备/同设备/服务器踢线。App/phone token不等价。

### 38ae组件已验证的实现

- `profile.client_ex`明确默认false，旧私有JSON省略false；只复用已有原incoming object/domain，不猜默认ID/域，不申请Alive。
- `rpc_client_ex`精确控制codec，版本/计数/截断/trailing/字节预算保持。无独立Alive注册证明时checkAlive返回false/0；非空通知hook map明确拒绝，不当短信吞掉。
- 原接收绑定可在login前作为私有DATA预武装；仅匹配的原ARC成功login才捕获本次token并激活控制dispatcher，覆盖同批登录/踢线竞态，不提前开放消息或SDK就绪。
- 本次token匹配的撤销在任何回复写入/排队/await前锁存在**原scope**。允许owning actor有界尝试该控制回复，但禁止新RPC/positive事件/readiness；写失败、取消、队列满或句柄关闭都不能把撤销改成可重连的IO错误。
- `SessionRevoked / app_account_session_revoked`独立非重连错误，停止原run、保留真实Join/fence与历史；前端只显示固定中文说明，不反射原reason/token。不自动新登录、SDK强制接管或改变手机业务。
- 空通知hint以一位合并，沿用原poll admission/routing/250ms周期与固定RPC/TLS预算，不续期token/授信，不提前ACK短信。batch控制结果排队不是发送或peer ACK；撤销后的batch结果不主动另发新poll。
- 新30项Rust与3项前端回归已由自身两个Actions实际执行；原1309测试名/断言、旧前端断言与两工作流过滤器/预算/lint均保护。**本机205项静态/备份/假ADB复核通过；完整软件制品核验完成，仍不等于真实授信或收件成功。**
- 首76532bd双Actions编译和两ABI构建通过，但同一原有Common bootstrap回归栈溢出。38ae只改两公开路径：原native入口堆分配大型future，错误包装只持BackendFuture指针；保留原分派字面契约/全部测试/标准栈与超时。该原同名回归在本次两工作流均过，失败官方ZIP/CRC/log/annotations、本机首字面guard/下载部分/包目标triplet误判均保留，未靠提高栈或减断言收尾。
- 只读子审查再次401无结果，不是signoff；主助手直接审查。旧Capstone平台库不能加载，未安装工具，改用IDA原汇编与reloc字节复核。失败记录保留。

## 已取得的本人手机静态材料

先前用户明确授权后只读导出175普通App文件（121 XML、36 files、18 databases）和43外部App scoped文件、8.9.4.0 APK/libmtc；另一台Nexus_4未读取。原Java样本8.9.7.0，库相同；Java版本差异不混用。App UUID、独立ZpandDevice UUID和device_id.xml标识各自保留，前后hash/普通ZIP CRC和私有ACL已核；Realm FIFO仅元数据，不跟随。两份SBA1/v1封装仍未解密，不将历史托管缓存当当前授权。

## 本次已验证的原生未限定接收装配（a1c6d88）

基于已验c531916/38ae，新增安装侧`profile.native_receivers`（缺省None；显式`{}`使用原空deny-list DIRECT域策略），仅支持原Router/Common，不能与旧`incoming/call_incoming`同时配置。源码不把App UUID、DomainName或出站domain当接收域。

- 已核原ClientAdapter的本地激活和`MessageReceiver`/`CallClientEx`注册；DIRECT连接快照域策略与匹配外层的batch adapter分发不同。仅处理精确无facet/adapter/cloud限定的本地对象，未知引用仍拒绝，不猜动态绑定。
- 原native接收DATA在登录前有界预装配，仅匹配本次login后同步激活，覆盖同批首条消息/来电。ClientEx仍按配置启用、本次ARC token撤销仍不可重连，checkAlive不造独立Alive正向。
- native模式`Sip.Request`自动走原短信消费器；确实没有一个固定必填接收domain，私有inbox用NULL表达，不伪造空域、不回填旧行。原精确绑定的非空domain约束保持；native离线游标另有上下文标记，不混用旧cursor。
- 早到native短信只可在原HostingPending的16条/1MiB队列等待原HostingAck，未确认前不落库/通知/ACK，不建立新deadline；拒绝/过期/撤销仍停止。来电控制/媒体仍需要原配置，不从接收器存在推导就绪。
- 新契约与回归覆盖DIRECT/batch缺域区别、原scope/去重/ACK、首包竞态、真实Common/账号事务/离线游标；本机213静态检查通过，自身双Actions各1370精确provider回归与正式制品已核，不借38ae旧绿标完成。
- 撤销还同步原媒体IO门并唤醒逻辑媒体等待器：已持有或迟到的正向通话结果不能恢复媒体；不必等终止控制回复写完或物理EOF，也不伪造远端release确认。3项新增回归与全部旧媒体/来电用例均过。
- 首cdbbe02的Build成功，Beta仅一项新增早到短信测试失败：账户夹具与后端夹具的原selector不同，共用helper错用了后者的精确预期。修为各自核对本来的selector，保留UID/token/selector/逐字节ACK断言、全部旧断言及原预算；同名用例在a1两工作流通过。失败官方ZIP/digest/全CRC/log/annotations保留。核验时匿名GitHub API限流403按返回重置时间等待后通过，失败记录同样保留。
- 新私有证据目录`.local/jegotrip/native-hosting-20261113/`。31份首段及18组后续只读IDA结果保留；历史SDK日志只输出形状，确认独立Alive的`stcp`引用仅有h/p选项，不能把RouterClient.TlsOnly、主连接登录或历史registerAlive成功当成本次独立Alive注册。该路径仍待单独实现与验证，不改写成猜测的ARC/TLS地址。
- 新只读子审查401无结果，主助手直接审查；0手机/Provider/410/SDK执行/IDB写。

## 当前追加：AccountAlive纯编解码（ed4bc65已验证）

仅新增`rpc_alive`无网络编解码前置：type0、单项方法版本列表[0]、原UID/selector、完整64位请求时间与32位间隔，结果bool/Common footer。原590C08与593000使用相同+2328/+2152字段对；不把它们替成App UUID或phone token。原数值包括零/最高位仅作为DATA保留，不读取系统时钟、不计算续保期限、不发注册或重试。

16项新增回归含独立字节夹具、截断/版本/长度/重复footer/错误码/脱敏，以及匹配SDK登录后解码true仍不能把checkAlive变成true的反例。`RawRegisterAliveResult`没有owner、scope或RPC关联品牌，不是注册许可。原`AccountAliveState::Unsupported`与checkAlive false/0、全部连接/配置代码保持；不增加任何TLS或非TLS通道。已由ed4bc65自身两个Actions和正式制品验证（各1386 provider/26HTTP、原前端与双ABI），原1370测试断言逐字保持，未借a1旧绿。

独立stcp/mode1通道的安全边界仍待明确确认；多次Retry不当作授权。这项纯编解码工作不依赖该选择，也不授权真实账号请求/410部署。私有前置证据`.local/jegotrip/alive-protocol-20261113/`，本次源码/核验目录`.local/jegotrip/alive-codec-20261113/`。

## 通知可靠性修复（e227996已验证）

本轮实际修改两个运行路径，不再重复已通过的只读核验：

- 原`Notify.Command=Message`（MsgIdx/Box）及`Invite`（CallId/Caller）的严格三键形状已从原库定位。它们会进入独立的MTC活动通知与业务关联回调，不等于实际SMS/来电。对这些已识别但尚无完整消费者的提示，当前回原格式的明确false，而不是把整个已认证连接当作损坏；不产生成功hint、用户消息/来电、Alive或新就绪。未知命令、额外/缺少键、重复map及畸形流仍拒绝。正常收件与原250ms轮询规则不改变。
- 本地通知worker的拥有者等待超时/取消不能证明远端未收到：改为保留原claim并终结为uncertain，不自动重发可能已送达的内容；构造dispatcher future延迟到分支真正被轮询，并在进入前复核停止与截止。原30秒attempt、24小时TTL、4 worker、队列及显式失败5次/Deferred策略保持，不回填旧历史。

新增20项回归已由自身双Actions实际执行：10项原控制拒绝、8项通知deadline/claim、2项真实Router/TLS/Common连接与消息票据分离。原1386测试正文/断言只增加模块挂载、其余逐字保留，工作流/前端/API/依赖与原预算不变；本机225静态检查再次通过，完整正式制品也已核。非空SDK/OEM通知的实际业务关联仍未完成，不能把false回复当作已处理成功；显式Failed仍按原有限重试策略处理，本修复不声称已重新判定各通知渠道内部错误的确定性。独立Alive、非TLS确认及实网/410边界均保持。权威证据`.local/jegotrip/notification-reliability-20261113/`。

## 410部署与当前实网阻碍

- 已通过密码SSH、原服务端指纹和machine-id确认目标，并通过SSH内loopback HTTP用密码验证SimAdmin管理登录。原LAN地址不可达后，仅采用既有记录中同一服务端密钥的备用地址，没有扫描或建立新主机信任。
- `e227996`正式ARM64包已实际部署：仅替换`simadmin/www/meta`并重启主服务，不运行installer/device-init、不安装资源/units、不更改MM/NM/helper。旧前端未知文件拒绝、保护文件哈希、旧历史列投影与设备标识前后校验通过，辅助服务PID保持，无回滚备份。安装和运行哈希、HTTP200健康已确认，部署子任务已退出。
- 检查发现现有同账号本地标识不是原手机App UUID；已按原复用目标通过版本守卫的管理API导入手机原值，保留旧值历史、账号归属及别名，不生成新UUID，不改手机。SDK的另一个标识没有与App UUID混用。
- 最早一次`check_device_trust`因无外网以`app_login_transport_failed`结束，原失败记录保留，没有重放。用户随后指定Wi-Fi并明确英文密码：当前已连接用户第二个网络的唯一实际广播名称，IPv4默认路由/DNS/NTP同步及约1–2秒时差已验证，原App实际证书与已有固定证书匹配。USB管理路由保留；密码经stdin交付，未写argv/env/文件/日志。只建RAM-only连接，未修改旧保存热点/客户端profiles或启用计费蜂窝；NetworkManager重启后不保证自动恢复该临时连接。
- 网络/时间/证书恢复后，已各发起一次新的只读诊断与`reuse_hosting`，均不enroll、不写业务开关、不启动SDK。只读诊断的`existing_device`返回1、业务布尔为true；但独立复用流程**自己的最新getCallStatus**返回`authentication_code=2`、该设备托管false/localCsForward1，以`app_login_device_untrusted`停止。不能使用前一次诊断或业务布尔代替这个门禁。
- 已只读确认前述请求对应同一App账号、同一原UUID标识行/revision4，配置revision1；静态enrollId不存在、extra_body为空。此拒绝发生在电话凭据/账号创建前，当前native账号、运行租约、进行中登录均为0。用户已明确确认手机授信未过期，并提供更新的临时账号凭据；不能据此把410查询的2改成1，也不能反过来声称手机授信必然过期。
- 11月14日已用最新明确账号/测试密码发起一次新`reuse_hosting`登录：账号认证和号码读回通过，但本次getCallStatus仍auth2/业务false/local1，未取电话凭据、创建native账号或启动SDK。密码只以内存/stdin使用，符号不经shell展开，不回显或另存；旧请求/凭据不重放。
- 用户随后已确认官方App重新登录后可直接开启托管。按此新条件已另做一次独立`official-app-refresh-reuse`，仍auth2/业务false/local1。该请求已终态，不能因Retry再发；后续只读确认同账号/原UUID rev4、配置rev1，native/租约/活动登录均0。用户确认已收到，不再重复要求登录、授信或索要密码。
- 对实际手机8.9.4.0 APK核验原UUID getter、单一device_id普通JSON后，进一步核到同origin、8项配置接口、HTTP匿名参数/签名盐/正文保护和原leaf均与410一致。原`UserCallAssetsEntity`读同名授权/业务字段，页面仍exact auth1后才报告设备；无依据放宽门禁。**8.9.4密码登录模型没有8.9.7的enrollId字段**，不能借旧版本路径擅自补enroll。配置快照来自11月12日，不等于手机当前wire。
- 续接时发现主服务PID已为490且临时上行已丢失（两种默认路由0/NTPfalse）；随后以新状态守卫、原授权网络和最后英文密码**单次恢复RAM Wi-Fi**。IPv4/DNS/NTP/时差0、Unsaved true已核；同boot前后服务状态/保存profiles/6保护文件与USB管理保持，没有重启MM/NM、持久化PSK或重发Provider登录。此前PID519899与上行读回不再当作当前状态。
- 已修复旧诊断仅保存`getCallStatus`业务布尔而丢弃同次授权码/本地业务码的问题；865已完成自身验证与保护性部署，HTTP200、实际运行哈希/PID26706、旧历史列投影（含数据库配置）与6保护文件/辅助服务均核对通过，部署子任务已退出。原复用门禁未变。
- 新版仅执行一次独立设备诊断（当前明确账号/原UUID，不enroll、不取新运营商令牌、不写业务、不取电话凭据、不启动SDK）：**同一次流程中security_verify2、existing_device1，随后getCallStatus仍2/业务false/local1**。读回确认同账号/同身份rev4、配置rev1未变，native/lease/active0，短信39/通话0；旧诊断与失败attempt未覆盖。差异不只是换了登录会话，也不能据此说手机授信过期。
- 原8.9.4还存在`receiveFail→querySafeDevice("")→$2`路径，其**已有设备查询自身auth1**后报告`updateDeviceId`并进入`verSuccess`；不是先把getCallStatus2改成1。这证明不同接口分支，不证明报告必然修复当前拒绝。410尚未执行此设备上下文同步；它是服务端写操作，须明确范围和本人原资料、当次账号关联后才可另行实现/验证，不能擅自强报或放宽SDK门禁。
- 已进一步配置接收前置：通过管理API的版本守卫，将有效安装上下文从`legacy_file/revision0`写入独立数据库配置`revision1`，仅开启已验证的`native_receivers`原空deny-list、`client_ex`及`offline_reception`。HTTP/SDK/TLS/身份/其他配置、旧安装文件及账号历史未变，没有启动接收器或请求运营商服务。页面现在能投影短信/来电对象“已配置”，但原生账号仍为0、接收未激活；来电控制及媒体配置尚缺，不能猜路由/证书/SDP补齐。
- 2b已单次保护性部署，原UUID rev4/安装配置rev1及旧历史保持。新的**唯一一次**`prepare_local_receiver`使用当前明确凭据，通过已有设备认可/电话资料获取，并创建第1个native账号；随后SDK连接为`disconnected/app_account_disconnected`，尚无匹配bootstrap的Alive观察，短信/来电均未ready。不是App门禁继续阻止，更不是把getCallStatus2改成1。
- 已仅对这一次失败运行提交stop，HTTP200、`stopped`、own/global lease0、活动App登录0，保留账号/凭据与失败记录，不删除后重建或重发登录。独立读回当前2b/PID75130、attempt50、native1、SMS39/call0、配置和6保护文件/辅助服务未变。
- 对该账号冻结配置中的两个Router端点做无App/SDK凭据的TLS诊断：TCP与所配CA链通过，但同SNI下开启SAN主机名校验（无CN回退）均返回verify_code62。这是当前身份绑定问题的证据，仍不是已捕获的Rust具体失败阶段；不关闭TLS校验、不从证书猜新host/authority、不盲目重启SDK。首诊断脚本误把versioned private envelope当裸config，错误发生在连接前且已留证，后续按原格式解析。
- **最新df2真实诊断**：以自身验证包完成核心升级后，仅对保留native账号启动一次，不进行App登录/创建账号或修改业务/身份/TLS。实际返回503/app_account_disconnected，原进程的有限观察明确为`bootstrap / router_tls / jegotrip_tls_handshake_failed`。这已证明实际失败层，而不是仅凭旧探测猜测；TLS码本身仍不证明唯一证书根因，正确authority来源尚未解决。
- 已用本次run_nonce摘要守卫只停止该失败运行，HTTP200；随后独立SQL/进程/文件和API只读确认：df2/PID127787、native1/stopped、own/global lease0、App attempt50/active0、SMS39/call0、数据库ok，私有账号配置、原UUID rev4、安装rev1和6保护文件/辅助服务保持；部署子任务已退出。只读续接明确`start_replay_refused=true`，没有第二次启动。当前SDK未登录成功、未收真实Jegotrip短信/电话/媒体。
- 本轮原库进一步核实：Alive构造默认interval180，setAliveEnable启用时夹到60..600、禁用为0；wire时间是epoch毫秒，调度时间则为独立clamped accumulator。Call keepAlive来自另一CALL配置+28（缺配置才回退30秒），不能用Alive间隔代替。详见私有`native-call/TIMER_FACTS.md`；尚未实现这些运行时，不把研究当验证成功。
- **程序和本地准备已部署，不等于完整模块收件或接听成功。** 当前重点是正确Router身份/连接阶段，以及独立Alive、通知业务关联和电话/媒体装配；不能用核心health、配置投影或模拟peer代替真实SMS/接听与清理/时延证据。

私有证据：`.local/jegotrip/goal-replication-20261114/`保留完整R01–R14目标；`startup-diagnostics/`含818自身验证与两轮失败关闭，`native-call/`含df2自身1485回归/全工件及新通话/时钟只读依据（80条累计可归属IDA记录，不是80项独立新发现）；`deploy-native-df2ef9d/`含本次保护部署、保留账号唯一SDK诊断/守卫stop/独立读回。旧2b、865及Wi-Fi/Provider请求保持且不重放。IDA原库metadata/base/原SHA再核，未写/保存IDB，未加载或执行原Android SDK/SO。

## 接下来按顺序

1. 分层诊断、默认CallServerEx、内置音频生命周期和显式独立Alive软件已验证；真实Router身份、其余媒体/通知/通信API及实网验收仍未完成。不得关闭SAN、从证书猜authority、启用未批准非TLS或重放旧SDK启停。继续完整Message/Invite业务关联与第三方通知，以及R11的普通/加密通信操作、原attempt/CAS/不确定写防重放与UI。实际8.9.4新10方法精确请求形状见私有`communication-apis/REQUEST_SHAPES_AND_NEXT.md`：PhoneService query type1/write type2+phoneServerStatus数字，VoipProfile cs_forward字符串，CloseForward mobile/countryCode，JegoM三个字符串；App logout仍须核调用方URL。不要把已恢复字段当作API实现，也不把App业务操作当本地SDK启停。
2. 在原App UUID/同账号上下文中取得本次明确认可和业务读回，补正确接收、通话控制与媒体配置后启动；验收外来SMS落库/原回执、来电接听/双向媒体、断连、stop→logoutSession→Join与持续时延。若需新代码，仍先过自身双Actions再保护性升级；不能用核心health替代终点。
3. 授信TTL/撤销/替换与服务端积压重放规则不可由客户端fixture证明。不用新VTD/UUID、缓存授权、收费外发或设备变更补造成功。**完整用户目标仍未完成。**

## 保持的其他边界

原在线direct/batch收件、严格Sip.Request文本、共享SMS历史/删除/通知、基本来电及限定媒体/Trunk和Join清理软件保持。外呼/发送仍501/outbound_unsupported，不回退SIM/IMS/CS或制造外发历史；TLS不降级，SRTP/ICE/P2P/转码等未知组合不假称支持。原IMS全局注册兜底、同卡/同PDN、安全参数、worker/资源租约和历史保护不变，不加运营商特例或改独立carrier_Bundles。

[手机材料](JEGOTRIP_PHONE_BACKUP_IMPORT.md) · [原生积压](JEGOTRIP_NATIVE_OFFLINE_INBOX.md) · [最小配置](JEGOTRIP_MINIMUM_CONFIGURATION.md) · [原生交付](JEGOTRIP_NATIVE_HOSTING_DELIVERY.md) · [开发与Actions](DEVELOPER.md) · [文档导航](README.md)
