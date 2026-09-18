# SIM-04：MM 双栈请求、实际授予与精确 IPv4 对照（2026-09-18）

> **当前结论：`e8bff12` 的实现、CI 和候选包校验通过，SIM-04 仍未注册。**
> 本轮保持 ModemManager 管理承载；没有启用 native、借用 MM 的 WDS client、运行 beta8、修改初始 EPS 设置、触发重新附着、发送短信或拨号。
> T01 准备中止、T02 P-CSCF 失败、T03 IPv4 承载失败分别保留。全部已结束；08:28–08:37（Asia/Shanghai）独立复核原服务、profile、MM/proxy 和候选/timer 停止状态。

## 1. 开发位置与检查点

- 工作树：`SimAdmin-1.1.5`，分支 `dev/1.1.5-modem-backends`。
- 功能提交：**`e8bff12affb7b691d864606f9303156d496760d2`**。
- 相邻 `SimAdmin` 保留旧 IMS 基线和参考分析；不切分支、不覆盖其业务源码。
- 本轮从 `2026-09-17.jsonl` 恢复：该会话已提交 `272aa9b`，但双栈修改只完成 settings/session/lifecycle 的部分内容，驱动尚未接完。
- `272aa9b` 的 MM IP/DNS 与 AT 地址关联、9/17 临时 IPv6 profile 试验见 [9/17 专项记录](IMS_SIM04_MM_REPAIR_2026-09-17.md)。
- 应用版本仍为共同基线 `1.1.4-beta3`，不是发布了正式 1.1.5。恢复后的设备运行原 `684e2a7`，不是本轮候选。

## 2. beta8 参考与本轮补证

用户本轮明确补充：**SIM-04 可以在 beta8 注册 IMS**。这是修复的参考事实；本轮没有重新运行 beta8，也没有取得新的同条件 A/B 报文，不能把本项目的失败反写为 beta8 不支持这张卡。

已完整阅读 [beta8 综合比对](../../SimAdmin/docs/IMS_DERIVATION_BETA8_COMPARISON_2026-09-17.md)。该文源码基线是 `269be65`；其中“未读 MM IP 属性”“驱动只消费首族”等描述必须结合 `272aa9b` 和本轮更新阅读，不能当作最新源码。

### 同哈希 IDA 核验

IDA MCP 当前输入与参考文件均为：

- SHA256：`210c35b11f54dd240a83e90dd08d5e8a8f4f2cea227ce3a0503a9ced4140f9b7`。
- MD5：`d0903ceab475bacaf00e7ef45d1403c5`；IDA base=0。
- 本轮只读 metadata、`0x196634` 反编译、`0x1A0A80` 反汇编，并在同哈希本地文件核对格式块；没有执行样本或修改 IDB。

补证如下：

| 位置（IDA base=0） | 证实的行为 | 边界 |
| --- | --- | --- |
| `0x196B58`，格式块 `0x518660` | MM 创建命令包含 `--create-bearer=profile-id=…,apn=ims,ip-type=…,allow-roaming=…` | beta8 也传 profile pin，不是仅依赖 `ip-type` |
| `0x1A0AB0`，格式块 `0x51819A` | 准备目标 context 时构造 `AT+CGACT=0,…` | 不在当前设备重放，不能强断既有 CID 2 |
| `0x1A0AE8`，格式块 `0x518846` | 按本轮 PDP 类型定义目标 CID / `ims` | 须结合它的 lease 路径；不授权覆写别人的 profile |
| `0x1A0B44`，格式块 `0x518860` | 定义后构造目标 CID 的 P-CSCF 上报 `1,1,1` | 是时序差异证据，不是本轮根因已经确定 |

### MM 1.18.4：profile-id 优先于 ip-type

