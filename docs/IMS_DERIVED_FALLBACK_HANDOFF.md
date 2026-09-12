# IMS 派生配置与兜底注册测试：独立交接

> 整理日期：2026-09-13；设备实测基线截至 2026-09-12。本文用于从 1.1.5 开发切回 IMS 测试，或交给其他 AI 做只读分析。
> **角色约定：只有一个开发 agent 可以修改代码、配置、部署和发起主动测试；其他 agent 只读分析并向它汇总问题。**
> 本文是可提交 GitHub 的脱敏版。用户要求的密码、Cookie、项目 Git 私钥放在仓库外私密完整版：
> `/root/.codex/private-handoffs/simadmin/IMS_DERIVED_FALLBACK_PRIVATE.md`。
> 私密版目录应为 0700、文件为 0600；迁移环境时通过安全渠道携带，不能提交 Git、上传公开附件或在回复中展开。
> 私密版包含固定 SSH host key、完整只读启动器源码及辅助材料；同目录的 `resume_read_only.py` 可直接在配置了依赖的本地 Python 中启动。只读 agent 不需要原始会话 JSONL。

## 1. 先读结论，避免重复或误操作

1. **SIM-03 已在当前程序完成派生配置的完整认证、初始注册和一次原会话自然续期。** 不是“卡不支持 IMS”，用户也已人工验证过手机和参考版本能注册。
2. 已验证的程序代码是 `684e2a71e0227dc66f9b7591ab982485933bde15`，版本字符串仍为 `1.1.4-beta3`。后续文档提交的 HEAD 不是新的设备程序。
3. 最新设备内只读采样为 **2026-09-12 21:33（Asia/Shanghai）**：SIM-03 仍注册，refresh=1、reconnect=1。**这不是阅读本文时的实时状态。**
4. **派生首槽成功不等于整条兜底阶梯已通过。** 当前保存的是 derived → carrier_catalog → database；要测试真正的后置兜底，必须单独记录前置候选为什么未使用/失败及最终 effective source。
5. SIM-01/02 在旧正式 beta3 上成功，不代表最新 MM 主承载实现已经兼容；尤其还有一次 `45400 / IPv6` 路由失败和 modem 消失待查。
6. SIM-03 的 IPv4/UDP 注册/续期不代表四库专属参数、IPv6、IPsec、双注册、短信或通话全部通过。
7. **当前 IMS 测试线继续使用 MM；不要因为 1.1.6 计划移除 MM，就在这台基线设备上停掉它。**
8. 开始任何主动测试前确认唯一操作负责人、实际卡/程序/配置和无活动通话。只读 agent 不改这些条件。

## 2. 分支、目录和单一写入者

| 工作 | 分支约定 | 本机工作目录约定 | 权限 |
| --- | --- | --- | --- |
| 1.1.4 IMS 派生/兜底修复与基线 | `fix/sim02-catalog-aka-baseline` | `/mnt/d/Program/Learning/AI/ProjectOfRong-lilith/SimAdmin` | 由唯一开发 agent 修改 |
| 1.1.5 统一接口、MM 适配和原生后端 | `dev/1.1.5-modem-backends` | 相邻独立 worktree `SimAdmin-1.1.5` | 同一个开发 agent 修改 |
| 其他 AI 的分析 | 指定分支/commit 的独立副本或只读工作区 | 不共用开发者的可写目录/index | 只读代码和状态，提交问题报告，不自行修补 |

