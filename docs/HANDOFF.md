# 当前接手与项目状态

> 更新：2026-09-28。**本文件是唯一当前接手入口**；历史记录在 [archive](archive/README.md)，
> 私有操作材料在本机 `.local/`。不要根据旧文档的“当前版本/下一步”重放操作。

## 最新用户待办与离线边界：2026-09-29 08:24 UTC

用户明确设备与eSIM卡暂时下线，正在用手机测试IMS兼容性。**停止SSH/API连接、轮询、部署、切卡、拨号和短信操作**，直到用户确认上线。

新增UI待办已记入 [NEXT_AI_HANDOFF §0.14](NEXT_AI_HANDOFF_2026-09-28.md#014-用户新增ui待办与设备离线边界2026-09-29-0824-utc)，尚未实现：
1. 概述线路控制的飞行模式移除冗长说明，与另外三个控件显示形式一致。
2. IMS与Trunk页移除额外“IMS注册模式”选择。**只有VoWiFi与蜂窝IMS两个开关都开启，才允许尝试双注册；两路都注册成功且网络协商允许，才算双注册成功。** 仅开一项只用该项，不擅自打开另一项；双注册不成立则在已启用/可用的接入中VoWiFi→4G/5G IMS回退，保留原会话及资费保护。
3. eSIM自动检测后的profile列表将“可用”改成可实际执行的“切换”按钮，复用现有授权、维护锁和绑定校准，不再仅展示或强制跳完整配置管理。

本地数据库只读对比可以继续，现场结论等用户手机测试补充，不把手机结果当成本项目实机已验收。

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