已核对 [上游 `mm-bearer-qmi.c`](https://github.com/linux-mobile-broadband/ModemManager/blob/1.18.4/src/mm-bearer-qmi.c)：

- `load_settings_from_bearer()` 遇到有效 `profile-id` 后提前返回，忽略额外 APN/IP 类型等设置。
- `get_profile_ready()` 读取该 profile，再经 `load_ip_type_settings_from_profile()` 决定 IPv4/IPv6；`MM_BEARER_IP_FAMILY_IPV4V6` 会打开两族处理。
- 连接结果分别保存实际成功的 IPv4/IPv6 handle 和 IP config；请求两族不保证两族都成功。

**因此要区分三个值：线路请求、D-Bus `Properties.ip-type`、实际 `Ip4Config/Ip6Config`。**
旧程序把切片截为第一族并仅消费该族确实是缺口，但仅看到旧请求 flag=2，不能断言 MM 在已 pin 的双栈 profile 上只尝试过 IPv6。
本轮不自动丢弃 pin、不覆写原 profile 来强制某族；精确 family lease 仍是单独待实现能力。

## 3. 本轮产品代码

涉及 `backend/src/hardware/devices/qcm410/`：

- `primary_ims_settings.rs`：`MmIpFamily` 接受 `[4]`、`[6]`、两种双族顺序；MM flags 为 **1/2/4**，不是 QMI 4/6，也不是把 1|2 当 IPv4v6。
- 同一个已持有 bearer 的 typed GetAll 解析双族；两族都未发布则有界等待，只有一族时只报告真实授予。错误类型、非法地址/前缀、DHCP/PPP 不因另一族有效而被隐藏。
- `ims_bearer.rs`：完整 family 请求贯通 MM，按请求顺序配置所有实际获授地址；`ImsBearerInfo.ip_type` 来自实际授予。日志分别记录 requested/granted 与各族 DNS 数量，不把 DNS 冒充 P-CSCF。
- 配置第一族之前，一次性持久化完整网络计划。两族使用同一个确切 primary netdev，不探测 DATA6、不另开 WDS owner。
- 整个配置批次由 shielded task 持有 lease activity guard；调用者取消后，清理必须等待已提交的配置工作结束。
- `primary_ims_lifecycle.rs`：双地址采用 v2 receipt；旧 v1 JSON 仍可读。禁止已登记计划被替换、缩减或重排；两族都尝试清理，任一验证失败保留记录供恢复。
- **降级边界**：旧二进制拒绝 v2，而不是只清第一族。升级/回滚前必须用新候选清完它的 v2 receipt，不能盲删账本让旧程序通过。
- `netdev.rs`：MM IMS 清理后只读确认目标地址、私有 family 路由表和规则已消失。地址观察不加 `-4/-6`，避免“已无该族地址但接口仍存在”被 `[]` 误判为接口丢失。
- 共享 netdev 的 `ip` 修改命令增加 10 秒 deadline，并 kill/wait 子进程；标准错误读取有上限。普通 DATA6 的接口选择、默认路由和业务策略未改，但本轮没有普通数据硬件回归。

UE 已有的两族地址/DNS 配置和 IMS IPv6 `nodad noprefixroute` 保留，不将 IMS 专属 flags 扩散到普通数据或 VoWiFi。

## 4. 验证与产物

- 本地 **70 项 Python**、全仓 `cargo fmt --check`、`git diff --check` 通过。
- 新增 **23 项 Rust 回归**：实际单/双族授予、请求顺序、非法配置、取消 guard、v1/v2 receipt、计划防缩减、逐族失败后重试、清理 JSON 和命令超时/reap。
- 另外把既有 netdev 回归组接入两套 workflow。Rust 编译、测试和打包均只在 Actions 执行。
- [Validate Beta Refactor 35251229659](https://github.com/autisticryptic/SimMaster/actions/runs/35251229659)：success。
- [Build-Release 35251229505](https://github.com/autisticryptic/SimMaster/actions/runs/35251229505)：success；前端、Rust/私有 D-Bus、arm64/amd64 成功；**Publish Release skipped**。
- 下载测试日志 artifact 并校验 ZIP digest，逐项确认新增用例实际执行成功。候选包也已下载验证，不只是查看 API 元数据。

| 校验项 | arm64 | amd64 |
| --- | --- | --- |
| artifact ID | `10511175241` | `10509504537` |
| ZIP SHA256 | `9a7a98fbf6659498c4a832c0cbefe65a00277e5ee47b5905b35cfe15d5c8dd3a` | `1c229403c36598aaf7c9d85f92f7eda85e228adf009b9e6643d4a98b146b732f` |
| tar.gz SHA256 | `d2cc45f0625473c213aa4321da335295da0ac376f04238fd7166ca6d961c0520` | `235cfb58bd03b25403c534b454eab1499cf58f8ed398e9b938d2888421abc7ff` |
| 二进制 SHA256 | `0060e0745453ddb4eda9acbd2cecef52c48b49f743f8e6a1454891cc73d35ebc` | `5b23e0d79a468e3f03fd98a9dd60946c9c32ec31f550ec67e99c9c6a514aef2a` |

ZIP 摘要与 GitHub API digest 一致；包内 commit/version/target、MD5、ELF64 架构和嵌入 commit 已核对。只有 arm64 进入以下设备窗口。

## 5. 共同设备与安全条件

- SIM-04 / home `45507`，Qualcomm 410，Debian 11/aarch64，内核 `5.15.0-handsomekernel+`，MM 1.18.4。
- 原程序 `684e2a7`，IMS/普通数据/VoWiFi/Trunk 均关闭，线路启用、漫游允许及保存的三槽/地址族顺序不改；切换前均核对无通话。
- 管理走 wlan0；MM/proxy 保持原进程。DATA6 initializer 在本轮开始前就是 inactive/PID 0，结束仍如此，不能写成“恢复了运行中的 DATA6”。
- 每轮独立目录、SQLite backup API 一致性备份、克隆配置/DB、回环 API 13000；不覆盖原安装。自动化、通知、eSIM 写入口和普通业务在候选内关闭。
- 停止原应用之前先设置 25 分钟回滚 timer。若有未知 profile 创建结果、剩余 receipt、owner 或列表变化，则停止自动清理，不猜 ID、不争抢资源。

## 6. T01：准备阶段上传中止，未切换应用

- case：`sim04-mm-e8bff12-20260918-T01`。
- 凌晨准备已完成一致性备份和本轮 marker；上传包时发生 EOF，远端只收到 131072 字节。
- **原应用未停止，候选未启动，IMS 未启用，切换回滚 timer 也尚未创建。**
- 用户继续后，08:02 重连核对原应用/MM PID、原程序/配置、管理路由及无 receipt；确认没有 `original-stopped` / `candidate-started-at`。
- 保留备份和失败记录，写入本轮中止标记后仅释放 T01 的占用 marker；没有删除原数据或盲目续用半包。
- T02 改用有目标白名单的 SSH exec 分块上传，临时文件经远端 SHA256 验证后才改名。离线模拟验证了目标约束、校验先于改名、shell 语法；不把模拟输出算作真实上传证据。

## 7. T02：真实 MM dual 请求只获 IPv6，P-CSCF 仍缺失

- case：`sim04-mm-e8bff12-20260918-T02`。
- 08:11 启动候选、随后单次开启 IMS；08:14 三槽耗尽；08:15 关闭/回滚，08:16:50 独立复核。
- 三次新配置日志均为 `requested_ip_type=ipv4v6`、`granted_ip_type=ipv6`，两族 DNS 数量均 0。
- 对当时候选 PID 所持有 receipt 指向的 bearer 做固定 MM unique owner 的只读交叉检查：

| 属性 | 实际值 |
| --- | --- |
| Connected / Interface | true / wwan0 |
| Properties | APN=ims，profile-id=2，ip-type=4 |
| Ip4Config | method=0，无地址、DNS |
| Ip6Config | method=2，有地址、/64、网关，无 DNS |
| Modem3gpp.Pco | 空列表 |
| receipt | v1，因为本次实际只安装单族；没有产生双族 v2 实机验收 |

| 槽 | requested → effective | effective profile | 结果 |
| --- | --- | --- | --- |
| 1 | derived → derived | `derived_3gpp_lte_45507` | P-CSCF 失败 |
| 2 | carrier_catalog → carrier_catalog | `profile-ct-mo-45507-046a9073cd` | P-CSCF 失败 |
| 3 | database → derived | `derived_3gpp_lte_45507`，database source unavailable | P-CSCF 失败 |

三槽均为 `volte_runtime_all_pcscf_failed`，没有 SIP/AKA 或自然续期证据。此轮验证了“完整请求/实际单族投影”，**没有验证真正双族授予、双族路由或 IMS 注册成功**。

候选关闭后 receipt 清空、网卡归还，原 DB 在原程序停止期间摘要未变，原程序/配置/设备资源校验通过；原服务和 recovery timer 恢复，MM/proxy、Wi-Fi 管理保持。

## 8. T03：经 MM 严格新建 IPv4 profile，承载被拒绝

- case：`sim04-mm-e8bff12-20260918-T03`。
- 先运行既有回滚逻辑的 **6 项离线 mock 测试**：只删已确认新 ID、列表变化/owner 变化/未知结果阻断、已删幂等、删除失败不能算成功。它们不是产品 Rust 用例或硬件成功记录。
- 08:23 由 MM `ProfileManager.Set` 严格新建：APN=ims、ip-type=1（IPv4）、allowed-auth=1，不传既有 profile-id。
- 返回新 ID **3**；逐项确认原 profile 1/2 未变、返回项与列表一致、owner 与列表序列化稳定后才登记归属。
- 只给本轮候选进程设置返回的 IMS CID；不修改保存的线路 family/profile 顺序，不改变初始 EPS。
- 08:24 开启 IMS。三次承载尝试均报：

```text
org.freedesktop.ModemManager1.Error.MobileEquipment.Unknown:
Call failed: internal error: pdn-ipv4-call-disallowed
```

每次随后都有一次 `family=6` 的 network-forced 复试，但固定的 IPv4 profile 仍优先，因此重复同一错误。全轮 **3 次计划承载失败 + 3 次 forced-family 失败，0 次 MM IP 配置成功日志**。

解释限制：

- 这是当前 modem/MM/网络状态下的 IPv4 PDN 拒绝，不是已取得运营商侧 3GPP 原因码，也不是所有设备上 SIM-04 都不支持 IPv4 的证明。
- 修改请求标签并没有把 profile 3 改成 IPv6；这一轮不能记成“新建 IPv6 profile 的有效重测”。
- 原三槽配置保留；首槽 derived 的结果曾被 API 捕获，但最终失败状态清空了 `profile_attempt_results`。不从预设顺序编造未捕获的逐槽 effective 元数据。
- 没有取得本轮可用 IP，未到 P-CSCF/SIP/AKA；也未触发 MM/基带重启。

08:27 左右关闭候选，通过 MM 删除**唯一已确认的新 profile 3**；原 MM profile 列表逐字恢复，原配置/DB/资源校验通过。08:28–08:29 独立复核：原服务健康、IMS 关闭、无通话、无 receipt/测试 marker、wlan0 管理保持，profile 列表仅 1/2。`InitialEpsBearerSettings` 仍为 **ims / profile 2 / flag 4**。08:37 再次核对 MM/proxy 仍为原 PID，T02/T03 候选和回滚 timer 均 inactive。

## 9. 剩余工作与下一步边界

1. **SIM-04 注册仍未完成。** 当前缺口是 IPv6 承载后的 P-CSCF/PCO 可见性或协商条件；不是靠再读一次 MM 字典或改变请求标签就已解决。
2. 若继续实机验证初始 EPS 上报时序，需要用户明确允许一次**经 MM 受控重新附着**的维护窗口：会短暂中断蜂窝驻网，保持 Wi-Fi、MM owner、备份和回滚。不因 `ctlte` 条目存在就擅自改初始 APN，更不强断 CID 2。
3. 不愿重新附着时，可继续只读审计同 owner 的 MM PCO 暴露能力，或收集 beta8 成功时的同卡/同网络/初始 EPS/profile/实际 family 基线。更换 MM 版本或服务本身也须另设维护窗口，不能偷偷借内部 WDS client。
4. **产品级精确 family lease 未完成。** 本次脚本试验不等于自动集成；需要将 profile 本身的 family 纳入计划/forced-family 决策，创建/取消/释放/崩溃时保留归属。不能通过自动覆写原 profile 或丢弃 pin 规避冲突。
5. 双族 UE 网络配置仍是整批失败即停止；一族 readiness/路由失败后保留另一族的独立降级未完成。这与“网络只授予单族”的已实现处理不是同一个场景。
6. 只读审阅另记录既有 MM 生命周期缺口：receipt namespace 仅有名称、底层 release 的代次/归还确认不如 native provider 完整；CreateBearer RPC 自身超时的迟到结果与运行期 owner handover 仍需独立加强。新批次 guard 和 v2 网络账本不应被称为覆盖了所有生命周期竞态。
7. SIM-01/02/03 回归、真实双栈、完整 P-CSCF 来源切换、自然续期、短信/电话/双注册仍分别待验收；本轮没有降低 AKA/IPsec 安全要求，也没有移植 beta8 的宽泛 plain fallback 或 XFRM flush。

## 10. 证据与复核入口

- 本文与 [项目交接第 12 节](PROJECT_HANDOFF_2026-09-12.md#12-后续多卡测试记录持续追加) 记录逐轮结果。
- 候选校验：`SimAdmin/.codex-cf-candidates/e8bff12/verification.json`，包含两架构摘要和 CI run。
- 本地会话/CI 摘要：`SimAdmin/.codex-session-resume-20260917/`。
- 本地受控脚本和脱敏观测：`SimAdmin/.tmp/sim04-mm-20260918/`、`…-t02/`、`…-t03/`；不是新 clone 必备文件，不直接重放。
- 完整配置、DB、原始 journal、MM 属性/profile 快照留在仓库外受限私密目录，不提交到 Git 或转发到对话。
- 下一次操作仍须重新确认设备、卡、服务、boot/owner、管理路由和 receipt；上述时间点不保证阅读时仍相同。