- 本机路径是布局约定，新环境可以另行 clone；先用 `git status`、`git branch --show-current`、`git worktree list` 核对，不假定目录已经存在或分支已切好。
- 两条开发线应从同一修复基线派生，不能从较旧的 master 开始并丢掉 `684e2a7` 的修复。
- **不要让多个 agent 在同一目录执行 git switch/reset/stash/clean，或交替修改同一个工作树。**
- IMS 小修复与对应测试独立提交，不夹带版本号、大型重命名或后端重构；由开发 agent 用 `cherry-pick -x` 移植到 1.1.5，再验证行为。
- 1.1.5/MM 完成行为等价后，可成为主测试线；原生后端专属问题不要未经判断回灌 1.1.4。
- GitHub remote 为 `simmaster` → `git@github.com:autisticryptic/SimMaster.git`；`origin` 是旧 Windows 本地路径，不是发布远端。
- 只读 agent 即使取得全套凭据，也不得 commit/push、改共享分支、部署或切换设备配置；读取私有仓库所需的 clone/fetch 只能作用于自己的副本。
- 分支/worktree 的隔离不等于设备隔离：**同一台 modem 只能运行一套被选定的程序/控制器，不能同时实测 1.1.4 和 1.1.5。**

## 3. 设备与已验证程序

| 项目 | 历史基线 |
| --- | --- |
| 入口 | `https://qca410ssh.davidden.com/`，Cloudflare WebSocket 承载 SSH，SSH 用户 `root`，不是直连公网 22 |
| 系统 | Debian 11 / aarch64 / `5.15.0-handsomekernel+` |
| 软件工具 | MM 1.18.4、qmicli 1.28.6、libqmi-glib 1.30.2 |
| 安装 / 服务 | `/opt/simadmin` / `simadmin.service` |
| 配置 / 数据 | `/opt/simadmin/config.yaml`、`/opt/simadmin/data.db`，还需考虑 SQLite WAL/SHM |
| 管理网络 | 默认出口 wlan0，不能为 IMS 测试破坏它 |
| API | 设备本机 `http://127.0.0.1:3000`，需单独的 Web 登录会话 |
| 代码 / 版本 | `684e2a7` / `1.1.4-beta3` 开发候选，不是 beta4 |
| 二进制 SHA256 | `18408e9b2c60815a70d48d1a44dbcf85bb77bd0608357769a22b89acae58c167` |

**端点契约：**QCA410 的 IMS 使用主 `/dev/wwanNqmi0` + MM/qmi-proxy 管理的私有 bearer；DATA6 只用于普通数据。IMS 专属网卡进入对应 UE namespace，不能回到宿主网络绕过隔离。DATA6 在此设备上可能表现为受忽略的 AT 名称端口，不要凭名称猜测主/次 QMI。其它硬件不能强套 QCA410 布局。

历史线路 ID 为 `line-50ad5391cd09c09936f1081bd479139c`，namespace 为 `sa-ue286e0c9d2870`。以下数值只用于识别历史证据，**换卡、重启、换机后必须重新发现，不得硬编码进配置**：

- 21:15 采样：MainPID=443，MM=433，qmi-proxy=655，DATA6 initializer=662。
- modem `/org/freedesktop/ModemManager1/Modem/2`，私有 `/Bearer/3`，D-Bus owner `:1.10`。
- UE namespace inode=4026532440；IMS UDP socket 为 MainPID=443/FD=19，inode=12411。
- IMS tuple 为 `10.26.25.247:5060 → 10.5.236.166:5060`；P-CSCF 地址来自网络，不能把这个历史地址写死给其他卡。

### 构建、包与备份

- CI Validate `34681227857`、Build `34681227851` 已通过，含 arm64/amd64；Release 未发布。
- 已部署包：`simadmin-candidate-684e2a7.tar.gz`，SHA256：
  `e2a07b6e6c9d3c317d60549480bdfc34c6d2fda855a3e462ba5dfb67a2fcb496`。
- 本机包目录 `.codex-cf-candidates/684e2a7/`，不随普通 clone 携带。
- 历史下载地址：`https://nightly.link/autisticryptic/SimMaster/actions/runs/34681227851/pkg-arm64.zip`；
  artifact 标记到期为 `2026-09-15T07:44:03Z`，不能假定以后仍可下载。
- 备份：`/opt/simadmin-backups/20260912-before-684e2a7-attempt1`；
  一致性配置/数据库备份：`/opt/simadmin-backups/20260912-before-mm-lifecycle-tests`。
