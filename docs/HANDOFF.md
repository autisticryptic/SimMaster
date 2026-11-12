# 当前交接

本页是唯一当前交接入口。原始过程和正式制品证据保存在 Git 历史及忽略的 `.local/`；旧 commit、PID、手机连接和设备状态不是实时事实，不能直接重放。

## 用户要求与操作边界

1. 本人手机合法授信后，保持**原 App UUID + 同账号/号码**复用已有认可，完成账号登录、自动通话凭据、正常业务读回与原生短信/来电接收。用户提供同一时间仅一个设备授信的规则，本轮据此保护，不申请另一 UUID/VTD。
2. 只开发 `SimAdmin/master`，普通 push `simmaster`；不强推、清未知文件或提交资料。Rust、前端、浏览器及编译后的协议 peer **只在 Build-Release / Validate Beta Refactor 两个 Actions**执行；本机只编辑、Python静态/假ADB、PowerShell语法、Git及只读分析，不使用 WSL。
3. IDA 仅使用用户手动打开的原 `libmtc.so`，每轮核 metadata/SHA/base；不启动/加载目标、执行 SDK/SO/APK、修改或保存 IDB。新工具由用户手动安装，不自装。
4. 410 程序/前端及相应数据库升级已获授权，但**不做回滚备份**；保护 SIM/IMS 配置、数据库历史、units、MM/NM/helper，不写 NV。真实 provider 登录/业务/取号、付费外发和手机变更不是代码或部署授权的自动扩展。
5. 手机已被用户带走；当前静态材料齐全，**不再发 ADB 查询**。真实短信/来电/媒体需要新条件时提前请求连接和相应确认；不强停/恢复 App、清缓存、重装或改 UUID。
6. 账号密码、UUID、App/运营商/电话 token、SipPwd、SDK/安装资料不回显、不进 Git；根 APK、报告、固件、备份、JSONL、私有交接和 `.local/` 保持不提交。

## 当前已核验的软件基线

- `0678f219521f489a76b088aab6e3b9aaf206d6a3`：本地原始备份 UUID-only 读取及两处 React 输入/CAS/生命周期取消；双 Actions 各1282相关 provider回归、79前端单元、4浏览器用例/12图通过，官方摘要/全CRC/源码父链已验。
- `a1ad162` 已完成保持原 UUID 的 `reuse_hosting` / `enable_trusted_hosting` 软件路径：不 enroll/VTD/旧 enrollId/自动换身份，本次 getCallStatus认可码1和业务读回后取得凭据，模板校验后再次读回；可选真实设备 sync 使用原 UUID 和本次 accountid，不确定写入不重放。
- 这些是软件门禁，不是完整实网迁移。410最后有完整部署证据的是11月4日 `6f85149`；本轮没有新查或部署410，不沿用旧 PID 当当前状态。
- 权威基线证据：`.local/jegotrip/resume-20261112/{DELIVERY_0678f21.md,FINAL_STATUS_0678f21.json}`、`.local/jegotrip/verified-native-0678f21.json`；旧复用证据在 `trust-reuse-audit-20261104/`。

## 已取得的本人手机静态材料

- 用户11月12日明确授权后，仅只读导出本人 `com.cmi.jegotrip`：175普通文件（121 XML、36 files、18 databases）及43 App scoped外部文件、当前 APK 与 libmtc；另一台 Nexus_4 未读取。
- 手机 App为8.9.4.0，原Java样本8.9.7.0；手机 libmtc与原IDA样本完全相同，SHA `57086cd624d8227dc1b1b64a63d7c8b2258f87b261779b681a9bfd306ad9d2e8`，base0。
- 原 App UUID 已在私有 JSON；独立 ZpandDevice UUID 和 device_id.xml 的标识不是 App UUID，不互相代替。前后身份配置/归档摘要及普通ZIP全CRC已验，私有导出ACL仅当前Windows用户/SYSTEM；Realm FIFO仅元数据，不跟随。
- 两份用户原 `SBA1/v1:::` 备份仍未解密；实际可读材料来自后来授权的 root 原始导出，不能说两份封装都已解码。
- 证据：`.local/jegotrip/device-readonly-20261112/{PHONE_EXTRACTION_VERIFIED.json,phone-identity-roles.redacted.json,phone-sdk-material-export.redacted.json,phone-installed-apk-copy.redacted.json}`。

## 本轮续接中的新增代码（须自身 SHA 验证）

