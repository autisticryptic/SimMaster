# SimAdmin 开发计划与验收进度

> 最后更新：2026-09-08 03:21（Asia/Shanghai）
> 用途：持续更新的开发进度报告、发布门禁和后续核对依据。
> 状态：beta1 / e55780a 已发布、部署并通过原通道自然续期；当前网络两路均未接受 outbound，实机双注册不标为完成。现进入阶段 B，在独立分支 refactor/1.1.4-beta2 开发纯 Rust DNS 与命名迁移；不改动设备或 beta1 发布。朋友的手机卡问题仍暂缓。

## 1. 目标与不可变约束

- 主版本维持 **1.1.4**。本次只发布用户指定的 **1.1.4-beta1**、**1.1.4-beta2**。
- 两阶段分开提交、构建、验证；不擅自生成 1.1.5/1.1.8 等正式版本。
- Beta 必须是 GitHub pre-release，不设为 latest 正式版；版本、commit、包内 meta.json、校验和必须对应。
- **本机不编译后端**。后端编译与回归测试通过 GitHub Actions；设备只安装校验过的 Release 二进制。
- 410 设备使用用户指定的密码 SSH。此文档及提交中不保存密码、私钥、会话令牌、IMSI 或认证报文。
- 部署前检查所有线路的活动通话，备份二进制、前端、配置及数据库；直接 SCP 未压缩文件，保留回滚路径。
- 不清理已有 `.tmp-*`、`.codex-*` 工作文件，不覆盖无关修改。
- 不恢复固定 120 秒 REGISTER 测试间隔，不改变已验证的单路 refresh 的安全上下文/事务行为。
- 业务优先级维持 **VoWiFi → 4G/5G IMS → CS**，不擅自开启用户禁用或硬件不支持的业务路径。
- 正常 refresh 必须续期原绑定；重新初始注册、重建 bearer/SA 不算 refresh 成功。
- 用户再次明确故障顺序：蜂窝 IMS 先注册成功，VoWiFi 后注册成功，此后蜂窝 IMS 无法 refresh；重新协商安全协议可以恢复注册，但这不构成问题已修复的证据。
- 第一目标是保留两个真实有效的注册并独立续期。不得将主动关闭蜂窝 IMS、只保留 VoWiFi 包装成“双注册修复完成”。
- 缺少 `Require: outbound` 是当前流程未协商 RFC 5626 的证据，不应泛化为整个运营商永远不支持双注册；同时必须遵守所选 IMS 多注册机制的协议前提，不能伪造协商结果或无依据强行建立第二流。
- 双注册确实不可用时，IMS 接入/业务回退按 VoWiFi → 4G/5G IMS → CS；CS 是电路业务回退，不是第三个 IMS 注册。
- 日志只进行游标增量采集，筛出关键事件后本地留存并分析；不反复查询完整 journal。
- “两路开关开启”不等于双注册成功；不能用修改状态标志或绕过网络协商伪造并发成功。

## 2. 接手时已核实的基线

| 项目 | 当前事实 | 验证级别 |
| --- | --- | --- |
| 源码 HEAD | `3b5078e`，规范化 REGISTER 的 Supported/Require 并增加协商诊断 | 本地已核对 |
| 工作区 VERSION | `1.1.4-beta1`；HEAD 中错误提交的是 `1.1.8` | 本地已核对，待纠正提交 |
| 现有代码 CI | run `34043239586`，前端、IMS 回归、ARM64、AMD64、Release 均成功 | 已有 CI 结果，非 beta 发布 |
| 设备上次核实版本 | `1.1.7 / 0930fd9`，2026-09-07 00:08 服务 active | 需重新读取当前状态 |
| 错误版本上传 | `1.1.8 / 3b5078e` 仅上传到暂存目录，未执行激活 | 不使用该版本部署 |
| 双注册实测 | 2026-09-06 WLAN 200 OK 无 Require: outbound；Path 无 ob，仅回显 reg-id | **双注册未通过** |
| 120 秒测试间隔 | 既有记录为已关闭 | 提交前再次静态核对 |
| beta1 / beta2 | 尚无本轮可核实的提交、构建、部署证据 | 未完成 |

已实现的 outbound 客户端机制不等于当前运营商已经接受双注册。旧抓包认证请求存在 IP 分片观察盲区，不能据此断定客户端丢头。需使用已改进的分片重组采集和最终发送日志核实。