- 同为 beta3 的程序可能完全不同，必须核对 commit/哈希；不要因为包到期就重新覆盖仍正常运行的设备。

## 4. 当前已知的逐卡结论

| 卡别名 | 归属 / 访问 PLMN | 已验证 | 未完成 |
| --- | --- | --- | --- |
| SIM-01 | 45400 / 46001 | 正式 beta3 `05de680` 的标准派生 IPv6/UDP 注册及自然续期；9/9 05:47 初始、06:37 续期 | 新 MM 主承载回归；不能拿旧版成功替代新版本测试 |
| SIM-02 | 46000 / 46000 | 正式 beta3 的标准派生注册及自然续期；9/9 10:18 初始、11:08 续期 | 新 MM 路径及 Pixel ready 配置初始 AKA 修复的实机回归 |
| SIM-03 | 45403 / 46000 | `684e2a7` 派生首槽、IPv4/UDP，完整认证/注册、正常关闭/恢复、崩溃恢复及首次原通道自然续期 | 其它地址族、库专属参数、双注册与真实业务不在本次通过范围 |

SIM-03 的准确自然续期证据：

- session_started_at=2026-09-12 19:13:22，registered_at=19:14:13（Asia/Shanghai）。
- 21:04:14.418477 发 REGISTER CSeq=4；21:04:14.830347 日志 `register_phase="refresh"` 成功。
- 网络租期 7200 秒，正常间隔 6600 秒；renew 后同样取得 7200 秒，未缩短租期。
- 48 个只读样本保持 registered=true、reconnect=1，refresh 从0到1；注册/会话起始时间未变。
- 原进程、MM bearer/owner、唯一归属账本、UE namespace、UDP tuple 和 socket inode 均未更换。
- 关联身份数2、Contact binding数1、Service-Route数0、无 Security-Server、outbound 未协商。
  因此是 UDP 注册/续期通过，**不是 IPsec、双注册或通话通过**。
- 21:33 最后一次 API 读取仍为原会话、refresh=1；观察器已退出，之后没有持续观察的保证。

较早的下午会话因设备重新启动而中断，不能把晚间续期成功回填给下午会话。设备启动早期存在墙钟校时跳变和 wtmp 1970 条目；排查重启须结合 boot ID、uptime，不能单凭墙钟日志下结论。

## 5. 必须区分的三种“派生/兜底”

| 测试类型 | 证明什么 | 不能宣称什么 |
| --- | --- | --- |
| A：derived 首槽 | 标准派生配置本身可以认证、注册、续期 | 不能证明前面其它来源失败后的阶梯恢复 |
| B：来源内部解析兜底 | 请求 database/catalog，但因缺项/不适用而实际解析为 derived | 不能称该库的专属 LTE 参数通过；也不一定走到了第三个外层槽 |
| C：外层候选阶梯兜底 | 前置槽有明确失败/未使用原因，后续槽最终有效配置为 derived 并成功 | 不能省略前置结果，也不能用配置里的顺序代替实际执行证据 |

每轮都记录 `requested_source`、`effective_source`、`effective_profile_id`、候选 index、
fallback reason、`profile_attempt_results` 和 `connection_attempts`。**配置写了三槽，不代表三槽都执行过。**

当前设备保存的顺序是 **derived → carrier_catalog → database**。代码默认顺序是
database → carrier_catalog → derived，但这不覆盖当前保存值。

历史四库：`ios-ipcc`、`iPhone16ProMax26.6.1`、`Pixel Mustang`、`Xiaomi15Ultra Xuanyuan`，
来自 `autisticryptic/carrier_Bundles` 的 `v0.3.0-catalog-v7`（sealed v7）。
此前一些“四库通过”实际最终使用同一 derived；不要扩大结论。SIM-03 两套 iOS 通用项的
LTE 状态曾为 unknown，不能擅自改成 ready 或只凭 PLMN 冒用其它 MVNO profile。

