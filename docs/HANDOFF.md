# 当前接手与项目状态

> 更新：2026-10-05。**本文件是唯一当前接手入口**；历史记录在 [archive](archive/README.md)，
> 私有操作材料在本机 `.local/`。不要根据旧文档的“当前版本/下一步”重放操作。

## 最新纠正与部署：原线路界面恢复，Globe已注册（4bc3f77）

用户否定db54abc的替代清单页面，现已撤销该设计。**原列表、详情、选择、7标签、线路控制布局恢复**；
受阻时也使用原API数据结构和真实保存配置，只以内联提示/read_only约束危险操作，不再换页面或清空线路。
主分支 **4bc3f77** 的Build37350592866、Validate37350592895、Frontend37350592884全部通过，
77相关回归/54注册场景及实际浏览器原布局测试通过，截图已核验；没有本机编译或新增分支。

已部署正式4bc3f77/PID9311，18:26:56 UTC开始同一次 **Globe51502 derived/IPsec/IPv4**注册，
18:36:26收尾仍registered、last_error=null、reconnect_count1。API正常线路1/read_only=false，20HTTP资源摘要通过。
当前SIM是重新插回的Globe，不能把此成功归给先前CMCC卡的421问题；CMCC仍待同卡验证。

旧跨boot账本经双absence证明后只归档，未删modem profile。升级中同owner清理延后，停止预检留证后由
原有启动恢复完成；未改写owner/SIM。配置/运行库保持，MM538/boot未变、无新fatal、timer恢复，守卫accepted。
辅助monitor在维护窗口停/恢复，最终PID1409。设备当前走既有pin的WLAN管理连接，USB路由不在。
详情及失败/纠正过程：[原线路界面恢复记录](PASSIVE_LINE_INVENTORY_2026-10-05.md)。

## 历史误判阶段：db54abc只显示替代清单、SIM未就绪（不代表问题已解决）

已按用户要求修复并部署 **db54abc / 1.1.5** 到 **http://192.168.68.1:3000**，主PID34800。
物理MM设备仍在，但启动门禁因旧跨boot账本和device-init监视进程返回pending，原线路API503导致页面空白。
现在独立读取MM缓存显示1条只读物理线路：API200、display_only_lines1、data空、blocked_reason明确，
不创建worker/namespace、不绕过SIM/IMS/网络写入保护。20前端HTTP/磁盘SHA通过，>180秒稳定及独立收尾通过。

**当前IMS没有注册。**用户说刚拔卡；12:53 UTC `AT+CPIN?`仍为SIM failure、MM搜网u2。
MM缓存保留46002/SIM对象不等于卡可读，不能假称已插卡/已注册。需要用户插回测试SIM，再核验资源和注册。
本次没有主动尝试SIP、强删旧账本、重启MM/基带或辅助监视进程；配置/DB复制窗口/运行catalog及旧账本保持。
MM550/:1.18/原boot不变，secondary347仍运行旧映像；主服务守卫accepted、recovery timer恢复active。

主分支Build37309875238、Validate37309875266、Frontend37309875262全部success，76相关回归及54注册场景通过。
双架构官方digest/文件清单核验，未本机编译、未增新分支；用户私有文档和诊断日志未提交。
详情与后续安全恢复条件：[被动线路展示修复](PASSIVE_LINE_INVENTORY_2026-10-05.md)。
**下面旧Globe成功是前次设备状态，不代表当前CMCC卡注册成功。**

## 开发分支收敛：以master统一承载最新已验证代码

用户要求只保留主分支，现已完成：**本地和GitHub仅保留master**。
整合提交 **ae3926e** 使用最终验证快照6ddc751的构建输入，保留9个父提交及全部旧验证历史，纳入技术文档。
主分支Build37276695025、Validate37276695143、Frontend37276695033全部success，
两套68项相关回归/54场景及双架构包验证通过后，原子删除5条临时dev分支，并归档清理8条本地快照引用。
不改Release标签、不操作设备、不提交私有eSIM文档移动或诊断日志；后继仅文档收尾提交，构建输入保持。
维护规则及备份：[单主分支整合记录](BRANCH_CONSOLIDATION_2026-10-05.md)。
整合/CI/分支删除的实际SHA和核验保存在 `.local/evidence/branch-cleanup-20261005/`。
以下候选和部署记录保留原日期事实；“未合并master”是当时的阶段状态，不再作为当前开发方式。

## 最新候选：CMCC 421→403 定向修复6ddc751通过Actions，尚未实网验证

用户追加的诊断已确认 `derived_3gpp_lte_46002`，缺配置后的派生兜底生效，双栈承载获IPv6。
第二P-CSCF标准请求421+Security-Server后，旧逻辑转generic去掉空AKA/安全声明，再收到403 Authentication Failure，
auth_rounds始终0。实际421机制/警告正文缺失，不能归因为SIM鉴权计算错误或未订阅。

新候选 **6ddc751** 在该明确形状下，只允许一次收窄至原首选AES机制的重报价，保留身份和完整声明；
401/407、423、AUTS、续期/注销绑定实际单机制，不允许回到generic或未保护成功。
未认证提示不能引导新增null/MD5重试，普通403/无hint旧路径、地址族/身份域/原预算不变。
新增有界白名单机制/Warning诊断，修补未知扩展值语法及CSeq尾随字段隐私边界。

Build37268498822、Validate37268498818、Frontend37268498810全部success；两套日志68相关回归及
24标准+18历史+12hint场景通过，双架构官方digest/包SHA/源码核验完成。
ARM64包SHA `08962fe848c97154098c8100129b0fd7751c1058ab83dee8e3f3387a2f302895`。
代码在专用dev验证分支，用户HEAD/索引保持；未覆盖Release、未本机编译、未访问或改变410。
**这是一版待同卡实网测试的候选，不是已证明CMCC注册成功。**详情及下载入口：
[CMCC 421候选](CMCC_421_CANDIDATE_2026-10-05.md)。

## 最近部署记录（2026-10-04）：71c970d运行410，Globe注册通过

以下是前次16:40 UTC已核验状态，不代表用户新CMCC日志对应设备的实时状态。

用户明确授权部署后，已将GitHub Actions验证产物 **71c970d / 1.1.5** 部署到
**http://192.168.68.1:3000**，正式PID769258；运行SHA256
`acf44ca39f074ea63c0d9529f933bb1b8e048a5206011913be576ca9e338fe12`与ARM64制品一致。

Globe于 **16:31:57 UTC** 新注册成功，16:40:52独立收尾仍同一registered_at，
**derived / IPsec / IPv4、last_error=null、reconnect_count1、NRestarts0**。
本次requested/owned family4，实际IPv4，默认双栈→IPv6→IPv4未改。
旧3169c7b会话已正常shutdown并完成profile清理；没有遗留recovery标记，也未为部署执行跨owner强制清理。
当前新CID3是正常活跃租约，不要作为旧残留删除。

- 只重启SimAdmin；MM474743、owner:1.511及原boot未变，未重启MM/基带，无新kernel fatal。
- config.yaml、停服复制窗口data.db和运行catalog摘要保持，启动后四配置表指纹保持；没有换运营商数据库。
- 20前端资源磁盘/HTTP摘要通过，设备侧守卫accepted，原recovery timer已恢复active；没有本次遗留hold/drop-in。
- 备份在 `/opt/simadmin-staging/reconciliation-20261004-71c970df703f/backup/`，不要当作孤儿自动删除。
- 新会话自然续期仍0；旧版本的2次续期不计入本次。通话/音频和跨owner故障注入本轮未验收。
- 所有编译仍只在Actions，本机没有编译；用户HEAD/索引保持。

部署证据：`.local/evidence/ims-reconciliation-20261004/deployment/` 内
`package-verified.json`、`deploy-stage.json`、`deploy-install-accept.json`、`final-verified.json`。
详情：[跨MM/SIM恢复补强](IMS_CROSS_OWNER_RECOVERY_2026-10-04.md)。

## 部署前代码验证记录：跨MM/SIM恢复补强71c970d通过Actions

新增独立reconciliation事务：旧资源确实absence时可自动归档；仍present的AT profile因没有唯一归属标签、
存在同值重建歧义，必须先`inspect-stale`取得plan，再显式`reconcile-stale --expected-plan`确认。
不改旧账本owner/SIM、不放宽原identity/release。命令前持久化意图，超时/取消后只读核验、不重发未知写入；
孤立journal、归档中断、从备份恢复旧账本也不能重置命令预算。活动/未知CID或其他worker/namespace/会话均拒绝。
同boot旧账本也进入启动前置门禁；代码不会为了恢复而自动停其他线路、删namespace或重启MM/基带。

最终源码 **71c970d**：Build37214027592、Validate37214027581、Frontend37214027570全success，
两套日志43项定向回归（24新+19既有）及24标准+18历史矩阵通过，ARM64/AMD64产物全摘要验证。
全程无本机编译，用户HEAD/索引保持、只推送独立dev验证分支、Publish skipped。
详情：[跨owner资源恢复补强](IMS_CROSS_OWNER_RECOVERY_2026-10-04.md)。

**截至15:48 UTC尚未部署；后续正式部署见首节，未做实机故障注入。**当时只读检查：410仍为3169c7b/PID732029，
Globe自14:00:45起同一次derived/IPsec注册，last_error=null、reconnect_count1，已自然续期2次，MM474743不变。
此次开发没有停止健康会话，不能把3169c7b的现场成功冒充71c970d的实网恢复测试。

## 最近设备部署：Globe已实网恢复，新精简数据库已公开发布（14:09 UTC收尾）

用户继续要求完成后，已完成受控维护和正式部署。410访问 **http://192.168.68.1:3000**，
当前正式程序为Actions验证快照 **3169c7b / PID732029**，14:00:45 UTC开始同一次
**derived / IPsec / IPv4**注册；14:09:11收尾仍registered、last_error=null、reconnect_count1、NRestarts0。
原双栈→IPv6→IPv4策略未改；本次requested/owned family4、实际IPv4，不是手工固定IPv4。

旧账本属于上一张SIM，未放宽自动跨owner/SIM清理。维护停主服务和recovery timer后，确认无worker、
无bearer/通话，双快照核验CID3 inactive、完整AT/MM定义与原记录一致、CID1/2/EPS/reporting其他项不变；
仅恢复CID3 reporting000并删除精确CID3。原账本先完整备份，随后由已验证程序
`inspect-retired`/`retire-absent`证明profile及旧网络absence后归档。现在的新CID3是正常活跃自有租约，
**不要再次清理**。这是显式维护，不是把不同SIM的旧租约伪装成当前归属。

仅更新正式程序/对应前端/meta；20资源HTTP+磁盘摘要通过。安装窗口config.yaml/data.db/运行catalog
摘要保持，启动后四配置表保持；未换运行数据库。MM474743/boot保持、未重启MM/基带、无新kernel fatal，
守卫accepted、recovery timer已恢复active。自然续期计数仍0，通话/音频/真实来回换卡本轮未验收。
详情与实际部署证据：[换卡与历史回归](IMS_SWITCH_REGRESSION_2026-10-04.md)。

