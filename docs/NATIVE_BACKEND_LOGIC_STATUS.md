# 原生后端逻辑候选：默认 MM，硬件验收延期

> **2026-09-24 更新**：当前审计见 [原生硬件接口审计](NATIVE_BACKEND_AUDIT_2026-09-24.md)，
> 已有 [只读发现](NATIVE_MODEM_DISCOVERY.md)、[AT/URC 事件](NATIVE_AT_EVENTS.md)、
> [短信持久化与送达报告](NATIVE_SMS_INBOX.md)，并补充
> [显式资源恢复](NATIVE_RESOURCE_RECOVERY.md)；最新代码/CI状态见
> [本次接续计划](NATIVE_SMS_AND_RECOVERY_PLAN_2026-09-24.md)。开发统一在 `SimAdmin/master`。
> 用户确认 SIM-04 自然续期、SIM-05 手测完成；下一主线是 SIM-06 中国电信注册失败。
> native 接管仍无端到端实机通过记录；未知资源不因控制代次改变就自动释放。
> 下文为对应日期的历史检查点，不覆盖以上最新状态。
>
> **新阶段入口**：用户随后在 2026-09-13 授权自有设备实测。最新过程见
> [Native 自有设备实测与交接](NATIVE_BACKEND_DEVICE_VALIDATION_2026-09-13.md)。
> 该实测记录已纳入版本控制。
> 下文保留先前“仅逻辑、延期验收”阶段的范围和结论，不代表新阶段仍禁止测试。
> 2026-09-15 前一轮续接仅完成代码与 CI；后续已按新授权测试 SIM-04 的 **MM** IMS，
> 未注册并已恢复原服务。不是 Native/混合后端验收，见
> [P-CSCF 实测与 beta8 二进制对照](IMS_PCSCF_BETA8_COMPARISON_2026-09-15.md)。

> 2026-09-13，`dev/1.1.5-modem-backends`。
> 用户要求先推进非 MM 逻辑；现阶段继续以 MM 为主，等 IMS 多卡基线收敛后再做接管测试。
> 本轮不部署、不连接测试设备、不切换 owner。代码接线、离线验证和实机支持必须分开记录。

## 1. 默认与接管边界

- 旧配置没有 `cellular_backend` 时仍选择 `modemmanager`；默认值不额外写入配置文件。
- 可以预存 native 设备描述，但 MM 模式不会探测或接管这些端口。
- Native 必须显式设置 `mode: native` 和 `allow_unvalidated_native: true`，缺省拒绝。
- 后端仅在进程启动时选择；没有活动通话/IMS 会话中热切 owner 的 HTTP 开关。
- Native 当前要求 **MM daemon 未运行**，不自动停 MM，不通过探测激活 MM。
- 未知 native 目标、协议失败、能力缺失都不回退到 MM。
- 存活的 native 物理锁或未解决的会话 receipt 会阻止 MM 启动，避免接管未清理资源。

“MM 优先”指默认仍使用 MM，不是 MM/native 同时争用同一个基带。
同机不同设备分别用 MM/native 的混合模式尚未接通；当前是全局后端选择。

## 2. 已有代码接线