## 6. 保存的配置意图与费用边界

以下为最后已核对值，新的 agent 应只读确认差异，不自动恢复成这些值：

| 配置 | 历史值 |
| --- | --- |
| IMS / video gate 镜像 | true；镜像不代表视频业务通过 |
| VoWiFi / 普通蜂窝数据 / Trunk | false |
| `trunk.vowifi_only` | true，但 Trunk 本身关闭 |
| `roaming_allowed` | true，不代表漫游免费 |
| `sms_path.force_vowifi_send` | **false**，不能声称已启用“短信仅 WiFi”的资费保护 |
| profile 顺序 | derived → carrier_catalog → database |
| 地址族顺序 | ipv4v6 → ipv6 → ipv4，auto=false |
| IMS 接入意图 | concurrent；仅蜂窝可用/无 outbound 协商不能算双注册 |

不得为测试擅自发短信、拨号、启用 Trunk/普通数据或修改漫游/费用策略。飞行模式、
开机驻网控制等 1.1.5 新需求不应混入 1.1.4 兜底测试变量。

## 7. 连接准备与只读操作入口

### 凭据与工具

- Cloudflare、SSH 和应用 Web 登录是三层认证；完整私密版包含最近成功使用的值及项目 Git 私钥，公开版不含。
- Cloudflare Cookie 有有效期；1033/HTTP 530 是隧道入口问题，SSH 中断不是 IMS 失败。
- 私钥用于指定 GitHub 项目，不是设备的 SSH 登录密钥。不要删除原 push key、关闭 host-key 校验或把凭据写入命令行/Git。
- 本机曾使用 Python `paramiko` + `websocket-client` 将 SSH 流封装进该入口的 WSS，并固定设备 host key。
- 仓库中的 `.codex-*` 脚本默认被忽略；普通 clone 不包含它们。私密完整版附必要连接辅助材料，不能依赖原 Windows 会话目录。
- 目标设备缺 Python3、jq、sqlite3、Perl JSON::PP。不要为了读取状态安装系统包；可在 agent 本机内存解析 JSON，仅输出脱敏白名单。
- 不打印原始配置、完整 journal/SIP 鉴权帧或整份历史会话。登录成功也不授权调用 auth reset、reveal password、部署或 retry。

### API 契约

所有请求针对设备本机 API，经已有 SSH/认证工具访问。只读 agent 仅允许查询和必要的登录：

| 方法与路径 | 用途 |
| --- | --- |
| GET `/api/cellular-ims/lines` | 响应 `status="ok"`，线路在 `data[].modem.line_id`；配置在 profile，运行态在 runtime |
| GET `/api/cellular-ims/lines/{line_id}/profile-selection` | 当前三槽、可选配置和运行态；不要原样打印整个大 catalog |
| GET `/api/modem/lines/{line_id}/calls` | 原生活动通话；`/api/calls` 的404不等于没有通话 |
| GET `/api/modem/lines/{line_id}/cellular-ims/call/status` | IMS 通话状态 |
| POST `/api/auth/login` | 仅用于建立读取会话，密码从私密材料在内存中加载，不重设密码 |

注意：modem 的 `state=registered` 是蜂窝状态，不等于 `runtime.registered=true` 的 IMS 注册。
脱敏工具可能把 `associated_uris` 过滤为空；判断网络关联身份应结合成功日志中的数量。

以下仅供**唯一开发 agent 在受控测试窗口**参考，不是读到本文就应执行的命令：

- PUT `/api/cellular-ims/lines/{line_id}/profile-selection`：
  `{"attempts":[{"source":"database"},{"source":"carrier_catalog"},{"source":"derived"}]}`。
  必须恰好三槽；省略 `profile_id` 表示来源内自动匹配，显式 ID 必须属于对应来源并具备 LTE ready 条件。
  **该 PUT 会保存配置，并可能主动断开/重启 IMS，即使只是想“确认一下设置”也不能在续期观察中调用。**
