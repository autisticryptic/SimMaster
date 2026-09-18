# SIM-04：MM 配置读取修复与临时 profile 对照（2026-09-17）

> **后续续接（2026-09-18）**：`e8bff12` 已补完 MM 双栈请求与实际授予处理并通过 CI；新双栈/临时 IPv4 窗口仍未注册，均已恢复。最新证据见 [9/18 专项记录](IMS_SIM04_MM_DUAL_STACK_2026-09-18.md)。以下保留 9/17 的时间边界，不代表双栈代码仍未完成。

> **当前结论：代码与 CI 已推进，SIM-04 仍未注册。** 三轮窗口分别记录；中止、失败、恢复均不回填为成功。
> 全程使用 ModemManager。没有启用 native、借用 MM 的 WDS client、运行 beta8、重启 MM/基带/系统、发短信或拨号。
> 本轮没有修改初始 EPS 附着配置，也没有强制断开 AT CID 2。下一步涉及重新附着，需要单独明确维护授权。

## 1. 应从哪里继续

- **实际开发/测试树：`SimAdmin-1.1.5`，`dev/1.1.5-modem-backends`。**
- 当前功能提交：`272aa9bf38c647fe76ebf6263a3c71f4aa77888e`。
- 相邻 `SimAdmin` / `fix/sim02-catalog-aka-baseline` 留作旧 IMS 基线；其后端仍为 `684e2a7`，不会自动获得新修补。
- 设备恢复后运行的也是原 `684e2a7`，不是本轮候选；当前 IMS 保持关闭，避免原程序重复失败。
- 共享版本字符串仍为 `1.1.4-beta3`，不是正式发布了 1.1.5。不要仅凭版本字符串判断程序。

## 2. 已提交的修补：272aa9b

### MM 自身的 IP/DNS

QCA410 provider 通过固定 MM unique owner 的 `GetAll(Bearer)` 读取 `Ip4Config/Ip6Config`：

- 验证 Connected、Interface、APN，以及读取前后的 MM owner / session liveness。
- 仅接受当前实际启动地址族；解析 typed method/address/prefix/gateway/dns1..dns3。
- 未发布配置可有界等待；类型/地址族/前缀错误、未支持的 DHCP/PPP 不再通过猜 AT 地址掩盖。
- DNS 不冒充 P-CSCF；MM 1.18 的标准 Bearer IP 字典没有 P-CSCF 键。
- 日志明确 `settings_source="modemmanager_bearer_ip_config"`，可观察 family、DNS 数量与网关是否存在。
- 仍由 MM 创建/持有 WDS；数据网卡继续在该线路 UE namespace 内，控制节点不移动。

代码：

- `backend/src/hardware/devices/qcm410/primary_ims_settings.rs`
- `primary_ims_lifecycle.rs::MmBus::ip_settings`
- `primary_ims_session.rs::PrimaryImsSession::read_ip_settings`
- `ims_bearer.rs::wait_for_current_settings`

### AT 补查的关联约束

`cellular_ims/pcscf.rs::discover_active_pcscf_with` 仍仅读 active IMS APN/context，固定并复核集合；新增要求 P-CSCF 对应族的 AT 本地地址出现在已持有 bearer 的地址集合中，不能只凭相同 APN 借用另一上下文。

**已知限制**：这是一条保守的完整地址匹配规则，不把“同 /64”直接当成完整会话归属证明。T03 观察到 MM/AT 地址不同但同 /64；需要进一步审查不同 IPv6 IID 的合法关联语义和旧卡回归。T03 的 AT 结果本身没有 P-CSCF，因此本次没有证据表明该过滤规则丢弃了一个本来可用的 P-CSCF。不可为制造通过而直接放宽成任意同前缀。

### CI 与产物