| 范围 | 实现 |
| --- | --- |
| 启动配置 | `backends/config.rs`：默认 MM、显式实验 opt-in、设备/端口/网口唯一性检查 |
| 兼容接口 | `cellular/control.rs`：保留原 API/服务签名；MM 分支沿用原实现，native 分支进入独立 controller |
| 物理控制器 | `backends/native.rs` / `io.rs`：按物理设备串行、sysfs/字符设备归属检查、flock、受限参数、超时/输出上限 |
| 发现/身份 | 显式设备表生成 `ModemBinding`，沿用物理锚点 + slot 的 line ID，不制造 MM 对象路径 |
| 射频/驻网 | QMI DMS/NAS、MBIM Radio/Registration、AT CFUN/CEREG/COPS；未知不等于 RF-on 或 home |
| QMI 偏好 | `management.rs`：设备能力约束的 RAT、运营商、LTE/NR band；白名单 NAS/DMS TLV，不改无关紧急/漫游策略 |
| SIM/AKA | QMI UIM 使用同一物理操作门；AT CCHO/CGLA/CCHC 复用 USIM 解码器；保留 PC/SC 路径 |
| IMS 入口 | Native 不再要求先执行 mmcli；PDP/P-CSCF/语音信箱 AT 查询经 facade；MM 路径保留 |
| QMI/MBIM bearer | 显式 endpoint/interface/session、CID 保留/释放、地址族/前缀检查、存活观察、receipt |
| UE 数据面 | 复用 mandatory UE worker/netns、地址/路由及 generation 检查；不提供宿主 bearer fallback |
| 基带短信 | AT PDU 发送/存储轮询；复用 SMS codec、DB 去重和通知；删除前核对索引内容 |
| 基带电话/USSD | AT 查询、拨号、接听、挂机、DTMF、呼叫等待、CUSD；失败不走 MM Voice |
| eSIM | Native lpac 与 SIM/射频共用物理门；按 QMI/AT/MBIM 选择 reader，拒绝越线设备 |
| 退出/取消 | 禁止 shutdown 后新命令；等待 pending setup/物理操作；不确定分配/清理保留 receipt |
| 运维 | `modem-backend-mode` 无硬件解析配置；`GET /api/modem/backend` 只读脱敏；安装/恢复资源按后端选择 |
| 漫游保护 | 禁止漫游时，查询失败/未明确 home 都拒绝数据连接，不把未知当成未漫游 |

### 两种 “native bearer” 不可混淆

- **MM 默认路径**：QCA410 主 QMI IMS bearer 仍由 MM 创建/持有，独占接口进入 UE。
  已实测的 MM 路径没有被原生实现替换。
- **Native 候选路径**：新 controller 管理 QMI/MBIM 会话，没有 MM CreateBearer/Connect。
  旧 SIM-03 的 MM 注册/续期记录不算新 native 路径的验收证据。

## 3. 仍未完成或未覆盖

### 3.1 离线逻辑强化（2026-09-14，2026-09-15 续接并通过 CI）

- `identity.rs` 修正 EF_AD 低半字节读取、IMSI 长度/MNC 校验，以及只从 `Application ID:` 字段解析 USIM/ISIM AID；兼容同一行值和 qmicli 真实的紧邻缩进值行，不能跨空行/其他字段扫描。异常/超过 16 字节的 AID 不再被截断接受。
- UE worker 增加专用 `ImsIpv6AddrReplace` 和 `AddrWaitReady` 操作：只有 IMS IPv6 使用 `nodad noprefixroute`，普通数据、veth、VoWiFi TUN 保留正常 DAD；P-CSCF/DNS 路由前在同一 namespace 验证精确接口地址非 tentative、非 dadfailed 且仍有效。
- 网络配置批次现在捕获并复核 worker generation；请求不会在代次变化后重放到替换 worker。Native IMS/普通数据路由使用原始 binding，并检查 worker 返回的 `ok/error`，避免路由失败被报告为成功。
- MBIM IP 配置解析改为严格校验地址族、前缀、网关/DNS，去重并拒绝未标记或跨族值；识别真实 mbimcli 输出中带 `[设备路径]` 的地址族标题和没有分组标题的 IP/DNS 字段。普通 native 数据网关先建立 host route，再安装对应族默认路由。
- Native 接口归还必须由原 worker binding 串行执行，再由 provider 核对宿主接口/物理 owner 并保存确认。归还失败、取消、旧 generation 或确认写盘失败时保留 namespace receipt 与接口占用；停止 WDS/MBIM 不再被误当成接口已归还。未确认的 Drop 清理仍停止已知会话，但不删除恢复账本。
- worker 的 net-config/socket 请求采用作用域清理 guard，调用方取消时立即移除 pending 关联项；它不取消已经入队的内核操作，资源仍须显式释放或保留 receipt。
- 这些改动已在 `f148842` 通过 Rust 编译、回归、前端和双架构 Actions（见第 5 节）；本轮未部署或做设备验证，SIM-04 仍没有 AKA/SIP 注册证据。

### 3.2 1.1.5 身份与 IMS 兜底强化（2026-09-15，6391732）