数据库 **[v0.3.1-catalog-v7](https://github.com/autisticryptic/carrier_Bundles/releases/tag/v0.3.1-catalog-v7)**
已发布：target814b057，安全dry-run37206088071/正式发布37206201366成功；20公开文件重新下载核验，
12只读SQLite与Actions产物一致，44.24→19.60MiB（55.70%），小米380静态WFC ready保持。
旧v0.3.0/tag/assets不变；未合并或移动两仓用户HEAD/索引。详见[数据库Actions验证](CATALOG_ACTIONS_2026-10-04.md)。
全程没有本机编译，仍只使用GitHub Actions产物。

## 本轮较早阶段：换卡屏障与历史回归通过Actions，尚未维护现场

**本轮禁止且没有本机编译**。隔离验证快照 `3169c7b` 已在GitHub Actions完成Build37200807750、
Validate37200807900、Frontend37200807803，全success。两套后端日志均核验24标准+18历史场景
（26模拟成功、16预期拒绝），以及15个清理/flock、2个库存预留、2个SHA1别名新增测试。
ARM64/AMD64包的官方digest、SHA清单、ELF、meta均已验证，Publish skipped。

修复清理失败仍继续切卡的问题：lpac之前要求持久资源absence和同一设备flock，持锁跨MM恢复；
新增registry库存预留阻止检查后的新线路接入。无MM admission ticket、多已知modem或slot冲突拒绝。
已兼容SHA1同算法两种拼写，未放宽未报价算法；补上上轮CI漏选测试及非Actions拒绝执行保护。
工作HEAD仍5c378f8、索引未变；只推送专用dev验证分支，未合并master、未部署或发布。

**11:51 UTC只读现场仍未注册**：Globe51502/漫游50212；主PID348115、MM474743、旧98d0e09程序。
旧owner:1.18的CID3 IPV4V6/ims及reporting111真实存在、账本cleaning/abandoned，不能视为absence。
未重启MM/基带、删资源、清预算或重试注册；恢复现场仍需确认受控维护窗口，不以离线通过替代实网恢复。
详情及产物：[换卡与历史回归](IMS_SWITCH_REGRESSION_2026-10-04.md)。

carrier问题确认是发布滞后：旧v0.3.0资源实际10月3日上传，未启用runtime-minimal。
新隔离快照814b057的artifact-only Actions **37201477372已success**，复用固定3个完整库、
重建小米完整OTA和12变体；官方digest及全部12库只读校验通过，总计44.24→19.60MiB、减少55.70%，
小米三版均保留380静态WFC ready。详情：[数据库Actions验证](CATALOG_ACTIONS_2026-10-04.md)。
此处是维护/发布前记录；新tag后续已发布，见首节，旧Release保持。

## 以下为历史：蜂窝IMS多机制协商已部署，实机选中第二机制注册通过

410访问 **http://192.168.68.1:3000**。当前正式本地快照`98d0e09`、PID348115；MM957/boot保持。
实际蜂窝运行时已修复Security-Client仅取首项、cipher_null错误传CK、端口冻结不一致；派生LTE
明确提供SHA1/AES及SHA1/null并严格拒绝未提供机制，认证重复原报价，未新增承载/认证重试循环。

**本次设备真实选择了第二项SHA1/null**：内核两条SA为hmac(sha1)+ecb(cipher_null)，
derived/IPsec注册、last_error=null、reconnect_count1，同次连续180秒以上和收尾通过。
这是IPsec完整性保护，不是AES加密或无安全。新会话续期仍0，不冒用旧版本计数。
配置/DB/unit/默认族策略不变，前端同步新错误提示，20资源HTTP摘要通过；守卫accepted、timer恢复。
未重启MM/基带或清预算，没有物理卸载DB；实际选择派生分支，不等于完全移除库后的验收。

586定向Rust、24SIP场景（14正10负）、262项目Python、98数据库、36前端单测及8E2E通过；
本地私有NET的真实内核对照证实旧null+CK拒绝、新null空密钥/AES+CK接受。
数据库脚本/新证据`2966036`已推送，裁剪规则未扩大；SimAdmin仅本地build快照，用户HEAD/索引保持。
详情：[蜂窝IMS实际补强及部署](CELLULAR_IMS_SECURITY_NEGOTIATION_2026-10-03.md)。

## 派生协商前一轮已部署410（以下cc918f5为历史）

访问 **http://192.168.68.1:3000**。新正式后端本地快照`cc918f5`、PID255232，MM957/boot未变。
补齐同MODP2048组内的已支持SHA512/PRF组合及CHILD提案，强化SA_INIT元数据验证，明确IKE_AUTH
认证/授权拒绝贯穿兜底各层止损；首选/2DH组×2传输路径预算和默认双栈→IPv6→IPv4保持。

实机derived/IPsec注册、last_error=null、reconnect_count1，同次注册连续180秒以上及额外收尾通过。
申请/自有族4，实际IPv6。仅更新后端与metadata，配置/DB/前端/unit/VoWiFi开关未改，当前Pixel库未换。
守卫accepted、recovery timer已恢复，未重启MM/基带或清预算。**新版本续期仍0；真实VoWiFi本轮未验收。**

485定向Rust、261项目Python、98数据库Python、21SIP场景通过。数据库`74bfd6c`已推送并验证：
LTE required按已验证首包能力放行，WiFi required/disabled及未知策略继续保留；12新库189派生回退、
12196其他投影和NR保持。实库实际删除数未增加，不以扩提案为由删掉未覆盖策略。
SimAdmin以本地build-snapshot构建，未移动用户HEAD/索引，未推送SimAdmin或发布Release。
详情：[派生补强与部署](DERIVED_REGISTRATION_HARDENING_2026-10-03.md)。

## 小米完整固件已重建出380条VoWiFi配置，测试后推送7fab7a5（先前数据库记录）

用户授权下载后，完整8.41GiB固定OTA的SHA256与旧源一致，已实际提取Android分区/APK资源，
按DEX核实的覆盖顺序、引用及选择条件编译策略，不从MCFG字符串猜配。
数据库仓库 **7fab7a5** 已在本地测试完成后提交推送并核验远端；97Python测试通过。
941条Profile中380 VoWiFi静态ready（原来为0），644 LTE/655 NR ready；明确禁用560、未知1。
实际SimAdmin逐条加载380 VoWiFi+644 LTE，858项非ready拒绝；WFC有APK证据，ePDG/IKE明确为标准派生，
不是380条实网注册验收。初稿335条已被审查后最终380条替代。

新12库在`../carrier_Bundles/data/variants/2026-10-03-xiaomi-vowifi-final/`，小米三版均保留380 ready，
其full/no-icons约8.76MiB、minimal约2.66MiB（本地跳过图标同步）。集合189派生回退、12196其他投影、NR
保持与所有库完整性/外键/摘要通过。**本轮未操作410、未换运行库、未发布Release**。
详情：[完整小米VoWiFi修复](XIAOMI_VOWIFI_FULL_OTA_2026-10-03.md)。动态更新和全部MVNO/实网行为仍未保证。

## 较早数据库后继：体积减半；小米提取缺陷修正（以下缺输入状态已由上节更新）

数据库仓库 **3442c0b** 已推送，59测试通过。新增运行时精简在不改配置/匹配/NR/8表结构的前提下
移除字段审计证据行（完整版保留证据）；四库37.48→18.03MiB、减少51.9%，不是额外删去同等比例注册配置。
新12库在`../carrier_Bundles/data/variants/2026-10-03-runtime-minimal/`；实际客户端618既有派生回退、
11326其他投影和NR保持。此轮未操作410，运行状态仍以下节设备记录为准。

小米旧库721条均来自APN；修复product首匹配提前结束扫描及无关pb误收集，旧缓存不能绕过新扫描。
**尚未重建出真实VoWiFi条目**：缺完整OTA/Android分区，本地MCFG虽含IWLAN XML但有禁用/紧急域和多版本，
不能直接猜配。固定OTA可下载，约8.41GiB；后续需取得真实资源、重提取并实现有证据的APK/MCFG解析。
详见[运行时精简与小米调查](CATALOG_RUNTIME_MINIMAL_2026-10-03.md)。不要把提取器修正说成小米VoWiFi已验收。

## 重启恢复/EID修复已部署，数据库推送完成（2026-10-03）

详细记录：[重启恢复与eSIM显示修复](IMS_REBOOT_RECOVERY_2026-10-03.md)。410访问
**http://192.168.68.1:3000**；运行本地已验证快照`b3e96c6`，PID156230，MM957和boot保持。
EID首6末4可见、中间掩码，仅复制；自定义容量功能已移除，20前端资源HTTP摘要通过。

旧跨boot账本由新正式程序在同SIM/拓扑、双快照profile及网络absence证明后自动归档，不删除modem profile。
新增启动前置门控防止MM/SIM未就绪时先建namespace阻断后续恢复，并支持安全续接中断的归档。
最终IPsec registered、last_error=null；后继发生过一次数据接口变化并自动重连，**并非始终同一会话**。
最新会话已约56分钟且自然REGISTER续期1次，reconnect_count2。没有本轮MM/基带重启或内核fatal记录。
默认双栈→IPv6→IPv4保持，当前实际IPv6不代表单族配置。

312定向Rust、259项目Python、35安装测试、35前端单测、8浏览器流程、类型/lint/前后端构建通过。
首次收尾观察器缺陷曾触发600秒守卫按deadline停服，失败证据保留；第二窗口完整接受后timer已恢复，
本次抑制drop-in已删除。secondary旧失效命令循环已停止并换canonical unit，但未在线运行初始化。
SimAdmin仅创建`refs/build-snapshots/...`本地构建快照，工作HEAD/用户索引未变，未推送或发布。

数据库独立仓库`c445d53`和报告修复`f9cc1d3`均已推送并验证远端。用户新增的minimal疑问已审查：
四库仅减0.6461%，不等于其余都验证为派生失败。已修复将不存在接入/通过项计入保留的报告口径，
48数据库测试及四源删除决策/7表逐项一致核验通过，**未扩大删库范围，也未再次换设备库**。
详见[精简差距审查](CATALOG_PRUNING_AUDIT_2026-10-03.md)。进一步逐条注册需求建模、拆分其他业务策略、
证据瘦身与空接入/NR覆盖仍需专门实现验证，不能声称“已砍掉一半”。通话/音频本轮未验收。

## 原格式直接裁剪安装记录（以下为10月2日历史，推送与IMS状态已由上节更新）

详细记录见 [派生模拟、数据库直接裁剪与410安装](IMS_SIMULATION_PRUNING_2026-10-02.md)。
19项模拟场景及46项数据库回归通过；最终12库仍是schema v7/contract v1，不含v2格式。
614个LTE IMS及4个VoWiFi接入直接删除（其中2整行），实库618项派生解析/11326其他投影核验通过。

数据库仓库本地提交`c445d5327e721407505643d328595378e8849a2a`已创建，**GitHub认证缺失，push尚未成功**。
410现通过 **http://192.168.68.1:3000** 访问；全部12库在`/opt/simadmin/catalogs/direct-pruned-c445d53/set/`，
当前启用Pixel精简库，API确认usable/sealed，配置备份在同目录`backup/`。本次没有重启任何服务。

**IMS尚未恢复**：安装前后均disabled，主PID527；旧跨boot lease仍指向此前PID93382/runtime active，
导致`mm_ims_profile_runtime_recovery_unresolved`。须严格核验后恢复，不得直接删账本或声称注册成功。
下节的同次IMS注册是早先UI部署当时的验证，已非实时状态。

## 410 前端已热更新（2026-10-02，以下为先前记录）

用户随后授权部署供手动查看，已将当前前端部署到 **http://192.168.100.13:3000**。
前端标识 `ui-20261002T041821Z-531902b18dfe`，源树 SHA256
`531902b18dfe6d96071d982ae1c45b5762090b75e46c536a2ea32c246315e234`。
21 个新前端文件的磁盘/HTTP 摘要逐一通过；保留旧哈希 assets 兼容已打开的标签页。
后端相对已部署 `448c98a` 只有 `cfg(test)` 新增，故保留已验证程序，不冒充新后端构建。
主 PID93382、MM 及 boot 不变，未停止服务；前后两次 IMS 均保持同一 registered_at，
IPsec 注册正常、last_error=null，配置表/文件和当前 catalog 不变。旧页面与 metadata
保存在 `/opt/simadmin/.ui-stage-ui-20261002T041821Z-531902b18dfe/`，不要误作孤儿资源删除。
证据：`.local/evidence/latest-ui-20261002/{build-start,package-ready,deployment}.json`。
本轮仍未提交/推送/发布；数据库未装入运行设备。用户新增的派生模拟与加强精简正在继续。

## 已完成本地代码：安装器、eSIM 内联管理与三种数据库变体

本轮已续接 `2026-10-01T.jsonl` 最后的三项需求。SimAdmin 与相邻 `carrier_Bundles`
仓库均有已验证的工作区变更；以下是热更新前的本地交付摘要，部署状态以上节为准。
完整改动、产物与日志见 [本轮交付记录](INSTALL_ESIM_CATALOG_2026-10-02.md)。

- 新安装包包含同版本安装器/unit/资源及完整 SHA256，停服前预检 ELF、配置和服务设置；
  默认只安装文件，保留用户数据，拒绝覆盖关闭的设备/网络操作权限。详见 [安装指南](INSTALL.md)。
- eSIM 完整管理已整合到动态列数卡片：详情/重命名/切换/删除；两行摘要、EID 只复制、
  剩余存储及真实 eUICC 厂商，保留线路操作锁和写卡保护。
- `../carrier_Bundles/data/variants/2026-10-02/` 已有四来源×三变体，共 12 份 SQLite。
  5973 个 Profile 全保留，精简 12478 个等价默认字段；没有按“标准运营商”标签整行删库。
- 35 安装测试、254 项目 Python、41 数据库 Python、34 前端单测、3 隔离浏览器流程通过；
  前端/E2E 类型检查、完整 lint、构建及 25 Rust catalog 测试通过。
  另实库 23892 次查询等价比较、12 库完整性/外键/行级覆盖/摘要全部通过。
- 标准派生不保证所有 4G/5G/VoWiFi 注册；当前没有 NR/5GC 独立 catalog 投影。
  本轮未进行新的实机 IMS 验收，**下节是 10 月 1 日最近一次已记录的部署状态，不是实时状态**。
- 用户既有 ESIM_IMS_PROFILE_TEST 文档移动保持原样，不要混进本轮提交。

## 最近实机部署记录：448c98a / 默认双栈策略下IMS注册成功（2026-10-01 16:20 UTC）

**410正式运行1.1.5 / `448c98acddaf0f805e9162a6c51ebb5177509b07`，PID93382，16:15:58 UTC实际注册IPsec成功。** 连续五次采样及16:20:25收尾均为同一registered_at、last_error=null、reconnect_count1、NRestarts0，活动通话0。不是维护探针成功。

- **多卡生产策略已恢复并固定为双栈→IPv6→IPv4**：四个新旧线路地址族字段在设备DB中均已消失；API和配置不再允许指定单族。活跃自有profile的`requested_family=4 / owned.family=4`（即MM IPv4v6，**不是IPv4**），实际网络授予IPv6。看到运行状态`bearer_ip_type=ipv6`不代表IPv6-only配置或跳过双栈。
- 新归属证明实机通过`mm_owned_at_disjoint_context_ipv6_prefix`：自有双栈profile、实际IPv6-only grant取得目标自己的2个P-CSCF，首候选完成注册。没有改回单族、没有为注册增加无限承载循环。
- Validate36888761168 / Build36888761291全部success；实际两套下载日志129累计新增+14兼容回归均ok；双架构制品digest/ELF/meta核验，Publish skipped。本地首轮324定向Rust、后继56定向Rust、最终254Python，21前端单测、TS及定向lint/构建通过。前端全量lint本地超时未伪称通过，Actions前端门禁通过。
- 主服务停机复制窗口config.yaml/data.db摘要保持，旧字段由程序迁移而非替换DB；最终其余线路字段指纹一致。MM始终PID472，原boot未变、QMI在位；整个本次维护窗口内核fatal查询无记录。未重启MM/基带、未清预算。
- 两个新动作通过完整原库存双快照证明创建未发生后，仅归档旧creating元数据；没有删除modem profile。当前CID3自有`IPV4V6/ims`、runtime Active/PID93382/Bearer25是正常使用资源，不要删除。
- 临时600秒设备端监护在注册验收后`accepted`退出；recovery timer恢复active，recovery service为原active-exited/MainPID0，secondary保持inactive。无遗留维护hold。
- **风险边界**：本次默认双栈申请并成功注册，没有再发生fatal；未单独强制IPv4试拨，也不等于已证明`dhcp_client_mgr.c:263`固件根因修复或所有卡均验收。OPTIONS仍可超时，注册保持；本轮自然续期、通话/音频未验收。故障保护保留原分类及冷却，不以永久禁族代替恢复。
- ARM64 artifact11175657566，包SHA256`912b96c8ffa1288b4791505d31569bd91d14eaf9ea7d7da5549c58dc4d9edac3`，运行SHA256`a9b25e65b846bfa4288e520a6f7208c3dac271c500148de30a4e9656b9e5e7de`；AMD64 artifact11175802348、包SHA256`91f69d92e48fd5765f703f7410943096d916be3f47f102f30fc3fb521d8babc5`。证据`.local/evidence/default-family-deploy/448c98a/{stage,inspect-uncreated,retire-uncreated,install,observe,finish,closeout}.json`及`.local/evidence/ims-route-completion/448c98a/verified.json`。

## 首次默认策略试验失败及后继修复（以下为历史）

`f5cd715` 已通过两套CI及双架构核验，部署到410后旧四个地址族字段已由新程序迁移。首次默认计划请求IPv4v6，MM实际授予IPv6，未出现新fatal，但P-CSCF失败、未注册。600秒试验监护按时限停止主服务；**主服务和recovery timer当前均停止，MM仍PID472、原boot；不能按下方旧“注册成功”状态交接。** 停止后承载/namespace/profile清理曾全部通过。

后续一次补采观察器误读`owned=null`，提前停止了准备中的服务，留下v2 `creating/owned=null/runtime Profile`记录（旧进程已退出）。不得直接删记录。后继新增显式`inspect-uncreated/retire-uncreated`：同boot/owner/SIM、原完整库存/EPS/reporting双快照相同、无承载/网络、原进程死亡及源记录静置120秒后，凭token仅归档元数据；不放宽自动恢复，不删除modem profile。

已定位P-CSCF策略缺口：自有profile定义为IPV4V6、实际仅IPv6，仍被原`IPV6`定义门槛拒绝。后继允许此实际单族授予，但要求MM无IPv4、目标仅一IPv6行、CGPADDR完整同址（可伴随未授予IPv4占位0.0.0.0）、其他活动上下文全部分离和双快照复核；双族真实授予不在例外范围。56定向Rust、254Python通过，后继CI/部署仍待完成。证据`.local/evidence/default-family-deploy/f5cd715/`，失败记录保留。

## 已完成代码：移除单卡地址族覆盖，恢复多卡默认兜底

用户要求所有线路固定 **双栈→IPv6→IPv4**，删除指定 IPv4/IPv6 的生产配置能力。代码已移除线路字段/API/前端契约，数据库加载事务仅清除新旧四个地址族字段；正式连接不再接收线路地址族参数。维护探针单族选择仍隔离且不落库。另补齐清理包装保留基带故障分类、MM失联之前记录故障冷却，以及防止非手动入口绕过冷却。

本地324项定向Rust回归（含私有D-Bus和真实HTTP）、253项Python、21项前端单测、TypeScript检查及前端构建通过。新旧地址族入口移除测试与存储迁移测试已加入两套CI。完整前端lint一次达到本地执行时限，后续定向契约lint通过；不将超时记成全量lint通过。

**以下 `6c6fcfd / IPv6-only` 记录现已过时，不是当前部署或新策略。** 默认策略版本的正式IMS注册验收失败，见首节；不能把旧IPv6成功当作新默认兜底通过。固件IPv4 fatal根因仍未证明修复；没有将其改为永久禁族或新增无限重试。详细行为与验收边界见 [蜂窝IMS地址族策略](IMS_ADDRESS_FAMILY_POLICY.md)。

## 新卡已部署、注册及自然续期通过：6c6fcfd / IPv6 IPsec（2026-10-01 10:21 UTC）

**当前正式服务为1.1.5 / `6c6fcfd55c1982b52d8816b55dc7a8237dd66eca`，PID4973；新卡20408、驻50212，于09:27:07 UTC实际注册IPv6/IPsec。** 不是只运行维护探针。当前管理`192.168.100.13/wlan0`、MM PID472，保留`--debug --test-quick-suspend-resume`，部署没有再重启MM/基带。

- 两套CI Validate36840587599/Build36840587632全部success；实际日志100累计新增+14兼容均ok，双架构制品完整验证，Publish skipped。包括旧Maxis421→400兼容测试、403提示边界及派生配置回归，未把39b387b早期失败说成通过。
- 唯一有意的配置改动：该线路`cellular_ims_ip_families=[ipv6]`、`cellular_ims_ip_families_auto=false`（之前为全族自动）。用于规避本卡已实证的IPv4连接触发固件fatal，不代表固件根因已修。**这是线路级设置，换回其他卡时也会保留，需要重新评估，不能说是自动随SIM切换的策略。** 全局默认/普通数据/其他配置字段和表不改，配置更新在停止状态下事务校验，复制程序时config.yaml/data.db摘要保持。
- 正式服务首候选`standard_3gpp_conservative`现在完整声明sec-agree，实际下发Security-Server、AKA/IPsec后注册成功；自有IPv6 profile与默认INTERNET上下文经严格分离证明关联PCO。已连续六次采样同registered_at、last_error=null、reconnect_count1、NRestarts0，未触发重试或再次复位。
- **本次维护保护已全部撤销**：main drop-in部署前移除，其余三条和hold marker09:29移除。recovery timer active；发现历史skipped导致oneshot仍inactive后，09:43按原状态恢复recovery service，返回healthy / active-exited / MainPID0，不重启MM或DATA6。secondary保持原inactive；所有临时证据目录保留。
- **自然续期已实机通过**：10:17:08 UTC实际REGISTER refresh成功，API计数1与成功日志一致；09:27:07首次registered_at未变、reconnect_count仍1、NRestarts0，10:21:22仍registered/IPsec/last_error=null，已同会话持续约54分钟。每次lease3600秒、3000秒后续期，protected=true；没有POST retry或重连冒充续期。
- 这张卡OPTIONS保活未回应的事实仍保留，但**实际REGISTER续期已重新收到成功响应**，不能把OPTIONS超时直接当会话断开；也不能因此声称通话/音频均已验收。当前活动通话0、HTTP200、整个新boot无基带fatal；IPv4固件错误仍只是通过IPv6线路设置避开，未根治。
- 本地246 Python及文档/格式检查通过。最终ARM64 artifact11151577416，包SHA256 `8d5aad1c70b4e946e5fecd3d0e4e4f071edb15864d2de7ad49720c38fb52ed3c`；AMD64 artifact11151657090，包SHA256 `6f8f33d9b41178666df877ba21befcad957d5faf60c74db964eac698ed391c13`。活跃v2 profile是正常资源：CID3/IPv6/ims，runtime Active、PID4973、Bearer11/Modem5/wwan0/本线路namespace；不要作为孤儿删除。
- 证据`.local/evidence/new-sim-ims/deploy/`：`6c6fcfd.json`、`6c6fcfd-startup.json`、`6c6fcfd-stability.json`、`6c6fcfd-stability-facts.json`、`6c6fcfd-maintenance-restored.json`、`6c6fcfd-recovery-service-restored.json`、`6c6fcfd-natural-refresh.{json,log}`、`6c6fcfd-closeout{,-facts,-resources}.json`、`6c6fcfd-verified.json`；CI证明`.local/evidence/ims-route-completion/6c6fcfd/verified.json`。正式运行SHA256 `0869403532edffcc87a8896b2c0ba4148a4bf5b6ead39063d488e77f2a5db489`、前端MD5 `01505b0195870511cc8428e1d730b53c`均匹配制品。

## 新卡逐步诊断：控制口恢复、旧租约结案和IPv6归属（以下为历史）

用户明确允许测试设备操作后，**只执行一次整机重启**，未直接写remoteproc state。当前管理`192.168.100.13/wlan0`、MM PID472，主服务/beta8/secondary仍停止，尚未注册新卡。

- 为防开机自动重放，四个相关unit加了本次专属`zz-simadmin-new-sim-maintenance.conf`，ConditionPathExists指向`/var/lib/simadmin/new-sim-maintenance/hold`。**该维护hold仍在，收尾必须只移除本次drop-in/marker并按记录恢复；不能忘记导致之后不能自启。** 原enable状态未改；配置/DB/安装程序哈希重启前后相同，旧profile元数据未删。/run预算元数据已检查，本次原目录没有预算文件（0项）。
- 新boot `1b7022ed-0f3d-458a-a524-963fdf226700`，QMI控制口与Modem/0恢复，后来自动恢复50212漫游驻网/attached。MM加载新卡HPLMN20408；没有再触发IPv4或基带复位。证据`post-reboot-connection.json`、`post-reboot-sim-and-budgets.json`、`post-reboot-radio-state.json`。
- **cc2fcc1安全结案已实机通过**：inspect-retired双快照证明原自有ID4在当前MM+AT均不存在、reporting000、旧owner/网络/namespace无残留；匹配token后只归档原v2记录为`retired/absent-5df955da…receipt`，未删除任何modem profile，当前原始库存2项。证据`retirement-{inspect-retired,retire-absent}.json`。
- **两个明确有界IPv6窗口均建承载成功、未发送REGISTER**：`ipv6-probe`与`ipv6-association`自动选空闲CID3，实际IPv6/64、MTU1280，失败`context_address_unassociated`，释放后原2项profile/EPS/reporting核验恢复。不是重试IPv4，也未更改生产族顺序。
- 第二窗口原MM owner只读快照证明：bearer profile-id3/APNims/typeIPv6，MM与AT的IPv6地址共享同/64但IID不同，CGPADDR3等于CGCONTRDP3；CGACT1、3均active。CID1默认APN定义为空、实际协商INTERNET，前缀与IMS完全不同；CID3行实际带2个IPv6 P-CSCF。**原sole-active保护因默认EPS也active而误拒绝目标自己的PCO**。这不是缺少PCO，也不是420或AKA失败。
- **IPv6 P-CSCF修复候选 `547c15753ca213fc13553eb885beca53974a7d73` 已通过CI/实机归属验证**：Validate36816098925 / Build36816098902全success，两份下载日志95累计新增+11兼容回归均ok、双架构制品核验。严格CGPADDR完整地址、所有其他active context逐一排除前缀/EBI/APN歧义、双快照和最终MM绑定，普通pin/exact/sole行为不变，不取AT作为主机地址。
- **新失败已到认证阶段**：`ipv6-proof-547c157`与`ipv6-auth-metadata`有界窗口均通过`mm_owned_at_disjoint_context_ipv6_prefix`取得目标CID3的2个P-CSCF并装路由，首401没有Security-Server；USIM返回可计算的AKA后发出CSeq2/3，最终仍401/auth_rounds2，未注册。每次均回收承载及临时profile，原2项/EPS/reporting恢复，未再次IPv4或复位。
- 第一次被动auth元数据脚本未展开WWW-Authenticate折行，**响应侧“nonce/algorithm为空”是观察器缺陷，不是网络事实**。生产SIP解析早已支持折行；出向Auth可见40字节nonce、不同RAND/AUTN、qopauth、nc1、32hex response、无AUTS、用户名/realm/URI与SIM派生相符。UICC仅USIM，无ISIM。不能据此改RES编码、截短nonce、切身份或增加认证轮数。
- **安全协商对照成功**：`0ae80aebb4f972b372ebdad20622c90ee305235b` Validate36828711573/Build36828711574全success，97累计新增+11兼容及双架构制品核验。`ipv6-required-0ae80ae`显式`probe-required`仅一P-CSCF/一候选、首包完整Require/Proxy-Require；实际收到Security-Server，AKA材料长度RES8/CK16/IK16，07:29:41 UTC注册IPsec成功，随后注销confirmed、承载及profile回收通过，原2项库存/EPS/reporting恢复。没有增加认证轮数，未使用IPv4。
- 此轮旁路auth观察器退出1，**不能说抓包通过**；结论来自候选程序受保护会话报告与挑战/成功元数据。之前观察器折行缺陷已修代码但没有因此改生产Digest。证据`ipv6-required-0ae80ae/{probe-required,release,inspect-after}.json`。
- 正在将已验证行为接入标准派生LTE的首候选：Security-Client既已存在，则首包直接声明Require/Proxy-Require sec-agree；维持`sec_agree_mode=auto`及原generic候选/原候选预算，不因认证失败再自动尝试其他形状，WLAN/catalog不改。这是首包配置修复，不是把401归类成可重试。
- 生产首包配置初版39b387b的CI发现两项旧兼容回归失败：本地主动声明被误标为server-required会触发不该有的400/403动态身份提示。已在**bdffdef73593ed99550e0fcf1db16f9e43dd8c07**分离声明/服务端要求标记，并将legacy测试fixture明确保持旧字段；Validate36836669433/Build36836669460、98累计新增+14兼容及双架构完整核验通过，不掩盖早期失败。
- bdffdef首次部署在**上传/覆盖/配置修改之前**被预检拒绝：实际MM启动参数含`--debug --test-quick-suspend-resume`，旧startup helper会因drop-in文本不等而覆盖并重启MM。后继 **6c6fcfd55c1982b52d8816b55dc7a8237dd66eca** 增加实际MainPID/exe/NUL argv只读检测：已运行debug则保留原平台参数/owner，不改drop-in、不重启。Validate36840587599/Build36840587632运行中，尚未宣称后继CI/部署通过。
- 后续正式部署需为**当前新卡线路明确设置IPv6-only**，避开实证的IPv4固件崩溃；不修改全局默认族顺序/普通数据/PDP。私有脚本`.local/active/lan/deploy_new_sim_candidate.py`只改该线路族数组/auto字段（全其他表/字段指纹核验），复制程序前后保留配置DB，检查实际SIM/MM/无通话/主服务停止，并按阶段移除本次维护保护。**尚未改线路配置或启动正式服务，hold仍在**。自然续期/正式主服务注册仍待最终候选。
- 所有本轮证据仍在`.local/evidence/new-sim-ims/`；新代码未覆盖正式48e269c。本卡IPv4引发固件fatal的历史事实保持，不以本次重启称其已修复。手机蜂窝IMS及同驻网对照未获进一步信息。

## 新卡初始排查：控制口缺失与旧租约阻断（以下为历史）

**用户已停止主服务并手工运行beta8测试另一张卡；手机能注册，但beta8与本项目均失败。以下旧卡51502的6次续期不代表这张新卡已通过。** 本轮到目前仅只读，未启动任何程序、重启MM/基带、写PDP或清记录。

- 当前主服务/beta8/secondary均无运行进程，主服务inactive；MM已变成PID226233（非本轮操作）。管理usb0，数据库正常，无启用任务/活动通话记录。
- **本项目较早的新卡失败**：日志识别`derived_3gpp_lte_20408`、服务网50212，随后返回`mm_ims_profile_runtime_owner_changed`，未进入新卡SIP。旧v2 profile记录仍为ID4/IP/ims、old owner :1.25、Modem77、PID119010、runtime Cleaning/abandoned，保留bearer网络镜像。当前无/run bearer记录、无namespace，但这些尚不足以证明modem里ID4不存在；未删旧记录解锁。
- **当前更底层阻断**：`/dev/wwan0qmi0`和sysfs QMI port均不存在，RPMSG DATA5通道不在已公告列表，仅DATA1/DATA4 AT口有驱动；MM `No modems were found`，日志明确`at least a QMI port is required`。不能在该现场开展SIP/AKA诊断，也不能伪称只是420。
- **固件崩溃时序已核对**：beta8测试时间段MM实际请求IMS IPv6 Bearer1/4/7/10，获得IPv6地址/MTU1280，无DNS字段，保持约10–11秒、TX48/RX96；不能把这当IMS注册成功。之后每个IPv4 Bearer2/5/8/11 Connect后0.368–0.396秒均出现内核`dhcp_client_mgr.c:263` fatal，remoteproc自动恢复共4次。最后控制口再次消失而DATA5未重新出现。
- 这证明存在重复的IPv4连接/基带fatal紧邻事件，但没有固件源码根因，也没有beta8终端完整SIP日志，不断言手机/设备相同网络或beta8唯一原因。不可继续盲重试IPv4或擅自把全局族顺序改成IPv6-only。
- 已向用户询问一次受控基带复位，用户随后回复继续。**复位前SSH连接TimeoutError，尚未发任何复位/停服指令**；同时核对`baseband_faults.rs`及`QCM410_BAM_DMUX_MODEM_CRASH.md`发现直接remoteproc stop有整机重启风险，而QMI复位入口已缺失，因此没有绕过保护去写sysfs。02:04 UTC最后可达只读快照仍无QMI、无modem、主服务停止；设备恢复连接后先核实是否用户已自行重启。手机成功是否为关闭Wi-Fi的蜂窝IMS且同驻网仍待确认。
- 项目侧已编写显式`inspect-retired / retire-absent`维护候选：仅原owner已不存在、旧自有ID在当前MM+AT均缺失、reporting已复原、旧网络/namespace无残留及完整双快照一致时，凭token归档原元数据。绝不删除modem profile或把同号不同定义当缺失，不自动接进重试。**尚待CI/实机验证；设备QMI未恢复时连absence证明都不能做。**
- 用户新提供`192.168.100.13`后，已沿原SSH主机pin重连；仍同一boot/MM226233、主服务/beta8停止、无QMI/无modem，管理改为wlan0。Windows当前没有RNDIS网卡，旧192.168.68.1走WLAN默认网关，所以之前超时不是代码重启了设备。已安全更新仓库外目标地址，凭据未回显。
- 进一步只读确认modem remoteproc为running、bam-dmux runtime_status=suspended，但DATA5/QMI仍未恢复；不是已证实的runtime-PM error锁存。正常QMI reset无法使用，又不能直接写有整机重启风险的remoteproc state，已向用户提出**受控整机重启（先暂时禁用自动注册/initializer/recovery）**的维护确认，尚未执行。
- 候选 **`cc2fcc189ee168e862e14affff4da4936931962c`** 已推送：239 Python、定向格式/diff通过；Validate36806916655 / Build36806916679全success，两套实际日志89累计新增+11兼容均ok，双架构制品已核验。只增加显式absence证明/元数据归档入口，不改自动注册回退；尚未部署或实机结案。证明`.local/evidence/ims-route-completion/cc2fcc1/verified.json`。
- 证据`.local/evidence/new-sim-ims/`：`findings.json`、`initial-readonly.json`（最新LAN重连快照）、`qmi-topology.json`、`kernel-timeline.log`、`mm-attempt-summary.log`、`mm-request-metadata.log`、`mm-ip-metadata.log`、`lan-reset-precheck.json`。一次宽时间范围MM日志超过输出上限而中断，之后用有界筛选重读完成；未因日志读取失败重放设备操作。

## 最新只读验收：自然续期已成功6次（2026-10-01 00:43 UTC）

用户明确表示注销问题可跳过，**不再为注销返回500打断健康注册或继续主动测试**；这不是把旧rejected结果改成成功。

- 仍运行正式`48e269c / PID119010`，程序SHA与已验证制品匹配，主服务NRestarts=0；MM仍PID1028，secondary inactive（其重启计数为历史累计，不代表正在重启），timer active。
- 当前`registered / ipsec / derived_3gpp_lte_51502`，last_error=null；首次注册时间仍为`2026-09-30 19:07:23 UTC`，reconnect_count仍1，已保持同一注册约5小时36分钟，不是掉线重拨后冒充续期。
- API `register_refresh_count=6`，与六条实际REGISTER refresh成功日志逐项一致：UTC 19:57、20:47、21:37、22:27、23:17、次日00:07。最近一次`2026-10-01 00:07:33 UTC`（北京时间08:07），每次网络有效期3600秒、3000秒后续期，protected=true。
- 第六次还记录了`challenged refresh security association committed`，说明包含重新挑战认证和安全关联更新的续期也成功，并非只有无挑战续期。当前收发时间继续推进、活动通话0。
- 本轮只有SSH/API/日志只读核验，没有POST retry、注销、重启、修改配置或部署。证据`.local/evidence/ims-route-completion/48e269c/refresh-check/{runtime.json,journal.log,verified.json}`。
- 此结论覆盖当前SIM的正常自然续期；不扩大为通话中续期、长通话、真实换卡/故障注入或普通数据共存验收。

## 当前已部署：正式主服务 IMS IPsec 注册成功（2026-09-30 19:22 UTC核验）

**已按用户新授权完成生产注册修复与部署：正式程序为 `1.1.5 / 48e269ca8b536ef7ba82ed98b58d3623540f0547`，PID119010，主服务运行中，不是维护探针。** 本节优先于下方旧安装cf13a66、主服务停止或“候选未部署”的历史记录。

- **19:07:23 UTC正式注册成功**，实际`derived_3gpp_lte_51502 / ipsec / wwan0 / ipv4`。19:11–19:14五次独立只读采样均保持registered、同一个registered_at、last_error=null；收发时间继续推进、活动通话0。首次连接计数reconnect_count=1未增加，服务NRestarts=0。与最初注册时间相隔超过7分钟，不是单次瞬时快照。**19:22:34 UTC收尾复核仍为同一次IPsec注册、PID119010、Web HTTP200、last_error=null，已超过15分钟**；证明`production-closeout.json`，自然续期计数仍0。
- **生产profile生命周期已实际启用**：v2账本、`runtime.phase=active / abandoned=false / process_id=119010`，自有动态CID4为IPv4/ims，关联原MM owner和Bearer/155、Modem/77、实际wwan0/本线路namespace。**活跃profile与bearer receipt是正常在用资源，不要删除或按孤儿记录处理。**
- 原地址族顺序未变：本轮双栈准备被校验拒绝（`mm_ims_profile_lease_unverified`，未断言其唯一根因），IPv6连接收到GGSN拒绝，随后按既有流程新建exact-family IPv4 profile并成功。没有手工固定IPv4、覆盖CID1/2/3，或增加SIP超时后的承载循环。
- **部署已完整核验**：运行SHA256 `38558365459bc01285b297ebcdf7c899e0d0a16ff9481a0b1d15f0dad91f9c90`、meta48e269c和前端MD5 `01505b0195870511cc8428e1d730b53c`匹配ARM64制品。复制窗口config.yaml/data.db哈希相同，没有重建DB/建备份/清预算；服务启动后的正常运行写库不等同于覆盖原DB。MM仍PID1028，未重启MM/基带；管理仍usb0，recovery timer已恢复active。
- **遗留secondary服务入口已修正**：停掉旧`secondary-qmi-init`无效重启循环，安装包内canonical `device-init` unit；目前保持inactive，未在线执行硬件初始化。主线路普通数据仍关闭，其配置未变；没有借此重启MM或改变USB。
- 最终Validate **36760292016** / Build **36760292160**全success，实际下载日志核验 **83累计新增回归+11兼容/更新回归**均ok，双架构制品digest/meta/ELF/程序/前端校验通过。Linux235 Python与定向格式/diff通过。16bf1f9虽然workflow为绿色，逐名核验发现Validate漏跑新增423注销回归；48e269c补齐门禁后重新验证，未拿旧包替代。
  ARM64 artifact11118397115、包SHA256 `5785843b997b1a8c2ba37b1bd51b02df03c433913f4c342e62f19fb63cdab361`；AMD64 artifact11119515020、包SHA256 `7fc4a2b7852f81dd99a474f6a692b57f2bcb44694e6a53c7397b3c4a38535f61`。Publish skipped，旧Release/tag未动。
- 证据 `.local/evidence/ims-route-completion/48e269c/{verified.json,production-verified.json,production-install.json,production-active-profile.json,production-stability.json,production-final-facts.json}`。初次stage把正常子UE worker误判为额外程序，另一次遇到MM对象换代空窗；均在只读预检停止、未上传/停服，失败记录已保留。后续按真实父子进程关系和有限只读稳定库存核实后才操作。

### 明确保留的未完成项

1. **运营商注销仍返回SIP500/rejected**：修补了已认证Digest/nonce-count与Security-Verify，以及禁止423将Expires0变正数；48e269c维护窗口内再次成功注册，但注销仍被500拒绝，本地承载/namespace/profile回收均通过。不能声称网络注销修好；该项独立待排查，不再为此打断当前健康注册。
2. **当前版本自然续期已通过6轮实机验收，见首节**；长通话、呼入/音频、通话中续期、真实换卡/故障注入仍未验收，不用自然续期结果替代。
3. 普通数据共存不在首版新profile准入范围（当前数据关闭）；MM owner/SIM/boot变化、未知持久化/写入仍保留记录并阻断，需要维护，不承诺跨基带/MM重启自动删除旧profile。
4. 若再次部署或需要注销诊断，必须重新确认当前注册/通话状态和维护窗口，不根据下方“主服务停止”的旧快照直接停服。用户原有历史文档移动仍未提交且未改动。

## 生产候选开发与部署前现场（2026-09-30，以下为历史步骤）

用户在探针成功后明确要求完成项目修复并部署。**新现场不再是下方“服务停止”**：本轮首次只读连接已见旧cf13a66/PID535、MM/PID1028、secondary自动重启；这些变化在本轮连接前已发生。管理仍usb0，当前卡/线路派生51502、漫游50212，IMS未注册、已有真实420响应，数据关闭、无通话/启用任务。证据 `.local/evidence/ims-profile-production/{baseline.json,runtime-before.json}`。

- secondary日志明确旧unit调用已移除的`secondary-qmi-init`，以INVALIDARGUMENT反复退出（不是已证明的基带故障）。部署将保守停止该无效循环并安装包内canonical `device-init` unit，但不在线运行初始化、不重启MM/基带；普通数据当前关闭。没有为绕过问题增加危险的旧命令别名。
- 生产实现：设备opt-in、标准派生、IMS-only范围；原族循环每次准备独立AT profile；v2持久profile/bearer网络镜像、原owner/SIM、线路代次、设备flock、取消屏蔽、profile清理晚于bearer/网络。普通分支持有同一flock，未知profile不被按APN复用。启动恢复不明则禁止全局namespace搬移。
- 审查补强：每次承载新建接口选择状态但不换MM owner/SIM，代次谓词到达实际CreateBearer/Connect分发；服务退出按SIP/XFRM→bearer→profile顺序回收，持久写入不明保持阻断。旧8秒watchdog改为有界40秒，覆盖5+5+20秒清理预算。
- 注销代码问题已补：从已认证会话生成新nonce-count Digest并保留Security-Verify；Expires0注销不允许因423重建正数租期。历史网络500仍保留，不断言此代码差异是运营商返回500的唯一原因，实机注销仍待验收。
- 初版 `12bd0c7fe81d9b26ebd0d3ea0b918b3ae24c0ca4` Validate已通过；后继 **`16bf1f9309834f97fc649c146df79c08e3cebb48`** 进一步把profile受控setup期限放到外层，内层保留晚到Create结果到profile清理，不因独立90秒超时丢失生命周期衔接。Validate36758599751 / Build36758599884运行中，**尚未宣称最终候选CI、制品或部署通过**。
- 用户历史文档移动保留未提交。一次rustfmt递归触及无关文件的纯格式变化已根据任务起始干净状态恢复，仅保留本任务文件；初轮静态守卫失败后更新为新安全边界并全量重跑通过。Rust只在Actions。
- 私有部署脚本 `.local/active/lan/deploy_production_profiles.py` 分stage/open-window/install，要求最终verified.json，固定SSH主机pin、实际SIM/MM/配置/无通话任务/usb0预检；不复制DB/config、不清预算，候选包更新二进制/前端/设备资源。尚未执行停服或覆盖。

## 先前验收：临时 profile 上实际 IMS IPsec 注册成功（2026-09-30 16:29 UTC）

**本节优先于下方“尚未注册”的历史记录。`83354b620530c4b9dd26fa088a86a6d3e36e849f` 已在当前51502 eSIM、50212漫游网的有界维护探针中实际注册成功；不是主服务持续在线或生产集成完成。**

- 实际路径：独立、动态选择的CID4 `IPV4V6/ims` → MM授予IPv4、实际wwan0 → 2个P-CSCF路由 → 第1候选420 → 第2候选补齐Require/Proxy-Require sec-agree → 401/AKA与Security-Server → 现有REGISTER协议栈返回成功会话，探针`registered=true / registration_mode=ipsec / derived_3gpp_lte_51502`。没有改为固定IPv4或写死CID/接口。
- 420补强仅针对标准派生配置、auth_rounds=0、唯一Unsupported sec-agree以及明确“without sec-agree … is … on”警告词序；保留安全offer/AKA身份，补齐声明而非删安全头。不改catalog/地址族顺序/原大兜底/候选预算。初版b149b70因按原始标点/空格匹配，分支未命中、仍420；83354b6改为匹配已采得的7词序列，回归拒绝off/not/额外扩展/认证后等情况。
- **注销未获网络确认**：探针返回`unregister_result=rejected`，不能说已成功向运营商注销，更不能说现在仍可用/在线。本地承载已释放、namespace库存恢复、bearer receipts为空；仅删除本次自有profile，最终原3项profile/Initial EPS/reporting验证恢复。
- **16:30 UTC最终状态**：MM仍PID48819、当前Modem/5；主服务/beta8/secondary均停止，recovery timer active。正式安装仍cf13a66，配置与DB哈希从写前到清理后相同；没有创建配置/DB备份、没有重启MM/基带或清除恢复预算。此次未拨号、发短信、续期或启动监听。
- `83354b6` 本地228 Python、定向格式/diff通过。Validate **36743092348** / Build **36743092317** 全success，两套实际下载日志核验 **60项累计新增回归+8项兼容检查**均ok，双架构制品digest/meta/ELF/程序/前端核验通过，Publish skipped，旧Release/tag未改。
  ARM64 artifact11110863966，包SHA256 `011c46de075f10d6487c7751fd7e4308c3f2fa05c906184ba53da0fe3a9071fc`，程序SHA256 `2d4a6e4f94ac1685eeef771e1ef8ec0bfd5b6134acf646cb4495a9c6690f19b1`；AMD64 artifact11110853550，包SHA256 `813a20347f67544d7511dfd3b936d76689385d1036c434cd42e61b6928efcf0e`。
- 证据 `.local/evidence/ims-route-completion/83354b6/{verified.json,registration-verified.json,derived-security/}`。被动观察只记录SIP元数据，收到420与带Security-Server/AKA挑战的401；后继IPsec内的注册成功依据程序实际成功会话报告，不伪称AF_PACKET看到了加密内200报文。

### 仍未完成：主服务生产集成与持续在线验收

1. **不能直接将维护CLI接到自动重试**：它要求程序停止、无任何MM bearer；`drain_bearers()`调用全局shutdown，生产进程不能调用。需派生/MM/QCM410专用运行时能力，保留profile持久账本、原MM bus、不可变SIM/线路代次、设备级排他以及取消中的晚到结果。
2. 自有profile生命周期必须关联到pending/active bearer，等待原对象和网络清理完成后再恢复reporting/删除profile；MM对象换代只能重绑定profile清理，不能把旧bearer操作转发到新对象。未知结果保留并阻断，不能让普通APN复用误采纳遗留自有profile。
3. 普通数据共存尚未验收：维护快照拒绝任何MM bearer；应先限定无冲突的IMS-only场景，不为准入而停止数据/secondary。新探针wwan0与旧主服务wwan2有占用环境差异，不能宣称独立profile是唯一因果。
4. 当前成功来自IPv4v6独立profile，无需据此增加“SIP失败后另建IPv4”的新循环；地址族和profile来源大兜底继续保持原顺序。生产覆盖部署/主服务注册/自然续期仍待后继，不能把暂存探针当成已部署主程序。
5. 注销被拒绝需独立分析（保留该事实）；不要为了得到好看的结论自动重放REGISTER/注销、重启MM或清预算。用户的历史文档移动仍保留未提交。

## 路由补全续接：2026-09-30（以下为此前逐步记录）

已重新核对六份交接、Git、GitHub Actions、旧抓包脚本与实际调用链。**当前安装的是 `cf13a666c59d1194401bf4d4229f820ac55f448d`，主服务已由用户停止；此前运行PID3365。** 包含a269e9d路由补全及后继旧对象清理修复；两套CI/双架构、部署、路由和旧receipt自动结案均已验证，但本项目版本未注册成功。用户新提供的beta8同机成功对照见下节。

**最新进展：用户刚在同一设备手工运行 `/root/temp/simadmin`（beta8）并确认蜂窝IMS注册成功，随后已停止beta8，且没有重新启动SimAdmin主服务。10:56 UTC只读核实两程序均无运行进程，`simadmin.service` inactive；安装目录仍是cf13a66但不是正在运行的版本。** 不根据下方旧PID3365记录擅自启动服务。MM当前PID48819、secondary服务inactive，这些变化发生在用户测试前后，不是agent重启/停服；管理仍走usb0。

### 有界 REGISTER 续接（已收到420；派生安全声明补全待验证）

已读取本轮 JSONL 最后未完成代码并续接；不要把下述候选当成已部署修复。

- 初版 `f65b474f008cf0276e5aca059e2d683cfaf9e4f3` 增加显式 `probe`，复用现有派生身份/AKA/REGISTER，独立 namespace/内存数据库，不启动主服务或自动恢复。持久化一次性探针状态，限制一次底层承载；报告注销结果和清理结果，不把曾注册成功说成仍在线。
- 补上审查发现的 owner 交接竞态：临时 profile 的原 `MmBus` 直接传到底层建承载，不重新获取 well-known owner；普通生产路径仍按原逻辑发现 MM。清理检查内存与磁盘 bearer receipt，并在 profile release 前阻断遗留探针 namespace。
- 同 owner 对象重新枚举的 profile 清理要求旧对象消失、稳定 SIM/slot、物理控制口、原 profile/EPS 与双快照一致。旧 receipt 缺少新增稳定身份字段时保守保留，不自动迁移或删除解锁。
- 后继 **`1381e25623760ef9b45d4ce327e443e8d2d84e15`** 修复复审发现的 reporting 写入竞态：启用 reporting 也经原 unique-owner bus、串行锁内 SIM/静止检查并读回，不再通过探针核心里的 mmcli 可复用选择器写入。结果不明或取消后 `Probing` 不允许自动 release；不会清账本来绕过。
- 本地 Linux Python **227 项通过**、Windows同227项通过（其中28个POSIX项按平台skip）；定向格式/diff通过。一次Linux全量检查限时中断仅到15项，后来完整重跑通过，记录均保留。Rust仍只在Actions，最新 Validate `36735762535`、Build `36735762412` 运行中，**尚未宣称双套回归或制品通过**；较早f65b474不用于设备试验。
- **15:00 UTC只读现场复核**：主服务/beta8/secondary均停止，MM PID48819、Modem/0，bearer/call为空，DB ok、启用任务0，管理usb0；正式安装仍cf13a66/hash未变。没有新建profile、创建承载、REGISTER或重启MM。证明 `.local/evidence/ims-route-completion/probe-resume-readonly.json`。
- **1381e25两套CI/双架构已全部核验**：Validate36735762535/Build36735762412全success，两份日志56累计新增回归+8兼容检查逐项ok，制品digest/meta/ELF/程序/前端摘要通过，Publish skipped。证明 `.local/evidence/ims-route-completion/1381e25/verified.json`。
- **15:29–15:31 UTC首轮实机闭环**：创建独立IPv4v6/ims CID4并启用reporting，但探针在承载创建前返回 `ims_access_registration_parked`。这是独立维护线路缺少coordinator准入初始化，**没有创建承载或发送REGISTER，不是网络拒绝**。随后release恢复reporting、仅删自有profile；最终3项、pending=null、完整token与试验前一致，config.yaml/data.db哈希未变，原namespace列表未变、无bearer receipts，MM48819及服务状态未变。证据 `1381e25/dual-first/`。
- 后继 **`8c0e0e6cac91c24789df4e8ffb3ae8e87c7b7e39`** 为独立探针线路在transition lock内发布cellular-only准入；普通生产线路仍默认parked、WLAN不获准。Linux228 Python通过，Validate36737616859/Build36737616747全success；两份日志57累计新增+8兼容均ok，双架构制品完整核验通过，Publish skipped。
- **8c0e0e6实机已到SIP**：三个独立有界窗口`dual-first`、`dual-metadata`、`dual-warning`均使用临时IPv4v6/ims，动态CID4、MM实际授予IPv4、实际接口wwan0、精确AT关联2个P-CSCF并预装路由；每窗口4个初始候选均收到420，auth_rounds=0，未注册。后两窗口只增加被动元数据观测，不改请求字段；不把三个窗口说成一次REGISTER。
- 被动证据明确`Unsupported: sec-agree`、Warning399；第三窗口按词白名单保存的警告词序是`without sec-agree and <other> is <other> on`，未保存SIP包体/原始警告/身份。代码当前发送Security-Client但这些候选均无Require/Proxy-Require声明；后继只拟在**标准派生配置+420+唯一Unsupported sec-agree+此明确缺失声明警告+auth0**时，补齐既有安全声明，不删安全头、不扩预算、不改catalog/外层兜底。尚未把假设写成已修复。
- 每窗口均已验证承载/namespace回收并release临时profile，恢复原3项/原EPS/reporting，配置/DB摘要未变，服务仍停止、MM48819未重启。MM对象因接口归还而换代，release的同owner重绑定通过；因此跨窗口完整token因modem/SIM对象路径变化而不同，**不声称token始终相同**。
- 第三窗口首次release在PRE阶段看到MM对象列表为空而拒绝，未分发任何写；该失败记录已单独归档，待重新枚举完成后重新预检并成功release。没有删除未知receipt/预算或重启MM。证据 `.local/evidence/ims-route-completion/8c0e0e6/{verified.json,dual-first/,dual-metadata/,dual-warning/}`。
- 注意实际wwan0与旧主服务wwan2不同，都是MM动态选择且经过拓扑/独占核验；当前隔离窗口与旧主服务的数据承载占用不同，不能把“独立profile”认定为唯一因果，也不能写死网口。生产profile生命周期尚未集成。
- 当前待办：核验后继两套CI/双架构，再执行独立profile有界对照（仍先保持IPv4v6），安全回收后才决定后继；生产派生侧集成仍未完成。私有脚本 `.local/active/lan/bounded_profile_register.py` 每个动作保留独立记录，不能绕过 `verified.json` 或覆盖失败记录重放。
- 用户移动历史 `ESIM_IMS_PROFILE_TEST_2026-09-01.md` 的工作区修改保留未提交。

### Exact-family 显式维护入口（已完成CI与profile闭环，未注册对照）

用户已批准派生侧临时自有IMS profile生命周期和一次受控对照，并确认现有CID自动选择不应重做。已检查MM1.24源码：该QMI驱动IndexField=profile-id，Set不传ID走Create Profile，传ID走Modify；因此新增**显式维护命令**而非改变自动注册循环。

- `mm-ims-profile-lease` 默认inspect，acquire需要匹配当前快照的plan token，release要求APN/family匹配自有记录。严格新建并接受实际返回ID，验证唯一tag/MM与AT族/APN、完整原profile/EPS/reporting未变。
- 需要两套程序停止、无bearer/call/未知承载receipt；不创建承载、不启动服务、不修改现有PDP、Initial EPS、默认族顺序或profile大兜底。元数据账本在/var/lib持久保存，Set/恢复reporting/Delete结果不明时保留并阻断，不盲重试写操作。
- `c71bee2`与`b83a0f4`两套CI/双架构均通过；设备QMI Set创建请求均返回明确`invalid-parameter-length`，原3项profile/完整快照token未变。44字节tag缩至16仍失败，**不是已证实名称长度根因**。首版明确拒绝记录经MM日志/tag/owner/原快照验证后归档保留；新版Rejected由工具核验后结案，未删任何profile或预算。
- **最终维护候选 `6a77d9278e5d8fbaf4519fa0c77dfc2a6a5c7835`** 增加显式`acquire-at`：通过原MM owner的Command读取能力/活动、动态选MM与AT列表中均不存在的CID、写前重读、写后核验。不是direct-QMI，不会自动从QMI失败退到AT，不改变生产CID选择和大兜底。
- 本地**219 Python**、格式/diff通过；Validate [`36722549817`](https://github.com/autisticryptic/SimMaster/actions/runs/36722549817)、Build [`36722549812`](https://github.com/autisticryptic/SimMaster/actions/runs/36722549812)全success，实际下载日志核实**44累计新增/更新回归+8兼容检查**，双架构制品核验通过，Publish skipped。
  ARM64 artifact11101166888，包SHA-256 `99fb2b31f2b1d742473be503820a8322ea66218eef9fa180098d3c61b9170c86`，程序SHA-256 `d801b34fc7e7b6e817b6d91e3e319cb9dc0a7194d846d06b14df40b4a8756687`；AMD64 artifact11101677001，包SHA-256 `968e453ce637d6e43842b0130db83759a6f640bc598d5f32fce92962b5f21436`。
- **13:43 UTC profile闭环实机成功**：经inspect token准入，MM-AT自动选CID4创建独立`IPV4V6/ims`，MM/AT读回且原条目、InitialEPS/reporting保持；随后只删除本次自有profile，最终inspect回到原3项、pending为空、完整token与最初相同。没有修改CID1/2/3，**没有创建承载或发送REGISTER**。
- 主服务/beta8/secondary仍停止，MM PID48819未变，正式安装仍cf13a66；维护候选只放在独立staging运行，没有覆盖安装或创建配置/DB备份。recovery timer在写操作期间暂停并恢复；其service为已成功结束的`oneshot/RemainAfterExit=yes/active-exited/MainPID0`，一次预检误拒绝未发写，记录保留。
- 证明`.local/evidence/ims-route-completion/6a77d92/verified.json`与`profile-{acquire-at,release,inspect}.json`；前两版拒绝及归档证明保留。设计/命令/约束：[MM exact-family profile维护](IMS_MM_EXACT_FAMILY_LEASE_DESIGN.md)。
- **未完成：临时profile上的有界注册对照及生产派生侧集成。** 需要先把MM对象换代后的profile归属与清理衔接做好，不能直接把本工具接到无限重试，也不能把“profile创建成功”说成“当前eSIM注册成功”。

### beta8 同机成功对照（最新，优先于下方无成功对照的记录）

- `/root/temp/simadmin` ELF ARM64、8732248字节、SHA-256 `210c35b11f54dd240a83e90dd08d5e8a8f4f2cea227ce3a0503a9ced4140f9b7`，meta为**1.1.7-beta8 / 930365d**，与本地既有二进制分析完全一致，不是只看名称猜版本。
- 用户明确确认注册成功。beta8终端输出未作为本次日志保存，不能伪造捕获了SIP200；MM日志独立证明其IMS承载有实际收发：profile-id4/APN ims的IPv4 Bearer/2持续约31秒，TX4434/RX1463，最后收到用户Disconnect。
- MM请求时序明确：**profile4 + ip type ipv6**的Bearer/1连续三次被`ggsn-reject`拒绝，随后新建**profile4 + ip type ipv4**的Bearer/2成功；这不是从实际IPv4地址反推请求类型。当前项目此前使用profile3/IPV4V6，MM会按profile自身族决定实际WDS路径。
- 当前网络仍50212；停止后AT+CGDCONT与MM ProfileManager只读列表均只见1/2占位以及3=`IPV4V6/ims`，profile4已不存在。结合相同哈希二进制的既有分析，beta8使用临时独立IMS profile并随尝试族准备定义；**尚无这次profile4完整定义/网口/namespace/SIP参数的运行中快照**，不把数字4当成修复方法。
- **对照并非只差版本**：系统日志显示测试前主服务停止、secondary停止、MM重启；beta8成功时MM已是新PID48819。agent本轮只读，未重放这些操作。不能将成功直接证明为某个单一profile字段的因果，也不能声称必须重启MM或停secondary。
- 当前代码的确定性缺口：`prepare_ims_profile_context`仅按APN复用一次profile，随后所有family请求携带相同pin；MM1.24加载pin配置后以其PDP族为准。更重要的是dual bearer拿到IPv4便被视为成功，后面的SIP无响应**不会回到同一个bearer循环执行IPv6/IPv4**；所以只改preferred CID或单族请求标签不能复现beta8路径。
- 下一步需要独立、可回收的exact-family IMS profile对照，保持既有profile3/普通PDP/Initial EPS/预算不变，不写死4，不借用别人CID或切换direct-QMI。生产修复须明确派生侧准备边界、同owner/SIM/generation账本和失败/取消/删除验证；详见[exact-family设计](IMS_MM_EXACT_FAMILY_LEASE_DESIGN.md)。**这不只是调整REGISTER字段，尚未新建profile、改变地址族设置或启动任一程序。**
- 本地证据`.local/evidence/ims-route-completion/beta8-success/`：`stopped-snapshot.json`、`selected-config-and-mm.json`、`mm-bearer-metadata.json`、`profile-command-evidence.json`、`post-run-definitions.json`、`current-mm-profiles.json`。未提交凭据、数据库、真实SIM身份或原始日志。

### 继续排查：WDS绑定与包完整性（2026-09-30，最新）

用户已确认手机成功是**非VoWiFi蜂窝IMS**，但不记得当时漫游网。接入类型已确认，不能继续把这个问题当待回复阻塞；仍未证明手机与测试机同在50212网络。

- 09:03及10:40 UTC核实仍为cf13a66/PID3365、MM587/secondary346，管理usb0，DB正常、无活动通话/启用任务。09:03曾停止于`emm-invalid-state`；10:40在既有自动流程的REGISTER阶段、未注册。未修改profile/库/普通PDP、Initial EPS、USB或预算；MM/基带未重启。
- **120秒释放因果已缩小**：boot-relative日志显示，MM先收到WDS `Packet Service Status: disconnected` / 3GPP原因36 `regular-deactivation`，约9秒后应用才清理网卡并导致MM重新探测。不是应用先在120秒执行Disconnect所造成；这仍不证明运营商主动拒绝，也可能来自基带内部。
- 在用户要求继续修复范围内，通过原API**只POST一次retry（HTTP202）**进行有界采证，未清任何持久预算。临时将MM日志设DEBUG，远端210秒守卫自动恢复INFO，之后又显式确认恢复INFO；没有重启服务或直接QMI/AT激活/绑定。
  注意“一次”是一次API批次，不是一条REGISTER：原自动校准会继续建立后续承载，不能隐瞒成只有一次底层尝试。后续仅被动采样，没有再次POST。
- **已取得真实QMI绑定请求/响应**：IMS WDS IPv4 client4、IPv6 client5均请求`Bind Data Port: a2-mux-rmnet2`，对应事务返回SUCCESS；与已核验wwan2/dev_port2一致。此前“没有实际绑定应答”的缺口已补足，不再据此猜测应换网口。
- **WDS统计与Linux出向观测不一致**：对应IPv4 client在REGISTER开始前已有TX328，约30秒变1312后不再增长、RX0，随后120秒结束。不能把这1312字节算成已发送的几十个SIP包，也不能凭计数命名认定空口已送达。该差异尚需低层证据解释。
- **报文完整性被动核验**：另一次100秒只读AF_PACKET观测捕获32个出向UDP5060、0入向；IP/UDP校验和全部正确。同期wwan2 qdisc发送从16包/17639字节增长到49包/63614字节，drop/requeue/backlog均0。只保留地址/长度/校验/消息类别等元数据，不保存SIP身份或包体。
- 既有MM DEBUG记录还显示重新创建modem时WDA从802.3调整为raw-ip，Set/Get均成功、QoS=no、aggregation=disabled；后续IMS仍无应答。**不能把初始化前的802.3单独当成已证实持续格式错误**，也未手工修改WDA。
- 只读确认CID3仍`IPV4V6/ims`；未改定义。当前内核`CONFIG_FTRACE`、`CONFIG_KPROBES`均未启用，tracefs无现成追踪入口，不能用现有内核进一步证明DMA/基带内部收发；没有为此刷内核、重启或安装工具。
- **当前结论仍是未注册，未有证据支持再改域名、7200秒有效期或强制sec-agree就能解决。** 下一步需要同50212漫游手机成功对照/脱敏注册证据，或另行安排能观察驱动/DMA/基带的诊断窗口；不在既有限制下擅自改内核、跨CID/换网口试错或迁移native。
- 本地证据在`.local/evidence/ims-route-completion/cf13a66/`：`release-timeline.json`、`wds-single-observation.json`、`wds-message-summary.json`、`passive-data-format.json`、`passive-packet-integrity.json`、`kernel-observation-support.json`、`profile-definitions-readonly.json`。
  首轮元数据提取遇到journal的null MESSAGE而失败，日志级别已恢复；仅重读既有日志补齐证据，未重放retry。该次tcpdump输出未成功保存，**不把它记为新抓包通过**；32包完整性结论来自后来独立的被动AF_PACKET记录。

### cf13a66 最新实机验收与下一步

- 08:15重连看到旧a269e9d/PID489，MM587/secondary346，数据库ok、无通话/启用任务。PID变化发生在重连前，非agent操作。1条旧receipt由安装器只读证明对象和网络已消失，**安装器未删除它**。
- 08:19:03 UTC新程序cf13a66/PID3365运行，二进制SHA-256 `786c35352d4832aae291ec7dd3096a4493c30a8dbcba71ab125c1f6615b64847`匹配已验证ARM64；复制窗口config.yaml/data.db哈希未变，无备份，MM587/secondary346未重启，timer恢复active，预算目录未被清除。
- **旧对象清理修复已实机通过**：新程序自行结案旧记录；之后一次IMS承载结束/Modem再次换代时，日志再次出现`Retired old MM IMS ownership record after object and network absence verification`，校准后成功在Modem/2创建Bearer/8。新连接不再被旧Modem路径的receipt永久堵住；原owner/SIM/网络安全边界保留。
- **IMS注册仍未成功**：受控只读观察窗口内，两条动态P-CSCF路由均预装，UE抓包44个wwan2 Out UDP5060、0个入向；初始REGISTER和强制sec-agree候选均无完整响应/AKA轮数0，MM约120秒后结束IMS承载，仍记录tx1312/rx0。没有触发POST retry或修改库/profile。
- 08:28:32 UTC再次核验：PID3365/运行hash仍匹配，前端MD5 `01505b0195870511cc8428e1d730b53c`与制品一致，DB quick_check=ok，MM587/secondary346、timer active。
  证据`.local/evidence/ims-route-completion/cf13a66/{deployment-second,registration-observation,installed-verified}.json`；不要用a269e9d旧采样代替此轮。
- 本次08:28快照时尚缺手机接入类型和QMI绑定应答；**上面的继续排查已确认蜂窝IMS、补齐WDS绑定成功应答，并记录有界日志级别调整**。仍不能凭标准域名/库ready断言运营商必然接受LTE，不用VoWiFi ePDG当蜂窝P-CSCF。设备无strace，未安装工具或发direct-QMI绑定命令。
- 本地Bash已恢复，213 Python/文档检查重跑通过。最终目标“当前eSIM像手机一样注册”仍未完成；真实换卡/故障注入、长通话续期、Pixel受控A/B、MM网口移回导致重新探测等长期项仍保持未验收。

### 后续旧对象清理补强：代码/CI及部署已完成（下列连接失败为历史）

- 06:23 UTC只读复核仍运行a269e9d/PID3002，MM598/secondary353未变，管理usb0、DB正常、无通话/启用任务。IMS exhausted，错误引用Modem/0，当前为Modem/1；1条receipt保留。
- 代码追踪纠正“新连接缓存旧modem路径”的猜测：QCM410 transport是无字段provider，新连接会重读绑定；真正失败在Create前的`recover_owned()`，它先重试旧`OwnedLease.bus`上的清理RPC。旧owner仍存在使该检查通过，但旧modem与bearer已消失，UnknownMethod导致永远无法结案。
- 已只读看到旧modem/bearer均不存在，wwan2已回主机且地址为0、原namespace无wwan2。现补充程序化保守验证：原owner ObjectManager确认双对象缺失→只读验证原网口/地址/源路由/私有规则/namespace无残留→再次确认对象缺失，才结案。未知/失败/owner变更仍阻断；不删除预算或其他资源、不把UnknownMethod直接当owner丢失。
- **最终候选 `cf13a666c59d1194401bf4d4229f820ac55f448d` 已推送并验证**。213 Python、定向格式/diff通过；Validate [`36680824631`](https://github.com/autisticryptic/SimMaster/actions/runs/36680824631) / Rust job109775661618、Build [`36680824551`](https://github.com/autisticryptic/SimMaster/actions/runs/36680824551) / Rust job109776078962、两架构全部success，Publish skipped。
  下载两套日志逐项核实**27个累计新增/更新回归名（前轮20+本轮7）及8兼容检查**均ok，包括实际private-D-Bus双对象消失/UnknownMethod不能推断为空、网络残留/对象回归/owner变化保护。
- 初版39baf20两套CI编译失败E0283，已在6f7b190显式指定String错误类型修复；随后根据实机`ip -j -N rule`仍将table编码为数字字符串的事实补回归，最终为cf13a66。失败annotations/记录保留，不以早期SHA代替最终候选。
- 两包已实际下载核验官方artifact digest、`1.1.5/cf13a66`、ELF与程序/前端摘要：ARM64 artifact11081323457，包SHA-256 `60647bc441d390f8b808096aa7f35d42712360ba8416b5c0d7814cf40cf6a46a`，程序SHA-256 `786c35352d4832aae291ec7dd3096a4493c30a8dbcba71ab125c1f6615b64847`；AMD64 artifact11081328485，包SHA-256 `aa08fd9afb625a7455386f24fc0ef0ec07869ba6c2c577992a524e782c42da17`。
  证明`.local/evidence/ims-route-completion/cf13a66/verified.json`；首次下载途中DNS临时失败，重试后全部核验通过，旧Release/tag未动。
- 部署前已只读独立证明旧receipt的原owner/双对象缺失和主机/namespace网络无残留，**未删除receipt**。后继安装脚本只允许保留这种已验证记录给新程序再次核验并结案，不跳过未知资源检查。设备busctl对嵌套MM变体JSON输出报`Failed to create new json object`，这是工具编码错误，已改用完整typed输出保守核验，不当成空对象。
- **07:14:37 UTC后继部署未执行**：SSH连接TimeoutError，记录只有`phase=preflight`，未登录/上传/停服/改文件；Windows当前无192.168.68.*地址，访问192.168.68.1走WLAN默认网关。已询问用户确认设备/RNDIS或新IP，停止重复连接。
  已审阅的后继脚本`.local/active/lan/deploy_retired_lease_candidate.py`；连接恢复后仍须重新核对实际运行hash/SIM/服务/通话/任务/管理链路/receipt，不能直接重放。
- **首次REGISTER无响应仍未解决**，不能归因于这个后续清理缺陷，也不盲套7200秒/required配置。已询问用户：手机成功是否在关闭Wi-Fi、同样50212漫游下走蜂窝IMS，还是VoWiFi；尚待确认。
- 另已读取ModemManager1.24.0公开源码：qcom-soc按`dev_port`选择A2_MUX_RMNET0…7，并执行WDS Bind Data Port。实机wwan2 dev_port2与该映射一致，尚无实际QMI应答逐消息证明错误，不擅自改用其他通道。

### 第二次部署成功与实测结果（优先于下方中断记录）

- 05:58:59 UTC重连：旧828135b/PID490已运行，MM598/secondary353，管理usb0，DB正常、无通话/启用任务、receipt/create均0；PID变化先于本轮操作。曾见`emm-invalid-state`，未靠重启MM/基带清除。
- **06:00:28 UTC覆盖部署成功：a269e9d / PID3002**，运行SHA-256 `160cc0da071aab549c2c66ed564dcb704f0716f18e42becf8258234d96edf761`匹配已验证ARM64。无备份，复制窗口config.yaml/data.db校验未变；MM598/secondary353未重启，timer已恢复active，恢复预算未删除。
  使用独立暂存目录`ims-routes-a269e9d-second`，保留首次失败证据；成功记录`.local/evidence/ims-route-completion/a269e9d/deployment-second.json`。
- **全候选路由已实机证明**：同一承载CID3精确AT归属得到2个P-CSCF，日志`candidate_count=2 routed_count=2`早于首REGISTER；命名空间两条目标路由均走实际`wwan2`。抓到**44个wwan2 Out UDP5060包**，未见入向SIP响应，AKA轮数0，尚未注册。
- **不能再用tx_packets=0断言没有发包**：该机bam-dmux所有网口计数为0，但MM普通数据承载有收发统计；仓库`netdev.rs::send_probe`本已有此驱动不更新计数的说明。只读检查wwan2 `dev_port=2`、同bam-dmux父级，未见runtime-PM error；Linux6.17-rc6参考驱动也没有更新这些统计。AF_PACKET出向抓包证明内核交给该接口，不等于基带/运营商已经收到。
- 对应IMS承载约124秒后结束，MM报告tx1312/rx0（不能等同于44个SIP包已上空口）；网卡移回主机再次引发Modem/0→Modem/1。程序记录binding recalibrated后，后续尝试仍有旧Modem/0错误和保守保留receipt。**实际WDS/SIO绑定尚无逐消息证据，不猜测或用direct-QMI bind-mux试错。**
- 设备时钟比采证主机慢约89分钟，首次按主机UTC筛选journal为空；已补采本次boot的monotonic日志，以PID/承载/相对时序关联，未改设备时间。证据`registration-observation.json`、`datapath-readonly.json`、`channel-mapping.json`；未发送POST retry、通话或短信。
- **用户要求的数据库对照已完成**：见[51502连接配置参考](IMS_CATALOG_PIXEL_IOS_COMPARISON_2026-09-29.md#8-当前-esim51502-的连接配置参考2026-09-30)。三库确有Globe/51502参数：iPhone LTE ready，IPCC LTE unsupported（volte=false），Pixel LTE/NR ready。
  APN/domain/身份/PCO发现/ipv4v6基本与派生一致；iPhone明确required安全策略与7200秒请求有效期，三库有显式VoWiFi ePDG `weconnect.globe.com.ph`。静态值不证明当前无响应原因；iPhone Contact投影缺口、VoWiFi roaming/缺IDi边界均已标明。未安装库/改profile，源库SHA不变。

### 第一次临时 IP 上线与部署中断（历史，第二次已成功）

- 用户提供 `192.168.68.1` 后，使用既有 **root 密码认证**成功连接；原设备主机公钥 pin 匹配。pin 只验证服务器身份，不要求用户配置公钥登录。凭据只在仓库外，未回显/提交。
- 05:40:37 UTC 新现场：`828135b/PID482`，运行 hash 仍匹配旧验证制品；MM590/secondary341，Modem/0，数据库 ok、无通话/启用任务，receipt/create 当时均 0；管理实际走 `usb0`，不是旧 wlan0。这些 PID 变化在本轮连接前已发生，不是 agent 重启。
- 用户明确允许沿 `usb0` 部署后，再次预检并上传已验证 ARM64 包到 `/opt/simadmin-staging/ims-routes-a269e9d`。**激活失败：`deployment_phase=verify_owned_cleanup / exit_code=1`。**
  脚本已停止 `simadmin.service`，随后发现清理后的持久 ownership 记录未归零而保守退出；**未进入 `overwrite_verified_files`，没有覆盖旧828135b程序、前端、配置或数据库**。没有创建备份、删除receipt/预算、重启MM/secondary或拨号。`simadmin-modem-recovery.timer` 已由 finally 恢复active。
- 用户随即报告 RNDIS 使笔记本失去互联网，并明确为此把设备离线。此后只读检查了笔记本：当前 RNDIS 网卡已不在列表，IPv4互联网默认路径为WLAN；**未更改Windows网卡/路由/DNS，也未继续设备请求**。
  已建议后续让RNDIS只保留同网段管理地址、无默认网关/DNS，Wi-Fi保持上网，不改设备USB模式。
- **不能把这次操作称为部署成功。** 最后已知旧主服务被本脚本停止，离线后状态未知；设备上线第一步核实主服务/运行hash/MM与SIM/receipt和实际网络残留，处理服务可用性。不要直接重放脚本（暂存目录已存在），更不能删除未知receipt来绕过检查。
  本地证据 `.local/evidence/ims-route-completion/a269e9d/deployment.json`、`reviewed-activation.sh`；脚本 `.local/active/lan/deploy_route_candidate.py` 只作为已执行范式审阅。

- 03:32 UTC 固定公钥只读核实 LAN 目标仍为 `828135b / PID85175`，运行 hash 与既有已验证 ARM64 一致；MM577 / secondary343 未变，管理走 `wlan0`，数据库 `quick_check=ok`，无活动通话、无启用任务、未安装 catalog。
  IMS `registered=false / recovery_state=exhausted`，错误引用已不存在的 `Modem/1`，现存对象为 `Modem/2`；有 1 条 receipt、无 `.create`，**没有删除或重放恢复**。
- **纠正上一轮根因断言**：`capture.json` 的单条路由、零包/零接口计数是真实记录，但脚本 `route get` 使用写死的历史地址，不是本次承载动态候选；快照主要在首候选阶段。源码对每个实际候选先 await 路由安装，再创建绑定该接口的 socket，失败会传播；日志另有 WDS `tx=1312 bytes`。因此旧证据不能证明实际 REGISTER 经 veth 发出，也不能认定路由是零包的唯一根因。
- 蜂窝实现：在第一条 REGISTER 前为当前承载已接受的 P-CSCF 及本轮实际发现结果逐项预装路由；按 IP 去重，每个地址独立失败，未成功准备者不能进入 SIP；保留发送前幂等路由检查和代次/承载校验。不改变 PCO→精确归属 AT→配置/DNS 的既有发现优先级，不扩大 CID 归属规则。
- VoWiFi 实际链路：DNS 已保留 A/AAAA 列表，IPv4 veth 默认路由本来覆盖所有 IPv4 目标；真正的消费截断是完整握手只取前 5 个地址。现改为完整遍历、成功即止，浅状态探针仍只取 1 个，profile/内层地址族/proposal/path 顺序与限制保持。
  另发现 SOCKS5 原来只用于 DNS，live IKE 仍无条件直连；现通过同一捕获的 UE worker 创建 TCP 控制与 UDP relay，外层 socket 按实际 relay 族绑定，每个 IKE/NAT-T/ESP 数据报携带自己的最终地址/端口，不因代理失败偷偷直连。
- 保留边界：没有为测试机开启 VoWiFi；未新增主机/UE IPv6 上游配置，缺少外层 IPv6 网络不能靠虚构路由解决。既有代理主机名解析与 DNS 系统回退路径未迁移；不能宣传为所有 DNS 都在 worker 或完全无本地 DNS。UDP relay 私有模式及域名型 SOCKS relay 地址仍明确不支持；本次未改 NAT-T 源端口策略。
- 本地已通过 209 Python、21 前端 unit、TypeScript / ESLint、定向 rustfmt / diff；Rust 只交给 Actions。新增回归覆盖多候选、路由失败/代次、超过五个 ePDG、SOCKS 不同目标/族/端口、控制连接生命周期、8 KiB 包与超时。
  证据 `.local/evidence/ims-route-completion/`；Rust 验证结果如下，实机注册仍待设备上线。

### 候选已验证，部署因用户确认离线暂停

- 代码已通过 Windows `git.exe` 推送 `simmaster/master`：**`a269e9d6f7c5b359e142ae7734598c009f091914`**。
- Validate [`36671791336`](https://github.com/autisticryptic/SimMaster/actions/runs/36671791336) / Rust job `109748160357`、Build [`36671791369`](https://github.com/autisticryptic/SimMaster/actions/runs/36671791369) / Rust job `109748379048` 全 success；前端和 ARM64/AMD64 构建通过，Publish Release skipped。
- 两套日志 artifact 均已下载并校验官方 SHA-256，逐项确认 **20 个本候选新增/更新回归名 + 8 个重点兼容检查为 ok**，不是只看绿色 workflow。
- ARM64 artifact `11078356878`，包 SHA-256 `a7b270fa97fd53529ec88f0d92578153883a213a19eed2ea9aa685d549fffae6`；二进制 SHA-256 `160cc0da071aab549c2c66ed564dcb704f0716f18e42becf8258234d96edf761`。
  AMD64 artifact `11078283114`，包 SHA-256 `a0637d577c50b11a8178e78780f2c3f418593dfd52559c8f288f7307aa8ae565`。
  两包均实际验证 `1.1.5/a269e9d`、ELF 架构、二进制与前端校验；完整证明 `.local/evidence/ims-route-completion/a269e9d/verified.json`。
- **另一处旧文档事实已校正**：GitHub 现有 `v1.1.5` Release ID `398850006` / tag 指向 `09edc038f3110f23bce43dcbd77747955e0b1fa7`，发布于 **2026-09-29 05:34:39 UTC（本会话前）**，并非旧文档记载的16998ae。
  核验脚本首次因旧 tag 断言拒绝收尾，重新读取发布日期/tag/资产时间确认后才更新核验基线；没有移动/恢复 tag，也没有覆盖 Release。保留首次核验失败记录和 `release-observed.json`。
- 03:32 UTC 的 `828135b/PID85175` 是**最后可达实机证据**。之后两次固定公钥 SSH 在 TCP 阶段超时，Windows `192.168.100.13:22` 也超时；05:17:27 UTC 有界复核仍超时。
  用户随后明确“设备暂时离线，回头更新临时 IP”，因此停止后续连接。**未上传制品、未停止任何服务、未改配置/数据库、未动 receipt/预算、未触发注册重试或呼叫。**
- **收到新 IP 后继续**：安全更新仓库外目标凭据索引，沿用原 SSH 主机公钥 pin（不同则停止核验，不自动接受）；重新检查当前运行 SHA、SIM/MM owner、无通话/任务、数据库、`wlan0` 和遗留 receipt。
  `.local/active/lan/route_completion_readonly.py` 与 `route_inventory_remote.py` 是已审阅的只读参考；旧部署/抓包脚本含旧 PID/哈希/删 receipt/POST，不直接重放。receipt 归属与残留网络未核实时不得删除解锁。
  使用上述已验证 ARM64 包覆盖，不建备份，保留配置与数据库，临时停再恢复 recovery timer，不重启 MM/secondary；随后用**当次动态 P-CSCF、源地址、实际网口与同一承载窗口**核对路由/计数/包与 SIP 响应。若仍不出包，再只读核查 BAM-DMUX/WDS 对应关系。

## 本轮交接：2026-09-30（历史快照，以上续接结论优先）

**下一位 AI 的完整入口是 [NEXT_AI_HANDOFF_2026-09-30.md](NEXT_AI_HANDOFF_2026-09-30.md)**，
已包含当前实机状态、已批准但尚未实现的路由补全，以及用户提过但未进 todo 的全部遗留项。要点：

- 设备为局域网测试机 `192.168.100.13`（QCM410，aarch64，root SSH，管理走 `wlan0`）；凭据只在仓库外，不要提交或回显。
- 已部署 `1.1.5 / 828135b`（含 MM 数据接口修正），主进程 PID `85175`；该机全局配置曾丢失、现为默认值，
  未安装 `carrier-bundles.sqlite3`，profile 回落派生 `derived_3gpp_lte_51502`；IMS **尚未注册**。
- **当前最高优先级**：为本承载实际下发的**每个** P-CSCF 地址都补上走 IMS 承载的路由；
  VoWiFi 的 ePDG 域名多地址做同类补全。已实机证实根因是**本机路由漏装**而非运营商配置：
  IMS 承载已绑定 `wwan2` 且有地址，但 REGISTER 未从该网卡发出（`tx_packets=0`，两侧抓包为 0）。用户已批准该修正。
- 用户约束：只允许在派生配置一侧补强，不改动现有大的兜底逻辑；若确实无法通过派生配置注册，必须直接说明原因。
- 其余未完成项详见新文档 §5（通话中自然续期是否会断话的实机验证、Pixel 库呼入进语音信箱的受控 A/B、
  IMS 网卡移回主机导致 MM 重新枚举并短暂中断普通数据、真实换卡与自动重新附着故障注入验收；
  Android 移植仅作参考、不在本项目实现）。

## 上一轮：2026-09-29 10:37 UTC（UI 与离线边界，已被上文取代）

用户明确设备与eSIM卡暂时下线，正在用手机测试IMS兼容性。**停止SSH/API连接、轮询、部署、切卡、拨号和短信操作**，直到用户确认上线。

三项UI代码、本地检查及整批Actions已完成，候选 **`33d16f3d78ea0456682a2469810625efe29805ec`** 已推送，**设备离线、未部署**。记录在 [NEXT_AI_HANDOFF §0.14](NEXT_AI_HANDOFF_2026-09-28.md#014-用户新增ui待办与设备离线边界2026-09-29-0824-utc)：
1. 概述线路控制的飞行模式移除冗长说明，与另外三个控件显示形式一致。
2. IMS与Trunk页移除额外“IMS注册模式”选择。**只有VoWiFi与蜂窝IMS两个开关都开启，才允许尝试双注册；两路都注册成功且网络协商允许，才算双注册成功。** 仅开一项只用该项，不擅自打开另一项；双注册不成立则在已启用/可用的接入中VoWiFi→4G/5G IMS回退，保留原会话及资费保护。
3. eSIM自动检测后的profile列表将“可用”改成可实际执行的“切换”按钮，复用现有授权、维护锁和绑定校准，不再仅展示或强制跳完整配置管理。

199 Python、21前端unit、TypeScript、lint、定向格式检查通过。本机Vite限时构建未完成的事实保留；**正式前端构建已在Actions通过**。
最新 Validate `36555415147`、Build `36555415060`、Frontend `36555414932` 全success；下载日志逐项核实3新增Rust回归+6重点兼容回归通过，两架构制品已下载核验commit/架构/digest/二进制及前端校验。
证据 `.local/evidence/ui-offline/33d16f3/verified.json`；Publish Release skipped。当前已部署的LAN目标仍以07:14核验的7c6cf86为准，不拿候选当在机版本。
设备上线后再确认部署/实机切换窗口；原历史文档 `docs/archive/2026-09/ESIM_IMS_PROFILE_TEST_2026-09-01.md` 的非本轮未提交修改保留未动。

本地数据库静态对比已完成：[Pixel/iOS/IPCC分析报告](IMS_CATALOG_PIXEL_IOS_COMPARISON_2026-09-29.md)。六份源库哈希保持不变；三组对应新旧config_json无差异。
报告区分历史空Contact/MMTEL缺陷（已修）、来源回退与现存字段投影缺口；没有宣称用户原来那次呼入失败的唯一根因。
现场结论等用户手机测试补充，不把手机结果当成本项目实机已验收。

## LAN部署完成与数据库分析：2026-09-29 07:14 UTC

用户要求先部署新局域网目标，再研究 Pixel 与 iOS/IPCC 外置库差异。详见
[NEXT_AI_HANDOFF §0.13](NEXT_AI_HANDOFF_2026-09-28.md#013-新局域网目标部署及外置数据库对比2026-09-29-0643-utc)。

- 用户授权“直接重新安装”后，已在LAN目标重建 `/opt/simadmin` 并运行 **1.1.5 / 7c6cf86 / PID54742**。
  运行中SHA-256 `2051c398b58c28ffeea396240b7b29614bac67267807bd123963d7c37d12a829` 与已验证ARM64一致；前端校验/HTTP200、服务active/NRestarts0通过。
- 原目录被删除、旧PID470持有deleted DB的问题已处理：先用SQLite共享读锁保全数据库到唯一正式路径，再停旧服务；恢复前后全部表指纹相同，quick_check=ok。原账号/线路/任务/历史保留；全局配置按授权采用默认值，没有重置密码、没有创建备份。
- MM516/secondary339未重启，恢复timer已恢复active。证据 `.local/evidence/lan-deploy/{reinstall-result,installed-verified}.json`。
- **已按顺序开始外置数据库对比**：先核实本地文件来源/版本，再比较Pixel与iOS/IPCC配置；不切换设备配置或拨号。
  新LAN目标不能与旧Cloudflare实验机a6a327e混淆；未登录LAN受保护API，不宣称其IMS/通话验收已完成。

## 结果分类候选验证：2026-09-29 04:44 UTC

用户要求继续剩余todos。最新逐步记录在 [NEXT_AI_HANDOFF §0.12](NEXT_AI_HANDOFF_2026-09-28.md#012-剩余任务续接2026-09-29最新步骤)。

- **结果分类候选 `7c6cf8635971f738a8af3a79a01f50ba6af4e641` 的代码/CI/双架构核验完成，尚未部署**：typed report、当前接入真实180/远端初始INVITE终局证据、local/re-INVITE错误隔离、观察丢失与取消保护、成功详情展示。
- 本地195 Python、13前端unit/type-check/lint、定向格式/diff通过；新结果语义不改写历史失败记录。首版098ae04有E0004编译失败，已修复并保留失败证据，没有跳过门禁。
- 最新 Validate [`36522353436`](https://github.com/autisticryptic/SimMaster/actions/runs/36522353436) / Build [`36522353454`](https://github.com/autisticryptic/SimMaster/actions/runs/36522353454) 全success；两套日志实际核实 **18新回归+4重点兼容检查**均ok，ARM64/AMD64制品实际下载核对官方digest、meta版本/commit/架构、ELF与二进制/前端摘要。
  证据 `.local/evidence/dial-outcomes/7c6cf86/verified.json`；Publish Release skipped。
- 设备最后验证仍为a6a327e，本次没有部署、再次拨号、切卡或故障注入。
- MM换卡/自动恢复故障注入的维护窗口与切换方式已询问用户，待明确确认，不为完成待办打断已注册会话。

## 上阶段收尾：2026-09-28 16:52 UTC

**用户已确认测试来电送达，要求完成文档后阶段性结束。本轮不再操作设备。请优先完整阅读
[NEXT_AI_HANDOFF_2026-09-28.md §0](NEXT_AI_HANDOFF_2026-09-28.md#0-最新续接定时拨号失败与可选非漫游呼叫准入)，尤其 §0.0 的逐步记录。**
该节取代同文件上午 §1–§9 的过期状态，包含逐步验证、用户确认、阶段收尾、保留边界和后续任务。

**16:52:07 UTC 最终只读核验：** 实际主程序仍是 `a6a327e / PID808982`，二进制哈希与ARM64制品一致；
IMS `registered / ipsec / derived_3gpp_lte_46011`，本版本新注册时间 **16:27:54 UTC**，`last_error=null`，home、活动通话0，测试任务disabled。
证据 `.local/evidence/automation-dial/a6a327e/stage-closeout.json`。因此“最新含 CID 自动校准补强的版本可以实际注册 IMS”已有实机证据；
**不等于真实换卡后的自动校准全链路已验收**。本版本自然续期计数目前0，不借用旧dd8ba1f的续期证据。

本阶段完成：代码/CI/双架构核验、部署、新版本IMS注册、按用户标准的测试来电送达。
保留后续：§0.11的拨号任务结果分类、真实换卡/恢复故障注入、接通后音频/时长及原长期事项；不笼统宣称所有模块没有问题。

- **新增优先任务**：定时拨号立即 failed 的诊断，以及可选“已注册 VoWiFi / 明确已驻网非漫游蜂窝”语音准入。
  用户要求保留原限制开关，关闭后仍能主动允许漫游接打电话。旧严格模式默认不变，短信保护不变。
- **最新设备程序：`1.1.5 / a6a327e1246576c57655ff7c3d615ec4289e18aa`**，两套 CI 与双架构通过，**16:22 UTC 已按用户“现在执行”授权覆盖部署**。
  PID808982，运行中 SHA-256 `a50eba8dcc00fbcb266fb0dfc240a49786d956ea4df254b79e0da5fb0343726f` 与 ARM64制品一致；无备份，配置/数据库复制前后校验未变，MM410/secondary283未重启。
  已补齐 pending 取消/owner/重复请求边界、VoWiFi 快速路径、配置兼容、MM fresh-home/unique-owner checked call 和前端错误反馈。
  定时任务只操作自己创建的 IMS call ID，不再把同号码既有 modem 呼叫或 CLCC index 当成自有资源；取消后保留任务完成精确挂机请求。真实接通/音频/挂机仍需实机验收。
- 最新本地检查：**192 Python 全通过、11 前端 unit / type-check / lint 通过、定向 rustfmt / diff 通过**；
  12:38 静态守卫失败与格式 diff 已解决。最新 SHA 的 **28 项新增 Rust/mock/private-D-Bus + 4 项重点兼容测试**在两套日志中均逐项核实为 ok；Rust 只在 Actions 执行。
  Validate [`36446861013`](https://github.com/autisticryptic/SimMaster/actions/runs/36446861013)（Rust job `109011304514`）、
  Build [`36446860858`](https://github.com/autisticryptic/SimMaster/actions/runs/36446860858)（Rust `109011983233`、AMD64 `109011983037`、ARM64 `109011983099`）全绿。
  两架构包已实际下载核对官方 artifact digest、`1.1.5/a6a327e` metadata、ELF、二进制/前端校验；完整证据 `.local/evidence/automation-dial/a6a327e/verified.json`。
  Publish Release skipped；16:03 UTC 再次核对既有 Release/tag 仍为 `16998ae`。每步记录见上述文档 §0.0。
- **11:51–11:56 UTC 实机采样**：经固定 SSH 公钥的 Cloudflare Tunnel 连接成功，设备仍运行 `dd8ba1f`，
  `/proc/511308/exe` 哈希与已验证 ARM64 一致；IMS IPsec 已注册、续期计数 4、最近续期 11:14:31 UTC，calls 列表为空。
  没有部署、改资费配置、创建任务、主动拨号、发短信或切卡。
- 设备 **automation.tasks=[]、dial_call 日志为空**，只找到 09:32:42 UTC 历史 call 触发记录。
  `trunk.enabled=false / trunk.vowifi_only=true / vowifi.enabled=false` 是当前阻断条件，不是历史根因已复现的证明。
  该限制以前是刻意的资费保护，不应直接删除；用户新授权通过可选模式实现。
- 目标号码仅在 `.local/evidence/automation-dial/requested-task.json`，不提交原始号码；授权/执行状态已更新到该私有文件。最新一次测试结果见下方，不重复触发。
- **最新授权与实际呼叫**：用户明确要求现在拨号、任务时间可自行设置，以“用户接到电话”为成功。
  已设置并读回 `vowifi_only=true / allow_home_cellular_calls=true`，其他线路配置/短信未变。
  任务 `task-voice-acceptance-a6a327e` 已创建：目标与用户给定号码匹配，60秒、默认disabled（每日04:00仅作可编辑时间表）。
  **16:31:57 UTC 只立即触发一次**：预检 home/IMS注册/calls=0；设备 dialing→ringing，16:32:29终局 SIP408/Q85031，任务failed、未接通。
  **用户已确认看到了北京时间00:32的未接电话，只是错过接听**。按用户“收到电话即成功”的约定，**来电送达验收通过**；未接听，不代表音频或接通后时长验收通过。
  **16:34:32 calls=[]，没有重拨**，任务保持disabled。现有failed/408历史不改写。证据 `.local/evidence/automation-dial/a6a327e/{deployment,task-configured,immediate-trigger,call-observations,post-call}.json`，用户确认见本轮会话及私有requested-task.json。
- **上阶段新增、现已完成代码/CI的需求**：自定义/计划拨号的对端未接/超时受限成功分类，保留SIP/Q850和未接听事实，不把本端故障或所有408都成功化。
  详见 [NEXT_AI_HANDOFF §0.11](NEXT_AI_HANDOFF_2026-09-28.md#011-拨号任务成功与对端接听结果代码ci完成尚未部署)。设备a6a327e仍使用旧失败分类；只有部署7c6cf86后新语义才生效。
- 下面 `dc2355c` 校准候选的 CI/制品证明依旧有效，已部署语音程序包含其代码；真实换卡与自动重新附着故障注入仍独立待验收。

## 上一阶段已验证进展：2026-09-28 11:41 UTC

本轮已按用户要求恢复实现；历史交接快照保留在
[接手说明与可复制 Prompt](NEXT_AI_HANDOFF_2026-09-28.md)，以下记录优先于该快照。

- **已部署实测基线仍为 `1.1.5 / dd8ba1f`**：SIM-06 IPsec 注册及两次自然续期通过；本轮没有连接或部署设备。
- 校准首版 **`9a8fe726c55669d789bfed9586153689a3b4f42f`** 的 Validate `36411614418`、Build `36411614307`
  已核实均 success，含 Rust/D-Bus 测试步骤、ARM64/AMD64 构建；Release 按 push 门禁 skipped。
- 已保留并完成原 5 文件补强的进一步审查：MM 持久 reporting/profile 不再经旧 CID 清理；单卡 slot=0/缺失归一为 1，
  非法槽值仍拒绝；嵌套 live/恢复批次更新携带不可变 generation，未知库存不消耗 profile 重试预算。
- 新增补强包括：全线路先同步失效再异步 reconcile；MM discovery 失败不伪造 absence；派生 SIM 在 Create 前及 SIP 前后核验；
  终止性绑定错误贯穿设置读取/清理错误；恢复 reporting 在取得串行锁后重验 SIM/槽位/策略；未知网络清理保留 receipt；
  Create 前持久化 intent，歧义结果或 lease+Delete 失败保持阻断，不丢弃晚到的 Create 回复。
- **最新已验证校准候选：`1.1.5 / dc2355c095f08c075f4d6b334c3f5982a9cfc609`**，已推送 `simmaster/master`。
  本地 **188 项 Python、定向 rustfmt、diff 检查通过**；两套 Actions、14 项新增 Rust/mock/D-Bus 回归及双架构制品已核验，详见下节。
  后续仅文档提交不改变这个二进制候选 SHA，不用文档 HEAD 或旧 Release 代替制品 commit。
- 自动校准整体仍未实机验收；真实切卡/外部 eSIM 切换及自动重新附着故障注入须用户另行确认维护窗口。
- 设计与限制：[MM SIM/承载绑定校准](IMS_MM_SIM_BINDING_CALIBRATION.md)。

### 校准候选的代码/CI 验证（未部署）

- [Validate `36416118351`](https://github.com/autisticryptic/SimMaster/actions/runs/36416118351)：success；
  Rust 回归 job `108907556481` 实际完成编译、硬件无关测试和隔离 D-Bus 测试。
- [Build `36416117791`](https://github.com/autisticryptic/SimMaster/actions/runs/36416117791)：success；
  Rust 回归 job `108907860151`、ARM64 job `108907860170`、AMD64 job `108907860149` 均 success。
  前端构建/测试也通过；Publish Release 按 push 门禁 skipped。
- 实际下载两套测试日志并核对 **14 个新增测试名均为 `ok`**，不是仅查看 workflow 标题。
  覆盖 slot=0/非法槽、未知库存暂停、旧任务状态与 AKA 拒绝、IP 读取期间换卡、串行锁内 reporting 准入、
  终止性错误/族循环、owner 丢失 receipt 与 Create intent 保留。测试日志 artifact digest 同样已校验。
- 两架构包均实际下载：GitHub 官方 artifact SHA-256、包内 `1.1.5 / dc2355c`、ELF 架构、二进制与前端校验均匹配。

| 架构 | Artifact ID | 包 SHA-256 |
|---|---|---|
| ARM64 | `10967856151` | `cdf7ab74683778f8a59795e242f61294f5e98332b45dfbb66261e2a4851e9a25` |
| AMD64 | `10968306519` | `40bd4932f136d5b4197fb3b5ce0d271b512191448092142d4cc786eea96285d1` |

完整 artifact digest、二进制哈希、metadata、run/job/step 与逐项测试证据：
`.local/evidence/mm-cid-calibration/resume/dc2355c/verified.json` 及同目录 `jobs-*`、`tests-*`、`release-unchanged.json`。
再次只读核实：Release `397300521 / v1.1.5` 仍指向 `16998ae3c5172890075b2392ded3ce9c711d72b8`；未覆盖、未移 tag。

**下一步必须先确认维护窗口：** 当次核实设备状态、无通话及管理链路后，才可按用户批准覆盖部署候选（不创建备份，
保留配置/数据库/历史证据），随后分别验收正常注册/自然续期、用户安排的 eSIM/实体换卡、外部切换。
自动重新附着故障分支需独立故障注入授权及可用的一次性预算；预算已消耗时不清除、不靠应用重启重置。
未进行上述实机步骤，不能宣称自动校准或自动重新附着实机验收完成。未知 `.create`/network receipt 仍需独立人工核验，不自动删除解锁。

以下既有 SIM-06 修复/发布记录保留作已验证基线，不能与新校准候选混同。

## 1. 当前优先级

1. **版本/发布已完成**：源码、tag、发布包统一为 **1.1.5 / `16998ae`**，用户手动触发的
   Build-Release `36253740079` 已全绿，GitHub `/releases/latest` 已为 `v1.1.5`。
   两架构包实际下载后确认 SHA-256、包内版本/commit 与 ELF 架构一致，不是只改 Release 标题。
2. **SIM-06 中国电信 IMS 注册失败**：用户已确认上线并授权修复、提交 GitHub、部署新的 **1.1.5 构建**。
   2026-09-27/28 已通过固定公钥 SSH 与只读 API 重连；当前已覆盖为 **`1.1.5 / 02dfdc5`**，
   MM 默认后端。经本次明确授权的一次 reporting/重新附着窗口，**06:32:29 UTC 已实际注册成功（IPsec）**。
   有两个 P-CSCF，实际 profile 仍为标准 derived；前两槽认证失败，第三槽轮换 P-CSCF 后成功。
   **09:49 UTC 续接核验：设备实际已运行 `1.1.5 / dd8ba1f`，IPsec 已注册并完成两次自然续期。**
   下述 `02dfdc5` 为历史维护基线，不是当前运行版本。通用恢复及取消安全补强的 CI、制品和部署
   已核验；故意制造 P-CSCF 缺失的自动重新附着实机分支仍未验收，不打断健康会话来强测。
   当前 CID 1=`ctlte`、CID 2=`ctwap` 保持原样；CID 3=`IPV4V6/ims`。
   IPv6-only 对照无效且被用户指出会影响原兜底，已撤销，**不得用固定 IPv6 替代原地址族策略**。
   新现场与部署进展见 [IMS 诊断 §9](IMS_DIAGNOSTICS.md#9-sim-06-现场与-cid-修复2026-09-2728)。
3. native 真机及其他长期任务按 [开发总计划](DEVELOPMENT_PLAN.md) 和
   [后端路线图](MODEM_BACKEND_ROADMAP_1.1.5_1.1.6.md) 单独推进，不并行抢占同一设备。

SIM-04 自然续期、SIM-05 手动测试已由用户确认完成，不重复等待或验收；
它们的 MM 结果不代表 native 全能力通过，也不扩大解释 SIM-05 的业务测试覆盖。

## 2. 完成边界：不能称为全项目彻底完成

| 范围 | 状态 |
|---|---|
| IMS 命名/数据库迁移、Hickory DNS、Telegram 反代入口 | 约定代码阶段已完成，有回归/历史 CI；保留旧值兼容与各自实机限制 |
| beta8 与朋友提供的三网源码 | 主要流程及可移植边界有归档分析；不是穷尽所有二进制分支或全部移植 |
| native AT/URC、持久化短信、分片、逐片送达、受控恢复 | 已有代码和 CI；最终功能检查点 `302b70e` |
| native 真机、Quectel/DJI 维护、长稳/代次故障 | 尚未验收，不由 MM 测试替代 |
| 同机不同 modem 混合 MM/native | 尚未实现；目前全局二选一 |
| 未知孤儿资源自动恢复、完全统一跨旧 IMS 的去重/通知恰好一次 | 不在已实现承诺内；未知 receipt 保持阻断 |
| 双注册、多线路、VoWiFi/视频、UT/MWI、E911、CS 音频、1.1.6 | 保留各自实现或实机/发布门槛，不能一并勾选完成 |
| SIM-06 | `dd8ba1f` 已部署，IPsec 注册及两次自然续期通过；自动重新附着故障注入分支仍未实机验收 |

详见 [原生后端当前状态](NATIVE_BACKEND_STATUS.md)。旧总计划存在日期较早的条目，
逐项以最新代码和证据核对，不机械地把所有旧复选框重开或清空。

## 3. 只保留一个开发目录

- 唯一工作区为 **`SimAdmin` / `master`**，GitHub remote 为 `simmaster`（`autisticryptic/SimMaster`）。
  `origin` 是历史本地 remote，不要误推；实际 HEAD 以 `git rev-parse HEAD` 为准。
- 整理前 HEAD：`19c3c122afdf6f053d02b1f1a9a2e3b26a7595bd`。
- 原 `SimAdmin-1.1.5` 不是较新代码，而是 `586985e` 的 detached 快照，落后 12 个提交，
  完整历史已在 master；确认无独有源码、只有 Python 缓存和依赖后已移除该 worktree。
- 目录名、版本字符串、发布标题、tag、二进制 commit 是不同概念。不要重建旧目录来“切到 1.1.5”。

## 4. 验证与发布证据

### 1.1.5 已核验发布基线

- 真实源码提交：`16998ae3c5172890075b2392ded3ce9c711d72b8`。
- push 构建 `36252976546`、Validate `36252976632`、Frontend `36252976689` 均 success；
  push 的 Publish Release skipped 是原门禁行为，不是构建失败。
- 用户手动 dispatch 构建 **`36253740079`** 全部 success，含前端、Rust 回归、arm64/amd64 及 Publish Release。
- Release **`397300521 / v1.1.5`** 为非 draft、非 prerelease、latest；tag 指向上述源码 SHA。
  用户删除了旧 Release；不能继续建议操作不存在的 v1.1.7/v1.1.8 Release，旧 tag 与 Release 分开看。
- 2026-09-27 实际下载验证：两个 `meta.json` 均为 `1.1.5 / 16998ae`，target 与 ELF 机器类型匹配。

| 包 | SHA-256（与发布的 SHA256SUMS 一致） |
|---|---|
| amd64 | `34920b6d0760bc8b16463a736c8e20513457c01e0c75ada0b39e043747abc92f` |
| arm64 | `eba1704d6b760142b8a724547781bb4baa40be5df90cd8b822cb0ed555a5d491` |

- 本地检查为 **169 项 Python 测试通过**，含 28 项采证回归；Rust 仅在 Actions 编译/测试。
- 可核验记录在 `.local/evidence/release-1.1.5/github-verified.json` 与 `artifacts-verified.json`。
- 后续 docs-only 提交可晚于发布 tag；不要把新的文档 HEAD 当作已发布二进制的 commit。

### 历史与未验收边界

`302b70e` 的旧 CI 和早期接手快照仍保留，但不再代替上述真实 1.1.5 证明。
本次发布核验和首轮静态对照（`c601a09`）未部署设备、未改变 IMS 注册算法、未改 MM 默认。
后继 Security-Server 列表修补见下节，不能与先前发布源码混同。
此前对其他提交号、已完成身份错误码补丁或“native 全部验收”的口述不能当证据；
以 Git、明确的 CI run、实际下载包和当前源码为准。

### 后继安全列表修补：代码/CI 已完成，已随候选部署，未覆盖旧 Release

`dfda6cd2ed51e6ee450752a7b565cdb97d4b7906` 补齐多行/逗号 Security-Server 的候选边界与
完整 Security-Verify 回传，不改变默认客户端算法、MMTEL 或 MM 后端。
新增 17 项 Rust 测试及既有 refresh 接线回归，两套 Actions 已实际运行通过：

- [Validate `36291639402`](https://github.com/autisticryptic/SimMaster/actions/runs/36291639402)：success。
- [Build `36291639395`](https://github.com/autisticryptic/SimMaster/actions/runs/36291639395)：前端、
  Rust 回归、amd64/arm64 musl 编译及打包全部 success，Publish Release 按 push 门禁 skipped。
- 本地 172 项 Python 检查、定向 Rust 格式和 diff 检查通过；未在本地编译 Rust。
- 明细证据：`.local/evidence/sim06/security-agreement-ci.json`，对应提交与 job/step 结果已核对。

具体范围及保留的客户端单候选限制见 [IMS 诊断 §8](IMS_DIAGNOSTICS.md#8-后继候选修补security-server-列表2026-09-27)。
这不是 SIM-06 根因/实机修复结论，**已发布 v1.1.5 仍对应 `16998ae`，不包含此后继修补**。
不能重新对现有 tag 直接发布覆盖资产；若需发行或部署候选，应另行明确版本及目标。

### SIM-06 CID 修复与已授权部署（2026-09-28）

- 初版 `8df57a98b27ec41a532b17d9e3b7607b21b4fe8a` 已推送；Build `36325894646` 与
  Validate `36325894682` 均 success，含 Rust 回归、前端和双架构。未修改 Release 发布门禁。
- 部署前补强：新建前读取 `AT+CGDCONT=?`，只选该 PDP 类型支持的空闲 CID；不新建 CID 1，
  不覆盖已有定义（包括空 APN 占位），检查活动状态、重读定义防止覆盖，并读回新建结果。
  对无法确认的写入不自动重复或删除；新定义保留供后续重用。补强版 `e0ade97` 的 Build
  `36368975285`、Validate `36368975330` 均 success；7 项新 Rust 回归实际执行，本地 173 项通过。
- 设备已明确报告 `IP/IPV6/IPV4V6` 支持 CID 1–16；不是只根据 reporting 表猜测支持范围。
- 部署采用 **Actions 制品的版本 + commit + 官方 artifact SHA-256** 核验，版本仍为 1.1.5。
  公开下载中转只能传输公开制品，必须与 GitHub API digest 一致；不向中转发送设备或 GitHub 凭据。
  现有 Release `v1.1.5 / 16998ae` 不覆盖、不移 tag，不能拿该旧包代替新修复。
- 已于 2026-09-28 部署 `1.1.5 / e0ade97`，实际主进程 SHA-256 为
  `badb454b965a4e68d3e69b7c7ddf1dbaf035fe4eebaea62527ef5189894a6c33`；主服务更新，MM/secondary 未重启。
  实机发现 `mmcli` 的 `response: '…'` 外壳被新校验拒绝，仍未创建 IMS CID，注册未成功；
  已追加外壳归一化和真实格式回归，后继 `02dfdc5` 的 Build `36370931907` 与 Validate `36370931916`
  均通过，9 项新增回归实际执行。已直接覆盖部署，主进程实际 SHA-256 为
  `8041d6860fd5866245517bdf450183a643e82e307df792632711b186358e4784`。
- `02dfdc5` 已实际创建 CID 3、打开 reporting，但 P-CSCF 仍缺失；AT 活动 CID 与 MM profile pin
  的对应关系仍待排查，不能认定“创建 profile 就一定修好”。仅本次新建的 CID 3 曾临时改为 IPv6，
  对照无效；用户指出会干扰原兜底后，已于 2026-09-28 03:23 UTC 明确撤销为 **IPV4V6**。
  当前主服务 PID 307733，仍为 `02dfdc5`；配置 `ipv4v6 → ipv6 → ipv4` 未改，原 CID 1/2 未改。
  固定 MM profile 的 PDP 类型会优先于请求族，后续应修正真实兜底接线，不再用固定 IPv6 绕过。
- **用户最新要求：实验机直接覆盖，不再保留备份。** 本次新建的部署备份已按要求删除；
  后续不再创建备份，不删除既有历史诊断、私密资料或用户数据。仍先确认无通话、管理走 `wlan0`、
  制品与目标一致；不得重启 modem/MM、修改 Initial EPS、NV/USB 或扩大到 SIM-04/05 测试。

## 5. MM 维护结果与通用恢复边界

- 用户再次明确：**本轮只修 ModemManager**；native/直接 AT 硬件控制迁移留后续，不再做 AT/QMI 旁路实验。
- 已完整读取用户指定的 [P-CSCF 对照 §7](archive/2026-09/IMS_PCSCF_BETA8_COMPARISON_2026-09-15.md#7-sim-04-实机结论更新2026-09-20--2026-09-21)。
  该节的较新实测结论是：SIM-04 先启用 reporting，再经 MM 做一次 `Disable → Low Power → Enable`
  重新附着，才取得 P-CSCF 和注册；不是固定 IPv6或临时直接 AT 激活的效果。
- 用户已明确批准本轮的一次 MM reporting/重新附着操作。执行前核验唯一活动上下文的实际 APN、
  与原 MM grant 的关联、唯一自有 bearer、无通话和 Wi-Fi 管理路径；只将已确认上下文的 reporting
  打开，经原 MM owner 执行 Disable/Low Power/Enable。未更改 PDP 定义、Initial EPS 或地址族策略。
- 本地脚本误把 MM 的 REGISTERED 状态 8 写成 `>=9`，因此其等待阶段报告超时；这不是网络未恢复的
  证据。只读日志随后确认驻网恢复，程序取得两个 P-CSCF 并在 06:32:29 UTC 完成 IPsec 注册。
  MM/secondary PID 未变，只有主服务按维护操作重启。证据在 `.local/evidence/sim06/deploy-02dfdc5/`。
- 已实现并通过 CI、部署核验的通用恢复（`7896e05`，取消安全补强 `dd8ba1f`）：只在派生配置最终停于 P-CSCF、普通发现/各 profile 槽位耗尽后考虑；
  严格绑定原 MM owner/lease/SIM/实际 grant，唯一活动且实际 APN 匹配的上下文仅作恢复提示，
  **不放宽现有 P-CSCF 地址归属规则**。无通话/数据/VoWiFi或同 modem 其他线路冲突时，释放原 lease
  后至多一次恢复，再运行原 profile 和地址族顺序。预算持久化到 `/run`，普通重试/应用重启不重置。
  不硬编码 MCC/MNC/APN，不调用 native/direct WDS，不向初始 EPS 或现有 profile 写入新值。
- ZIP 与 6 份生产入口文件的字节已再次核验一致；ZIP 注册走 Python 独立 WDS 路径，不能将其
  固定 IPv6/3gnet 激活直接套入本项目 MM 修复。暂不需要新 IDA 解析；确需具体 beta8 分支时再通知用户开启 MCP。

### 2026-09-28 09:49 UTC 最新验收

- GitHub Build `36392766359`、Validate `36392766357` 均 success，对应完整提交
  `dd8ba1f7314150e10c5ce38cafcb18c8c0cf735c`。两架构制品摘要、包/二进制哈希及 21 项新增 Rust
  回归重新核验通过；本地 179 项 Python 检查通过。
- 当前 `/proc/511308/exe` SHA-256 为
  `3ae12007b981bbeb0efca220365b8701f391ab16009346fa7ad457f072900e4d`，与 ARM64 制品一致。
  MM PID 410、secondary PID 283；本轮仅只读采证，没有重复部署或重启。
- 同线路 API 确认 `registered=true / ipsec`，有效 profile 为 `derived_3gpp_lte_46011`。
  初始注册 07:54:27 UTC；08:44:28、09:34:30 UTC 两次自然续期成功，计数为 2。
  地址族顺序仍为 `ipv4v6 → ipv6 → ipv4`，本次实际 IPv6 不代表固定 IPv6。
- 已完成：设备重连、恢复补丁 CI/制品/部署核验、SIM-06 初始注册及自然续期核验。
  未验收：故障注入触发的自动重新附着全链路及取消分支；不关闭 reporting、不清预算强测。
  API 的 `recovery_source=automatic` 不能单独证明新重新附着分支执行过。
- 证据：`.local/session-review/verified-runtime.json`、`connection-result.txt`、`current-ci.json`、
  `python-tests.log`，及 `.local/evidence/sim06/deploy-dd8ba1f/`。旧 Release 仍未覆盖。

## 6. 本地资料布局

| 路径 | 用途 |
|---|---|
| `.local/active/ims/connect_readonly.py` | 当前只读 Cloudflare/SSH 入口；其依赖及已保存主机公钥 pin 同目录 |
| `scripts/ims-readonly-evidence.sh` | 随源码维护的规范采证脚本 |
| `.github/scripts/test_ims_readonly_evidence.py` | 采证工具回归 |
| `.local/evidence/sim06/` | 脱敏访问记录，不是当前在线状态 |
| `.local/evidence/ci/` | 历史 CI 核验与后续构建证据 |
| `.local/checkpoints/pre-cleanup-2026-09-26/` | 整理前 15 文件快照、补丁和测试记录 |
| `.local/cleanup-2026-09-26/` | 原目录清单、52 份原文档、移动/删除清单 |
| `.local/archive/` | 原 `.codex-*`、`.tmp-*`、`.tmp/`、旧 release 包和会话；保留唯一数据及证据 |

`.local/` 不随 Git 分发，其中历史资料可能含凭据，不公开打包。普通 clone 不包含访问权限。
原根目录三份 carrier SQLite 数据保留原位置；主目录的可用前端依赖和构建资源也保留。
Git 私钥及仓库外私密交接未删除或覆盖。

需要原始用户原话时，才查 `.local/archive/sessions/2026-09-24T.jsonl`，更早参考
`2026-09-19.jsonl`；程序化定位并脱敏，不整段输出 Cookie 或原始工具结果。
历史脚本仅作证据，不批量执行、不自动重建旧 bundle、不恢复旧部署流程。

## 7. 设备上线后的只读第一轮

1. 用户确认上线后，先核对本工作区 Git/diff、规范脚本及 `.local/README.md`。
   审阅只读入口后使用既有 Python 环境；不要运行通用客户端的写操作主程序：

   ```sh
   /root/.cache/simadmin-mm-resume-venv/bin/python .local/active/ims/connect_readonly.py
   ```

2. 私密凭据和 host-key pin 缺失时由用户安全提供；不猜密码、不自动信任新主机、不回显秘密。
   最新固定公钥连接已成功（2026-09-27/28），早期 HTTP 530 / Cloudflare 1033 是过期离线观察。
   新错误仍须重新分类，不能认定 Cookie 永久有效/过期；失败不连续轮询。
3. 先看 `/proc/<MainPID>/exe` 哈希、安装 metadata、服务 PID/start ticks/启动类型。
   采样期间进程稳定不证明历史 journal 全部来自该程序版本。
4. 经既有授权应用登录，另行只 GET `/api/cellular-ims/lines`、该线路详情、`/api/modem/backend`。
   核实实际 `line_id`、SIM 作用域和 MM/native 后端，不把“SIM-06”直接当 API line ID。
5. 获取 `phase/stage/last_error`、连接/候选尝试、恢复和下一重试时间及未解决 receipt。
   按 bearer/IP 族 → P-CSCF → AKA/安全协商 → 真实 SIP 响应定位，归属必须和线路/时段交叉核对。
   只读入口不会自动登录应用、查询这些 API 或认定日志属于 SIM-06。
6. 无证据不猜 APN/身份/运营商特例；明确缺陷后才改代码。部署需当次确认，核验目标、无通话、
   制品 commit/版本/架构/校验和；不根据目录名或 Release 显示名称部署。

采证字段、脱敏和输出边界见 [IMS 只读诊断](IMS_DIAGNOSTICS.md)。

## 8. 不得被旧摘要覆盖的约束

- MM 保持默认；native 必须显式实验 opt-in，不能自动停 MM、接管、写 NV/USB 或重启设备。
- 单一 agent 修改代码/配置/部署，其他 agent 只读；不自动拨号、发短信、改变费用保护。
- 用户取消了测试窗口自动回滚；不恢复固定 120 秒 refresh、旧包激活或续期轮询。
- 重插/重启/节点换代不等于资源消失；未知 receipt 不删除，已释放 CID 不重放。
- `a4a83c2` push 失败、SIM-04/05 等待验收等旧摘要已经过期。
- 持久化 `volte_ims` 已通过迁移归一到 `cellular_ims`，保留旧值读取；不要回退迁移，
  也不机械替换剩余历史名称或真实 VoLTE 语音语义，见 [命名与兼容](IMS_NAMING_MIGRATION.md)。
- Rust 编译/测试/双架构构建只在 Actions。本地允许 Python、格式/语法和文档检查。

## 9. 新对话可直接复制

> 请先读 `docs/HANDOFF.md`，核对当前 Git/diff 和版本修正/CI 状态。
> 唯一开发目录是 SimAdmin/master，旧 SimAdmin-1.1.5 已安全收口，不能当较新分支。
> 若我已确认设备上线，再按文档只读核实 SIM-06 中国电信的实际版本、线路和失败阶段；
> 否则不要连接或轮询设备。不要重做 SIM-04/05 验收，不清理 `.local/` 或私密材料，
> 不把旧 CI、安装 metadata 或历史日志当新程序实测。无现场证据不猜根因，部署需另行确认。

更多文档按 [文档导航](README.md) 查阅；新进展更新本文，不再另建根目录接手副本。