- `scripts/Export-JegotripTrust.ps1` + `jegotrip_root_trust.py`：单USB手机自动选，多设备须Model/Serial；原App/SDK身份双读、符号链接拒绝、同导出根、限额、私有DACL写前复核、不覆盖旧输出，不取密码/token、不写手机。19假ADB/离线静态测试及真实既有ZIP离线比较通过；PowerShell语法/WhatIf通过，无手机命令。
- 新 `profile.offline_reception` 默认false，旧私有JSON省略false；必须已有原receiver绑定，不从名字猜对象、域、TLS或媒体。
- `rpc_offline / offline_inbox / offline_reception` 接入原ARC/actor/账号消费器：hint不是SMS，原UID/selector/token取件；原包、支持的SMS/通知/事件和64位cursor同事务提交后才取下一页；未知嵌套格式quarantine、不推进，未知kind保留原包。
- 位置是signed64，原初值-1、limit100、独立条目blob与Common footer；不重复内层方法版本。原type2 JSON的顺序读取槽不支持，不猜字段。
- 新连接先从-1核本次native上下文，再用相同账号修订/UID/selector/AccountEntry/receiver的持久cursor。删除SMS不复活；旧行ID、自增水位、direct/batch唯一性与未知原包保留。无进展/回退不自旋、不自动重置，不确定提交不重放。
- 原失败证据保留：标准权限子进程 `Set-Acl` 误需SeSecurityPrivilege，已改只设置新目录Access DACL，不动审计SACL；只读子审查401没有结果，不能当signoff。当前由主助手直接审查。
- 新离线源码契约7项、原native静态契约53项已过；新增Rust/整链用例尚须本次两个 Actions/官方制品，不借0678的旧绿结果。
- 本轮恢复日志/验证文件：`.local/jegotrip/resume-20261112-1631/`。两个历史JSONL共1485+878条逐行解析、0失败，原始用户正文/摘要/最后任务已恢复；当前没有活动Goal，不复用旧Goal ID。

## 接下来按顺序做

1. 完成新代码的静态审查、原测试保护及冻结清单，正常提交/推送；两个Actions编译并执行新/旧回归，失败仅修根因，不删测试/减断言/放宽预算。
2. 核验自身SHA的官方artifact digest、全ZIP CRC、精确测试名、前端报告SHA/run及ARM64/AMD64，更新本页和私有交付记录后才作为软件阶段交付。
3. 继续按实际证据核验 MessageReceiver/ClientEx 真正对象与域、适用 AccountAlive、媒体/通知装配；未知不填假默认。
4. 具备本次服务器认可、正确原生模板和明确操作条件后再按授权部署410，验收外来短信落库/原回执、来电/双向媒体、断连/stop→logoutSession→Join和真实端到端时延。
5. 服务器认可的TTL/撤销/替换规则、积压删除或重放行为不可由客户端fixture证明；保持同本人账号/原UUID，不用新VTD、缓存授权、收费外发或设备变更补造成功。

## 支持子集与历史主线

- 已有原在线direct/batch收件、严格Sip.Request文本、共享SMS历史/删除/通知、基本来电及限定媒体/Trunk、原owner/scope/事务/回执和Join清理软件；厂商离线唤醒、全部ClientEx/Alive/媒体模式与实网及时性不宣称完成。
- 外呼/发送仍501/outbound_unsupported，不产生外发历史或回退SIM/IMS/CS；信令TLS不降级，SRTP/ICE/P2P/转码等未知组合不假装支持。
- 原IMS全局注册兜底、同卡/同PDN安全参数、原worker/资源租约和历史保护保持；不新增运营商/MCC/MNC特例或改独立carrier_Bundles，不把410部署或应用账号计费当SIM授权。
- 旧详细检查点可用 `git show 0678f21:docs/HANDOFF.md` 与[档案索引](archive/README.md)定位，不重新复制过时部署命令。

[手机材料读取](JEGOTRIP_PHONE_BACKUP_IMPORT.md) · [原生离线积压](JEGOTRIP_NATIVE_OFFLINE_INBOX.md) · [最小配置](JEGOTRIP_MINIMUM_CONFIGURATION.md) · [原生分层交付](JEGOTRIP_NATIVE_HOSTING_DELIVERY.md) · [开发与Actions](DEVELOPER.md) · [历史变更](CHANGELOG.md) · [文档导航](README.md)