- qmicli 应用枚举先按实际 one-based `Slot [n]` 筛选，再解析 AID；完整 AID 复用于 UIM 身份读取，不从其他槽位借用应用。
- IMS 的 CIMI 查询经统一 AT facade；MNC 元数据缺失时增加只读 CRSM/EF_AD 路径，前后核对同一 IMSI并限制总预算为 12 秒，不猜测 MNC 或修改 SIM 配置。
- 两条自动 profile resolver 共用 home 边界：显式 SIM 事实优先，否则有效自定义元数据与 catalog 规则须无冲突。拒绝两位/三位 MNC 最长前缀猜选、一位 MNC；唯一自定义归属元数据可支撑缺库时的 derived 兜底，显式 pin 语义不变。
- PDP 准备不覆写已有定义/空 APN 占位项；只复用匹配项，或定义已确认缺失且未活动的 preferred CID。不假定其他空闲 CID 被设备支持；无法确认时不写该定义，保留既有 APN-only 后续路径。
- provider 的 `BasebandWedged` 信号有独立错误码，贯通 family/profile 两层终止与前端提示；普通失败不被错误升级为永久 netdev 故障。
- 该阶段只完成逻辑与 CI。DNS RR owner/CNAME/SRV 端口后续已在第 3.3 节补强；P-CSCF 来源/override 优先级、配置预览与 runtime effective 一致性仍待完成，硬件/混合 owner 门槛不变。

### 3.3 P-CSCF 只读等待与 DNS 归属（2026-09-15）

- `1cf849f` / `269be65`：对照同哈希 beta8 的实际 IDA XREF/反编译，活动 IMS 上下文最多读 6 轮、轮间 1 秒、包括 IO 的总预算 12 秒。固定已观察的 CID/APN/PDP 定义，拒绝变化和重复/矛盾 CGACT 行，接受地址前再次核对。不激活、不改 PDP，也不借用 MM 内部 WDS CID。
- DNS 严格验证问题与响应，只接受 Answer 中匹配目标或合法有界 CNAME 链的数据；拒绝无关 glue、异常压缩和跨 RDLENGTH。SRV 端口贯通到 UDP SIP，TCP SRV 不被误用；DNS 仍经 UE worker。
- 本地 61 项 Python、6 项前端 unit 和格式/diff 检查通过；相对 6391732 增加 20 项硬件无关 Rust 回归。新代码未部署，不构成 SIM-04 注册通过。
- SIM-04 的 6391732 / MM 窗口已建立 IPv6 bearer，但 AT 可见 DNS/P-CSCF 缺失，三槽均未进入 SIP/AKA。精确 IMS 地址 flags 和清理/恢复有证据；不能等同于网络/WDS 没有 PCO，也不能称旧路由问题完全验收。逐卡记录见项目交接第 12 节。

不能因为代码能够构建，就将以下项目标记完成：

1. **Native 硬件验收仍未收敛**：BAM-DMUX/data-port 映射、QMI/MBIM 固件差异、SIM/AKA、
   IMS 注册/续期、IPv6、短信/电话、掉线恢复须分别验收；SIM-04 的 MM 失败窗口不替代这些项目。
2. **混合 owner**：尚未实现 MM 在线时对不同 modem 的 inhibition/端口隔离式并行管理。
3. **自动代次恢复**：控制节点代次变化目前要求重启/重新核验；不确定 receipt 需要受控
   reconciliation，自动孤儿会话恢复器尚未完成。不能盲删 receipt 后重连冒充续期。
4. **协议/设备扩展**：AT-only PPP/ECM/NCM 数据面、MBIM/AT 的厂商 RAT/band/reset、
   多槽/MEP 和自动槽位切换仍需专门适配。缺失能力明确 unsupported。
5. **事件与业务长稳**：native 短信/通话当前主要用 AT 存储/CLCC 轮询；原生 WMS/VOICE、
   完整 URC 事件源、索引复用、补充业务等仍需完善及验收。
6. **接口归属证据**：显式配置不代表端点一定能收到 SIP；有 IP 或 netdev 不等于 IMS 成功。
   主 QMI IMS 与 DATA6 普通数据的映射必须分别核对，不能复制其他端口/型号的数字。

因此当前还不能声称已完成所有替换项，更不能据此删除 MM 默认实现或发布 1.1.6。

## 4. 配置与诊断——目前只阅读，不在测试机执行

默认等价配置：

```yaml
cellular_backend:
  mode: modemmanager
  allow_unvalidated_native: false
  devices: []
```