- POST `/api/cellular-ims/lines/{line_id}/connection`：`{"enabled":true/false}` 是持久化意图修改，不是纯查询。
- POST `/api/cellular-ims/lines/{line_id}/retry`：202只是受理，409须看原因；已注册时不要反复调用。

只读 shell 可以核对时间/uptime、程序哈希、限定字段的服务状态、MM对象、UE接口及数字 socket tuple。
MM 服务未运行时不要用查询隐式拉起它；不要直接从 qmicli 打开主 QMI，更不要自行猜端口/绑定参数。

## 8. 多 agent 协作：一个开发者，多个只读分析者

### 开发 agent 的职责

1. 保持两个独立工作目录，统一处理读者报告，执行代码修改、CI、提交与修复移植。
2. 统一安排设备操作窗口、软件版本、卡片、配置和观察基线；换卡由用户/设备拥有者配合。
3. 部署前确认无通话、备份一致性配置/数据库、核验包与二进制，只替换计划资产。
4. 只清理自己创建且归属明确的资源；不停止 MM/proxy/DATA6 来“碰碰运气”，不破坏管理网。
5. 测试结果统一写回第 11 节指定位置。读者不直接修改测试文档或发布状态。

### 只读 agent 的职责

- 可以查看指定 commit 的代码、已有脱敏记录，以及经授权的只读 API/限定状态查询。
- 可以在自己的临时目录保存脱敏报告；不写共享工作区、不改设备业务配置、不部署、不重试注册、不切库/卡。
- 报告统一包含：问题编号、观察时间、代码/设备版本、卡标签、实际 profile/地址族、证据、影响范围、事实与假设、建议开发者验证的最小步骤。
- 即使认为修复显而易见，也交给唯一开发 agent 执行；不能自行 cherry-pick、修改服务或“验证性重启”。
- **全套 root/Git 凭据本身并非只读凭据。只读限制必须由 agent 工具权限和操作约定共同执行。**

### 同一测试机的交接

- 主动测试、部署和配置变更只能由唯一开发 agent 持有操作窗口；只读分析可以并行，但不要混用一个交互式 SSH 客户端的 stdin。
- 若以后需要转交开发者，建议采用设备端共享占用标记 `/run/simadmin-agent-test-owner/`，
  用原子 mkdir 取得占用，记录非秘密的 agent别名、case ID、分支、boot ID、开始时间和目标。
  **这是约定路径，写本文时没有在设备创建该目录。**
- 已有占用或持有者不明时，先核对/协调，不按 SSH 断开、PID消失或固定超时擅自删除占用；
  两小时续期观察可能仍在继续。只读 agent 不争抢此占用。
- 占用标记不是内核级访问控制，也不能代替 MM bearer/namespace 的真实归属检查。
- 结束窗口时移交配置差异、程序版本、当前卡、是否仍注册及证据；不要把重新初始注册计为原窗口续期。

## 9. 继续测试的顺序

1. **先只读重建基线。** 核对角色/分支/设备程序哈希、实际卡、配置、通话、MM/UE资源和时间；
   历史 PID、Bearer ID、P-CSCF、session 时间都不是下一轮的默认参数。
2. **明确本轮目标是 A、B 还是 C。** 若只是确认 SIM-03 同版本派生首槽已有结果，优先复核已有证据，不重新等两小时。
3. **优先补旧卡与 IPv6。** SIM-01 的旧版 IPv6 基线适合作对照；再做 SIM-02 和后续新卡。
   没有目标卡时先做代码/日志分析，不假装完成该卡实测。
4. **需要改变条件时由开发 agent安排。** 保留前一配置和所有失败证据；真正的候选阶梯测试须说明前置候选
   是缺失/不适用、解析成 derived，还是已向网络尝试后失败。不要篡改 ready/unknown 或冒用其它卡身份制造路径。
