# 下一位 AI 接手说明与 Prompt 模板

> **最新续接：2026-09-29 11:16 UTC（北京时间19:16）。用户确认设备上线并授权部署，LAN目标已运行33d16f3；新增当前eSIM的IMS承载失败修复任务，见§0.15。不得据此自动切卡/拨号。**
> 总入口仍是 [HANDOFF.md](HANDOFF.md)。本文件同时保留上午的 MM 校准交接快照和下午新增的定时拨号/呼叫准入交接。
> **上一阶段已完成部署 a6a327e、IMS注册与用户确认的00:32来电送达。最新结果分类候选7c6cf86已完成代码/CI，尚未部署；真实换卡/故障注入仍待维护窗口，不宣称所有todo或全项目全部完成。详见 §0.11–§0.12。**
> §1–§9 是 10:52 UTC 的历史快照，其中“5 个未提交文件”和首版 CI 状态已过时；§0 的最新状态优先。

## 0. 最新续接：定时拨号失败与可选非漫游呼叫准入

### 0.0 恢复实施的逐步记录（本节优先于下方 12:38 快照）

- **恢复实施／进行中**：用户取消交给另一位 AI，要求当前 AI 继续，并每步更新本文。
  已核对保留的 20 个在途功能文件；基线 docs-only `fa891c5`，没有重置工作区。
- **步骤 A：代码审查与回归补齐／完成，Rust 执行结果见 C2**（代码 15:42 UTC，验证 16:03 UTC）。已修 pending action 的 owner/重复请求/取消优先级/异常清理和队列上限；注册 VoWiFi 不等待蜂窝观测；新号码发起/回退有绑定检查；API IMS answer 只报告已请求接听，不提前标成已接通。
  新增配置兼容、路由准入/取消、定时拨号所有权、private-D-Bus home 观测和 checked MM owner 变更回归。严格模式仍有原拒绝测试，仅拒绝码改为更明确的 `voice_vowifi_only_required`。
  **安全收口**：定时拨号使用唯一 IMS call ID、订阅启动前的拒绝事件、受保护任务在取消/超时后请求精确挂机；不再借用同号码既有 modem 呼叫或可复用 CLCC index。它暂不做 CS/直接 AT 自动拨号兜底。
  政策检查的普通 MM fallback 改用 unique-owner MM Voice 对象，Create/Start/Accept/失败 Delete 不改投新 owner，不跟随歧义 ATD 自动重拨；旧兼容未检查函数保留，但新 API 调用 checked 入口。
  诊断只发布可识别阶段/原因码，不转发任意 provider 错误文本。真实接通/音频/自动挂机完成仍须实机确认，不能把“已请求挂机”说成网络已结束。