未来 native 维护窗口需要核对：

- `hardware_key` 沿用现有物理槽位锚点，不能使用临时 `/Modem/N`。
- `sysfs_anchor` 是精确物理祖先，不是整个 `/sys/devices` 等宽泛目录。
- `protocol` 为 qmi/mbim/at；`control_device`、可选 `at_device` 必须属于同一物理设备。
- `uim_slot` 必须真实匹配；QMI 会核对 primary GW slot，MBIM/AT 多槽需驱动扩展。
- `ims` / `data` 分别指定控制端点、独占网口及 session ID；需要时填写有设备证据的
  `qmi_data_port` / `qmi_binding`。不提供可以直接复制到真实设备的猜测映射。
- Native 普通数据 APN 为空时不猜 `internet`，需要明确配置。

`modem-backend-mode` 只解析配置；`--require-mm` 可作为服务条件。
API 的 `hardware_validated: false`、`native_hardware_validation: deferred` 是真实验收状态。

## 5. 验证记录

- 本地仅运行 Python 边界检查、rustfmt、shell 语法检查。
- Rust 编译、离线用例、私有 D-Bus API 回归及双架构构建通过 Actions 执行。
- 新用例覆盖默认 MM 不碰预存 native 设备、协议解码、参数/slot 校验、AKA/PDU、
  NAS 字段白名单、session 释放/回滚和超时保留 receipt。
