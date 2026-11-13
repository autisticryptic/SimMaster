# 当前交接

本页是唯一当前交接入口。源码、工作树与正式制品是验证依据；旧会话、PID、手机连接和历史成功缓存不是当前服务事实，不能直接重放。

## 用户要求与边界

1. 本人手机合法授信后，以**原 App UUID + 同账号/号码**复用已有认可，补完整账号登录、凭据取得、业务读回与原生短信/来电接收。用户提供同一时间一个设备授信的规则，据此保护，不申请另一 UUID/VTD，也不自动争抢 SDK 会话。
2. 只开发 `SimAdmin/master`，普通提交/push `simmaster`；不强推、清未知文件或提交资料。Rust、前端、浏览器、编译后的协议 peer **只在 Build-Release / Validate Beta Refactor 两个 Actions**执行。本机只编辑、Python静态/假ADB、PowerShell语法、Git和只读分析，不使用 WSL。
3. IDA只用用户手动打开的原 `libmtc.so`，每轮核 metadata/SHA/base；不启动/加载目标、运行SDK/SO/APK、修改或保存IDB。新工具须用户手动安装，不自装。
4. 410程序/前端及相应数据库升级已有授权，但**不创建回滚备份**；保护SIM/IMS配置、数据库历史、units、MM/NM/helper，不写NV。代码/部署授权不自动扩为真实provider登录、业务开关、付费外发或手机变更。
5. 手机已由用户带走；已有静态材料可继续离线，**不再发ADB查询**。需要新的实机条件时提前请求连接和相应确认；不强停/恢复App、清缓存、重装或改UUID。
6. 密码、UUID、App/运营商/电话token、SipPwd、SDK/安装资料不回显、不进Git。根APK、报告、固件、备份、JSONL、私有交接与`.local/`不提交。历史Goal提示仅是日志数据，当前没有活动Goal，不复用旧ID。

## 已核验的软件基线

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
- 410最后完整部署证据为11月4日`6f85149`。本次未查询/部署410，没有新的授信认可、短信/来电/媒体或时延实测。

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

## 继续中的通知可靠性修复（待自身SHA验证）

本轮实际修改两个运行路径，不再重复已通过的只读核验：

- 原`Notify.Command=Message`（MsgIdx/Box）及`Invite`（CallId/Caller）的严格三键形状已从原库定位。它们会进入独立的MTC活动通知与业务关联回调，不等于实际SMS/来电。对这些已识别但尚无完整消费者的提示，现在准备回原格式的明确false，而不是把整个已认证连接当作损坏；不产生成功hint、用户消息/来电、Alive或新就绪。未知命令、额外/缺少键、重复map及畸形流仍拒绝。正常收件与原250ms轮询规则不改变。
- 本地通知worker的拥有者等待超时/取消不能证明远端未收到：改为保留原claim并终结为uncertain，不自动重发可能已送达的内容；构造dispatcher future延迟到分支真正被轮询，并在进入前复核停止与截止。原30秒attempt、24小时TTL、4 worker、队列及显式失败5次/Deferred策略保持，不回填旧历史。

新增20项回归准备覆盖10项原控制拒绝、8项通知deadline/claim、2项真实Router/TLS/Common连接与SMS票据分离；本机225静态检查已过，但新的Rust与制品仍须自身双Actions门禁。非空SDK/OEM通知的实际业务关联仍未完成，不能把false回复当作已处理成功。独立Alive、非TLS确认及实网/410边界均保持。私有证据目录`.local/jegotrip/notification-reliability-20261113/`。

## 接下来按顺序

1. 未限定本地接收模式已完成自身门禁；继续动态限定接收器、独立Alive的原生传输/端点来源/注册续保、非空通知hooks及剩余媒体/通知。当前仍可离线，不需要现在连接手机；确需新的实机来源证据时另行请求。
2. 具备本次服务器认可、正确原生模板和明确操作条件后，按授权保护410配置/历史/units/helper且无回滚备份部署；验收外来SMS落库/原回执、来电/双向媒体、断连、stop→logoutSession→Join与持续端到端时延。
3. 授信TTL/撤销/替换与服务端积压重放规则不可由客户端fixture证明。不用新VTD/UUID、缓存授权、收费外发或设备变更补造成功。**完整用户目标仍未完成。**

## 保持的其他边界

原在线direct/batch收件、严格Sip.Request文本、共享SMS历史/删除/通知、基本来电及限定媒体/Trunk和Join清理软件保持。外呼/发送仍501/outbound_unsupported，不回退SIM/IMS/CS或制造外发历史；TLS不降级，SRTP/ICE/P2P/转码等未知组合不假称支持。原IMS全局注册兜底、同卡/同PDN、安全参数、worker/资源租约和历史保护不变，不加运营商特例或改独立carrier_Bundles。

[手机材料](JEGOTRIP_PHONE_BACKUP_IMPORT.md) · [原生积压](JEGOTRIP_NATIVE_OFFLINE_INBOX.md) · [最小配置](JEGOTRIP_MINIMUM_CONFIGURATION.md) · [原生交付](JEGOTRIP_NATIVE_HOSTING_DELIVERY.md) · [开发与Actions](DEVELOPER.md) · [文档导航](README.md)