- [Validate Beta Refactor 35170296636](https://github.com/autisticryptic/SimMaster/actions/runs/35170296636)：success。
- [Build-Release 35170296661](https://github.com/autisticryptic/SimMaster/actions/runs/35170296661)：success；前端、Rust、arm64/amd64 均成功，Publish Release skipped。
- 新增 15 个 Rust 用例：8 个 typed 配置解析、3 个私有 D-Bus、4 个 AT 地址关联；已接入并由上述 Actions 执行。
- 本地 65 项 Python、Rust 格式和差异检查通过；没有本地 Rust 编译/测试。
- 临时 profile 的回滚另有 6 项本地离线 shell/mock 测试，通过后才操作设备；不是产品代码 Rust 回归的一部分。

| arm64 核验项 | 值 |
| --- | --- |
| artifact ID | `10476581920` |
| ZIP SHA256（与 API digest 一致） | `a97f334f792a8d8b348c713754cc78c6072b217f39c665af953310493331bfe0` |
| tar.gz SHA256 | `d97cc89f293cb07910d6c74718ad9fb9c2bc53bf5b97b027b6642a0627860a3f` |
| 二进制 SHA256 | `7498ec4e5524226ca0b296cf52b719ca16979b862928fefd743ec8a7fadce0b8` |
| metadata | commit `272aa9b`，`aarch64-unknown-linux-musl`，ELF64 AArch64，MD5/嵌入 commit 已核对 |

## 3. 设备与共同边界

- SIM-04，归属 `45507`；既有漫游观测为服务网 `46011`。注册失败不能直接归因运营商。
- Debian 11 / aarch64 / `5.15.0-handsomekernel+`，MM 1.18.4；主数据网卡 type=519（raw IP）。
- 候选独立目录、克隆 SQLite/文本配置、回环 API 13000；原程序、数据库、配置和设备资源不覆盖。
- 普通数据、VoWiFi、Trunk、eSIM 控制、自动化和通知在候选内关闭；保留原三槽顺序、漫游和费用意图。
- 每次主动切换前检查无通话；25 分钟回滚 timer 在停止原应用之前建立。MM/proxy 不停止，Wi-Fi 默认路由不变。
- DATA6 initializer 在开始本轮之前已是 inactive/PID 0，本轮保持该状态；不能称它已恢复。

## 4. T01：前置门禁中止，不是 IMS 失败

- case：`sim04-mm-272aa9b-20260917-T01`，约 09:37–09:39（Asia/Shanghai）。
- 测试脚本只接受恢复服务 inactive，但设备实际是 `active/exited`，MainPID/ControlPID=0，执行已结束。
- 门禁退出74时，**原应用尚未停止、候选尚未启动、IMS未开启**。
- 已立即恢复 timer、释放本轮标记并停用已完成回滚的 timer；原程序和 MM PID 不变。
- 门禁改为“无主/控制进程 + dead/exited/failed 静止子状态 + 无恢复中标记”，不把 active/running 放行。

## 5. T02：MM IP 字典读取成功，但 DNS/PCO 仍空

- case：`sim04-mm-272aa9b-20260917-T02`。
- 约09:46启动独立候选，09:47开启IMS；09:52完成关闭/回滚，09:53复核原服务与API。
- 实际三槽：derived→derived、carrier_catalog→CT-MO、database→derived；均未注册。
- 每槽均出现新的 MM IP 配置日志：family=6、dns_count=0、has_gateway=true。
- 对候选 PID 的私有 receipt 对应 bearer 做 D-Bus 只读复核：
  - Ip4Config：method=0，无地址。
  - Ip6Config：method=2，包含 address/prefix/gateway/mtu，**没有 DNS**。
  - Modem3gpp.Pco：空 `a(ubay)`。
- 没有 P-CSCF，未进入 SIP/AKA。没有 DNS 查询成功证据，更不是自然续期或业务通过。
- 原 DB 在原应用停止期间摘要未变；原程序/主配置/设备资源摘要未变；MM/proxy保持原PID，未遗留IMS receipt。

这排除了“只是程序漏读了 MM 字典里已经存在的 DNS”这一假设；**仍不能从空 Pco 属性断言网络没发 PCO**。

## 6. T03：经 MM 严格新建 IPv6 IMS profile 的对照

### 为什么可以做，为什么不改原 profile

已核对 MM 1.18.4：公开 `ProfileManager.Set` 始终 strict=true；QMI provider 的 check_format 不允许预选新ID。省略 profile-id 时直接进入 Create Profile，不走 best-match 更新。

因此本轮：

1. 先备份完整 MM profile 列表并固定 unique owner；不向 Set 提交既有 profile-id。
2. 通过 MM 新建 APN=ims、IPv6、无PDP鉴权的临时 profile；核对返回新ID不在原列表、旧条目未变。
3. 只在这份候选进程设置实际返回的 `SIMADMIN_VOLTE_IMS_CID`，不改线路持久配置。
4. 回滚先停候选并清理其 bearer，再核对 MM owner / profile 列表快照，只删除已确认的新ID；未知结果或外部变化均阻断自动删除。
5. 删除后核对原 profile 列表恢复，再恢复原应用。没有借用 MM 的 WDS CID，没有调用 AT CGACT 强制去激活。

### 实际结果

- case：`sim04-mm-272aa9b-20260917-T03`。
- 16:55左右进入维护窗口，MM返回新 profile ID **3**；原 profile 1/2 未变。
- 16:56候选启动，16:58只开启IMS；三槽仍全部在 P-CSCF 阶段失败。
- MM bearer 的 Properties 确认请求 `profile-id=3`、APN=ims、`ip-type=2`（MM flag，IPv6）；Ip6Config 有地址/网关、DNS=0，Pco为空。
- AT 只读观察如下（同一MM路径代发）：

| 项目 | 观察 |
| --- | --- |
| CGDCONT | 1=IPV4V6/ctlte，2=IPV4V6/ims，3=IPV6/ims |
| CGACT | CID2活动，CID3不活动 |
| P-CSCF 上报标志 | CID2为0,0,0；CID3为1,1,1 |
| CGCONTRDP=3 | 无可用context行 |
| CGCONTRDP=2 | APN=ims，共7列，DNS为空、未提供P-CSCF列 |
| MM/AT地址交叉采样 | 全地址不同，但属于相同 /64；两者IID均非零 |

**解释边界**：MM profile-id、AT CID和初始EPS上下文不能未经证明一一等同。上表不证明MM忽略了profile3，也不证明它与CID2完全无关；“新建了IPv6 profile”不能被扩张为“已验证全新独立IMS PDN且排除了所有承载差异”。

17:08左右关闭候选、删除临时profile3并逐字核对原MM profile列表恢复；程序/配置/DB/设备资源校验通过，原应用恢复，测试标记与IMS receipt清空。随后Web登录与无通话状态复核通过。结束脚本的一次立即登录早于原HTTP就绪，后续复核已通过；它不表示回滚主体失败，脚本现已补HTTP就绪等待。

## 7. 新观察：初始 EPS 附着本身就是 IMS

T03恢复后的独立只读观察（约17:10）：

| MM 属性 | 值 |
| --- | --- |
| InitialEpsBearer | `/org/freedesktop/ModemManager1/Bearer/0` |
| InitialEpsBearerSettings | profile-id=2，APN=ims，ip-type=4（IPv4v6） |
| 初始EPS bearer的Properties | profile-id=-1（未提供有效ID），APN=ims，ip-type=2（IPv6） |
| EpsUeModeOperation | 3 |

MM QMI源码从 LTE attach PDN list 的首项加载 default_attach_pdn；这里读取到的是MM公开的初始附着设置，不是根据普通 Bearer.Properties 猜测。

这是一条重要线索：当前初始附着已使用IMS APN，而应用在其后设置普通PDP上报/创建bearer，并不等同于让初始附着重新协商PCO。但还需保留以下限制：

- **未证明初始EPS用ims一定是错误配置。** 某些网络/终端策略允许这样工作。
- 未证明该设置由哪一版程序、MM/NV、SIM或人工操作引起。
- IPv6地址同/64、不同IID不等于完整WDS会话归属证明。
- MM 1.18.4 的QMI bearer Current Settings读取了DNS等字段，但未请求/暴露标准Bearer IP字典中的P-CSCF；QMI provider 的源码未见对应Pco更新链。Pco属性存在/为空不等于网络层有/无PCO。
- 本轮仍没有网络SIP拒绝、AKA失败或运营商侧拒绝码，不能宣布“漫游未开通”或“IMPI/realm派生错误”。

## 8. 下一步与维护边界

保持在 `SimAdmin-1.1.5` / MM 分支，不部署 beta8 整包，不接管主 QMI。

1. 先明确是否允许一次**由MM执行的初始EPS重新准备/重新附着**窗口：会短暂影响蜂窝驻网，但不停止MM、不重启系统、不改变Wi-Fi管理链路。
2. 若涉及初始附着 APN/IP类型，必须先保存MM完整原设置并确认有效目标值；不能因为设备有 `ctlte` 条目就擅自把它认定为应使用的漫游策略。
3. 初始附着设置可能是持久化状态，不属于当前纯应用配置克隆的隔离范围；不能悄悄变更，更不能在后台重试中自动切APN/重置射频。
4. 若获准，分别记录上报设置在附着前/后的时序、MM/AT/PCO来源、实际地址族/上下文、SIP是否开始，结束时按约定恢复。重新附着不能计作原会话续期。
5. 另一条软件方向是审计/扩展 **MM provider 的PCO暴露能力**，不是在SimAdmin偷偷借MM内部WDS client。涉及MM版本/服务变更同样应另设维护窗口。
6. 不应仅因本次没有P-CSCF而继续改AKA/安全算法、猜P-CSCF地址或把外网DNS当IMS P-CSCF。

当前尚未自动集成新profile租约、改变默认双栈语义、调整初始EPS或完成SIM-04注册。临时profile试验不能冒充产品已实现这些能力。

## 9. 可复核位置

- 产品源码与CI：本文件第2节；逐卡记录见 [项目交接第12节](PROJECT_HANDOFF_2026-09-12.md#12-后续多卡测试记录持续追加)。
- beta8静态对照：[相邻IMS树的综合文档](../../SimAdmin/docs/IMS_DERIVATION_BETA8_COMPARISON_2026-09-17.md)。它冻结于此前源码基线，272aa9b的变化以本文为准。
- 本地候选校验：`SimAdmin/.codex-cf-candidates/272aa9b/verification.json`。
- 本地安全事件摘要/脚本：`SimAdmin/.tmp/sim04-mm-20260917/`，含T01～T03分离记录；不是新clone必备文件。
- 完整配置、MM profile快照、原始journal与AT结果保留在仓库外受限私密目录，不随本文提交。不记录完整IMSI/ICCID、Cookie、密码或AKA材料。
- MM 1.18.4参考源码：`mm-bearer-qmi.c`（get_current_settings/get_ipv6_config）、`mm-broadband-modem-qmi.c`（profile manager、initial EPS）、`mm-iface-modem-3gpp-profile-manager.c`（strict Set）；来自 [上游1.18.4](https://github.com/linux-mobile-broadband/ModemManager/tree/1.18.4)。