## 3. 阶段 A：既有修复收尾与 1.1.4-beta1

### A1. 基线与发布流程

- [x] 核对工作区、版本差异、最近提交及已有交接资料。
- [x] 建立本计划；明确真实验收标准及未完成项。
- [x] 再读设备版本、服务、无通话状态、两路注册策略：19:46 设备仍为 1.1.7/0930fd9、active、无通话，仅 VoWiFi 注册。日志继续沿用增量游标。
- [x] 本地 VERSION 已改为 1.1.4-beta1，不递增正式版；尚未提交或发布。
- [x] 已本地修正 push/dispatch 的 beta pre-release / 非 latest 规则；12 个 Python 回归测试通过，包含 workflow 接线检查。尚未执行本 commit 的 Actions。
- [x] 已读取远端 release 清单，仍有 v1.1.7 / v1.1.8 正式发布；未删除或改写历史发布。

### A2. 代码与实机验证

- [ ] 复核已有 UI/eSIM/lpac、refresh 改动的代码及已有证据；不把此前未验证内容笼统标记完成。
- [x] 以 RFC 3261/5626、3GPP TS 24.229 的具体条文检查请求与成功响应，不凭猜测放宽双注册门禁。
- [x] 复核“尚未协商”与“当前成功响应未接受多流”的区分、第二接入的能力验证及安全回退；不以默认单注册代替实现双注册。
- [ ] 对照两路注册前后的 Contact/Call-ID/CSeq 摘要、IP-CAN 路由、P-CSCF、SA 生命周期和 reg-event 证据，区分绑定替换、传输失效与 refresh 实现错误。
- [x] 检查初始、认证、refresh、注销 REGISTER 中规范化的 Supported/Require、稳定 instance 和独立 reg-id。
- [x] GitHub Actions 前端、IMS 回归及双架构构建通过，取得对应 Release 校验信息；最新 run 34142206598 / e55780a。
- [x] 部署已校验的 beta1 候选产物，保留可回滚备份；先排除活动通话。
- [x] 采集完整 WLAN 初始/认证/refresh/注销请求及蜂窝初始请求的脱敏字段；蜂窝受保护认证/响应以发送与解析元数据、实时 API 为依据，不假称抓包已解密蜂窝 ESP。
- [x] 核对响应：两路均 200 OK 且匹配自身 binding，但 Require 无 outbound、第一跳无 ob；这不是双注册验收通过。
- [ ] 如协商成立，验证第二接入启动、两个独立绑定、各自自然 refresh，且不使另一接入失效。
- [x] 协商不成立的两路证据已记录；保留当前接入范围的结论，不泛化为运营商永久不支持，**不能宣称双注册修复完成**。
- [x] 最新 e55780a 于 01:46 完成 VoWiFi 原通道自然续期，发送时真实剩余租期 535 秒；固定 120 秒测试为 None。蜂窝已有单路续期证据与本轮回归，不冒充当前网络的双路续期验收。

### A3. beta1 收尾门禁

- [x] 本阶段提交、CI run、release/tag、包 SHA256、设备 SHA256、备份路径已记录；最终候选为 e55780a。
- [ ] 发布说明逐项标明“修复完成”的已验证范围；未完成/网络限制另列，不承诺未经验证的双注册。
- [ ] beta1 验收记录完整后进入阶段 B；未通过的项目不得悄悄勾选。

> 为实机取得二进制而生成的 beta 候选包可先标“待实机验收”；只有实测通过才能更新为“修复完成”。这不等于提前宣称修好了问题。

## 4. 阶段 B：命名迁移、纯 Rust DNS 与 1.1.4-beta2

### B1. IMS/VoLTE 语义与兼容迁移

- [x] 初步清点：42 个后端文件约 2084 处标识符引用、12 个前端文件约 160 处；7 组旧 HTTP 路由，以及持久化配置、活动日志和历史数据库字段需分别兼容，不能全局字符串替换。
- [ ] 用 `cellular_ims` 表示蜂窝（4G/5G）IMS 接入/注册；真正的 LTE 语音功能可保留 VoLTE 语义，不能机械全局替换。
- [ ] 新增语义准确的 API/字段并更新项目内调用；旧端点与持久化配置提供兼容别名或受测迁移。
- [ ] 覆盖旧配置/旧数据库读取、新写入、前端展示、短信、补充业务、语音选路和活动日志。
- [ ] 不改变业务能力判断和 VoWiFi → 蜂窝 IMS → CS 优先级，不用单个注册开关等同短信/语音能力。

