# 下一位 AI 接手说明与 Prompt 模板

> 状态快照：2026-09-28 10:52 UTC（北京时间 18:52）。
> 当前接手总入口仍是 [HANDOFF.md](HANDOFF.md)。本文专门说明本轮新增的 **MM SIM/CID 绑定校准**任务、未提交修改和下一步。
> **以下为历史暂停时的快照。用户随后已要求继续实现，最新进展和 CI 以 [HANDOFF.md](HANDOFF.md) 为准。**
> 当时尚未完成的内容和未提交列表保留作审查依据，不是当前工作树清单。

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
