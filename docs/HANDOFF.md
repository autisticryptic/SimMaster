# 当前交接

本页是唯一当前交接入口。旧过程见[历史摘要](archive/README.md)；文档中的 PID、SIM 和脚本不是实时状态，不能直接重放维护操作。

## 用户要求与操作边界

1. 修复**全局蜂窝 IMS 注册兜底**，不增加运营商/MCC/MNC 分支或特例测试。
2. 日常只维护 `master`；**编译、Rust 测试和注册模拟仅在 GitHub Actions 执行**。
3. 保留原线路列表、详情和标签布局；文档只更新固定入口，不新增日期流水账。
4. 2026-10-07 的 410 部署属于已结束的历史授权；本轮只实施 IMS 补全和 Actions 验证，**未部署、未访问设备，也不主动连接 IDA**。
5. 私有 eSIM 报告、诊断日志、JSONL、凭据和 `.local` 不提交，不清理未知用户数据。

## 当前：TCP/TLS 已验证，执行诊断待 CI

N6 代码 **`6c0c14067941c9e6cd3d226c5e1bca12e3f587f9`** 已通过 Build-Release `38335177956`、Validate Beta `38335177980` 和 ARM64/AMD64；Release skipped。两套测试 ZIP 官方摘要、各 219 项相关回归及四矩阵 86 场景已核验，报告 `.local/evidence/ims-sip-stream/verified-6c0c140.json`。worker 内 SIP TCP、四向 TCP/IPsec、TLS1.2/1.3 与精确原 authority DNS/IP SAN、有界 DNS TC→TCP/NAPTR D2T 已接通，保留旧 UDP 与跨 profile 防降级。TLS 不绕过 required-IPsec，不冒称私有 CA、mTLS 或新的反向 TLS/IPsec 组合已支持。

N7 当前实现接入实际 provider/UICC/端点/请求/响应观察、epoch/尝试 token、统一 API 和原 IMS 工作台只读折叠区；类型/生产回归/HTTP/格式化/浏览器测试已经加入，**尚待本提交的 Actions 执行与制品核验**。第一轮静态检查发现共享诊断重复了蜂窝错误码字面量，已改为适配层使用 canonical 常量，未绕开守卫或让 core 依赖 modem 错误表。

N7 首轮 `ee2b440` 的编译/lint 未通过：缺少错误类型 import、Child-SA 配置路径错误，以及新 Node 单测/严格比较的 lint 问题已修正，待重新 Actions。首轮 annotations 与经官方 SHA 核验的 Beta 测试 ZIP 已保留；没有取消门禁或拿旧提交结果替代。

第二轮 `e5f4387` 的 Beta 与前端 lint/type/build/43 单测通过；新 browser fixture 漏掉顶层 `line_id` 导致 WiFi 响应未匹配，已按真实契约修正，未放宽断言。双接入 SIP 前归属失败和 IKE 鉴权失败的诊断另补生产回归，仍需新提交全套验证。

新审查确认两项仍须实施：**蜂窝刷新 423→400 的候选预算连续性**与 **WiFi 普通 REGISTER 的 503/Retry-After 实际门禁**。诊断将响应等待要求和真正执行的 not-before 分开；已有 reg-event 等待不能代替普通 REGISTER 恢复。随后继续 N8 承载能力、相关 catalog/provider 与完整 EC20 固件复审，不在 N7 或 CI 绿色后结束整体目标。

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

上一阶段 AUTS/stale/跨候选 423 在 `344308c` 已验证；两套 Actions（[38138978950](https://github.com/autisticryptic/SimMaster/actions/runs/38138978950)、[38138978978](https://github.com/autisticryptic/SimMaster/actions/runs/38138978978)）各核验 46 项相关回归和四矩阵。独立 AKA 来源、受保护 stale 不重装旧 ESP、AUTS 不冒充证明、423 尝试级预算均保持；WiFi 初次认证后 423 仍明确 unsupported。

**完整补全任务尚未完成。** N4～N6 已验证；N7 诊断、刷新预算/WiFi 普通等待补漏、N8 承载能力继续推进。跨项目 catalog/provider、IDA 与实机验收保持独立授权边界，见[开发计划](DEVELOPMENT_PLAN.md)。

## 第一批：ISIM、P-CSCF 与临时拒绝补全

代码基线 **`5aaf3eab6515aba9f5f5eb43553ea00a1cec3264`**（前一实现提交 `e702cef`），已推送 `simmaster/master`。

**这是第一批实现及验证闭环，不是原 EC20 审查全部建议已完成。** AUTS/stale 与跨候选 423 后续已验证；rspauth、reg-event 后续已验证，当前继续诊断、恢复补漏与承载能力；完整范围和跨项目/IDA 边界见[开发计划](DEVELOPMENT_PLAN.md)。

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