### B2. Hickory DNS

- [x] 查阅官方 crate 0.25.2 的 resolver/hosts/system_conf 源码；与 reqwest 0.12.28 所需 0.25 系列一致，启用 tokio/system-config；锁文件只新增所需依赖，未编译后端。
- [x] 清点普通系统解析、HTTP 和 SOCKS5 代理端点的隐式解析；运营商指定 DNS、NAPTR、SOCKS5 UDP DNS 已是 Rust 专用路径，暂不改动其出站选择。
- [x] 已实现共用纯 Rust 解析接口、HTTP builder；数字 IP 不做 I/O，hosts 先于系统配置，4 秒解析上限，空结果和错误不冒充成功；待 Actions 验证。
- [ ] 尊重系统 resolv.conf/hosts，以及 UE 网络 namespace/运营商下发 DNS 的上下文；不得用公共 DNS 替代 IMS 私有解析。
- [x] 每次解析读取当前 hosts/system-config、创建有界 resolver，不共享跨 runtime/namespace 的 DNS socket/cache；保留调用方原来的网络上下文，不借迁移偷偷改变出口。
- [x] 生产 HTTP builder 统一使用新解析器，同时开启 reqwest hickory-dns 防止遗漏路径退回 libc；TS.43 的地址固定覆盖仍保留。
- [x] 新增本地 A/AAAA、数字地址、hosts 别名/更新、配置隔离、NXDOMAIN、超时、resolv.conf/search 测试；新分支 CI 不发布任何版本，结果待运行。

### B3. beta2 验证与发布门禁

- [ ] 本地格式/静态检查及前端测试；后端测试和双架构编译只用 Actions。
- [ ] 在 410 验证设备发现、旧配置兼容、DNS、ePDG/P-CSCF/Trunk、UI 和两路 IMS 状态。
- [ ] 记录实际观察的 refresh 结果；初始注册成功不能替代续期验收。
- [ ] 单独提交和构建 **1.1.4-beta2**（pre-release、非 latest）；核验包和部署 commit 一致。
- [ ] 更新变更说明、兼容性映射、验证结果与回滚步骤，再标记已通过项修复完成。

## 5. 证据与进度记录

每条记录使用绝对本地时间，至少包含：变更/观察、commit、测试或日志依据、结果、下一步。

