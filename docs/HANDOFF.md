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

### 当前新实现

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

## 接下来按顺序

1. 继续默认/动态MessageReceiver、CallClientEx的真实adapter/域策略，非空通知hooks、独立AccountAlive端点/信任/注册与续保、来电通知/媒体装配。客户端正向Alive状态没有闭合，不能将三控制子集称完整SDK。
2. 具备本次服务器认可、正确原生模板和明确操作条件后，按授权保护410配置/历史/units/helper且无回滚备份部署；验收外来SMS落库/原回执、来电/双向媒体、断连、stop→logoutSession→Join与持续端到端时延。
3. 授信TTL/撤销/替换与服务端积压重放规则不可由客户端fixture证明。不用新VTD/UUID、缓存授权、收费外发或设备变更补造成功。**完整用户目标仍未完成。**

## 保持的其他边界

原在线direct/batch收件、严格Sip.Request文本、共享SMS历史/删除/通知、基本来电及限定媒体/Trunk和Join清理软件保持。外呼/发送仍501/outbound_unsupported，不回退SIM/IMS/CS或制造外发历史；TLS不降级，SRTP/ICE/P2P/转码等未知组合不假称支持。原IMS全局注册兜底、同卡/同PDN、安全参数、worker/资源租约和历史保护不变，不加运营商特例或改独立carrier_Bundles。

[手机材料](JEGOTRIP_PHONE_BACKUP_IMPORT.md) · [原生积压](JEGOTRIP_NATIVE_OFFLINE_INBOX.md) · [最小配置](JEGOTRIP_MINIMUM_CONFIGURATION.md) · [原生交付](JEGOTRIP_NATIVE_HOSTING_DELIVERY.md) · [开发与Actions](DEVELOPER.md) · [文档导航](README.md)
