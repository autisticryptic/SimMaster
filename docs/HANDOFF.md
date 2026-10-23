# 当前交接

本页是唯一当前交接入口。旧过程见[历史摘要](archive/README.md)；文档中的 PID、SIM 和脚本不是实时状态，不能直接重放维护操作。
## 用户要求与操作边界

1. 修复**全局蜂窝 IMS 注册兜底**，不增加运营商/MCC/MNC 分支或特例测试。
2. 日常只维护 `master`；**编译、Rust 测试和注册模拟仅在 GitHub Actions 执行**。
3. 保留原线路列表、详情和标签布局；文档只更新固定入口，不新增日期流水账。
4. 2026-10-07的部署授权已结束；本轮**未部署、未访问设备或写NV**。用户已手动开启并确认当前 `libmtc.so` 分析ELF，可做只读IDA/静态分析，不修改IDB或扩大设备授权。
5. 私有 eSIM 报告、诊断日志、JSONL、凭据和 `.local` 不提交，不清理未知用户数据。

## 当前：无忧行可信TLS/明文流扩展待Actions（真实runtime/业务未接通）
- 在902f4f5/ba81db3各208模块回归/双架构/官方SHA基线上继续。新增tls_engine：显式imported DER root/原authority/exact DNS或IP SAN、rustls TLS1.2/1.3内存握手，SNI取原配置，无earlydata/resumption/公有root兜底/明文或弱TLS；验证前不发login。tls_plaintext按原RPC字节长度及mode3固定16382 canonical分片自定界，不猜内层BE32，不把read/record当packet；cipher/carrier/plaintext残片有固定deadline/EOF/回退时钟门禁。原ARC一次TLS/transport slot、私有verified packets仅交原offline scope，TLS verified不授SDK/hosting/channels。237 Rust仅源码清单待Actions，376本机静态/mock通过；共享exactSAN逻辑与旧SIP算法/原测试路径保持。两辅助review超时无结论，main独立核对，首次静态illegal HTTPS fixture命中旧guard已改中性scheme fixture留证。不需IDA，本机ELF/FDE够用。仍缺live socket/Router/NetStream/close-cancel-finalconsume actor、ENC provisioning端点、SMS分类落库/providerAPItrunk/callmedia/费用续期并发；providerunsupported/transportimplementedfalse/outboundfalse，无设备/服务/部署/本机Rust。证据`.local/jegotrip/tls-offline-evidence.md`。
- **R8/R9（9f1625d已验证）**：`identity_source` 变为行为开关——`auto`（默认，旧标签不变）/`isim_required`（缺 ISIM 失败关闭）/`usim_derived`（忽略 ISIM，走 USIM AKA）；新增每线路只读 QMI UIM refresh 订阅（0x002A/0x002C/0x0033），忙时 deny≤20s，完成后重读 R5 快照，变化置 identity_stale 并等通话排空再重建；不支持则退回边界检查。两工作流/双架构、各 542 项相关回归/86 场景通过。
对全球版 EM05-G（MDM9607，26 份 MBN / 2,905 条记录 / 2,097 EFS）做了只读交叉核对，目的是排除“EC20 是中国特供、机制可能不全”。结论：**注册机制无新增**，`Service UnAvailable` 14 类、P-CSCF 列表处理、423/Min-Expires/Retry-After、reg-event、non-IPSec 构造均同构；差异只在配置覆盖面（优先级 4 种取值、`IMS_enable` 00/01、新增 22 个媒体类 EFS 项）。两条新线索均属接口/数据层：基带存在真实 ISIM refresh 事件并经 QMI UIM 上报；Verizon/Jio 等使用非标准 ePDG 域名，需显式 catalog host。报告`.local/evidence/ims-em05-review/EM05_GLOBAL_REVIEW.md`，未连接 IDA/设备、未执行固件；后续实现待讨论。