| 时间（Asia/Shanghai） | 操作/观察 | 结果 | 下一步 |
| --- | --- | --- | --- |
| 2026-09-07 15:41 | 本地状态复核 | HEAD 仍为 3b5078e，仅 VERSION 有已跟踪改动，尚未进入 beta 发布 | 先固定计划与版本流程 |
| 2026-09-07 15:48 | 按用户要求创建 plan.md | 开发/CI/实机/发布验收分开；双注册保持未通过 | 重连设备，纠正 beta 发布逻辑 |
| 2026-09-07 17:54 | 从指定历史会话恢复并核对当前工作区 | HEAD=3b5078e；beta 发布规则未提交；版本策略 12 项测试再次通过；无本地后端编译 | 修正诊断脚本的 API schema，继续双注册验证 |
| 2026-09-07 本轮核对 | 用户再次明确目标与故障顺序 | 双注册及各自续期是阶段 A 目标；重建安全上下文后注册仅算恢复，不能算 refresh 通过 | 按该标准复核实现和实机证据 |
| 2026-09-07 19:46 | 修正诊断脚本的 `status=ok`、`modem.line_id` 解析后读取 410 | 旧版 active、0 通话，仅 VoWiFi；未部署/重启 | 构建新候选包后验证实际协商 |
| 2026-09-07 20:42 | 修正多流能力/即时保活混淆、安全通道提交次序，增加蜂窝先注册等回归 | 本地 Rust 格式及 12 项发布规则测试通过；Rust 未本地编译，新回归尚待 Actions | 独立提交 beta1 候选，CI 全通过后部署 |
| 2026-09-07 21:49 | 提交前本地检查完成 | ESLint、Vite production build、TypeScript type-check、Rust 格式检查、12 项版本策略测试均通过；未本地编译后端 | 提交 beta1 候选，由 Actions 验证后端 |
| 2026-09-07 21:56 | e5bde5f 的 Actions run 34129622356 全通过 | IMS 测试、前端、ARM64、AMD64、Release 成功；v1.1.4-beta1 为 pre-release 且非 latest | 核验并部署候选 |
| 2026-09-07 22:48 | 部署 e5bde5f 到 410 | 包 SHA256 `64bb31a1edcf552f32e8833fd8aaedeb1c7b18a44ed31960ceb7ac6be6e6c5be`；二进制 SHA256 `1a067011360412aac889f7bdad331a0c2cbf96d75f53a0cb0b5b5501f27b05c2`；备份 `/opt/simadmin/manual-backup/20260907-224802-beta1-e5bde5f`；健康检查通过 | 验证实际协商和自然续期 |
| 2026-09-07 22:49 | 新版 VoWiFi 初始及认证请求均提供 outbound | 200 OK 回显自身 reg-id=2，但 Require 无 outbound、第一跳无 ob，租期 3387 秒；当前流为 not_supported，不是双注册成功 | 继续观察自然续期并对照蜂窝先注册 |
| 2026-09-07 23:26 | 核对时发现新增第二流的验证误挡单路切换，补充修正 | 候选选择与实际并行 REGISTER 准入分开；新增蜂窝→WLAN 优先回退及显式接入切换测试；新修改待独立 CI | 同一 beta1 候选修订，核验标签/commit/包一致后再激活 |
| 2026-09-07 23:37 | e5bde5f 的 VoWiFi 自然 refresh 抓包成功 | CSeq=3，同 Call-ID 摘要，原保护通道；单行 Supported；1653 字节请求重组完整，约 0.36 秒后 200 OK，租期 2711 秒；仍未协商 outbound | 不计为双注册成功 |
| 2026-09-07 23:41 | d8e2148 的 run 34138406522 全通过；自然续期日志另暴露 VoWiFi 剩余租期错误 | 回退切换补丁已通过 CI，尚未部署；VoWiFi 超时分支误用 refresh/cache deadline 当作 hard expiry，已补修正及 32 秒超时/实际过期边界测试 | 合并下一候选验证，不反复部署中间版本 |
| 2026-09-08 00:19 | e55780a 的 run 34142206598 全通过 | 前端、IMS 回归、ARM64、AMD64 和发布均成功；后端未在本机编译 | 最终候选校验部署 |
| 2026-09-08 00:49 | beta1 候选标签有条件前移并部署 e55780a | master、v1.1.4-beta1、meta.json 和设备一致；pre-release / 非 latest；包 SHA256 `13f79154ecf317258411bdb41671276eb6b357a4f8a6c5631e2e854aa49b59e0`；二进制 SHA256 `972696214b3190e2f3a2be2f1d8307b4d0152dda367aab00e6d3141c0c0f30c3`；备份 `/opt/simadmin/manual-backup/20260908-004903-beta1-e55780a` | 保留旧候选与备份，做有界蜂窝先注册对照 |
| 2026-09-08 01:01 | 临时选择蜂窝优先，保持两个开关不变 | 蜂窝注册成功，reg-id=1、offered=true、自身 binding 匹配、租期 3286 秒，但 Require 无 outbound、第一跳无 ob；不是只根据 WLAN 一侧推断 | 恢复 concurrent，验证正常优先级回退 |
| 2026-09-08 01:02 | 恢复原 concurrent 偏好 | 蜂窝显式注销 result=Confirmed 后释放，再建立 WLAN，reg-id=2、租期 3173 秒；同样未协商 outbound。恢复标记 true、保护定时器 inactive | 等最新候选自然续期，禁止算作双注册成功 |
| 2026-09-08 01:06 | 最终设备状态复核 | e55780a / 1.1.4-beta1，服务 active、0 通话、两个配置开关仍开；单注册选择 VoWiFi，当前有效流 not_supported | 不进行额外重连，保留自然续期观察窗口 |
| 2026-09-08 01:46 | e55780a 原通道自然续期实测通过 | CSeq=3，security_verify=true、reused_access=true；发送时剩余 535 秒，约 0.38 秒后 200 OK，续得 3041 秒；没有重建安全协议来伪装 refresh | 按已验证单注册保护及当前网络限制收尾阶段 A，进入 B |
| 2026-09-08 02:20 | 独立分支完成普通系统 DNS 迁移第一批代码 | hickory-resolver 0.25.2 + system-config；显式及 SOCKS5 隐式 lookup_host 已移除；HTTP 接入同一入口；cargo metadata 仅解析/下载依赖和更新锁文件 | 在新分支运行不发布版本的 GitHub Actions；命名迁移尚未动手 |
| 2026-09-08 02:40 | DNS 分支 0257374 / run 34151549316 首轮验证 | 后端全部测试编译、前端与 15 项 Python 检查通过；3 项 DNS 网络测试错误地使用 .invalid，Hickory 按 RFC 6761 本地拒绝，未到测试服务器 | 改为 .test 并验证服务器收到真实 A/AAAA 查询；补充命名迁移映射，重新运行 CI |
| 2026-09-08 03:00 | DNS 修订 87eead1 / run 34153925916 全通过 | 本地 DNS 真实 A/AAAA 查询、配置隔离、NXDOMAIN、超时和现有 ePDG/SOCKS5/Trunk/IMS 回归，以及前端、15 项 Python 检查均通过；不发布、不部署 | 开始命名迁移 |
| 2026-09-08 03:21 | 实施第一批类型和 HTTP 名称迁移 | 蜂窝接入类型用 CellularIms*，蜂窝/WLAN 共用 profile 类型用 ImsProfile*；新增 7 组 canonical API、8 个后端 handler 和 7 个前端 client 方法新名称；旧端点和 JSON 字段保持原状 | 新增私有 D-Bus HTTP 鉴权/响应一致性测试；随后单独做持久化字段与模块迁移 |

