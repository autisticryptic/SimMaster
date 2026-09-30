# 下一位 AI 接手说明（2026-09-30）

> 状态快照：2026-09-30（UTC 09-29 16:40 前后；设备本地时间 09-30 00:40）。
> 本文是**当前唯一的新接手入口**；跨会话总入口仍是 [HANDOFF.md](HANDOFF.md)，上一轮的长交接记录在
> [NEXT_AI_HANDOFF_2026-09-28.md](NEXT_AI_HANDOFF_2026-09-28.md)（仍有效，但其中的“未完成清单”已被本文更新）。
> 用户指示：**由另一位 AI 接手继续完成剩余任务**。本文已包含用户在本轮提出、但尚未进入 todo 的内容。

## 最新续接校正（2026-09-30）

本轮重新接手的代码、现场与证据边界见 [HANDOFF 最新续接](HANDOFF.md#路由补全续接2026-09-30本节优先)。
**下方“已定位唯一路由根因”是旧结论，现已撤回其确定性**：旧抓包查询的是历史硬编码地址，源码会在每个实际候选发送前装路由并绑定接口；单条路由快照不足以证明后续候选经 veth 发出。全候选预装仍是用户批准的补强，但其注册效果须新制品部署后实测。

03:32 UTC 最后可达实机仍运行 `828135b/PID85175`，MM577/secondary343 未变；IMS 恢复耗尽、旧 Modem/1 错误、现存 Modem/2，有 1 条 receipt 未动。

**路由补全与 VoWiFi 全地址/实际 SOCKS 出口候选 `a269e9d6f7c5b359e142ae7734598c009f091914` 已推送并完成两套 CI、20 个新增/更新回归名 + 8 兼容检查、双架构制品实际核验；尚未部署或证明注册成功。** 验证证明在 `.local/evidence/ims-route-completion/a269e9d/verified.json`，run/artifact/hash 详见 HANDOFF。

**用户最新确认：设备暂时离线，稍后更新临时 IP。停止旧地址 SSH/API/轮询和部署，等新 IP 后先验证原 host pin 与现场状态。** 本轮未上传/停服/删 receipt/清预算/重试注册/拨号；不重放旧抓包脚本的删 receipt/POST retry 部分。

另已实时核实：现有 `v1.1.5` Release/tag 是本会话前 2026-09-29 05:34 UTC 发布的 `09edc03`，旧文档中的 `16998ae` Release 状态已过期。本轮 Publish skipped，未操作 Release/tag，也不能把旧 Release 包当作新路由候选。

## 0. 先读这一段：现在到底卡在哪（旧快照，以上校正优先）

当前设备上的 eSIM **能在手机上正常注册 IMS，但 SimAdmin 还注册不上**。已经用实机抓包定位清楚，
**问题不在运营商配置，而在本机路由**：

| 结论 | 状态 |
|---|---|
| IMS 承载本身已能建立（这是本轮修好的） | 已实机验证：绑定到 MM 实际返回的 `wwan2`，拿到 IPv4 地址、CID 3、发现 2 个 P-CSCF |
| REGISTER 请求**没有从 IMS 网卡发出** | 已实机验证：`wwan2` 的 `tx_packets=0`；隔离网络空间和主机两侧抓包都是 0 个包 |
| 根因 | 隔离网络空间里只给**一个** P-CSCF 地址加了走 `wwan2` 的路由，实际发送用的地址不在其中，于是回落到默认路由（内部虚拟网卡 veth），根本没进 IMS 承载 |
| **用户已批准、但还没实现的修正** | 见 §3：**为所有下发/发现的 P-CSCF 地址都补路由**；VoWiFi 的 ePDG 域名多地址也做同样的补全 |
| 用户对本轮的约束 | **只允许增加现有派生配置一侧的补强，不要改动现有大的兜底逻辑**。若确实无法通过派生配置连接 IMS，要直接说明原因 |

**为什么不能靠派生配置解决（已向用户说明，用户随后批准了路由修正）：**
派生配置管的是 IMS 域名、身份模板、Contact、安全协商等 REGISTER 内容；当前 REGISTER 连设备都没发出去，
改这些字段不会有效果。写死某个 P-CSCF 地址也只是碰运气——这台设备每次建立承载拿到的 P-CSCF 都不相同
（本轮已观察到 4 个不同地址），因此用户批准的是“把当前实际下发的每个地址都补上路由”这个通用修正。

## 1. 代码与验证环境事实

- 唯一工作区：`D:/Program/Learning/AI/ProjectOfRong-lilith/SimAdmin`；Bash（WSL）路径为
  `/mnt/d/Program/Learning/AI/ProjectOfRong-lilith/SimAdmin`。文件工具用 Windows 路径，Bash 用 WSL 路径。
- 远端只有 `simmaster`（`autisticryptic/SimMaster`）。`origin` 是历史本地 remote，**不要误推**。
  不要 force-push、不要重置工作区。
- **Rust 编译/测试只在 GitHub Actions**。本地允许：Python、前端（`node.exe`）、格式和文档检查。
- 推送到 GitHub：WSL 的 `ssh` 会因 Windows 权限映射（0666）拒绝现有私钥，需要用 Windows 的 `git.exe`：
  ```sh
  git.exe -c core.sshCommand='ssh -i D:/Program/Learning/AI/ProjectOfRong-lilith/SimAdmin/.git/codex_push_key -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes' push simmaster master
  ```
- 前端检查（本机 `node` 不存在，用 `node.exe` v24.14.1）：
  ```sh
  cd frontend
  node.exe --experimental-strip-types --test tests/imsRegistrationPolicy.test.ts tests/cellularImsErrorFormat.test.ts tests/automationConfig.test.ts tests/automationOutcome.test.ts tests/uiWorkflows.test.ts
  node.exe node_modules/typescript/bin/tsc -b --noEmit
  node.exe node_modules/eslint/bin/eslint.js . --max-warnings 0
  ```
- 本地 Python（WSL）：`python3 -m unittest discover -s .github/scripts -p 'test_*.py'`（当前 202 项，
  含本轮新增的 3 个守卫脚本；**注意**：本地曾因用户把一份历史文档移出仓库而短暂失败，见 §6）。
- 设备脚本用 WSL 的 Python：`/root/.cache/simadmin-mm-resume-venv/bin/python`（已装 paramiko）。
  裸 `python3` 没有 paramiko，会 `ModuleNotFoundError`。

## 2. 设备与当前部署状态

目标机是用户指定的局域网测试机 `192.168.100.13`（QCM410，aarch64，root SSH；管理连接走 `wlan0`）。
用户已明确：**整台 410 是测试机，后续授权可以直接执行，不必逐次确认**。

- **凭据只在本地，不进仓库**：`/root/.codex/private-handoffs/simadmin/LAN_DEPLOY_2026-09-29.json`（0600），
  含 `host` / `username` / `password` / `web_password`；SSH 主机公钥 pin 在
  `.local/evidence/lan-deploy/known_hosts`，索引文件 `.local/evidence/lan-deploy/target-reference.json`。
  不要回显密码，不要提交这些文件。
- **当前运行版本**：`1.1.5 / 828135b`（`fix(mm): bind IMS networking to the verified bearer data interface`）。
  `git log` 顶部即该提交。部署后主进程 PID `85175`，运行中二进制哈希与 ARM64 制品一致。
- **这台机器的全局配置曾经丢失**：接手时 `/opt/simadmin` 已被删除、旧进程持有已删除的 `data.db`。
  已按用户“直接重新安装”的指示，先把数据库一致性恢复到正式路径（账号、线路、任务、历史保留，
  `quick_check=ok`，恢复前后全部表指纹一致），再用默认全局配置重建安装，**没有创建备份、没有重置 Web 密码**。
- **副作用（重要）**：因此这台设备的全局 `config.yaml` 是**默认值**；`/opt/simadmin/carrier-bundles.sqlite3`
  **未安装**，所有 profile 来源都回落到派生配置（当前生效 `derived_3gpp_lte_51502`，`profile_origin=derived`）。
  若后续需要对照外置库，需要先通过 Web 界面安装 schema-v7 数据库。
- 其他事实：ModemManager PID `577`、`simadmin-secondary-qmi.service` PID `343`（均未重启）；
  `simadmin-modem-recovery.timer` 处于 active；线路 ID `line-50ad5391cd09c09936f1081bd479139c`；
  当前 SIM 归属 PLMN `51502`、在 `50212` 上漫游；SIM 类型为 eSIM；IMS **尚未注册**。
- 设备上的 `automation.tasks` 为空；用户给的定时拨号号码只保存在本地
  `.local/evidence/automation-dial/requested-task.json`（在另一台 Cloudflare 测试设备上配置过 disabled 的 60 秒任务）。
  **不要把真实号码写进任何提交内容。**

## 3. 已授权但**尚未实现**的修正（下一优先事项）

用户原话（2026-09-30）：**“所有的下发的 P-CSCF 地址，都设定这条对应的路由，防止某个 ip 出现无法连接的情况，
同时 vowifi 的域名解析也可能会出现多个地址，也需要做相同的补全。”**

### 3.1 P-CSCF：每个地址都要有走 IMS 承载的路由（蜂窝 IMS / VoLTE）

- **现象证据**（只读采证，本地文件）：`.local/evidence/current-esim-ims/capture/capture.json`。
  在 UE 隔离网络空间内，`wwan2` 已 UP 且有 `100.122.213.125/30` 地址，但统计为
  `tx_packets=0 rx_packets=0`；空间内只有一条针对某一个 P-CSCF 地址的 `via <gw> dev wwan2` 路由；
  实际发送使用的其他几个候选 P-CSCF 地址没有该路由，`ip route get` 显示它们走默认路由（内部 veth）。
  两侧 `tcpdump 'udp/tcp port 5060'` 都是 0 包，与“包没离开设备”一致。
- **代码位置**：
  - `backend/src/connectivity/modems/ims/cellular_ims/live.rs:2752` — 在 `connect_family` 前调用
    `route_pcscf_in_worker(bearer, pcscf.ip(), worker_binding)`，即**每个候选进入 REGISTER 前只装它自己的那条路由**。
  - `backend/src/connectivity/modems/ims/cellular_ims/bearer.rs:688 route_pcscf_in_worker`、
    `:696 route_media_host_in_worker`、`:704 worker_host_route_op`。
  - 地址来自 `bearer.settings.pcscf`（MM 承载 PCO）以及 `discover_pcscf_via_active_at_context` 的
    `at_cgcontrdp_exact_address` 观察结果；两者都可能给出多个地址。
- **要求**：让**当前承载实际下发的全部 P-CSCF 候选**（含 MM bearer PCO 与精确地址 AT 观察得到的地址）
  在进入注册流程前都拿到走该 IMS 承载的路由；不要只给一个地址装路由，也不要写死某个地址。
  同时保留：源地址归属校验、不跨 CID 借用地址、不固定地址族顺序 `IPv4v6 → IPv6 → IPv4`、
  不删除未知资源、不放宽 P-CSCF 归属判定。
- **回归**：需要覆盖“多候选地址全部装路由”“某个地址缺路由时不再回落到默认路由”“路由安装失败时
  该候选明确失败而不是静默用错接口”，并保持现有 `worker_host_route_op` 的族校验与表/规则语义。
  Rust 仍只在 Actions 编译执行。

### 3.2 VoWiFi：ePDG 域名解析多地址的同样补全

- 解析本身已经返回**地址列表**：`backend/src/connectivity/modems/ims/vowifi/epdg.rs`
  `resolve_epdg_with_dns_override` / `resolve_epdg_via_socks5` / `resolve_connection_plan`，
  结果类型 `ResolvedEpdgEndpoint { host, port, addresses, route_policy }`。
  消费侧 `vowifi/live.rs:2996` 已经用 `take(endpoint_limit)` 逐个尝试，`live.rs:2812` 仍在检查
  worker 空间里的接口/地址/默认路由。
- **要求**：把“同一域名解析出的多个地址”在**路由/绑定/代理目标**这一层也补全，避免只对其中一个地址
  建立可达性、其余地址直接不可用（与 3.1 同一类问题）。**不要假设**到底是哪一层只选了单地址——
  先实际追踪 `choose_route_policy` / `ResolvedEpdgEndpoint` / NAT-T 目标 / SOCKS5 与 UDP relay 的
  `route_addr` 使用点，再改，并补回归。
- 现状提醒：本机该线路 `vowifi.enabled=false`，VoWiFi 未启用；3.2 属于通用补强，验收时可以先用
  另一条已启用 VoWiFi 的线路或受控配置，不要为本机强行开启。

### 3.3 实施与验收顺序（建议）

1. 审查 3.1/3.2 的所有调用点，确认不存在“只给一个地址装路由/只对一个地址做可达性”的分支。
2. 补 Rust/mock 回归与（如需要）Python 守卫；本地跑允许的检查。
3. 提交并推 `simmaster`，等**对应完整 SHA** 的两套 Actions（Validate + Build-Release）与双架构制品核验。
4. 部署到 `192.168.100.13`（见 §7 的部署方式），随后**只读观察**一次重试：确认
   `wwan2` 的 `tx_packets` 是否增长、P-CSCF 是否出现在正确的承载上、是否收到 SIP 响应（401/200）。
   用户已授权直接执行；但**不要主动拨号**，也不要为此切换 eSIM 或清恢复预算。
5. 结果写入 [HANDOFF.md](HANDOFF.md) 并更新本文。

## 4. 本轮已完成的内容（有证据）

按时间顺序，均已提交；Rust 结论均来自 Actions 下载的日志逐项核实，不是只看 workflow 绿色标题。

| 范围 | 提交 | 验证与边界 |
|---|---|---|
| MM SIM/承载绑定校准（换卡/eSIM 切换后自动失效与重建） | `9a8fe72` → `dc2355c` | 两套 CI + 14 项新增 Rust/mock/D-Bus 回归 + 双架构制品核验；已部署到旧 Cloudflare 设备 |
| 语音呼叫准入：保留原“仅 VoWiFi”开关，新增“VoWiFi 或**明确非漫游**蜂窝”条件模式；短信策略不变 | `a6a327e` | 两套 CI + 28 项新增回归 + 双架构；部署到 Cloudflare 设备后实机注册成功；**用户确认收到北京时间 00:32 的测试来电**（未接听，音频未验收） |
| 定时拨号：不再“立即 failed”；错误链、保存反馈、target 规范化 | `5ae4f8e`、`098ae04`、`7c6cf86` | 首版 `098ae04` 有 E0004 编译失败（已修，失败记录保留）；`7c6cf86` 两套 CI + 18 新回归 + 双架构；已部署到 LAN 设备 |
| 拨号结果分类：对端未接/非本端超时**不算任务失败**，但保留真实 SIP/Q.850 与未接听事实；本端/未知/清理失败仍 failed | `7c6cf86`（含于其内） | 同上；**只有部署后新语义才生效** |
| UI 三项：飞行模式文案精简；移除“IMS 注册模式”选择（后端只规范化旧值、兼容接口明确拒绝已移除模式）；eSIM 列表直接“切换” | `33d16f3` | 前端 21 项 unit + type-check + lint 通过；Actions 的 Frontend/Validate/Build 全 success + 3 新 Rust 回归 + 6 兼容回归 + 双架构；已部署到 LAN 设备 |
| MM IMS 数据接口修正：不再要求必须是主网卡，改为绑定 **MM 实际返回**的承载网口，并校验 MM `Ports`、同 `remoteproc` / `:bam-dmux` sysfs 拓扑、无其他承载占用、DBus owner 与 SIM 一致 | `828135b` | 两套 CI + 5 新回归 + 5 项归属回归 + 双架构；**已部署并实机验证**：成功 pin `wwan2`、IPv4 grant、CID 3、2 个 P-CSCF，REGISTER 已进入发送阶段 |
| LAN 设备抢救性重装 | 无代码变更，运维操作 | 先从已删除 inode 恢复数据库（共享读锁 + 内存反序列化），`quick_check=ok`、全部表指纹一致；再重装为已核验制品；无备份、未重置密码、未重启 MM/secondary |
| Pixel / iOS / IPCC 外置库离线对比 | 文档 `docs/IMS_CATALOG_PIXEL_IOS_COMPARISON_2026-09-29.md` | 只读比对，**没有宣称找到历史呼入问题的唯一根因**；源库 SHA 未变 |
| 旧承载记录的收尾 | 运维操作 | 在“MM 已不认识该 bearer 且该网口无地址”两个前提同时成立时才结案；随后触发一次受控重试（HTTP 202） |

证据目录（本地，不随 Git）：
`.local/evidence/{mm-cid-calibration,automation-dial,dial-outcomes,ui-offline,lan-deploy,lan-ui-deploy,current-esim-ims,carrier-voice-compare}`。

## 5. 其他未完成事项（含用户提过但还没进 todo 的）

### 5.1 已进入 todo
- **#5 真实换卡 / 自动重新附着故障注入的实机验收**：仍未做。需要明确切换目标（实体卡或指定 eSIM）与窗口；
  注入前必须确认一次性恢复预算**未被消耗**；**不得删除预算文件或重启 MM 来制造新预算**。
- **#26 / #27 当前 eSIM 的 IMS 注册**：主体是 §3.1 的路由修正。收尾后仍需一次完整注册验收。
- **MM 对象换代与旧承载清理**：本轮已能正确结案**已验证为陈旧**的记录；但“对象换代本身”未修（见 5.2）。

### 5.2 用户提出、但不在 todo 里（必须接续）
1. **把 IMS 网卡移回主机时 MM 会重建 modem 对象**。日志证据：`base-manager: additional port wwan2 … added
   after device probing has already finished, but we have a new port addition, will retry` →
   `creating modem with plugin 'qcom-soc' and '12' ports`；`Modem/0` 变 `Modem/1`。副作用是**主机普通数据承载
   被杀**、NetworkManager 约 6 秒后自动恢复。用户已表示这是测试机、可直接处理，但**修法需要先确认**：
   倾向让 MM 不再接管这个 IMS 专用网卡（会涉及 udev 规则），或让清理不产生可被 MM 感知的端口事件。
2. **通话中自然续期是否会断话**。用户问过，我只给了静态分析：正常续期复用原保护通道、不应挂断通话，
   但 `RebuildAccess` 分支确实会清理会话（可能断话），且续期事务进行时 live 循环忙于 REGISTER，
   挂断等通话控制可能被延后。**实机未验证**，需要专门测试（长通话跨一次自然续期）。
3. **Pixel 外置库“可注册但呼入进语音信箱”**。离线分析（已提交的报告）结论是：历史上“空 Contact 参数表导致
   漏发 MMTEL”的缺陷（`2b743e2`）**已经修好且包含在当前代码里**，所以不能再用“Pixel 未声明语音”解释；
   同时列出仍存在的投影缺口（`user_agent_template` 与消费者读的 `user_agent` 字段名不一致、
   `/ims/security_agreement` 与 `/sip/common/register/security_agreement` 路径不一致、
   `sip.lte/nr/vowifi` 的 access-specific 覆盖未合并、显式非 audio 的 Contact 列表可能推导出
   `include_mmtel=false`）。**需要在同卡同版本同接入下做受控 A/B 才能定论**，不要直接改代码。
4. **IMS 数据通道为什么没能发出包**（除路由外）：本轮 `wwan2` 统计为 0，需在修好路由后复测；
   若仍为 0，要检查该 BAM-DMUX 通道与 WDS 会话的对应关系（MM 报 `multiplexed: no`）。
5. **Android 移植**：用户问过“能否移植到安卓做用户态 IMS/VoWiFi”，并**明确说明不要在本项目里做**。
   已给出口头可行性结论（复用协议逻辑可行；SIM AKA 访问、承载获取、系统电话集成是主要难点，
   root 不代表可用）。**仅作参考，不要在这里实现。**

### 5.3 参考信息（不是待办）
- `deploy/devices/qcm410/system/simadmin-modem-recovery.sh`：QCM410 的 ModemManager 设备发现自愈脚本。
  只处理“QMI 端口已就绪但 MM 未正确识别”，顺序是 udev remove/add 重播 → 仍未恢复则**本轮最多重启一次
  ModemManager**；**不重启 MPSS/操作系统**。每轮独立计数，不是全生命周期只一次；触发时仍可能短暂影响数据/IMS/通话。
  它与“一次性 IMS 重新附着预算”是两套不同机制。

## 6. 隐私与本地材料边界（必须遵守）

- `.local/` 一律不提交。凭据、Cookie、私钥、原始 SIM 身份（IMSI/ICCID/MSISDN）、真实电话号码都不进仓库。
- **用户本轮把一份历史文档移出仓库**：`docs/archive/2026-09/ESIM_IMS_PROFILE_TEST_2026-09-01.md` 在本地被删除，
  内容被移到未跟踪的 `docs/ESIM_IMS_PROFILE_TEST_2026-09-01.md`（含真实号码，**不要提交**）。
  为保持仓库文档链接检查通过，`docs/archive/README.md` 中指向该文件的链接已移除，
  并在同一处说明该记录改为本地保存。**不要**把未跟踪的那份文件加进提交，也不要恢复被删的归档文件。
- 不要删除既有历史诊断、凭据或数据库。旧 Release `v1.1.5 / 16998ae` 不覆盖、不移 tag。
- 汇报时用“某个地址/某个号码”这类脱敏说法；具体值放 `.local`。

## 7. 部署到测试机的现行做法（用户已授权直接执行）

1. 只读预检：复用 `.local/active/lan/update_preflight.py`（同一 owner/SIM、服务 PID、无活动通话、
   无启用任务、数据库 `quick_check`、管理走 `wlan0`、磁盘余量）。
2. 制品必须来自**已验证的最新完整 SHA** 的 Actions 包（`.local/evidence/*/verified.json` 里记录了
   官方 artifact digest、包内 `version/commit/arch`、ELF 架构、二进制与前端校验）。
3. 覆盖安装：脚本 `.local/active/lan/deploy_interface_candidate.py` / `update_ui.py` 是当前可用范式
   （暂存 → 校验 → 停主服务 → 覆盖程序/前端/meta → 起服务 → 校验运行中二进制哈希）。
   **直接覆盖、不创建备份**；保留 `config.yaml`、`data.db` 与账号；不要重启 ModemManager/secondary；
   部署前暂停 `simadmin-modem-recovery.timer`，完成后恢复。
4. 这些 `.local` 脚本是为当时状态写的一次性脚本，**不要盲目重放**：其中的旧 PID/旧哈希/旧接口名可能已不成立。
   必要时按同样范式改写并重新审阅。

## 8. 可直接复制给下一位 AI 的 Prompt

```text
请接手 SimAdmin，继续完成“当前 eSIM 的 IMS 注册”修复。

先完整阅读：docs/HANDOFF.md、docs/NEXT_AI_HANDOFF_2026-09-30.md（本文，最新）、
docs/NEXT_AI_HANDOFF_2026-09-28.md（上一轮长记录）、docs/IMS_MM_SIM_BINDING_CALIBRATION.md、
docs/IMS_REGISTRATION_POLICY.md、docs/IMS_CATALOG_PIXEL_IOS_COMPARISON_2026-09-29.md。
然后核对 git log / git status / git diff 与最近 Actions，以实际证据为准。

现状（已核实）：
- 设备：局域网测试机 192.168.100.13（QCM410/aarch64/root SSH，管理走 wlan0）。凭据在仓库外
  /root/.codex/private-handoffs/simadmin/LAN_DEPLOY_2026-09-29.json（0600），不要回显或提交。
  用户已说明整台 410 是测试机，授权可直接操作，不必逐次确认。
- 已部署版本：1.1.5 / 828135b（含本轮 MM 数据接口修正），主进程 PID 85175。
- 该机全局配置曾丢失，现为默认值；carrier-bundles.sqlite3 未安装，profile 一律回落派生
  （当前 derived_3gpp_lte_51502）。数据库已从已删除 inode 抢救恢复，账号/线路/历史保留。
- 当前 SIM：归属 PLMN 51502，在 50212 漫游；eSIM；IMS 尚未注册。

已定位的根因（实机抓包证据在 .local/evidence/current-esim-ims/capture/capture.json）：
IME 承载已能建立并绑定到 MM 实际返回的 wwan2（有地址、CID 3、2 个 P-CSCF），
但隔离网络空间里只给“一个” P-CSCF 地址装了走 wwan2 的路由，实际发送使用的其他候选地址
回落到默认路由（内部 veth），导致 REGISTER 没离开设备：wwan2 的 tx_packets=0，两侧 5026 抓包都是 0 个。

用户已批准、需要你实现的修正（这就是当前最高优先级任务）：
1) 蜂窝 IMS：为当前承载实际下发的所有 P-CSCF 候选地址都补上走该 IMS 承载的路由，
   不要只装一个、不要写死地址。参考 live.rs:2752（route_pcscf_in_worker）与
   cellular_ims/bearer.rs:688/696/704（route_pcscf_in_worker / worker_host_route_op）。
2) VoWiFi：ePDG 域名解析可能返回多个地址，做同类补全（epdg.rs 的 ResolvedEpdgEndpoint.addresses、
   vowifi/live.rs:2996 与 :2812）。先实际追踪是哪一层只选了单地址（路由/绑定/代理目标），再改，不要假设。

约束（用户明确要求）：
- 只允许在现有派生配置一侧做补强，不要改动现有的大兜底逻辑。
- 保留地址族顺序 IPv4v6 → IPv6 → IPv4；不放宽 P-CSCF 归属校验；不跨 CID 借用地址；
  不改 Initial EPS/NV/USB；不覆盖已有普通数据 PDP 定义。
- 不清一次性恢复预算，不重启 MM/基带，不切换 eSIM，不主动拨号，不发送短信。
- Rust 编译与测试只在 GitHub Actions；本地只跑 Python、前端（node.exe）、格式和文档检查。
- 提交只推 simmaster；不要 force-push；不要提交 .local/、凭据或那份含真实号码的本地历史文档。

工作方式：
1) 先重建 todo，并报告你核实到的“已完成 / 未完成 / 下一步”。
2) 实现上述路由补齐 + 回归，本地检查通过后提交推送，等对应完整 SHA 的两套 Actions 与双架构制品核验
   （下载测试日志逐项确认新回归真的执行，不只看绿色标题）。
3) 部署到 192.168.100.13：先只读预检（无通话、无启用任务、数据库完整、管理走 wlan0），
   用已验证制品覆盖安装（不建备份、保留配置与数据库、不重启 MM/secondary），
   然后只读观察一次注册结果：wwan2 的 tx_packets 是否增长、P-CSCF 是否走在正确承载上、
   是否收到 SIP 响应。不要主动拨号。
4) 结果与残留问题写回 docs/HANDOFF.md，并把本文更新为最新入口。
另有明确未完成事项见本文 §5（含通话中自然续期是否会断话、Pixel 库呼入进语音信箱的受控 A/B、
MM 移回网卡导致 modem 重新枚举从而短暂断普通数据、真实换卡与自动重新附着故障注入验收）。
```

## 9. 本次文档维护动作（便于核对）

- 新建本文，作为 2026-09-30 起的新接手入口。
- 更新 `docs/HANDOFF.md` 顶部指针到本文。
- 修正 `docs/archive/README.md` 中因用户把历史文档移出仓库而失效的链接（改为本地保存说明），
  使文档链接检查在 CI 中保持通过；未提交那份含真实号码的未跟踪文档，也未恢复被删的归档副本。