5. **初始注册单独验收。** 记录承载、地址族、P-CSCF、请求/有效 profile、SIP阶段及真实认证；
   有IP不等于可达，SIP响应不等于所有业务可用。
6. **自然续期单独验收。** 采用网络租期；对比原 session_started_at/registered_at、reconnect、refresh、
   CSeq、新租期、UDP socket或实际协商的IPsec SA、MM bearer和UE资源。旧成功只适用于已记录的版本/卡/配置组合。
7. **有故障先分层，不先重启。** 读取错误与发生时序；必要时由开发 agent决定有界重试、修复或回滚。
8. **写报告并结束/移交窗口。** 初始、续期、业务分别标记通过/失败/中断/未测/不适用；不把单卡成功扩展成全卡通过。

### 续期通过的最小证据

- 原会话保持 registered，session起始/初始注册时间和 reconnect 不变。
- `register_refresh_count` 增加，`last_register_refresh_at` 更新，attempt 为 `register_refresh / succeeded`。
- 日志出现 `register_phase="refresh"` 的网络成功结果及新租期/下一次调度，CSeq合理递增。
- 原 bearer/UE/通道保持；UDP可对照 tuple、PID/FD及socket inode，IPsec按实际协商核对SA。
- SSH/观察器失联时只读重连；若计数已增加，验证已留存的证据，不重启或重新等待整段租期。

## 10. 已知问题与归因边界

- **45400 / IPv6**：9/12 17:45 记录两条 P-CSCF 路由 `Invalid source address`，随后 QMI hangup / modem 对象消失。
  当时尚未确认物理换卡；不能凭 operator_id 直接断言是 SIM-01 本身故障。需核对 IPv6 地址就绪、namespace迁移、
  路由安装和 modem/generation 变化的时序，不全局改成 IPv4掩盖问题。
- **SIM-02 / Pixel初始AKA**：曾收到 `Terminal has used different algorithm from initial register`。
  缺省初始AKA基线已修代码/CI，新承载路径上的实机回归仍需补；不要把未经证实的网络拒绝当成卡不支持。
- **iOS正常、Pixel呼入进语音信箱**：用户反馈，尚缺同卡/同版本/同接入只换profile的受控对照。
  后续检查effective profile、Contact/绑定、INVITE是否到达、路由/拒绝/SDP；注册成功不能替代该项验收。
- **旧 raw-QMI 实验**：早期 qmicli flags parser 会清掉 proxy 标志，已单独修正；独立qmicli仍未解决SIM-03。
  已验证的是完整MM承载管理，不是某个QMI TLV已被证明为唯一根因。不要重放任意SIO/QMUX绑定实验。
- **重启与入口**：曾有 Cloudflare1033和设备新启动；kernel的“intentional reset”不证明朋友手工操作。
  自动恢复服务可能重新通知端口/重启MM，须看实际日志，不能只凭PID变化归因。

当前已经通过的派生注册不应被这几项未完成问题抹掉；同样，也不能用它们的代码修复状态替代实机结果。

## 11. 记录、源码与现成证据

### 测试记录唯一追加位置

由开发 agent 将新卡测试或复测写到 `docs/PROJECT_HANDOFF_2026-09-12.md` 第12节，
插在 **`SIM_CARD_TESTS_APPEND_BEFORE_NOTE` 的HTML定位标记之前**，固定尾注永远留在末尾。
卡标签/日期/轮次不重复，失败记录不覆盖；第1/5节只更新摘要。本文不再维护另一份不断追加的测试流水账。

### 源码入口