## 6. 当前优先事项（阶段 B）

**beta1 保持设备上的 e55780a。阶段 B 在 refactor/1.1.4-beta2 分支独立开发和 CI 验证，完成命名兼容迁移与 DNS 实机验证后才发布 beta2。**

- [x] beta1 发布规则、代码、CI、部署与自然续期证据见上表，不能再沿用旧暂停时的版本/未提交状态。
- [x] 朋友的手机卡任务暂缓，无设备/失败材料，不推测其原因。
- [x] 恢复阶段 A → B；beta1/beta2 版本和真实验收条件不变。

### 暂停点与续接注意

- 本地 `.github/scripts/release_version.py` 和对应测试已完成，12 个测试通过。
- beta1 标签/master 均为 e55780a；当前开发分支不触发 Build-Release，只触发 Validate Beta Refactor。
- `docs/releases/1.1.4-beta1.md` 明确候选版和未验收双注册，不标注双注册修复完成。
- 部署前通话检查的 API schema 误用已修正，本轮验证 0 通话；实际部署前仍需重新检查，不能沿用过时检查。
- 最新日志游标为 `/tmp/.simadmin-beta1-e55780a.cursor`，事件文件 `/tmp/simadmin-beta1-e55780a.events.jsonl`。本地继续使用 `.codex-beta1-final-collect.sh`，不要重读完整 journal。
- 本地 `.codex-beta1-release-verified.json` 是最终包校验证据；`.codex-beta1-ci-run.json` 对应最新 Actions。e5bde5f、d8e2148 的 CI/候选历史另有本地留档。
- 本地 `.codex-beta1-final-wire-read.sh` 读取有界脱敏抓包；抓包仅含 REGISTER 元数据，自动停止，不保存 Authorization/密钥/SIP 原文。
- 最新 e55780a 的 01:46 原通道自然 refresh 已通过；蜂窝先注册顺序和优先级回退已验证，但本网络双注册没有通过。
- DNS 分支 CI 已通过；当前类型/API 名称迁移第一批正在验证。下一步处理模块、runtime 成员、持久化字段/枚举别名及前后端数据契约，不改历史 SQL 列或真正的 VoLTE 语音字段。全部验收后才改 VERSION 为 1.1.4-beta2 并发布。
- 命名迁移映射、旧 JSON/数据库/API 兼容要求见 `docs/IMS_NAMING_MIGRATION.md`；目前是待实现清单，不是已完成声明。
- 不清理已有临时文件或覆盖无关改动。不要把开发分支 DNS 代码直接安装到设备；仍只部署校验过的 Release。