- 首轮 `9aafa06` 的 Actions 编译发现 DNS 地址解析类型推导错误（E0283），由
  `32d4c0d` 修正。后者的
  [Validate Beta Refactor](https://github.com/autisticryptic/SimMaster/actions/runs/34739474728)
  和 [Build-Release](https://github.com/autisticryptic/SimMaster/actions/runs/34739474668)
  均 success，包含后端/前端回归与 arm64/amd64 构建；发布任务 skipped。
- 后续收尾增加：拨号调用者取消后的已确认呼叫清理；协议明确拒绝与结果不确定的
  区分；SMS 删除在同一物理门内核对 SIM/内容，保留部分清理进度；
  lpac 超时后等待子进程退出再释放操作门。
- 该阶段安全收尾提交为 `b1caafe`、`b94c9c2`。当时最终检查点 **`b94c9c2`** 的
  [Validate Beta Refactor](https://github.com/autisticryptic/SimMaster/actions/runs/34740891966)
  与 [Build-Release](https://github.com/autisticryptic/SimMaster/actions/runs/34740891971)
  均 success，包含新增/原有离线回归、私有 D-Bus API 测试、前端和 arm64/amd64 构建；
  `Publish Release` 已核对为 skipped。中间 `b1caafe` 两套 workflow 也均 success。
- 该阶段本地49项 Python检查与 Rust格式/语法、shell语法检查通过；没有本地 Rust 构建，
  没有部署、发布或 native 硬件测试。

### 2026-09-15 中断会话续接

- 恢复断点时 `3d2b363` 仅在本地提交；本轮已推送，并补充 `7f38e39`
  （namespace 归还确认、取消后的 pending 清理）和 `f148842`
  （真实 qmicli 多行 AID、mbimcli 设备前缀解析）。没有覆盖原 IMS 分支或升版。
- 该次代码 **`f148842fd8cee2100db6cc43be35b682ed8664ef`** 的
  [Validate Beta Refactor](https://github.com/autisticryptic/SimMaster/actions/runs/34872758400) 与
  [Build-Release](https://github.com/autisticryptic/SimMaster/actions/runs/34872758415) 均 success。
  已逐 job 核对后端回归、私有 D-Bus API、前端和 arm64/amd64 构建成功，
  `Publish Release` 为 skipped；前一轮 `7f38e39` 的两套 workflow 也通过。
- 本地 **54 项 Python 边界/发布规则检查**、`cargo fmt --check` 和 `git diff --check`
  通过。续接新增 **8 个无需硬件的 Rust 用例**，涵盖取消、未确认/失败归还、账本写入失败、
  Drop 保留归属、真实 CLI 格式及损坏字段；Rust 编译/测试/打包仅由 Actions 执行。
- 两架构 artifact 的 API 元数据均指向上述完整 SHA，查询时未过期：

| Actions artifact | ID | API ZIP digest（SHA256） |
| --- | --- | --- |
| `pkg-amd64` | `10359803807` | `bf0604c5b2a2c5d6d8825df39fe28ee3038173dc96c4d503d263079e8001a4d3` |
| `pkg-arm64` | `10358864149` | `c25fe45e35b629fc8f2ad4fdead69b8ececba62acf2409e7818e7584f6d3f8dc` |

这些是 Actions artifact ZIP 的摘要，**不是**内部 tar.gz 或二进制摘要；本轮没有下载校验
或部署。artifact 会过期，未来设备窗口仍须重新查询、下载并校验，不能直接复用历史路径。

### 2026-09-15 后续开发与候选校验

- 该阶段代码 **`63917329b0e09a074c6e526997b67e9db4df36c9`** 已推送；
  [Validate Beta Refactor](https://github.com/autisticryptic/SimMaster/actions/runs/34877802604)、
  [Frontend Checks](https://github.com/autisticryptic/SimMaster/actions/runs/34877802618) 和
  [Build-Release](https://github.com/autisticryptic/SimMaster/actions/runs/34877802723) 均 success。
  已核对 Rust 回归、前端与 arm64/amd64 成功，Publish Release skipped。
- 本地 58 项 Python 边界/发布规则、6 项前端 unit、rustfmt/diff 通过；Rust 回归净增 11 项，
  另将既有 PDP 覆写用例改为保护现存定义，并把 plan 回归组接入两套 workflow。
- arm64 artifact `10361877321` 已下载，ZIP SHA256 与 GitHub API digest 一致：
  `a7047c322c493bbb9b48d3ef097849f1ae39840d4dcf9f76636b441be95f84dd`。
  内部包 metadata 的 commit/版本/架构以及 ELF64 AArch64、嵌入短 commit/分支均已核对；
  二进制 SHA256 为 `bfee39504de0a871192f0a35295bf02cd0a54a91499bd9d4265e31079e51ed5f`。
  当时没有执行或部署二进制；后续 SIM-04 窗口另见下节，旧阶段记录不回填为注册通过。
- 用户限定实测范围为 SIM-04 IMS 注册、保持 MM；混合后端实测留给其他设备。
  过期入口凭据后来已安全更新，只读基线和受控测试已执行。

### 2026-09-15 SIM-04 窗口与 beta8 对照后续

- 6391732 受控 MM 窗口三槽失败，原服务/配置已恢复；没有电话、短信或混合后端测试。
- `1cf849f` 的 [Validate](https://github.com/autisticryptic/SimMaster/actions/runs/34925540533) 与
  [Build](https://github.com/autisticryptic/SimMaster/actions/runs/34925540557) 均 success；Rust、前端和两架构成功，发布 skipped。
- 最终 `269be65` 补充 CGACT 歧义回归与日志来源说明，已推送；
  [Validate](https://github.com/autisticryptic/SimMaster/actions/runs/34926823080) 和
  [Build](https://github.com/autisticryptic/SimMaster/actions/runs/34926823070) 均 success，发布 skipped。
  候选元数据与二进制对照见 [P-CSCF 专项记录](IMS_PCSCF_BETA8_COMPARISON_2026-09-15.md)。本轮新修补未部署。

**当前检查点（2026-09-15）**：P-CSCF/DNS 修补已推送，但 SIM-04 注册及第 3 节的完整后端缺口仍未完成。
下一名开发 agent 应从混合 owner / 代次恢复等项目继续，不把本轮当作完整替代已完成，
也不提前在 IMS 验证设备启用 `native`。

## 6. 后续 agent 接续

1. 检查本节提交/CI 状态，继续使用独立 `SimAdmin-1.1.5` worktree。
2. 继续第 3 节的逻辑缺口，不能把安全拒绝当作全能力覆盖。
3. 原 `fix/sim02-catalog-aka-baseline` 保留 IMS 基线；用户已授权用 1.1.5 分支的 MM 候选复测 SIM-04。
   真实凭据仍仅在本地私密交接文件，不经本文或 Git 分发。
4. 用户明确安排硬件窗口后，再备份、确认归属、释放已确认资源，分 MM/native 测试；
   不同时控制同一物理 modem。
5. 仅一名开发 agent 修改/部署，其他 agent 只读汇总。