已验证代码 **`d373ffa3b311aa7ceeebcd988a2ec82b1910cf10`**：Build **38999487086**、Beta **38999487107**、ARM64/AMD64均成功；两套官方测试ZIP各 **522相关回归/86场景**，342静态/mock通过。报告`.local/evidence/ims-xfrm-worker-binding/verified-d373ffa.json`。
- R7固定XFRM安装/回滚/旧SA退役/重选/清理的原worker binding；5项真实消息/排队/回包换代回归通过，旧命令不会被重新绑定到新进程。
- R5同卡材料边界检查与R6同PDN PCO重选均保留：旧保护/计数/等待不丢，失败不伪造新端点已注册，切换后重建reg-event/MWI。R5首轮计数缺口已修复，完整历史见CHANGELOG及本地证据。
- 双架构官方程序包已只读校验ZIP/包SHA、meta/ELF/嵌入提交、全文件清单及同run前端与安装资源；没有解包执行、安装或部署。报告`ims-xfrm-worker-binding/packages-verified-d373ffa.json`。前端未改，原49单测/12browser制品复核而非重跑。
- 截至本次核查，本轮已确认的软件缺口与有界离线复核已闭环；仅留实机注册/自然续期/恢复及MM重枚举现场复现。已明确不支持的扩展组合不升级为新必做项，不宣称全部固件模式等价。

R3 **`31d09ffd190a9dd79c15c7e9cf6a8fe58bce8d86`** 已通过 Build **38857492989**、Beta **38857493030** 与 ARM64/AMD64；两套官方ZIP各 **472项相关回归/86场景**，含10项新真实authenticator/worker/gateway-policy回归。初次AKA/ESP收到423交回原共享预算，保留保护通道/接收对并重算nc/proof，不重装SA。首轮缺Security-Server的测试夹具已修复留证，未放宽生产要求；328静态/mock通过。前端未改，复核ca38391的49单测/12browser制品，不冒称重跑。证据`.local/evidence/ims-initial-min-expires/verified-31d09ff.json`。仅继续本项目注册/固件查漏，不改独立catalog、扩诊断或操作设备。
N8 **`22646c8df7f8d50c991d2b4910e49e28af8bee70`** 已通过 Build **38675465347**、Beta **38675465477**、Frontend **38675465467** 与双架构构建；两套官方测试ZIP各 **417项相关回归/86场景**、前端 **46单测/4browser**、17份源码指纹/摘要/原工作台截图已核验，报告`.local/evidence/ims-bearer-capabilities/verified-22646c8.json`。321静态/mock通过。认证需求/driver门禁、原CID P-CSCF地址/域名、保留原IPv6观察、opt-in DHCP信息请求、API脱敏/旧保存兼容均接通；browser首轮断言错误已修复留证。通用PCO策略写入、MM认证变更及DHCP地址分配仍明确unsupported，未部署/设备/IDA。