| 文件/目录 | 重点 |
| --- | --- |
| `backend/src/connectivity/modems/ims/cellular_ims/plan.rs`、`runtime.rs`、`live.rs` | 派生、候选执行、错误阶段、注册/续期与资源恢复 |
| `backend/src/connectivity/modems/ims/cellular_ims/native_bearer.rs`、`pcscf.rs`、`channel.rs` | UE归属、地址/路由、P-CSCF及真实发送通道 |
| `backend/src/connectivity/modems/ims/vowifi/profile_store.rs`、`carrier_catalog_v7.rs` | 来源绑定、LTE ready/unknown、库缺失与derived解析 |
| `backend/src/hardware/devices/qcm410/primary_ims_session.rs`、`primary_ims_lifecycle.rs` | 私有MM bearer、unique owner、原子归属、取消/退出/恢复 |
| `backend/src/hardware/devices/qcm410/ims_bearer.rs`、`netdev.rs` | QCA410端点契约、地址族、netdev配置 |
| `backend/src/platform/netns.rs`、`backend/src/services/ue_netcfg.rs` | namespace、地址和路由操作 |
| `backend/src/platform/config.rs`、`backend/src/api/handlers.rs`、`backend/src/main.rs` | 三槽校验、API与保存配置引发的重注册 |

本机已有 `.codex-cfssh-results/sim03-natural-refresh-684e2a7-20260912-evening.verified.json`，
包含48个样本的首末引用、watcher verdict及续期前后快照路径；这些日志不随Git携带。
没有它们时，以本文明确记录的历史结论作背景，重新获取当前状态，不能捏造缺失的原始证据。

参考旧版为用户提供的：
`https://github.com/lilith-rong/SimAdmin-Enhance/blob/Backup-Vowifi-and-VoLTE/Volte/simadmin_1.1.7-beta8.tar.gz`
（metadata commit `930365d`）。可作静态对照，不默认覆盖当前设备；两条分支的版本号不能直接比较能力高低。

## 12. 提交、构建与发布约束

- 修改由唯一开发 agent负责。只读agent只提交报告；通用IMS修复单独成commit，移植到1.1.5后仍需回归。
- 不在本地进行 Rust 编译、测试或打包；使用项目 GitHub Actions。本地格式、文档、纯Python规则检查可以执行。
- 新开发分支必须先配置候选/验证CI，不能因为没有自动构建就调用未经检查的手动发布workflow。
- 1.1.4旧修复分支的push只产出候选，不代表旧 `workflow_dispatch` 也不会发布；执行前检查当前workflow门禁。
- 未完成必要旧卡回归前，不合并master、不发布beta4、不删除未合并分支、push key、备份或用户临时文件。
- 不提交私密完整版、原始会话、私钥、Cookie、完整身份/AKA日志。根目录`.tmp-*`和`.codex-*`不是可随意清理的垃圾。

## 13. 可复制给新 agent 的任务说明

```text
请先读取 docs/IMS_DERIVED_FALLBACK_HANDOFF.md；
需要连接凭据时使用用户安全提供的 IMS_DERIVED_FALLBACK_PRIVATE.md，不把内容输出或提交Git。
你默认是只读分析agent：检查指定代码/脱敏日志及授权只读状态，向唯一开发agent汇总问题，
不要改代码、配置、profile顺序、服务、部署、卡或触发retry，也不要争抢开发者的工作目录。

IMS测试线为fix/sim02-catalog-aka-baseline；1.1.5在独立dev/1.1.5-modem-backends工作区。
设备历史已验证程序是684e2a7/1.1.4-beta3，不要把文档HEAD当作设备程序。
SIM-03的derived首槽IPv4/UDP注册及一次原socket自然续期已通过；
派生首槽、来源内部derived解析和真正的外层阶梯兜底必须分开报告。
优先核对当前设备和已有证据，再分析SIM-01/02的新MM路径、IPv6问题及后续卡。
设备快照不是实时状态，前面的PID/bearer/IP都不能硬编码。
保留UE隔离、配置/费用意图、MM/proxy/DATA6和管理网络，不重放旧部署或实验脚本。
报告事实、假设、版本/卡/profile/地址族、证据和最小建议步骤，由开发agent统一修复。
开发agent新增测试记录统一写入主交接文档第12节、固定尾注之前。
```