- **步骤 B：本地允许检查／已通过**（15:42 UTC）：Python **192/192**，前端 **11/11 unit、type-check、lint**，定向 rustfmt 和 diff 检查通过。`resume-python.log`、`resume-frontend-unit.log`、`resume-types.log`、`resume-lint.log` 在 `.local/evidence/automation-dial/`。先前 12:38 的失败已过期；Rust 未在本地编译/测试。
- **步骤 C1：已提交并推送**（15:45 UTC）：语音候选 **`5ae4f8e6855b4590bc9af8299f884bd76564642c`**，remote `simmaster/master`，无 Release 发布。
- **步骤 C2：最新代码 CI 已完成**（16:03 UTC）：最终语音候选 **`a6a327e1246576c57655ff7c3d615ec4289e18aa`** 已推送，包含 Weak 引用防持有环及两项换代补强；首轮 `5ae4f8e` 亦通过 CI，但不再是部署目标。
  [Validate `36446861013`](https://github.com/autisticryptic/SimMaster/actions/runs/36446861013) 的 Rust job `109011304514` success；
  [Build `36446860858`](https://github.com/autisticryptic/SimMaster/actions/runs/36446860858) 的 Rust job `109011983233`、AMD64 `109011983037`、ARM64 `109011983099` 全部 success，Publish Release skipped。
  两套测试日志均已实际下载并核对官方 digest；**28 项新增 Rust/mock/private-D-Bus 回归 + 4 项重点兼容回归在两套日志中均逐项为 ok**，不是仅查看绿色 workflow。
  单独 Frontend Checks 对 backend-only push 不触发，但两套必需 workflow 的前端 job 均 success；本地 11 项前端 unit/type-check/lint 也通过。
- **步骤 C3：最新制品核验已完成**：ARM64 artifact `10981625555`、AMD64 artifact `10981635676` 均实际下载，
  核对 GitHub 官方 artifact SHA-256、包内 `1.1.5 / a6a327e`、ELF 架构及二进制/前端校验。
  ARM64 包 SHA-256 `d313a62d27aead78b93a2bb9a57c070ffd6f8a4f2851b8bd841d6e39fc34ff1a`；
  AMD64 包 SHA-256 `75999c2bd0c14df27021295c2de3c561cbae32a8597fd315c4545f2f1dc9cbb0`。
  完整证据 `.local/evidence/automation-dial/a6a327e/verified.json`、`jobs-*`、`tests-*`、`release-unchanged.json`；可复核脚本 `.local/evidence/automation-dial/verify_ci.py`。
  Release `397300521 / v1.1.5` 再次确认仍指向 `16998ae`，未覆盖、未移 tag。
- **步骤 D1：已授权覆盖部署完成**（16:22 UTC）：部署前确认 dd8ba1f、calls=0、管理 `wlan0`，旧会话续期计数10。已直接覆盖为 **a6a327e**，主进程 **808982**；运行中 SHA-256 **`a50eba8dcc00fbcb266fb0dfc240a49786d956ea4df254b79e0da5fb0343726f`** 与 ARM64 制品一致，服务 active/running。配置/数据库复制前后校验未变，未建备份，MM PID410 / secondary PID283 未变。证据 `.local/evidence/automation-dial/a6a327e/deployment.json`、`preflight.json`。
- **步骤 D2：任务配置完成**（16:30:54 UTC）：已读取并核验保存 `vowifi_only=true / allow_home_cellular_calls=true`，其他线路配置（含短信）、已有任务不变。
  已创建任务 `task-voice-acceptance-a6a327e`，目标与用户给定号码匹配，持续60秒；默认 disabled，固定每日04:00仅作可编辑时间表，不会自动运行。即将通过“立即执行”触发一次；证据 `a6a327e/task-configured.json`。
- **步骤 D3：一次呼叫送达验收通过（用户确认）**：16:31:57 UTC即时任务触发前，API registered/ipsec、MM RegistrationState=1 / State=11（home）、calls=0；16:32:08 dialing、16:32:21 ringing，16:32:29 任务 failed，历史为 SIP **408** / Q.850 **31**、answered=false、duration=0。
  用户随后明确表示：**看到了北京时间00:32的未接电话，只是错过了接听**。因此按用户事先约定“收到来电就算成功”，本轮送达验收完成；不是仅凭设备ringing推断。
  **未接听，不代表音频、接通后持续时长或双向通话已验收**。任务历史保留真实 failed/408，不伪造 answered；新结果语义需求另列 §0.11。
  没有重拨。**16:34:32 UTC 再查 calls=[]，主进程仍808982**，任务维持disabled，不自动再次执行。
  证据 `a6a327e/immediate-trigger.json`、`call-observations.json`、`call-history-safe.json`、`dial-journal-safe.json`、`post-call.json`。
  通话历史的 Q.850=31 / carrier_reason="Normal unspecified" 来自结构化响应解析；`ImsFailureDiagnostic::from_status(408)` 的本地合成路径不填这些字段，因此有收到终局响应的代码/数据交叉证据。
  用户确认补足了送达证据，但并未穷尽408的协议原因；不得把所有183/408都机械判定为送达成功。后续结果分类按 §0.11 实施，不循环重拨、不清注册预算。

- **步骤 E：最终只读核验与阶段性收尾／完成**（16:52:07 UTC）：经原固定公钥 Tunnel 核实，主进程仍 **808982 / a6a327e**，运行中二进制哈希仍与验证制品一致。
  IMS 为 **registered / ipsec / derived_3gpp_lte_46011**，该版本在 **16:27:54 UTC 实际完成新注册**，last_error=null；当前明确home，活动通话0，任务disabled，语音新条件模式保留。
  **该版本自然续期计数目前0**，不能把旧dd8ba1f的10次续期算在它名下；本次没有为收尾等待或触发续期。
  证据 `.local/evidence/automation-dial/a6a327e/stage-closeout.json`。用户确认收到来电已完成本轮约定的送达验收；未接听，不声称音频/接通后持续时长通过。
  用户要求在文档写完后阶段性结束，**不继续拨号、部署或扩大测试**。保留 §0.11 的新结果分类、真实换卡/恢复故障注入及其他长期事项为后续任务。

### 0.1 版本必须分开

| 范围 | 最新事实 |
|---|---|
| 已发布 Release | `v1.1.5 / 16998ae3c5172890075b2392ded3ce9c711d72b8`；没有覆盖或移动 tag |
| MM 校准代码基线 | **`dc2355c095f08c075f4d6b334c3f5982a9cfc609`**，两套 Actions / 双架构成功；已随 a6a327e 部署并验证当前SIM注册，未做真实换卡 |
| 最新已部署语音程序（含校准） | **`a6a327e1246576c57655ff7c3d615ec4289e18aa`**，两套 CI/28 新回归/双架构通过；16:22 UTC 已覆盖部署 |
| LAN目标最后已验证部署 | **`7c6cf8635971f738a8af3a79a01f50ba6af4e641`**，已重装并核验运行hash/PID54742，见§0.13；不要与旧Cloudflare目标混淆 |
| 最新UI候选 | **`33d16f3d78ea0456682a2469810625efe29805ec`**，前端/两套Rust CI/双架构已核验；设备离线，尚未部署 |
| 最近实机采样 | **16:52 UTC：`1.1.5 / a6a327e`**，PID808982、运行中哈希匹配；IMS已注册IPsec、home、last_error=null、活动通话0；新注册时间16:27:54，当前版本续期计数0 |

本轮开始语音实现前的代码 HEAD 为 `f3164480c1ab8954cfba1cf30f9244212ecc6866`（仅补充校准验证文档）。
语音功能已随 `5ae4f8e` + `a6a327e` 提交；此前的 20 文件未提交清单已转为历史记录。
后继 docs-only commit 不改变候选二进制 SHA。以 §0.0 和实际 Git 为准，不用文档 HEAD 替代候选 commit。

### 0.2 上午的 MM 校准任务已推进到哪里

已保留原 5 文件补强，并完成旧任务 generation/未知库存/全部线路先失效、Create 前预期 SIM、
绑定错误停止地址族循环、串行锁内 reporting 核验，以及未知 network receipt / Create intent 保留等补强。
详见 [校准设计](IMS_MM_SIM_BINDING_CALIBRATION.md) 及 [HANDOFF](HANDOFF.md) 的校准候选段落。

- Validate **`36416118351`** / Rust job `108907556481`：success。
- Build **`36416117791`** / Rust job `108907860151` / ARM64 `108907860170` / AMD64 `108907860149`：success。
- 两套日志实际核实 **14 项新增 Rust/mock/D-Bus 测试通过**；双架构包实际下载，核对 GitHub artifact digest、
  包内 `1.1.5 / dc2355c`、ELF 架构、二进制与前端校验值。Publish Release 按 push 门禁 skipped。
- 校准候选当时本地 **188 项 Python、定向 rustfmt、diff 检查通过**。
- 原始证据：`.local/evidence/mm-cid-calibration/resume/dc2355c/verified.json` 及同目录日志。
- 校准补强已随 a6a327e 覆盖部署并实际注册；**仍未完成**：真实实体/eSIM/外部切换、自动重新附着故障注入验收。
  不清一次性恢复预算，不自动切卡、打断健康会话或把 CI 当成实机验收。

### 0.3 用户新增要求（以本节为准）

1. 通过原 Cloudflare Tunnel 连接设备，修复“定时任务里的定时拨打电话立即报 failed”。
2. 用户提供了拨号目标，已规范化为 E.164，**只保存在本地**：
   `.local/evidence/automation-dial/requested-task.json`（公有文档仅记 `+86 …3423`）。不要把真实号码写入源码或提交 Git。
3. 用户明确允许解除设备当前严格 VoWiFi-only，并希望准入为：
   **已注册 VoWiFi，或明确已驻网且非漫游的蜂窝接入**；未知/漫游不能当成非漫游。
4. 用户随后补充：**必须保留原开关，可手动关闭限制，强行允许漫游时接打电话**。
   不能把“禁止漫游”写死，也不能把旧配置默默迁移成新条件模式。
5. 语音新模式不顺带放宽短信限制。VoWiFi/非漫游实际资费仍取决于运营商，不能保证“必然当地资费”。
6. 用户曾要求先交接，随后明确由当前 AI 继续并每步更新本文；最后在确认测试未接来电后，要求文档写完即**阶段性结束**。未完成的后续事项单独保留，不为“全部完成”扩大验收范围。

**最新授权边界（16:15 UTC）：** 用户已明确要求现在实施并拨打目标号码，计划时间可由 AI 设置，以用户接到电话为成功。
当前选择安全默认：任务 disabled、持续 60 秒、只用立即触发执行一次；不自动循环重拨。已可执行必要候选部署，但仍先核对当前无通话和独立管理路径。
no-backup 规则继续有效；配置/数据库/历史证据/凭据不删除。此授权不扩展到换卡、重启基带/MM 或故障注入。

### 0.4 实际设备证据，不猜历史失败原因

2026-09-28 **11:51–11:56 UTC** 使用固定 SSH 公钥 pin 经 Cloudflare WebSocket/SSH 成功连接，登录既有应用账户。
没有重置密码、改设备配置、创建任务、拨号、发送短信、切卡、重启主服务/MM 或部署。
只执行诊断脚本及 GET API；需注意旧的 calls GET 实现可能清理已结束的 MM call 对象，不应宣传为绝对零副作用接口。

- 主进程 PID `511308`，运行中 `/proc/511308/exe` SHA-256：
  `3ae12007b981bbeb0efca220365b8701f391ab16009346fa7ad457f072900e4d`，匹配 **dd8ba1f ARM64**。
  MM PID `410`，secondary PID `283`；采样期间稳定。
- 同物理线路 IMS：`registered / ipsec`、`derived_3gpp_lte_46011`；续期计数 **4**，最近一次 `11:14:31 UTC`。
- calls API：**空列表**。Voice service 为 `unknown / ims_voice_service_route_missing`；IMS 已注册不等于语音能力实机已通过。
- 配置：`vowifi.enabled=false`；`trunk.enabled=false` 但 **`trunk.vowifi_only=true`**；
  语音层 VoWiFi/Cellular IMS 均启用，data disabled、airplane disabled。
- **automation.enabled=true，但 tasks=[]；dial_call 日志 API 为空。**
  journal 只找到 `09:32:42 UTC` 的 `Triggering automation task: call (...)`，没有完整失败因果链。
  不能证明任务由谁删除，也不能说历史那一次已精确复现。

本地证据：
- `.local/evidence/automation-dial/connection.log`
- `.local/evidence/automation-dial/readonly.json`、`readonly-console.log`
- 新的诊断入口 `.local/active/ims/inspect_automation_dial.py`（仅本地、不随 Git）
- 原连接入口 `.local/active/ims/connect_readonly.py`；凭据 bundle 和公钥路径仍由它引用，不复制凭据。
- 设备无 `python3`；诊断在本地 WSL Python 环境运行，通过 SSH 下发已审阅命令/localhost curl。

### 0.5 已确认的代码问题及一次重要更正

**确定缺陷：**
- `automation/tasks/dial_call.rs` 用 anyhow context 包住底层拨号/挂机错误，`scheduler.rs` 却用 `{}` 格式化，
  导致数据库和通知只剩“执行失败: 定时拨号失败”，丢失 `voice_vowifi_only_required` 等真正原因。
- `AutomationCenter.tsx` 的 `updateConfig` 吞掉保存错误，外层仍提示添加/编辑/删除成功，Dialog 会关闭。
  同一函数还会把 scheduler 的全局 enabled 强制设为 true。
- 自动化 target 的保存校验接受前后空白，执行查表不 trim，而普通电话入口会 trim，造成不一致。

**先前口述已更正：** 一度将“关闭 Trunk 后仍限制本地拨号”称为路由 bug。随后完整核对
[IMS 注册与资费保护](IMS_REGISTRATION_POLICY.md)，确认这是此前刻意覆盖 API/自动化的资费门禁，不能未经授权删除。
用户现在明确批准新增条件模式并保留原开关，因此应实现兼容的新选项，而不是直接绕过原门禁或改默认值。
当前配置能解释为什么普通/定时拨号会被阻断，但历史失败日志缺失，**不是根因已实机复现的证据**。

### 0.6 在途设计及 12:38 文件清单（后续差异以 §0.0 / Git 为准）

拟兼容字段：`TrunkProfileConfig.allow_home_cellular_calls`，serde 默认 false。

| 原 `vowifi_only` | 新选项 | 期望语音行为 |
|---|---|---|
| false | 任意 | 保留原无限制模式；可手动允许漫游接打电话，仍遵守线路/无线/语音启用等既有约束 |
| true | false/缺失 | 保留严格仅 VoWiFi，不因为升级放开蜂窝 |
| true | true | VoWiFi 或经新鲜证据确认非漫游的蜂窝；未知失败关闭 |

短信继续只读原 `vowifi_only` 等短信策略，不因新语音选项改变。字段所在 Trunk 配置仍涵盖现有网关/API/自动化入口。

**这批修改还没有提交，不能直接部署：**

| 文件 | 在途内容 |
|---|---|
| `.github/workflows/{beta-validation,build-release}.yml` | 加入 automation scheduler/target/dial_call Rust 测试过滤器 |
| `.github/scripts/test_automation_dial_boundary.py`（新文件） | 接线/门禁静态守卫，目前有一项需适配新 helper |
| `backend/src/services/automation/scheduler.rs` | `task_outcome` 为 dial 保留错误链、屏蔽目标号码、长度/控制字符限制，修正 timeout 秒数，明确失败日志；新增测试 |
| `backend/src/services/automation/target.rs` | `canonical_line_id` 及空白规范化测试 |
| `backend/src/api/handlers.rs` | `cellular_call_cost_rule`、`same_voice_binding`、`admit_cellular_call_cost`；初始/当前策略、绑定与 generation 校验；拨号/接听走 checked adapter；保留 IMS + modem 错误 |
| `backend/src/hardware/cellular/{control,modem_manager}.rs` | `make_call_on_modem_checked` / `answer_call_on_modem_checked`；等待串行锁后、ATD/ATA/MM Start/Accept 前重查调用方授权；旧函数兼容 wrapper |
| `backend/src/hardware/cellular/observations.rs` | 新 `registered_home_voice` 只读 hook，默认 unsupported，不猜 native 能力 |
| `backend/src/hardware/cellular/mm_observations.rs` | unique owner、无缓存 GetAll、两次 SIM/ICCID/端口/slot/驻网观测；MM slot 0/缺失归一 1，home SMS-only 不准入 |
| `backend/src/platform/config.rs` | 新字段默认 false；尚需补序列化/配置/API 回归 |
| `backend/src/services/line_registry.rs` | binding 改为 Arc；注入弱耦合只读 home observer，观测前后比较绑定/generation；同步两个语音限制字段 |
| `backend/src/services/trunk/access_router.rs` | `VoiceCostGate`；初始/当前策略交集；`CostCheckedAction` + JoinSet 异步准入，避免阻塞主路由而吞掉 Cancel/Hangup；按 ticket 拒绝过期结果；新增 5 个 home/cancel 测试 |
| `backend/src/services/trunk/operator.rs` | `incoming_auto_answer_allowed`，条件模式不能未经 home 检查先自动发 200 |
| `frontend/src/api/contracts.ts` | 新 optional 字段，兼容旧 API |
| `frontend/src/pages/sim/TrunkProfileDialog.tsx` | 保留原开关，新增允许非漫游蜂窝语音的子开关和资费说明 |
| `frontend/src/pages/AutomationCenter.tsx` | 保存失败向 Dialog 传播；成功后才更新状态/关闭删除框；不强行开启全局 scheduler |
| `frontend/src/utils/automationConfig.ts`（新文件） | 可测试的持久化响应检查 |
| `frontend/tests/automationConfig.test.ts`（新文件）、`frontend/package.json` | 3 项保存/错误传播单元测试接入 unit 脚本 |

共 **17 个 tracked 功能文件修改 + 3 个新功能文件**，另有本次交接文档修改。以实际 Git 为准。
`.local/active/ims/inspect_automation_dial.py` 和证据目录受 ignore 保护；普通远端 clone 不包含这些材料。

### 0.7 12:38 暂停时的历史验证结果（较新结果见 §0.0）

| 检查 | 最新事实 |
|---|---|
| Python 全量 | **192 项，1 failure**（不是全部通过） |
| 失败测试 | `test_call_cost_and_radio_guards_are_not_removed_for_automation`：它只截取 start helper，期待内联 `voice_vowifi_only_required`；现在该错误在 `cellular_call_cost_rule` 中，测试要验证 helper 接线，不能删除门禁断言敷衍通过 |
| 前端 unit | **11/11 通过**，含 3 项新增保存反馈测试 |
| 前端 type-check | `tsc -b --noEmit` 通过 |
| 前端 lint | 全量 ESLint 通过；早期 7 条 unit 测试 lint 错误已修正并重跑 |
| Rust 定向格式 | `rustfmt --check` **exit 1**，有格式 diff，未观察到语法解析报错；不等于编译通过 |
| Rust 编译 / 单元 / D-Bus | **这批语音修改未执行**，仍必须在 Actions；禁止本地 cargo 编译测试 |
| `git diff --check` | 通过 |
| GitHub / 部署 | 语音工作树未提交、未推送、未部署、未真实拨号，设备目标号码和资费配置也尚未改动 |

最新日志在 `.local/evidence/automation-dial/`：
`handoff-python.log`、`handoff-frontend-unit.log`、`handoff-type-check.log`、`handoff-lint.log`、`handoff-rustfmt.log`。
早先该目录 `python.log` 的 192 全通过只覆盖新增条件准入之前的中间版本，不得覆盖以上较新失败结果。

### 0.8 12:38 审查清单（后续修复见 §0.0，仍不能省略 CI/实机验收）

1. **编译与接口完整性**：新增 async closure / JoinSet 推断、Send/lifetime、zbus Value/ObjectPath 转换、
   `TrunkProfileConfig` 构造点/JSON 默认/前端 roundtrip 均需确认；当前只有 rustfmt 解析及前端检查，未有 Rust 编译事实。
2. **异步取消边界**：最初 `run_router` 在主 select 分支 await home 查询会让取消晚于拨号。
   已改为 pending action + JoinSet/ticket，但尚未测试。复核 pending 同 call_id 重复请求、终止/取消/事件交错、
   API receiver 关闭、任务 panic、路由删除、历史初始策略是否丢失及队列上限。已添加 5 项 Rust 测试但均未运行。
3. **新鲜证据和重绑**：observer 核对 old/new binding/generation，checked CS helper 固定 expected binding；
   仍需 fake-D-Bus 证明 unknown/roaming、owner/SIM/slot/端口变化、串行锁等待后换代不会放行。
   特别复核两个采样结束后至真正拨号的间隙，不把一次 bool 当永久许可。
4. **注册 VoWiFi 应可用**：当前 home 条件模式可能也为本来只需 VoWiFi 的请求执行最多约 800ms 的 home 查询；
   查询失败应只剔除蜂窝、不影响已注册 VoWiFi，不能因慢观测让健康 VoWiFi 失效。
5. **入向与自动接听**：用户要求保留漫游接打的手动开关。当前条件模式让蜂窝来电进入 router 后检查，
   在核验前抑制底层 auto-answer，再对 `AcceptCall` 重新检查。需确认 `BoundImmediate`、API answer 的反馈、
   旧已接通通话控制、已发送 200 的竞争；不能既拒绝网络又把 UI 标成“已接通”。
6. **呼叫生命周期**：原自动化“start accepted + sleep + hangup”并不证明接通；异步 IMS 拒绝也可能最终显示 success。
   scheduler timeout 取消时是否确实挂断自有那通电话、不能误挂别人的呼叫，仍是待审范围，未声称修好。
7. **原 modem 路径缺陷尚未改**：MM Voice `ListCalls` 清理失败会在 ATD 前返回；ATD 发出但观测不到通话后又走 MM
   fallback 可能造成歧义重拨。无现场错误不要盲目忽略 busy/cleanup 异常或自动重拨；如要修，必须补 mock 和取消测试。
8. **诊断隐私**：新 dial 错误链目前只屏蔽本任务目标号码、限制长度/控制字符，不保证可公开原始 provider 错误。
   审查日志/通知是否可能带 URL、认证或其他身份，不将未经脱敏的现场日志提交。
9. **配置/UI 错误反馈**：AutomationCenter 保存路径已补强；TrunkProfileDialog 的保存仍需核对 API 在 error envelope
   时是否会抛出，避免条件开关未保存却关闭 Dialog。前端中文错误映射对 `voice_registered_home_required` 等尚未补。
10. **严格保护兼容**：保持现有 VoWiFi-only Rust 测试，不把它们改成“允许蜂窝”来掩盖回归。
    新字段缺失等价旧严格行为；关闭原开关才是用户主动允许漫游。短信限制、P-CSCF/IMS 兜底、native 后端边界均不放宽。

### 0.9 推荐执行顺序与未完成任务

- [x] 原校准补强提交 `dc2355c`，两套 CI、14 项新增测试和双架构制品核验。
- [x] 本次经 Tunnel 连接并核实版本、健康 IMS、空任务列表和语音限制配置。
- [x] 代码阶段审查及本地检查完成；Python 192 全通过、定向 rustfmt/diff、前端 11 unit/type-check/lint 通过。
- [x] 新资费兼容、fresh-home private-D-Bus、取消/回退/接听/mock 和前端反馈回归进入 CI；最新 SHA 两套实际通过。
- [x] 已推送 `a6a327e`；最新完整 SHA 的两套 CI、28 新测试与双架构制品核验完成，旧 Release 未覆盖。
- [x] 用户明确要求现在执行后，已覆盖部署 a6a327e；不建备份，配置/数据库保留；MM/secondary未重启。这不等于切卡验收。
- [x] home/IMS/无通话预检通过，新条件模式已设置并读回验证；其他配置与短信策略未变，原无限制开关保留。
- [x] 已按授权创建目标号码60秒任务（默认disabled、固定04:00），立即触发一次；没有循环或重复触发。
- [x] 用户确认北京时间00:32未接来电，按“收到电话即成功”的约定完成**来电送达验收**；未接听，音频/接通后时长未验收。
- [x] **自定义拨号结果分类代码/CI完成**：对端未接听/非本端原因超时的受限成功分类，见 §0.11–§0.12；候选7c6cf86尚未部署。
- [ ] 确认维护影响后部署7c6cf86。无新测试通话授权时不重复拨号，不改写旧历史。
- [ ] 原 MM 换卡和自动重新附着故障注入依旧单独待验收。

本地 shell 是 WSL，文件工具是 Windows 路径；无 `node` 但有 **`node.exe` v24.14.1**，前端命令可用：

```sh
cd frontend
node.exe --experimental-strip-types --test tests/imsRegistrationPolicy.test.ts tests/cellularImsErrorFormat.test.ts tests/automationConfig.test.ts
node.exe node_modules/typescript/bin/tsc -b --noEmit
node.exe node_modules/eslint/bin/eslint.js . --max-warnings 0
```

Git 仅推 `simmaster`。WSL 直接用 `.git/codex_push_key` 会因 Windows 文件权限映射 0666 被 SSH 拒绝；
本轮已验证 Windows **`git.exe`** 配合既有 `core.sshCommand` / 固定主机校验可推送。
不要复制私钥到公共目录、修改 key 为开放权限、force-push 或重置/丢弃在途工作树。

### 0.10 后续重新开启任务时可复制 Prompt（替代历史 §9）

```text
请接手当前 SimAdmin。上阶段已经按用户要求结束，不重放部署或拨号。先完整读 docs/HANDOFF.md、docs/NEXT_AI_HANDOFF_2026-09-28.md 的最新 §0、
docs/IMS_MM_SIM_BINDING_CALIBRATION.md、docs/IMS_REGISTRATION_POLICY.md，然后核对实际 Git/diff。

当前剩余工作：用户已明确设备/eSIM离线，先等待上线确认；之后明确目标和维护窗口，部署最新UI候选33d16f3并验收直接eSIM切换。
MM真实换卡/恢复故障注入独立授权，不清恢复预算。LAN目标已重装7c6cf86，不重放旧PID470/deleted DB恢复脚本。
用户要求保留原限制开关，关闭时仍可强制允许漫游接打电话；旧严格模式不改默认，短信保护不变。
目标号码在 .local/evidence/automation-dial/requested-task.json，不写入公有文档/源码。
用户后来已明确授权现在拨号、时间表可自行设置。任务task-voice-acceptance-a6a327e已创建（disabled、60秒），
16:31:57 UTC立即触发过一次，用户已确认北京时间00:32未接来电，按收到电话即成功的要求验收通过；
设备终局408/Q85031、未接听，16:34 calls为空。禁止把这次操作重放为新测试。

版本：Release仍16998ae；校准基线dc2355c已随a6a327e部署，a6a327e实际IMS注册且测试来电已送达。
最新结果分类候选7c6cf86已通过两套CI、18新回归和双架构核验，但尚未部署。设备最后只读证据为a6a327e，不把候选当在机版本。
语音候选的最新工作树/提交/CI状态以本文§0.0为准，不要把历史17+3文件清单当当前清单。
最新本地195 Python、13前端unit/type-check/lint及定向rustfmt已通过；098ae04编译失败已在7c6cf86修复，记录保留；
语音候选a6a327e的两套Actions、28新增Rust/mock/private-D-Bus回归和双架构已验证（见§0.0）；
16:22 UTC已覆盖部署；16:52只读再次验证运行中hash、IMS已注册IPsec、home、无last_error、无通话，
该版本16:27:54新注册成功，续期计数0。不要重开代码步骤或重复部署，也不混用旧版本续期证据。

§0.8是之前的审查清单，已实施补强与CI结果见§0.0；不必再次询问已确认的来电送达。
§0.11结果分类已随7c6cf86完成代码与两套CI/18新测试/双架构核验，详见§0.12；尚未部署，不一律把408成功化。
7c6cf86已按用户指定LAN目标重装；后续UI候选33d16f3已通过三套workflow/双架构，未部署，详§0.14。
本地Pixel/iOS/IPCC对比报告已完成，不宣称历史唯一根因；用户手机测试结果出来、设备上线后再确定现场A/B。不要重复做已通过的代码任务。
条件模式已为vowifi_only=true且allow_home_cellular_calls=true，
原开关false仍表示用户主动解除限制；短信保护不变。新一次真实呼叫先确认，不能重放已提交的立即执行请求。
保留原IPv4v6→IPv6→IPv4、P-CSCF归属和恢复预算，不扩大native、切卡、故障注入或自动重拨范围。
部署须确认维护影响，直接覆盖不建备份但保留数据；配置号码/执行任务前确认时间、持续秒数和测试授权。
请先报告核实后的完成/未完成和下一步，再继续。
```

### 0.11 拨号任务成功与对端接听结果（代码/CI完成，尚未部署）

用户在确认00:32未接电话后，明确要求将以下事项加入**之后要完成的任务列表**：

> 自定义拨打电话时，对方不接听、超时等不是拨打方的问题，不要记录为 failed，算成功。

- [x] **定义并实现任务结果分类**：已正常发起、具有送达/对端处理证据，之后对方未接听或因对端原因超时，应将**拨号任务**记为 success，附“对方未接听／对端超时”等说明。
- [x] **保留真实通话事实**：原 SIP 状态、Q.850、answered=false、实际持续时长仍如实保存；任务success不等于已接通、音频验证通过或满足通话时长。
- [x] **本端故障仍失败**：参数错误、准入/费用保护拒绝、无可用接入、发送失败、本地资源或所有权错误，不能被转换为成功。
- [x] **不将所有408/临时响应一律视作成功**：区分有送达证据的远端未应答与本地/网络传输超时；仅100/183、命令accepted或裸408不足以通用证明对方终端收到。具体证据与分类已落实为内部来源/事务标记和按接入尝试的独立观察状态，详见 §0.12。
- [x] **统一日志、通知与界面**：调度器数据库记录、AutomationEvent通知、任务卡片/历史保持同一分类，显示任务结果与通话结果两个维度，避免“未接”继续被误报成“拨打功能故障”。
- [x] **补回归并验证**：对端未接、对端超时、本端发送前失败、无应答且证据不明、手动取消、状态重复/迟到分别覆盖；Rust仍仅Actions。

已部署基线 a6a327e 的 `automation/tasks/dial_call.rs::observe_call` 对 `Rejected` / 提前 `Ended` 返回错误，
`scheduler.rs::task_outcome` 将该错误记为 failed；此处为当时登记的需求，**后继实施状态以 §0.12 为准**。
后继实现入口为这两个位置、`dial_outcome.rs` 及内部结构化观察事件，不是仅在 UI 把红色改成绿色。
没有修改此次已存在的 failed/408历史，也没有为此再次拨号；新真实测试须另行确认。

### 0.12 剩余任务续接（2026-09-29，最新步骤）

- 用户要求继续剩余 todos。**步骤 F1：拨号任务结果分类代码与本地检查已完成，Rust 执行待 CI**（04:21 UTC）。使用 `AutomationExecutionReport` / `DialReport` 区分任务成功与接听结果，不改写原 call history 或历史失败日志。
  - 当前接入实际初始INVITE的180振铃，随后远端408/480且无策略/资源诊断冲突 → success/对方未接；远端486/600忙、603拒接，以及Q.850=19的明确无应答亦有受限成功分支。
  - 仅命令accepted、100/183、裸408、无证据计时结束、本端合成486/408、re-INVITE失败、资源/媒体/费用拒绝、观察丢失、取消或清理失败仍failed。
  - 新 observation-only 通道区分真实初始INVITE接听与早期RTP/IP-leg Answered；回退前先清掉旧振铃证据，转发丢帧/接入失效/本地帧处理错误，手动挂机可被任务观察，不改变现有SIP桥接或call history事件流。
  - 任务成功详情保留SIP/Q.850、ringing/answered观察值与固定原因说明；成功卡片显示具体结果，不再只写“上次成功”。配置标签明确为“拨号观察时间”，不是接听后的通话时长。
  - 本地 **195项Python、13项前端unit、type-check、lint、定向rustfmt及diff检查通过**；证据 `.local/evidence/dial-outcomes/{python,frontend-unit,types,lint}.log`。未本地运行cargo。
- **步骤 F2：提交与编译问题修复已完成**：首版 **`098ae04ae1fa900ccd82293c74c50add56065416`** 已推送 `simmaster/master`，失败记录保留如下。
  **首轮失败事实（04:30 UTC）**：Build `36521722228`、Validate `36521722274` 的Rust编译失败（E0004），`handlers.rs` 旧通话历史监听器未穷尽新增 `AttemptChanged/Observation`；Frontend `36521722221` success。
  已定位并补显式忽略分支（元数据不修改历史），后继 **`7c6cf8635971f738a8af3a79a01f50ba6af4e641`** 已推送。
  **步骤 F3：新SHA的完整验证已完成（04:44 UTC）**。最终候选 **`7c6cf8635971f738a8af3a79a01f50ba6af4e641`**：
  [Validate `36522353436`](https://github.com/autisticryptic/SimMaster/actions/runs/36522353436)（Rust job `109257774963`）、
  [Build `36522353454`](https://github.com/autisticryptic/SimMaster/actions/runs/36522353454)（Rust `109258052658`、ARM64 `109258052556`、AMD64 `109258052622`）全部success。
  两套日志均实际下载、核对官方artifact SHA-256，**18项新增Rust/mock回归 + 4项重点兼容检查逐项为ok**，包含蜂窝/VoWiFi双会话生产入口、取消清理、旧资费保护。
  两架构包均已下载校验 metadata `1.1.5 / 7c6cf86`、ELF、二进制与前端摘要：ARM64 artifact `11013264211`，包SHA-256 `d62301fb11889d9f7bad8a21a0bd38340170295bbf4e2a7c30f3580c71a432e9`；
  AMD64 artifact `11013655082`，包SHA-256 `6688501e2f4b19d9313373e6fd4890af7c3d377ee2443661fc09aa33a7489a45`。Publish Release按push门禁skipped。
  完整证据 `.local/evidence/dial-outcomes/7c6cf86/verified.json`、`jobs-*`、`tests-*`；核验脚本同父目录 `verify_ci.py`。首轮失败日志和artifact摘要保留在 `098ae04/`。
  04:53 UTC再次核实 Release `397300521 / v1.1.5` 仍对应16998ae，未覆盖/移tag，证据 `release-unchanged.json`。
  **没有部署、没有重复拨号或改写旧失败记录**。设备最后核验仍为a6a327e；新“对端未接success”语义仅在7c6cf86候选，需确认维护影响后覆盖部署方可生效。
- **步骤 F4：剩余实机任务的维护前置／待确认**：
  1. 确认允许中断IMS注册的维护窗口；部署新候选只重启主服务，先核实无通话、管理走Wi-Fi、版本/架构/摘要、任务保持disabled，覆盖且不建备份。
  2. 由用户提供可切换的实体SIM或明确允许的eSIM目标，记录当前/目标身份的私有指纹及回切方案；未指定时不随机切卡。
  3. 测试稳定物理line_id下身份变化、准入暂停与稳定后重新注册；核实原普通PDP不改、旧会话不操作新资源。硬件无关CI不能替代此项。
  4. 自动重新附着故障注入需单独授权且**有未消费预算**，先确认恢复计划/唯一自有上下文及无其他线路冲突。预算已消费就报告不满足前置，不清文件、不重启MM来制造新预算。
  5. 测试后回切并只读确认IMS、数据/任务配置、无遗留活动通话/未知receipt；没有完成实测前不勾选实机验收。
  当前只完成以上执行前置整理，未切卡、未注入故障。
- 原换卡／自动恢复故障注入验收保持待维护窗口。已询问用户窗口及换卡方式，未切卡、未重启 MM/基带、未清预算。
- 本次不重复拨打上次测试电话。每完成实现／本地验证／CI步骤都更新本文。部署目标与在机版本仍分开记录。
- 基线 `b3f18aa`（docs），代码/部署基线 `a6a327e`；此续接开始时工作区干净。

### 0.13 新局域网目标部署及外置数据库对比（2026-09-29 06:43 UTC）

用户新增顺序要求：**先将最新已验证构建直接替换到指定局域网设备，再研究本地 Pixel / iOS / IPCC 外置数据库差异**。
目标地址/SSH凭据仅保存在本机私有材料，不复制到公有交接；入口索引 `.local/evidence/lan-deploy/target-reference.json`，
凭据实际在仓库外 `/root/.codex/private-handoffs/simadmin/LAN_DEPLOY_2026-09-29.json`（0600）。不要回显或提交内容。
本次是新目标，**不要套用旧 Cloudflare 实验机的 PID/安装状态/已部署结论**。

- [x] **G1：只读连接与身份核实完成**。直接LAN SSH公钥匹配本机已有known_hosts，不是自动信任新主机；root登录成功。
  系统Linux/aarch64，管理连接经wlan0；目标服务PID470，MM516、secondary339。最新可用制品仍为已验证7c6cf86 ARM64，不使用旧Release代替。
- [x] **G2：部署前数据保全已完成（用户授权重新安装）**。此前发现：`/opt/simadmin` 不存在；
  `/proc/470/exe` 指向 `/opt/simadmin/simadmin (deleted)`，cwd也标记deleted；fd10仍持有 `/opt/simadmin/data.db (deleted)`。
  普通SQLite通过/proc别名打开失败，因此使用标准rollback-journal共享读锁读取已打开inode，再仅在内存反序列化；
  **数据库约5.9MB，header为rollback模式，quick_check=ok**，可见auth_config/auth_sessions、config_documents、
  config_line_profiles、通知/任务/短信/通话等表，尚有保全数据的机会。
  用户随后明确 **“直接重新安装”**，已授权使用默认全局配置重建；不是擅自恢复为空数据库。
  **恢复过程**：SSH侧先打开并持有deleted DB句柄，按SQLite共享读锁保护一致性，将唯一正式恢复文件写入 `/opt/simadmin/data.db`；保持锁直到旧服务停止。
  恢复前后对所有表计算行数/内容指纹，全部相同，quick_check=ok，原账号、线路、通知、任务和历史保留。旧全局config.yaml不可恢复，按授权重新生成默认值；没有重置Web密码。
  未保留旧程序/数据库副本，未创建备份。停服前确认任务数0、未结束通话记录0、MM已有debug配置，避免启用程序时隐式重启MM。
- [x] **G3：最新制品重装与只读验收完成（07:14:48 UTC）**：LAN目标运行 **1.1.5 / 7c6cf86 / PID54742**，运行与安装SHA-256均为 **`2051c398b58c28ffeea396240b7b29614bac67267807bd123963d7c37d12a829`**，匹配已验证ARM64制品。
  服务active/running、NRestarts=0、cwd恢复/opt/simadmin；www校验通过、网页HTTP200、数据库quick_check=ok、原Web账号仍存在。
  MM516 / secondary339未重启；维护时临时暂停的恢复timer已恢复active。原数据库线路配置保留：蜂窝IMS和VoWiFi均enabled、trunk/data关闭；没有改为旧Cloudflare设备的单蜂窝配置。
  **未登录受保护API，不能据此宣称此LAN目标IMS/通话验收通过**。部署验证证据 `lan-deploy/reinstall-result.json`、`installed-verified.json`；脚本 `.local/active/lan/reinstall.py` 仅供审阅，不能重放（旧PID/FD前置已不成立）。
- [x] **G4：外置数据库离线静态对比完成**（2026-09-29 08:40 UTC）。报告见 [Pixel/iOS/IPCC对比](IMS_CATALOG_PIXEL_IOS_COMPARISON_2026-09-29.md)。
  已核实六份v7库来源/SHA；三组新旧profile_id/config_json内容相同（非整文件相同）。Pixel无显式Contact表，历史空表漏MMTEL的2b743e2修复已包含当前版本，不能再次当作现存必然原因。
  已记录有效profile回退差异，以及UA template、安全策略路径、access-specific SIP、Contact overlay解释等可验证覆盖缺口；**没有认定用户历史来电的唯一根因**，未改库或设备，源库哈希未变。现场对照等用户手机测试结果/上线授权。

本地证据：`.local/evidence/lan-deploy/{host-check,preflight-initial,layout,db-readonly,db-snapshot-readonly,web-access-check}.json`；
只读传输助手 `.local/active/lan/device.py`。注意初始baseline的readlink失败令exe字段与下一行粘连，安装状态以独立 `layout.json` 为准，不机械读取错误字段。

**后续数据库问题的需求记录：**

用户报告通过旁边carrier_Bundles生成的外置库，Pixel提取库可注册IMS，但呼入直接转语音信箱、设备没有IMS来电通知；
iOS/IPCC提取配置则可以注册并正常接打。当前派生配置以iOS提取模式为最初模板。可能较旧的对照数据库已下载本地。
部署完成后先确认文件来源、schema、版本和同运营商/同作用域条目，再比较REGISTER身份/能力声明、路由、接入、安全、
语音服务配置以及程序导入/有效配置映射。不以“注册成功”证明语音可达，也不先断定某个字段就是根因。
只读分析，不在未授权时自动切换生产配置、拨号或重放旧验收；不要提交真实SIM身份、凭据或完整私有原始库。

### 0.14 用户新增UI待办与设备离线边界（2026-09-29 08:24 UTC）

用户明确要求改回以下界面行为，随后要求继续完成。**三项代码、本地测试与整批Actions已完成，证据见本节末尾；尚未部署，不能宣称实机切换已验证。**

- [x] **概述 → 线路控制 → 飞行模式（代码/定向检查完成）**：大段说明已改为单行短状态，与其他控件布局一致；未知射频仍显示未知，不把保存意图当已生效。保留原开关请求/禁用条件和错误反馈。新增前端单测，type-check及定向lint通过；整批UI的完整CI已通过（见H2），未实机操作。证据 `.local/evidence/ui-offline/flight-*`。
- [x] **IMS与Trunk页面 → 移除IMS注册模式选择（代码/本地/整批CI完成）**：已移除模式选择组件，保留VoWiFi与蜂窝IMS各自的启用开关；旧保存模式在启动时仅迁移此字段为自动，原开关/资费不变，兼容API明确拒绝旧手动模式。定向前端unit/type-check/lint与Python通过，新增核心/配置/HTTP回归已在Actions通过。
  **用户补充前提：只有两个开关都开启，才允许尝试双注册；两路实际注册成功且网络双注册协商通过，才显示/保留双注册成功。**
  只开启一项时仅使用该项，不擅自开启或尝试另一项；两项都开启但双注册不成立时，仅在已启用、可注册的接入中按 **VoWiFi → 4G/5G IMS** 回退，均不可用如实报告。
  实现时同步审查后端默认和已保存旧选择，不能只隐藏UI却仍运行矛盾模式；不绕过网络协商、不强制不被允许的第二路注册，不取消原会话/通话与语音、短信资费保护。
- [x] **eSIM管理 → 自动检测后的profile直接切换（代码/本地检查完成）**：列表已提供“切换”按钮/确认框，复用现有profile enable和恢复进度接口，不要求跳转完整管理。一次POST后有界读取进度与新鲜profile，只有目标实际enabled才显示确认；不乐观改状态、不自动重发。
  已启用、PPR禁止停用、未知状态、离线、加载中和忙碌条目正确禁用；线路切换隔离迟到结果，切换期间不并发自动加载覆盖新状态。新增helper回归，前端unit/type-check/lint通过，整批CI已通过；没有真实切卡。

**最新操作约束：** 用户说本机设备和eSIM卡暂时下线，正在用手机测试哪些组合能正确注册IMS，测试后再安排修复。
因此停止对已知设备的SSH/API连接、轮询、部署、实际切卡和电话/SMS操作，直至用户明确确认上线与当次操作范围。
允许继续本地文件/数据库只读对比和代码审查；手机测试结果与本项目实机结果分开记录，不据此直接宣布项目已通过。
此离线要求优先于上文已经执行完毕的一次性部署/拨号授权，不重放旧脚本。

**H1：UI整批本地验证（2026-09-29 10:19 UTC）**：199 Python、21前端unit、TypeScript检查和全量lint通过；定向rustfmt通过。
新增检查覆盖飞行模式短状态、双开关/双成功/协商的显示门槛、eSIM一次POST/忙碌/失效线路/失败与读回确认；后端补旧模式迁移/HTTP契约/自动回退回归，Rust尚待Actions。
初次并行本地检查超时，Python已顺序重跑通过；本地产物Vite构建停于transforming并超时，**未记为成功**，将在Actions验证正式构建。
证据 `.local/evidence/ui-offline/`。本次没有设备连接，也未修改相邻carrier_Bundles源码。

**H2：整批候选及Actions已验证完成（10:37 UTC）**：`33d16f3d78ea0456682a2469810625efe29805ec` 已推送 `simmaster/master`。
[Validate `36555415147`](https://github.com/autisticryptic/SimMaster/actions/runs/36555415147)（Rust job `109363266803`）、
[Build `36555415060`](https://github.com/autisticryptic/SimMaster/actions/runs/36555415060)（Rust job `109363830306`、ARM64 job `109363830133`、AMD64 job `109363830098`）、
[Frontend `36555414932`](https://github.com/autisticryptic/SimMaster/actions/runs/36555414932) 全部success。
两套Rust日志下载后逐项核实 **3项新增迁移/启用前提/回退测试 + 6项重点兼容回归**为ok，含实际私有D-Bus HTTP模式接口测试；前端正式构建已在Actions通过，弥补本地限时构建未完成的验证缺口。
ARM64与AMD64包实际下载核验官方artifact digest、meta版本/commit/架构、ELF与二进制/前端校验；完整证据 `.local/evidence/ui-offline/33d16f3/verified.json`。
ARM64 artifact11027263175，包SHA-256 `b3c37cf12c106d66e15bce4f4645fff11c7caeba80ae42a6b726942ec84568f0`；AMD64 artifact11027742386，包SHA-256 `a676d39cb882654263725e956abe4ee2cdc0faf8e44cb4fcc5a6e823fe830307`。Publish Release按push门禁skipped。

**H3：待设备上线后的步骤**：用户明确设备离线，本轮未连接任何设备，也没有部署33d16f3或实测eSIM切换。
恢复上线后先确认目标/维护窗口、无通话和管理路径，再部署对应候选并验收UI切换；MM实体/eSIM换卡与自动重新附着故障注入仍单独待确认，不清预算。
历史文档 `docs/archive/2026-09/ESIM_IMS_PROFILE_TEST_2026-09-01.md` 有非本轮产生的未提交修改，未改动、未夹带提交；不要重置它。

本地库已定位：根目录三份2026-08-08 schema-v7库，以及 `.local/archive/root/.codex-cf-catalogs/` 中2026-08-19的四库（包括iOS 26.6.1）。
离线静态对比报告已完成，见 [数据库语音差异分析](IMS_CATALOG_PIXEL_IOS_COMPARISON_2026-09-29.md)；**尚不能认定用户历史呼入问题的唯一实机根因**。证据保存在 `.local/evidence/carrier-voice-compare/`。
旧的Pixel redfin/schema-v5文档不是当前Mustang/v7的直接解释；旧历史已明确当时SIM/版本/接入腿/有效profile未固定，不能盲目归因。

### 0.15 上线后UI部署与当前eSIM注册失败（2026-09-29）

- **I1 部署完成**（11:16:47 UTC）：用户明确设备上线并授权安装最新版本，按最近LAN目标固定host pin连接；预检旧7c6cf86、管理wlan0、无MM活动通话、无启用任务、数据库完整。
  已覆盖为 **33d16f3 / PID11064**，运行SHA-256 `cb2180653db7c937bd6e9695653b615c57d4e3bde9b72e511b0ee406180d5de4` 匹配验证ARM64；前端checksum/HTTP200、DB quick_check通过。
  无备份，复制前后配置/数据库哈希不变；MM577/secondary343未重启，恢复timer已恢复active。两次暂存后预检因本地脚本未strip换行误拒绝，未停旧服务；修正后执行成功，记录保留。
  证据 `.local/evidence/lan-ui-deploy/{preflight,before-ims-evidence,deployment,installed-verified}.json`。
- **I2 当前eSIM注册流程修复／进行中**：用户手机测试此eSIM可注册IMS，但项目不成功。已加入todo；未确认手机测试是LTE/NR IMS还是VoWiFi，已询问，不需要原始SIM身份。
  部署前：仅蜂窝IMS开启，VoWiFi关闭、airplane/data关闭；MM State11/RegistrationState5（漫游），原三槽停在 `cellular_ims_runtime_ims_bearer_start_failed`，没有SIP注册日志。外置catalog未安装，槽位database→carrier_catalog→derived。
  先采集新程序具体承载错误与实际SIM/PLMN/端点关联再修，不以手机成功直接猜运营商特例；不固定IPv6、不清恢复预算、不放宽P-CSCF归属。
  **I3 具体故障与候选修复**：后台登录成功；当前SIM归属51502、服务网50212漫游，实际derived_3gpp_lte_51502，失败码 `qca410_primary_mm_data_interface_mismatch`。
  host bearer使用wwan0，MM已给IMS尝试返回IPv4 grant但随后被程序固定主网卡检查拒绝；尚未发SIP。修复为绑定MM实际返回net口，并同时校验MM Ports、相同remoteproc/BAM-DMUX sysfs拓扑及exclusive bearer归属，再持久化lease并配置/搬迁网络；不抢host bearer、不把profile-id等同CID。
  新Rust/mock/D-Bus回归已写，待Actions编译执行。当前本地202 Python中仅文档链接检查受用户未提交的ESIM文档移动影响；其余通过，提交时用候选Git树另验，不丢用户文件。
- 用户此次授权上线部署与诊断，**没有授权自动切换eSIM或真实呼叫**。原独立换卡/故障注入验收仍待明确窗口。
- 历史eSIM文档现被外部改为 `docs/ESIM_IMS_PROFILE_TEST_2026-09-01.md`（原archive路径删除、新根docs文件未跟踪），不是本轮修改；仅修正新位置的HANDOFF相对链接，其余内容保留、不夹带提交。该文件含用户身份材料，不原样推送。

## 1. 先看这三个结论

1. **SIM-06 之前的 IMS 注册故障已恢复，并有实际 IPsec 注册及两次自然续期证据。**
   对应设备程序是 `1.1.5 / dd8ba1f`，不是此次新增的校准代码。
2. **“eSIM/实体换卡后自动校准 CID 绑定”尚未完成验收。**
   首版已提交到 GitHub `master / 9a8fe72`；最新查询为 Validate 成功、Build-Release 仍在运行。
3. **当前工作区还有 5 个文件的未提交补强。** 它们修正首版审查发现的问题；本地 184 项 Python 测试通过，
   但这些未提交修改没有经过 Actions 的 Rust 编译/测试，也没有部署。不能只 checkout 远端就认为拿到了全部修改。

## 2. 用户想要什么、明确不想要什么

### 用户的需求

- 保留现有 IMS 派生配置和兜底架构，增强通用兼容能力，不写 SIM-06/电信专用逻辑。
- 在 eSIM 切换、实体 SIM 原位更换、MM 对象/控制端口变化后，重新核验当前 SIM 与 IMS 承载的关联。
- 避免“程序选中的 profile 与模块实际使用的上下文不对应”，也避免旧任务跨 SIM 继续使用或清理新资源。
- 本轮仅修 **ModemManager 路径**。native/直接 AT/QMI 控制迁移留到后续。

### 必须保留的边界

- 地址族顺序仍是 `IPv4v6 → IPv6 → IPv4`。不能为了单卡注册而固定 IPv6。
- 不覆盖 `ctlte`、`ctwap` 或其他已有普通数据 PDP 定义，不修改 Initial EPS、NV、USB 模式。
- 不把 MM profile-id、AT PDP CID、eSIM profile 和 SIM logical channel 当成同一个编号。
- 不放宽 P-CSCF 归属校验；仅有相同 APN 或 IPv6 前缀不足以证明可以借用另一个 CID 的地址。
- 校准本身不无条件执行 Disable/Enable、重启基带、重启 MM 或设备。
- 已有的一次性恢复预算不得清空，不能通过反复重试/重启应用制造重新附着循环。
- 不为验收自动切 eSIM、拔插卡、打电话、发短信或打断已注册会话。
- 用户此前授权修复、提交 GitHub及实验机覆盖部署，并要求**不生成备份**；这不授权删除历史诊断、凭据或数据库。
  新的换卡/故障注入验收应先与用户确认维护窗口。
- 已发布的 `v1.1.5 / 16998ae` 不覆盖、不移 tag；同为 1.1.5 的后继 Actions 制品必须按 commit 区分。

## 3. 已完成的工作

| 范围 | 已完成内容 | 证据/边界 |
|---|---|---|
| 历史接续 | 读取根目录两份 JSONL，纠正过期交接结论 | `2026-09-26T02-09-12-727.jsonl`、`2026-09-27T1.jsonl`；旧会话曾有无事实支持的口述，必须以 Git/CI/设备证据为准 |
| 版本及目录 | 统一到 1.1.5；旧重复工作区已整理；Release 1.1.5 已发布 | 历史完成，不必重做；见 HANDOFF |
| 安全协商 | Security-Server 列表解析及完整 Security-Verify 回传 | `dfda6cd`，历史 CI 已通过；不是单独证明 SIM-06 根因 |
| CID 创建 | 首选 CID 占用时，按设备能力选空闲项；防覆盖、读回、mmcli response 外壳适配 | `8df57a9`、`e0ade97`、`02dfdc5`，已随设备后继程序部署 |
| 一次性 MM 恢复 | 实际上下文核验、普通兜底耗尽后受控恢复、持久化预算；取消后安全补偿 | `7896e05`、`dd8ba1f`，CI/双架构制品/部署已核验 |
| SIM-06 实机 | IPsec 初始注册 + 两次自然续期，仍用 derived 配置 | 最近只读采样 09:49 UTC，详情见下节；不代表后续每一刻都在线 |
| 校准设计审查 | 定位 line_id 稳定但换卡不一定触发失效的问题，提出统一观测及绑定准入机制 | 已完成静态分析，不等于代码已验收 |
| 校准首版 | 已写入代码、增加测试，并推送 GitHub | `9a8fe72`；完整验收未完成 |

### 最新已验证的设备状态（不是校准候选的实测）

- 程序：`1.1.5 / dd8ba1f`，后端 `modemmanager`。
- `/proc/511308/exe` SHA-256：
  `3ae12007b981bbeb0efca220365b8701f391ab16009346fa7ad457f072900e4d`，与 ARM64 制品一致。
- 有效 profile：`derived_3gpp_lte_46011`，不是数据库特制 profile。
- 初始注册：07:54:27 UTC；自然续期：08:44:28、09:34:30 UTC；计数 2。
- 地址族配置顺序未改，实际 grant 是 IPv6；这不是固定 IPv6 的证据。
- 本轮接续没有再次部署、重新附着或切卡。
- 本地证据：`.local/session-review/verified-runtime.json`、`connection-result.txt`、`current-ci.json`，
  以及 `.local/evidence/sim06/deploy-dd8ba1f/`。

## 4. 当前 todos：哪些没做完

任务 ID 只在当前会话有效；其他 AI 应按下列语义重建，不依赖 ID。

- [x] #1 梳理两份历史会话及剩余任务。
- [x] #2 核验 `dd8ba1f` CI、制品/部署、SIM-06 注册与自然续期。
- [x] #3 更新旧故障收尾交接记录，已推送 `3b6b7dc`。
- [x] #5 核对 SIM 变化与 MM 承载绑定生命周期。
- [ ] **#6 完成 MM 换卡失效和承载绑定校准**：当前是首版 + 未提交补强，尚未完成 Rust/CI 验证、完整审查和实机验收。
- [ ] **#4 独立维护窗口下的自动重新附着故障分支验收**：既有恢复代码有 CI，但不能把正常注册或 `recovery_source=automatic` 当成重新附着分支已经触发。
- [x] #7 编写本接手说明及可复制 Prompt（文档完成，不代表 #6 完成）。

### 下一位 AI 的执行顺序

1. 核对 Git/diff，保留本地未提交补强；查询 `9a8fe72` 的两套 CI 最新结果。
2. 审查并完成补强，尤其是所有异步失败出口、旧任务发布/清理边界及 slot=0 语义。
3. 做本地 Python、格式和 diff 检查；补齐需要的 Rust/mock/D-Bus 回归。
4. 提交补强并推送，等待 **对应最新完整 SHA** 的 Actions Rust 测试及双架构构建成功。
5. 验证测试确实执行，不只看绿色 workflow 标题；记录 run/job 与新测试结果。
6. 更新交接文档。若要部署，核验最新制品 commit/架构/digest 和设备现状，不拿旧 Release 代替。
7. 与用户确认真实切卡/外部切换/故障注入窗口，再做实机验收；未做的保持未完成。

## 5. 自动校准方案的思路

### 为什么需要

物理 `line_id` 设计上保持稳定，这是正确的。但原来的库存循环主要比较 `present`。
同槽 ICCID A→B、modem 始终存在时，旧 IMS runtime 会被复用，不能仅靠“掉线/上线”发现换卡。
同时，直接控制硬件也可能有连接复用、固件缓存和上报时序问题，不能断言所有问题都来自 MM。

### 本次实现方向

1. **统一观测，不只修 eSIM 按钮**：line registry 比较当前 SIM 身份、SIM/MM 路径、端口和槽位。
2. **确认变化先失效**：关闭新的 IMS 准入并递增 generation；旧任务不能发布新 Registered/Degraded 状态。
3. **未知与确认变化分开**：空 ICCID/暂时不可读不抹掉最后身份，不反复 teardown；新准入先暂停。
4. **稳定后清理**：连续两轮一致后，带 revision ticket 调度清理；获得 bearer/connect/advance 锁后重新检查。
   任务过期或锁忙就退出，下一轮再评估，不堆积重连任务。
5. **只释放旧自有资源**：不向新 SIM 发旧 SIP 注销，不向可复用的 modem/CID 回写旧设置。
6. **重新走正常选择流程**：重读 PDP 定义，安全复用/选择空闲 profile，然后建立新的 MM bearer。
   不硬改“CID 3 必须映射 CID 1”，不固定地址族。
7. **核对实际承载**：MM adapter 捕获不缓存的 SIM 快照，核验 owner、端口、SIM object、ICCID、逻辑槽；
   IP/P-CSCF 读取前后继续验证，上层核对 retained bearer 与派生配置所用 SIM 一致。
8. **恢复不是必经步骤**：只有原 P-CSCF/profile/地址族兜底耗尽且满足保护条件，才交给既有一次性恢复逻辑。

更详细的设计、行为及验证范围见 [IMS_MM_SIM_BINDING_CALIBRATION.md](IMS_MM_SIM_BINDING_CALIBRATION.md)。
注意该文档描述当前实现意图，**不是宣告其已经通过全部验证**。

## 6. 代码落在哪里

| 文件 | 本轮作用 |
|---|---|
| `backend/src/connectivity/modems/ims/cellular_ims/mm_binding.rs` | 新增 MM 校准状态机：最后已知身份、未知观测、稳定计数、revision、维护 guard 状态 |
| `.../cellular_ims/runtime.rs` | 观测入口、generation 失效、准入检查、带代次的最终状态发布、eSIM 维护 guard |
| `backend/src/services/line_registry.rs` | 新 runtime 初始化及既有线路 reconcile 前提交 SIM 观测；absent 暂停准入 |
| `backend/src/main.rs` | 在 `present` 不变的快速跳过之前检查校准 ticket，调度校准任务 |
| `backend/src/api/handlers.rs` | eSIM enable 前清理旧 MM IMS；校准任务的锁/ticket/缓存失效/原策略重连；过时 absent 任务的一处保护 |
| `.../cellular_ims/live.rs` | 准入、派生 SIM 校验、旧注册结果阻断、换卡丢弃会话、失败和旧 CID 清理边界 |
| `.../cellular_ims/native_bearer.rs` | 转交 retained handle 的 MM SIM 核验；BindingChanged 终止旧地址族尝试 |
| `backend/src/hardware/devices/transport.rs` | 兼容性扩展：MM SIM 核验 hook、BindingChanged hint；不是 native 控制迁移 |
| `backend/src/hardware/devices/qcm410/primary_ims_lifecycle.rs` | MM owner/endpoint/SIM/slot 快照及 IP/P-CSCF 前后校验、隔离 D-Bus fake 回归 |
| `.../qcm410/primary_ims_session.rs`、`ims_bearer.rs` | Create/Connect/监视和调用方 SIM 核验的接线 |
| `.github/workflows/{build-release,beta-validation}.yml` | 新状态机测试进入两套工作流；原 isolated D-Bus suite 继续运行 |
| `.github/scripts/test_mm_binding_calibration.py` | 接线/安全边界静态守卫，不能替代 Rust 或实机测试 |

路径中的 `.../cellular_ims/` 是 `backend/src/connectivity/modems/ims/cellular_ims/`。

## 7. 必须保留的未提交补强

快照时 `HEAD = 9a8fe726c55669d789bfed9586153689a3b4f42f`。以下 5 个文件有未提交修改：

1. `.github/scripts/test_ims_fallback_boundary.py`
2. `.github/scripts/test_mm_binding_calibration.py`
3. `backend/src/connectivity/modems/ims/cellular_ims/live.rs`
4. `backend/src/hardware/devices/qcm410/primary_ims_lifecycle.rs`
5. `docs/IMS_MM_SIM_BINDING_CALIBRATION.md`

本次交接另新增本文并更新 `docs/HANDOFF.md`；以实际 `git status --short` 为准。

### 补强来自两项必须修复的审查意见

**A. 尚未发布的连接任务仍有旧 CID 清理出口。**

首版虽然保护了已发布会话及一次建 bearer 后的校验，但 worker/netdev/namespace 等后续异步步骤失败、
以及 `connect_inner` 最终失败时，仍可能恢复旧 reporting/profile。

当前未提交修改选择：**MM reporting/profile 视为持久配置，MM 停止及失败清理不再通过旧 modem/CID
选择器回写；资源交给 retained unique-owner provider handle 清理。** native 原选择器清理保留。
这比只加一次 generation 判断更保守。下一位 AI 必须复核：

- 所有失败/取消出口是否覆盖，是否仍有旧任务写入/清理新资源；
- provider-only cleanup 是否完整回收自己的网络状态、正确保留失败 receipt；
- MM 正常停止后保留 reporting 的行为是否与其持久 profile 设计一致；
- `!is_native_selector` 分支是否误影响仍在使用的其他 legacy MM provider。

**B. MM 单卡 `PrimarySimSlot=0` 与 inventory 逻辑槽 1 不一致。**

首版直接比较原始值，会误拒绝单卡模块。未提交修改改为成功读取 `GetAll` 后按 inventory 规则
将 0/缺失属性归一为 1；读取失败仍失败，并新增 slot=0 的隔离 D-Bus 测试。

以上是**已写出的补强，不是已经通过 Rust 编译/测试的补强**。

### 验证事实

- 当前工作树 Python：**184 项通过**，日志 `.local/evidence/mm-cid-calibration/handoff-python.log`。
- 当前工作树 `git diff --check` 通过。
- 部分 Rust 文件已用 rustfmt 格式化；后加未提交修改仍须补做定向格式检查。
- 未在本地编译或运行 Rust。
- 首版 CI，截至最后查询：
  - Validate：`36411614418`，**completed / success**。
  - Build-Release：`36411614307`，**in_progress**，仍需核对 Rust 测试及双架构的最终结果。
- CI 查询原始记录：`.local/evidence/mm-cid-calibration/handoff-ci.json`；
  最后状态在 `handoff-final-ci.json`（10:52:43 UTC）。
- 即使这两套 CI 后来成功，也只验证 `9a8fe72`，**不覆盖上面 5 个文件的未提交修改**。

## 8. 工作方式、私有入口和长期边界

- 唯一工作区：`SimAdmin`；远端 `simmaster` 是 `autisticryptic/SimMaster`。
  `origin` 是历史本地 remote，不要误推。不要 force-push、重置工作区或丢弃这些未提交修改。
- **Rust 编译/测试只在 Actions。** 本地可运行：

  ```sh
  git status --short
  git log -5 --oneline
  git diff --check
  python3 -m unittest discover -s .github/scripts -p 'test_*.py'
  ```

- 文件审阅工具与 Bash 可能分别使用 Windows/WSL 路径。项目路径为
  `D:/Program/Learning/AI/ProjectOfRong-lilith/SimAdmin`；Bash 中是 `/mnt/d/Program/Learning/AI/ProjectOfRong-lilith/SimAdmin`。
- 私有 SSH/Cloudflare 材料只留本地。入口 `.local/active/ims/connect_readonly.py`，
  其依赖和固定公钥同目录；先阅读脚本，不执行通用客户端的写操作 main。
- 原有 WSL Python 环境：`/root/.cache/simadmin-mm-resume-venv/bin/python`。
  私有 bundle 由入口脚本引用，本文不复制密码、Cookie、主机地址或私钥。
- 不将 `.local/`、会话原始工具输出、凭据或原始 SIM 身份提交到 GitHub。
- 原始 JSONL 可作需求证据，不能把历史助手口述当成 Git/CI/设备事实。
- SIM-04/05 的既定验收不用重做；MM 的成功不代表 native 成功。
- native 真机验收、混合 MM/native、多线路/VoWiFi/语音视频、未知资源自动恢复等长期事项仍见
  [DEVELOPMENT_PLAN.md](DEVELOPMENT_PLAN.md)、[NATIVE_BACKEND_STATUS.md](NATIVE_BACKEND_STATUS.md)。
  本次没有把这些长期事项全部完成，也不要无授权扩大本轮范围。

## 9. 可直接复制给下一位 AI 的 Prompt

```text
请接手当前 SimAdmin 项目，继续“ModemManager 路径的 SIM/eSIM 变化后 IMS/CID 绑定自动校准”。

先完整阅读：
1. docs/HANDOFF.md
2. docs/NEXT_AI_HANDOFF_2026-09-28.md
3. docs/IMS_MM_SIM_BINDING_CALIBRATION.md
再核对 git status、git log、git diff 和最新 Actions；以实际证据纠正文档快照，不盲信历史助手口述。

重要状态：
- 已实机验证的程序是 1.1.5/dd8ba1f：SIM-06 IPsec 注册和两次自然续期成功。
- 自动校准首版已推送 9a8fe72；最后查询 Validate 已成功，Build-Release 仍在运行。
- 工作区还有失败清理及 MM slot=0 兼容性的未提交补强；Python 184 项通过不等于 Rust 已通过。
- 不要丢弃工作区修改，不要把远端首版当作全部最新代码，也不要直接部署这个工作树。

你的任务：
1. 重建 todo，将“完成校准补强与验证”和“后续实机维护窗口验收”保留为未完成。
2. 查询首版 CI；审查并完成未提交补强，尤其是旧任务跨 SIM 发布/清理、所有异步失败出口、
   provider-only cleanup 完整性、单卡 slot=0，以及实际 SIM 与派生配置/retained bearer 的一致性。
3. 保留统一观测 + generation/准入失效 + 稳定两轮 + 带 ticket 清理 + 重新走原 profile 选择的设计。
4. 补回归、跑本地 Python/定向格式/diff 检查，再提交推送；Rust 编译和测试只能跑 GitHub Actions。
   等最新完整 SHA 的两套 CI、Rust/D-Bus 回归及双架构构建确实通过，记录证据。
5. 更新 HANDOFF 和完成边界；切卡、故障注入及可能中断蜂窝的实机测试先与我确认窗口。

约束：
- 本轮只修 MM，不迁移 native，不写 SIM-06/电信特例，不复制参考项目代码。
- 保留 IPv4v6→IPv6→IPv4，不覆盖已有数据 PDP，不改 Initial EPS/NV/USB，不放宽 P-CSCF 归属校验。
- 校准不是修改 SIM logical channel，也不是强行指定另一个 CID；不无条件重新附着、不清一次性预算。
- 不自动切 eSIM、发短信、打电话或打断当前健康会话；不重做 SIM-04/05 验收。
- 实验机如获准部署则直接覆盖、不建备份，但保留配置、数据库、历史证据及凭据。
- 不覆盖既有 v1.1.5 Release，不移 tag；候选制品必须核验 commit/架构/digest。
- Git remote 用 simmaster；不 force-push，不提交 .local/ 或任何凭据。

请先简要报告你核实后的已完成项、未完成项及下一步，再继续实施。
```