R2 最终代码 **`c3fb0361853388ebe551f50657a8786a1155a877`** 已通过 [Build-Release 38520139107](https://github.com/autisticryptic/SimMaster/actions/runs/38520139107)、[Validate Beta 38520139336](https://github.com/autisticryptic/SimMaster/actions/runs/38520139336) 与 ARM64/AMD64；Release skipped。两套官方测试 ZIP 各逐名核验 **304 项相关回归**及四矩阵 86 场景，报告 `.local/evidence/ims-wifi-retry/verified-c3fb036.json`，316 项本机静态/mock 通过。前端输入与 N7 相同，旧 43单测/3browser 制品与源码已复核，不冒称本轮新跑 browser。未部署/程序包或实网验收。

- WiFi 初次端点/头型、刷新、注销、profile batch/重连均执行同卡 not-before；端点地址、原 worker、租期和结构化失败与诊断分开。等待只保留到原截止，TCP死流/身份拒绝不因此保留。
- API 按实际发送结果计失败轮次，轮询不增次数；清理保留已耗计数，只消费重建pending动作。换卡须正向确认，控制端口/worker变化不清等待。reg-event独立撤销serial防止晚到刷新重授旧ready。
- 首轮过宽分类误关认证前404，已恢复原分类而保留真正的等待/认证后/423/死流门禁；旧测试未改且两套最终日志均通过，新增真实controller404回归。失败ZIP、annotations与`failure-closeout.json`保留。
## 已验证：R1 刷新预算

R1 代码 **`b6a78884600684b2f4c2b31aa5f2f2b08621f4a2`** 已通过 [Build-Release 38502470702](https://github.com/autisticryptic/SimMaster/actions/runs/38502470702)、[Validate Beta 38502470703](https://github.com/autisticryptic/SimMaster/actions/runs/38502470703) 和双架构构建；Release skipped。两套官方测试 ZIP 各 **257 项相关回归**及四矩阵 86 场景已核验，报告 `.local/evidence/ims-refresh-budget/verified-b6a7888.json`。前端构建输入与 N7 相同，原 43 单测/3browser 证据重新校验但未冒称本轮重跑 browser；未部署。

真实刷新三候选共用 423 协商预算，结构化失败阶段/kind/Retry-After/Min-Expires 决定是否还能换头型；423→格式拒绝、新鉴权后400不重开候选，返回错误保留前序协商依据。正常首包格式兼容、423→2xx、旧 SA/租期/nonce/CSeq、503 等待和 TCP 死流处理保持。7 项新生产回归与 311 项静态/mock 通过。
## 已验证：N7 同源执行诊断

已验证代码 **`ee9ee28a5f5ba49111234a905d6188c90d5ea7cf`**：[Build-Release 38399859012](https://github.com/autisticryptic/SimMaster/actions/runs/38399859012)、[Validate Beta 38399859071](https://github.com/autisticryptic/SimMaster/actions/runs/38399859071)、[Frontend Checks 38399859035](https://github.com/autisticryptic/SimMaster/actions/runs/38399859035) 和 ARM64/AMD64 success，Release skipped。两套官方测试 ZIP 各逐名核验 **250 项相关回归**、四矩阵 86 场景；前端 **43 单测 / 3 原工作台 browser**（0 skip/flaky）、源码指纹与官方 SHA/截图均已核验。报告 `.local/evidence/ims-execution-diagnostics/verified-ee9ee28.json`，未部署、未验收程序包或实机。

- 实际 provider/UICC/端点/请求/响应/门禁观察进入同源 API 与原工作台只读折叠区；声明模板、请求/授予族、报价/协商、收到等待/真实门禁、历史成功/当前 readiness 分开，不输出身份、AID、nonce、密钥或原 SIP。
- epoch/serial、弱 runtime 与阶段原 owner 保护旧异步工作；双接入 SIP 前归属失败可见，IKE 鉴权拒绝不误报普通承载错误；读取不重置预算、不发硬件命令、不改配置/资费。
- 首轮 `ee2b440` 的错误类型 import、Child-SA 配置路径和 Node lint 失败已修正；第二轮 `e5f4387` 的 browser fixture 缺顶层 `line_id` 已按真实 DTO 修正，没有放宽断言。原 annotations/官方失败 ZIP 保留，308 项本机静态/mock 检查通过。

此前 N6 **`6c0c140`** 已验证 worker 内 SIP TCP、四向 TCP/IPsec、TLS1.2/1.3 与精确原 authority DNS/IP SAN、有界 DNS TC→TCP/NAPTR D2T；旧 UDP 和跨 profile 防降级保持。TLS 不绕过 required-IPsec，不冒称私有 CA、mTLS 或新的反向 TLS/IPsec 组合已支持。
## 已验证：reg-event

N5 代码 **`3d54aa87b1cc0ae5f2294991a46ad2ce43984e6a`** 已通过 [Build-Release 38204249021](https://github.com/autisticryptic/SimMaster/actions/runs/38204249021) 与 [Validate Beta 38204249122](https://github.com/autisticryptic/SimMaster/actions/runs/38204249122)、ARM64/AMD64 构建；Publish Release skipped。两套测试 ZIP 官方摘要、各 143 项相关回归和 86 矩阵场景已核验，报告 `.local/evidence/ims-reg-event/verified-3d54aa8.json`。

- 订阅使用成功 REGISTER 的实际 AoR/Contact/instance/reg-id，同一 owner 上处理 Event: reg 的 dialog/CSeq/full/partial/version；无关通知、订阅失败与版本缺口不使注册失效。
- 相关绑定被撤销时先 ACK，再降低本接入 readiness；保留活动对话排空，恢复等待不因同卡 profile/worker/端口重编号而清除，新 SIM 不继承旧等待。MWI/REFER 原路径保持。
- 初轮 Send/XML 编译问题、第二轮测试夹具缺命令消费者均已修复，失败记录保留；未部署或访问设备。
- N6 DNS 子阶段 `0c1fff8` 的 TC→TCP/NAPTR UDP 已随完整 N6 继续验证；当前 SIP TCP/TLS/D2T 状态以本页顶部的 `6c0c140` 为准，不再把 DNS 子阶段限制当作当前能力。
## 已验证：REGISTER 响应认证

已验证代码 **`3509e15474592f001fffde35f302fd3b45719c5a`**，已推送 `simmaster/master`；生产改动来自 `7054111`，后两次提交仅修正新测试 fixture。

- 蜂窝/VoWiFi 的首次 REGISTER、受保护刷新和注销捕获不可变 Digest 请求上下文，发送前校验最终 URI/Authorization/opaque，成功响应先校验 rspauth 与 qop/nc/cnonce，再提交 nextnonce、租期、身份或安全状态。
- 重复/畸形/错 scope/错误证明明确失败，不作为头型/profile/P-CSCF 降级提示；原 nonce-count 不回退，暂存状态回滚。可选 rspauth 缺失兼容，nextnonce-only 不冒充服务器认证证明。未扩大既有算法策略。
- [Build-Release 38197911130](https://github.com/autisticryptic/SimMaster/actions/runs/38197911130)、[Validate Beta 38197911116](https://github.com/autisticryptic/SimMaster/actions/runs/38197911116) 均成功；ARM64/AMD64 和前端检查通过，Publish Release skipped。
- 两套测试 ZIP 各逐名核验 **76 项相关回归**；四矩阵共 86 场景（45 个 fixture 成功、41 个预期拒绝），commit/run/源码/日志摘要一致。报告 `.local/evidence/ims-rspauth/verified-3509e15.json`，用户无需手工下载。
- 官方测试 artifact：`ims-refresh-tests` 11724212981，SHA-256 `f414bfb2ccf051d2ce7754e095ce443c2190e68f99b3150d4fa54864dd8ec4bb`；`beta-refactor-tests` 11723783190，SHA-256 `29ed1c4a9b90258a65774f890ca51dabaa9707043382d6cf170089a92a8258c2`。
- 首轮 `7054111` 编译通过，但新 plain-MD5 fixture 使用非法 nonce；`4240d8e` 修正后，共享/WiFi 新回归通过，5 项蜂窝 fixture 又因缺少合成 worker 的端口预留超时；`3509e15` 补齐现有 mock 适配后全部通过。失败 annotations/ZIP 及修复核验保留，没有放宽生产策略或删除测试。
- 本机只执行 Python 静态/mock 检查与只读产物校验，303 项检查通过；**未部署、未验收新程序包或实网，未发布 Release**。范围为现有 REGISTER 算法、qop=auth/无 qop、单一 Authorization scope，非 REGISTER 和多重鉴权不自动算已支持。

上一阶段 AUTS/stale/跨候选 423 在 `344308c` 已验证；两套 Actions（[38138978950](https://github.com/autisticryptic/SimMaster/actions/runs/38138978950)、[38138978978](https://github.com/autisticryptic/SimMaster/actions/runs/38138978978)）各核验 46 项相关回归和四矩阵。独立 AKA 来源、受保护 stale 不重装旧 ESP、AUTS 不冒充证明、423 尝试级预算均保持；WiFi初次认证后423已由R3在31d09ff重新验证，不沿用此旧结果。

**原报告六项及后续R1～R7均有软件验证；不等于实机或全固件等价。** 新离线汇编已定位P-CSCF当前节点/新列表比较分支；ISIM Refresh锚点确认属于状态文本匹配，不能伪造事件接口。R5是注册边界材料检查，R6是空闲续期时原PCO观察；实时UICC/PCO指示、其他来源动态DNS、未知mode/packet不自动宣称支持。实机首次/自然续期/注销/故障恢复仍需设备窗口，X1独立catalog和X2额外诊断不扩展。见[协议对照表](IMS_REGISTRATION_POLICY.md#ec20原报告注册缺口对照有界静态复核)。

## 第一批：ISIM、P-CSCF 与临时拒绝补全

代码基线 **`5aaf3eab6515aba9f5f5eb43553ea00a1cec3264`**（前一实现提交 `e702cef`），已推送 `simmaster/master`。

- 共享读取 ISIM 的 IMPI/DOMAIN/IMPU/P-CSCF；完整身份与认证 AID 一起选用，合法未配置时保留 USIM 派生；读取错误、损坏、换卡不假装缺省。VoWiFi 的 EAP 仍使用 USIM。
- QMI 按 slot 发现应用，native AT 通过自有通道读取 EF_DIR；PC/SC 每个进程重选完整 AID/文件，并用稳定 reader 名称防止索引重排。卡/slot/owner 检查是分阶段观察，不冒称可原子排除所有物理热插拔竞态。
- P-CSCF 保留端口、UDP 传输和来源，采用有界承载内 DNS 多候选。显式 TCP/TLS/sips 或不支持的 URI 参数明确失败，不再静默降为 UDP/5060。
- 蜂窝结构化 REGISTER 失败驱动端点等待/切换；同 SIM 的 profile、普通重连或 worker 变化不清空 not-before。候选耗尽后的下一批承载也受等待门禁；临时刷新拒绝只保留到旧租期截止，不延长租期或改成明文。
- 原地址族顺序、安全报价/白名单、24 个静态候选预算和原 UI 布局保持；未加入运营商分支、未改现有设备配置或库。

### 已核验的 Actions 与剩余验收

- [Build-Release 38052500633](https://github.com/autisticryptic/SimMaster/actions/runs/38052500633)：success；ARM64/AMD64 构建成功，Publish Release skipped。
- [Validate Beta Refactor 38052500773](https://github.com/autisticryptic/SimMaster/actions/runs/38052500773)：success。两套均包含新增非零过滤器门禁及既有四个注册模拟矩阵。
- 首轮 `e702cef` 编译成功，但旧 family-fallback 断言失败；已按结构化失败契约修正并在上述第二轮通过，不能省略首次失败。
- 本机 303 项 Python 静态/mock 检查通过；没有本机 Rust/前端编译或注册模拟。
- **两份测试 ZIP 已自动下载并与 GitHub 官方摘要核验一致，用户无需手工下载或提供凭据。** 最初 API 匿名下载为 401，随后通过公开 artifact 下载入口取得文件；以官方 SHA-256 而非第三方入口作为内容校验依据。
- 两套日志各逐名确认 **87 项相关回归**；四矩阵各为 24 标准、18 历史、12 安全提示、32 全局兜底场景，共 86 场景（45 个 fixture 成功、41 个预期拒绝）。每个矩阵的 suite ID、commit/run、源码文件指纹及日志 SHA 均核验，不代表实网成功次数。
- 官方测试 artifact 摘要：`ims-refresh-tests`（11670501045）为 `a945fbecc99aec037aaafc64967c1055c35e7d07a04a086e7f4ee1e677b2f3c0`；`beta-refactor-tests`（11670590696）为 `0bb4998139467e4ef668bb7a9a9e808490720431f0cf7d75e709f3ce5d0d8547`。
- 完整核验报告：`.local/evidence/isim-endpoint-retry/verified.json`；同目录保存原始测试 ZIP 和只读校验器 `verify_artifacts.py`，未执行解压内容或本机注册模拟。API/静态检查原始材料另存 `.local/session-recovery/20261010-resume/`。此次下载校验的是测试产物，不是已部署程序包。
- **未部署、未做新版本初始注册/自然续期/业务/热插拔实机验收**。第二批 rspauth、reg-event 和传输/诊断/承载缺口另列于[开发计划](DEVELOPMENT_PLAN.md)。

## 已完成的全局修复（上轮）

源码提交 **`c174551705bc6c0699cd2d7775e56c803e09b92b`**，远端 `autisticryptic/SimMaster`（本地 `simmaster`）。

- 通用候选保留初始空 AKA 身份与 `Supported: sec-agree`，只允许撤回本地主动的非强制声明。
- 本 P-CSCF 已明确要求的安全条件跨静态候选继承；超时探测不冒充服务器的明确要求。
- 裸 421 的 Security-Server 仅作未认证提示，不强制单机制重报价，也不能作为 Security-Verify/SA。
- 已确认需要安全协商时，401/407 缺少可用安全参数且没有现存受保护通道，在 AKA 前停止；无保护 200 也拒绝。
- 继承后按请求语义去重，不因候选名称不同重复消耗 24 次预算；已有身份、算法与保护约束参与判断。
- 首包配置、算法白名单/strict 校验、双栈→IPv6→IPv4、配置来源顺序、认证后止损及原 UI 不改。
- `7bdc59f` 的运营商专用尝试早已由 `0502395` 撤销，未部署；本轮没有恢复它。

实现及边界见[IMS 注册协议](IMS_REGISTRATION_POLICY.md)。

## 上轮 Actions 与产物证据

- [Build-Release 37561979219](https://github.com/autisticryptic/SimMaster/actions/runs/37561979219)：success，Publish Release skipped。
- [Validate Beta Refactor 37561979212](https://github.com/autisticryptic/SimMaster/actions/runs/37561979212)：success。
- 两套日志均逐项核验 **29 项相关 Rust 回归＋86 个场景**：32 个全局兜底、12 个安全提示、24 个标准、18 个既有历史场景。
- 32 个新增全局场景为 15 成功、17 预期拒绝；全部四矩阵为 45 成功、41 预期拒绝。不是运营商实网成功计数。
- 本机仅运行 303 项 Python 静态/mock 检查、编辑和校验下载；没有本机 Rust/前端编译。
- ARM64/AMD64 的官方 artifact digest、包 SHA、ELF、commit 和各 30 个包内文件均核验。
- ARM64 包 SHA-256：`4a2c66cef0695bfb9146fad131aec571d77691e44615af2910a8b6c1771d50d0`。
- 证据：`.local/evidence/global-register-fallback/verified.json`。文档收尾提交不代表另一个二进制。

## 上轮 410 部署与原配置验收

**2026-10-07 03:18:11 UTC 独立收尾，03:25:30 UTC 再次只读复检**，已 pin 的 WLAN SSH `192.168.100.13`：

- 正式程序 **c174551 / 1.1.5 / PID481703**；原 Globe `51502`、`derived_3gpp_lte_51502`。
- 自 **03:13:56 UTC** 保持同一次 **derived / IPsec / IPv4** 注册，最终复检已约 12 分钟。
- `last_error=null`、`reconnect_count=1`；新会话自然续期 **0**，没有缩短租期或冒称续期通过。
- 与首次维护前相比，四个配置表、配置文件、运行 catalog 和 SIM 身份摘要一致；20 个前端文件磁盘/HTTP 摘要通过。
- MM PID538、辅助进程 PID1409、boot 未变；MM 内部 modem 对象最终为 `/Modem/5`，不能说对象编号未变。
- 恢复定时器 active、设备侧守卫 accepted，无新增内核故障；原界面保持。

### 部署期间的失败与恢复，不能省略

首次停旧版 `4bc3f77` 时，清理超时，MM 对象消失，AT1 连续超时后对象被标记 invalid。安装预检在替换前停止，**当时新二进制尚未安装**。
恢复原服务并对现有 MM 单次 ScanDevices 后，原卡重新 IPsec 注册；保持超过 3 分钟，原配置/程序一致，定时器恢复。
随后只重试一次部署：停服后有界等待/单次扫描，重新核验同一 MM 进程、SIM、控制口及空闲资源，才安装新包。
旧同 boot/已停止 owner 的账本保留，由正常启动恢复流程处理，**未手工 Delete profile、删账本或重启 MM/基带**。
这证明升级前的关闭/重枚举路径有独立缺口；部署助手的有界处理不是该生产缺口已被修复。

- 首次失败/原服务恢复：`.local/evidence/global-register-fallback/deployment/`。
- 成功重试/备份路径：`.local/evidence/global-register-fallback/deployment-retry1/`，远端 `/opt/simadmin-staging/global-fallback-c174551705bc-retry1/backup`。
- 汇总：`.local/evidence/global-register-fallback/deployment-final.json`，最终复检 `final-live-check.json`。备份用于明确授权的回滚，不自动恢复旧数据库或账本。

## 剩余验收与历史基线
- **当前原配置的初始注册未发现回归，但不能代替新会话自然续期、通话/音频或中国移动同卡验收。**
- 中国移动实际 421 的完整安全参数仍缺失；不能据此认定缺算法、放宽 strict 或新增 MD5。
- 用户给出的九月成功时间附近为 `7c6cf86 / 09edc03`；Cloudflare 文档明确成功样本是 46011，不能冒认中国移动。
- 已确认 `39b387b` 改全局首包 Require/Proxy-Require，`bdffdef` 区分主动声明与 server-required；旧 generic 丢身份的问题因可达路径变化暴露。
- `98d0e09` 的完整报价/strict 变化是独立线索，本轮没有猜测回退。历史原文检索见[历史摘要](archive/README.md)。
- 停服导致 MM 重枚举/AT 超时需单独调查；当前正常新租约不是旧残留，不得重放旧清理命令。
## 固定入口
- [开发与 Actions](DEVELOPER.md)、[未完成计划](DEVELOPMENT_PLAN.md)、[诊断](IMS_DIAGNOSTICS.md)。
- [MM 生命周期](IMS_MM_EXACT_FAMILY_LEASE_DESIGN.md)、[运营商配置](CARRIER_PROFILES.md)、[文档导航](README.md)。
- 独立 catalog `v0.3.1-catalog-v7` 已发布；本轮未重建、裁剪或替换设备库，未发布新的程序 Release。
- 文档保持 15 份受控主题入口；旧 72 份全文保留在 Git 历史及 `.local/evidence/docs-consolidation-20261006/docs-before.zip`。
