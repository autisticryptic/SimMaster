# SimAdmin 未完成开发计划

> 状态：2026-08-30 整理版。本文是本仓库唯一的后续开发与验收计划，只记录尚未完成、尚未通过外部验收或仍需收口的事项。
>
> 2026-08-30 合并了原先分散的四份清单（`IMS_REGISTER_FOLLOWUP_PLAN.md`、`BACKEND_REVIEW_TODO.md`、`IMS_ACCESS_REFACTOR_DEVICE_TESTS.md`、`HARDWARE_EXPANSION_TODO.md`）。已完成项和历史验收记录不再保留在文档里——那些在 git 历史中。架构设计说明移到 `ARCHITECTURE.md`。
>
> 本文不把代码中已有的基础能力直接视为产品完成。每项能力只有在对应的自动化测试、真实硬件、运营商网络或发布流程验收通过后，才能从本计划移除。

## 当前结论

SimAdmin 的单线路 VoLTE → SIP Trunk → Asterisk 普通语音路径已经完成实机验证，不属于本计划的待办范围。当前已确认：

- 目标机 VoLTE IMS bearer、P-CSCF、IPsec/XFRM 和 IMS REGISTER 正常。
- Asterisk trunk REGISTER、真实普通号码外呼、运营商 200 OK、双向 RTP、SIP INFO DTMF 和 BYE 清理通过。
- 目标机到 WSL Asterisk 使用 mirrored 网络直连；Asterisk 使用 UDP 8060，未依赖临时 UDP relay。
- 最终 aarch64-musl 版本已用 Zig 构建并部署到目标机；既有 Linphone 账号未被修改。
- VoLTE 定向测试 182 项通过，trunk 定向测试 74 项通过；前端 ESLint、TypeScript 类型检查和 Vite 构建通过。

当前不能称为整体完成，因为多线路真实硬件矩阵、VoWiFi 业务、视频、Ut/XCAP、MWI、E911、CS 音频适配器和正式发布流程仍不完整。

## P0：回归门槛（本轮已恢复）

- [x] 修复或隔离 `connectivity::modems::ims::vowifi::channel::tests::udp_channel_recv_chunk_reassembles_oversized_datagram` 长时间不结束的问题；当前单独运行与全量回归均结束。
- [x] 完整后端回归通过。数量随开发变化，不在文档里固化——以 `cargo test --bin simadmin -- --test-threads=1` 的当次输出为准（2026-08-30 为 1400 passed / 3 ignored，其中 ignored 需要外部 Asterisk/Linphone 或超 MTU 的 WSL2 loopback）。
- [x] 清理默认 Linux binary 的 dead-code / unused warning；未接线的 E911 provider 仍以明确待办保留，不用 warning 掩盖状态。
- [x] 增加前端 `pnpm lint`、`pnpm type-check`、`pnpm build` 的 CI 检查，并固定非交互依赖安装方式（`.github/workflows/frontend-checks.yml`）。

前端显示层已完成一轮与线路隔离契约的收口：Dashboard 分别显示设备、IMS 和 Trunk 状态；线路选择器标明读卡器、离线和槽位冲突；VoWiFi `scaffold_only` 状态明确显示为“未接线”。这些显示调整不代表对应能力已经通过真实硬件或运营商验收。

## P1：VoWiFi 业务闭环

### 语音

- [ ] 使用真实运营商完成 VoWiFi 外呼接通、被叫接听、拒接、未接记录和挂断验收（代码路径与本地模拟已覆盖，仍缺运营商实测）。
- [x] 代码层覆盖 VoWiFi/VoLTE early media、180 provisional、带 offer 的 answer、CANCEL、超时、媒体方向和 re-INVITE 路由；[ ] 真实运营商矩阵仍待执行。
- [x] 本地协议测试覆盖双向 RTP、SIP INFO DTMF、telephone-event、媒体方向、hold/resume 和资源清理；[ ] 真实 VoWiFi 运营商验收仍待执行。
- [ ] 将普通号码测试结果按线路、access、codec、SIP 状态、RTP 计数和脱敏 trace 独立记录（需要授权的真实测试号码）。

2026-08-19 已完成一轮 QCM410/50212 飞行模式实测：VoWiFi IMS、trunk、7201 绑定、`100/183` 和失败资源清理通过；运营商以 `480 Release Call received from CAP` 在接通前释放，故 RTP、DTMF、hold 和视频仍未验收。代码已补齐 trunk/API 呼叫的统一 `Started` 生命周期事件和历史记录竞态测试。

### 视频

- [ ] 完成 VoWiFi H.264 SDP、视频 RTP relay、音频/视频 re-INVITE 的真实运营商互操作。
- [ ] 完成 Asterisk/Linphone 与 VoWiFi 的视频矩阵，包括音频→视频、视频→音频、拒绝升级、保持/恢复和双线路并发。
- [ ] 明确 RTCP、RTCP-mux、视频 payload type 和 codec policy 的支持范围；不支持的能力必须显示为 unsupported。

## P1：多线路和 SIM 身份隔离

至少需要两条真实线路，最好两张相同 PLMN 和两张不同 PLMN 的 SIM，逐项验收：

**这一整节被同一个前提阻塞：410 上目前只插了一张卡，`line_profiles` 只有一条线路。** 代码侧的按线路隔离已经落地（键的设计见 `ARCHITECTURE.md`，`line_id` 只由物理槽锚点 + UIM slot 生成，SIM 覆写另用 `SimBindingKey`），并有双线路单元测试，但没有第二条真实线路就无法验收。

- [ ] 两条线路同时建立 VoLTE bearer 和 IMS REGISTER，各自使用自己的 QMI、netdev、P-CSCF、路由表和 runtime。
- [ ] 两条线路同时建立 VoWiFi TUN/ePDG/IKE/ESP/REGISTER，TUN、代理、DNS、route 和 operator session 不互相覆盖。
- [ ] 相同 PLMN 的两张 SIM 使用不同 effective profile、IMEI、E911、UT、MWI 和 trunk 配置时不串值。
- [ ] SIM 从 modem A 移到 modem B、eSIM profile 切换、拔卡再插回、modem 编号变化后：物理线路配置留在槽位，SIM 覆写跟随 ICCID / EID + profile ICCID。独立读卡器换卡同样按 SIM 键重选覆写，不得用 reader `line_id` 误绑旧 SIM。
- [ ] 一条线路停止、断网、认证失败或 bearer 重建时，另一条线路的 REGISTER、通话、RTP relay 和历史记录不受影响。
- [ ] 两条线路同时使用 Asterisk trunk，验证 AOR、auth username、local port、Call-ID、RTP socket 和 incoming/outgoing binding 不冲突。

UE 隔离（netns/veth/worker）本身的分阶段验收清单不在这里重复——它带着 feature flag 名称、每项的日期/提交号证据和被阻塞原因，见 `ue-isolation-migration.md` 第 8 节。

## P1：IMS 补充业务和运营商回读

代码基础已经存在，但必须取得真实运营商响应，不能只用本地 XML 或 mock 标记完成：

- [ ] Ut/XCAP 对 communication-waiting、communication-diversion、OIP/CLIP、OIR/CLIR 完成 GET → If-Match 条件 PUT → GET 权威回读。
- [ ] 验证 VoLTE 与 VoWiFi 使用当前 access 的源地址、Service-Route 和 AKA provider，不跨线路或跨 access 复用会话材料。
- [ ] 完成 MWI SUBSCRIBE/NOTIFY、401/407 challenge、刷新、注销、超时和 subscription 清理的运营商验收。
- [ ] 完成运营商语音信箱号码发现、按线路拨号和 MWI 状态持久化；区分运营商语音信箱与 Asterisk 本地 voicemail。
- [ ] 完成 Caller ID、Privacy、CLIP/CLIR/OIP/OIR 在 API、日志、Asterisk、Linphone 和 call history 中的一致性审计。

## P1：E911 / TS.43

E911 只能通过运营商非紧急 provisioning/validation 流程验收，不得拨打真实紧急号码：

- [ ] 使用真实支持的 SIM 完成 TS.43 entitlement query、EAP-AKA challenge、状态解析和 token/config version 持久化。
- [ ] 完成可信 catalog endpoint、provider evidence、HTTPS/host allow-list、DNS/IP/redirect/response-size 限制的实网验证。
- [ ] 完成标准 websheet 或已验证 native provider 的地址登记流程，验证运营商回读状态。
- [ ] 按 `SimBindingKey` 隔离 E911 状态、token、cookie 和地址意图；热插拔/eSIM 切换不得串状态。
- [ ] UI 和 API 明确区分“运营商要求地址”“地址已保存在本机”“运营商已确认”和“紧急呼叫未验证”。
- [ ] emergency registration、`urn:service:sos` 路由、PIDF-LO、callback 和 CS fallback 另行设计并取得合规测试授权后再做。

## P2：设备抽象与 CS

- [ ] 让 `DeviceKind` 真正参与 DATA6 / secondary QMI 的准入判断。
  - 现状：`detect_device_kind()`（`hardware/devices/mod.rs`）确实做了 sysfs 探测——按 remoteproc 的 `name` 认 `4080000.remoteproc`，刻意不把邻居 `a204000.remoteproc`（WCNSS Wi-Fi/BT）当基带，认不出就返回 `Unknown` 而不是默认 `Qcm410`。这部分是对的。
  - 缺口：`DeviceKind` 目前**只**用于选择 baseband fault policy（`devices/baseband_faults.rs`）。DATA6 和 secondary QMI 的开关是环境变量 `SIMADMIN_ENABLE_SECONDARY_QMI`（systemd unit 里设的），与 `DeviceKind` 无关。也就是说在一台 `Unknown` 设备上，只要那个变量为 1，仍会去枚举/绑定 DATA6。应改为「`Unknown` 一律不进 DATA6 路径」，环境变量只能在已识别设备上作为额外开关。
- [ ] 将 QCM410 `ImsBearerTransport` 通过 provider/capability 注入 runtime；generic ModemManager 路径不能依赖 QCM410 类型。
- [x] 已删除未实现的 `DataTransport`、`VoiceTransport`、`SmsTransport`、`RegistrationTransport` stub；保留实际 `ImsBearerTransport` capability seam。
- [ ] 为 EC20/EC25/EG25/EG600 与 USB SIM reader 完成真实设备验收；本轮只完成静态线路隔离审阅。逐型号矩阵：
  - [ ] EC20：discovery、AT、SIM 身份、短信、通话、QMI 数据和代理流量。
  - [ ] EC25：同上，加热插拔。
  - [ ] EG25 家族：接口组成、QMI 数据、radio mode 控制。
  - [ ] EG600：真实 USB/PCIe 组成、驻网、数据和支持的 radio 控制。
  - [ ] USB 读卡器：无卡、实体 SIM、PIN 锁卡、USIM AKA、读卡器热插拔。
  - [ ] USB eUICC 读卡器：经 PC/SC lpac 完成 profile 列出/下载/启用/停用。
  - [ ] 用物理 eUICC 读卡器验证 lpac reader name/index 选择。
- [ ] QCM410 逐项确认：DATA6 被 ModemManager 忽略且普通数据留在主 QMI 口；定时流量任务在持久化数据开关关闭时成功并恢复为关闭；定时通话能启动、自动挂机并容忍对端提前挂断。
- [ ] 仅当 PC/SC 服务/包是 SimAdmin 自己安装的才卸载（需要 installer 状态追踪）。
- [ ] 只有找到真实双向音频数据面后，才实现 CS trunk；仅有 ModemManager 呼叫控制不能标记为 CS trunk ready。
- [ ] 验证 QCM410 数据与 IMS bearer 并发时的 slot allocator、baseband wedge guard、恢复和 modem 重启行为。

## P2：媒体和 codec 能力

- [ ] 明确 codec 支持矩阵；`trunk.codec_allow` 必须真正参与 SDP offer/answer，而不是只保存配置。
- [ ] EVS 当前只有 SDP/model 基础；若要宣称 EVS 可用，必须提供编解码、转码、jitter buffer 或明确交由 Asterisk/外部媒体后端处理并完成实测。
- [ ] 补齐 RTP/RTCP 配对、RTCP-mux、丢包、乱序、端口重启和长通话媒体指标验收。
- [ ] 完成 hold/resume、双通话、媒体方向、失败 re-INVITE 保留原 relay 和资源回收的 VoLTE/VoWiFi 矩阵。

## P1：eSIM MEP 预留接口

代码里目前**完全没有 MEP 相关实现**（全仓库搜不到 `MEP`/`mep_` 符号），所以这是一个尚未开工的模块，逐项任务清单在 `docs/ESIM_MEP_INTERFACE_PLAN.md`，此处不重复。

- [ ] 按 `docs/ESIM_MEP_INTERFACE_PLAN.md` 完成预留接口（capability、Port、Profile-to-Port、SIM 来源、可插拔 APDU/modem backend）。
- [ ] 优先支持“一个 Port 走蜂窝 VoLTE、另一个 Port 只走 WiFi VoWiFi”的线路模型；读卡器不要求蜂窝联网。
- [ ] 没有真实 MEP eUICC/读卡器之前，只做 Mock 与线路隔离测试，不标记真实 MEP 完成；型号本身不构成能力证明。
## P1：IMS REGISTER 收口

代码层与本地回归已完成：REGISTER 事务过滤（Call-ID + CSeq + method）、channel requeue、候选阶梯、三态 `omit` 全链路端到端断言、自定义 DNS 端口、每线路动态接入上下文。2026-08-30 已在 410 上闭环 ePDG/IKE/Child SA/ESP 和 **IMS REGISTER 200 OK**。

剩下的都是实机业务矩阵和少量代码缺口。

### 代码缺口

- [ ] VoLTE 与 VoWiFi 对相同 profile 字段的解释完全统一（当前只做了相关路径的局部修复）。
- [ ] 用真实运行时上下文生成 VoLTE `Cellular-Network-Info`（PANI 已用真实上下文；CNI 只有测试夹具驱动的断言）。
- [ ] home / visited network 区分的单元测试（现有测试覆盖 FDD/TDD 和 LTE/NR，不含漫游差异）。
- [ ] 从 QMI 读取注册 PLMN 与注册状态作为 ModemManager 不可用时的兜底。解析器（`parse_qmicli_serving_system_output`）已完成并用真实设备输出做过夹具，但接线点被撤销——它原本挂在 10 秒刷新路径上，会和 `get_cells_data_for_modem` 并发抢同一个 QMI 控制口。需要一个能串行化 QMI 的调用点。
- [ ] 明确 QCM410 固件能稳定提供哪些字段及其刷新事件（需实机采样）。
- [ ] 是否用 ModemManager 信号替代 10 秒轮询（纯优化，当前 10s 采集 / 30s TTL 已有界）。
- [ ] 前端 `current.ts` 调用 `/vowifi/carrier-profiles/import`，但 `main.rs` 未注册该路由，`aosp_apns`/`aosp_carrier_config`/`ipcc` 三种导入格式后端没有实现——类型定义领先于实现。
- [ ] 全局 `cargo fmt --all -- --check`（当前工作树有大量既有跨平台/换行差异，只做过定向格式化）。

### 可观察性

- [x] REGISTER 日志记录实际 PANI/CNI 来源。`volte/live.rs` 在 REGISTER 生命周期开始处输出 `pani_identity_source` 和 `cni_identity_source`，取值来自 `AccessIdentitySource`：`dynamic` / `static_profile` / `compatibility_fallback` / `omitted` / `required_dynamic_missing`。
- [ ] refresh 降级成功时记录被移除的头（不记录敏感字段）。
- [ ] 每条线路统计 refresh 成功率和 access rebuild 次数。失败侧已有 `live_ims_refresh_failure_count_for_line()`（按 line_id 计数，API 已读取）；缺的是成功计数、成功率和 rebuild 次数。

事务键脱敏摘要、跳过帧诊断和失败原因分类已经存在（`RegisterTransactionKey::summary()` 输出 Call-ID hash，五种失败原因串，`authorization_and_nonce_never_reach_the_transaction_log` 固定安全不变量）。

### 实机验收矩阵

REGISTER 路径：

- [ ] 401/407 AKA challenge 后成功（当前 200 OK 是 registrar 直接接受，`auth_rounds=0`，没走 AKA 挑战——和历史记录的 `401 → AKA → 200 OK` 路径不同，需要确认是运营商行为变化还是实现问题）。
- [ ] 423 Min-Expires 协商后成功。
- [ ] 421/494 sec-agree 升级后成功。
- [ ] refresh 等待期间收到 MWI NOTIFY / SMS MESSAGE / INVITE，不掉注册且该帧最终被处理。
- [ ] refresh 首候选失败、降级候选成功时不重建 bearer/ePDG；全部失败才重建并有明确诊断。

Profile 兼容性（至少两个不同运营商，避免为单一 Maxis 行为过拟合）：

- [ ] 完整 MMTEL feature tags + 动态 PANI/CNI；SMS-only Contact；显式 omit PANI / omit CNI；sec-agree auto / required / disabled；有 Route 与无 Route；roaming visited network identity。

业务能力：

- [ ] VoLTE 主叫、被叫（不进语音信箱）、双向 RTP 与静音恢复、SMS over IMS、MWI SUBSCRIBE/NOTIFY。
- [ ] VoWiFi 主叫、被叫、切换和 refresh；长时间 refresh / 重注册。
- [ ] ViLTE capability 与视频媒体协商（若当前版本宣称支持）。
- [ ] 实机抓包确认 REGISTER 里的 PANI 内容与网络侧观测一致（设备上没有 `tcpdump`，需先安装）。

VoLTE profile 三槽位编排：

- [ ] 用户数据库 profile / 下载 catalog profile / 派生兜底各自完成注册与完整业务矩阵。
- [ ] 切换候选时抓包确认 Call-ID、CSeq、Route、安全关联、P-CSCF 和 profile lease 不跨 profile 污染。
- [ ] 两条不同基带线路配置不同顺序并同时运行，以及独立读卡器线路保存/恢复自己的顺序，确认互不影响。
- [ ] 真实 bearer/QMI endpoint、AT CID、xfrm/IPsec、P-CSCF reporting 和 IMS profile lease 释放的集成测试。

### VoNR / 5G SA

当前只有 LTE/NR 通用数据模型基础，**不代表支持 VoNR**。

- [ ] NR SA IMS PDU session/bearer 建立；5GS QoS flow、QFI 和语音媒体承载映射。
- [ ] 从 modem/provider 获取 NR serving cell、NCI、TAC、注册域与 IMS capability。
- [ ] 生成符合 NR 接入的 PANI/CNI，禁止仍标记为 E-UTRAN。
- [ ] EPS fallback、RAT handover 和 registration continuity。
- [ ] VoNR capability 探测——不能仅凭设备支持 5G 就报告 VoNR ready。
- [ ] NR SA 注册、主叫、被叫、双向 RTP、DTMF、BYE、短信及回落场景测试，并在支持 NR IMS 的真实硬件和运营商网络上验收。

## P2：配置、故障和安全验收

- [ ] 做真实掉电、磁盘满、只读文件系统、SQLite 损坏、WAL 恢复和服务强制终止测试。
- [ ] 验证配置库、运行数据、carrier catalog、E911 secret state 和备份文件的权限、符号链接拒绝和恢复边界。
- [ ] 验证日志和诊断包不泄露完整号码、ICCID、IMSI、IMEI、EID、AKA/Digest 材料、token 或 E911 地址。
- [ ] 验证每条线路的 API、数据库写入、通知、自动化、短信、通话记录和流量统计都拒绝空 `line_id` 或错误归属。

## P2：发布工程

- [ ] 定义正式版本、制品命名、catalog 分发、签名/校验、架构兼容、原子替换和回滚契约。
- [ ] 在契约稳定前继续保持手动安装；`install_latest.sh`、`uninstall.sh`、旧 OTA 和在线升级入口不能标为可用。
- [ ] 为 aarch64-musl Zig 构建、前端静态资源、carrier catalog、systemd unit、secondary-QMI 资源和 lpac 建立可重复发布流程。
- [ ] 增加发布前备份/恢复演练，明确 `config.sqlite3`、`data.db`、WAL/SHM、catalog 和 E911 secret state 的独立升级策略。

## 验收规则

1. 普通号码实机测试只使用已授权的测试号码，记录必须脱敏。
2. REGISTER 成功不等于语音、视频、Ut、MWI 或 E911 完成；每种 capability 单独报告。
3. ignored、超时或仅 mock 的测试不能标记外部验收通过。
4. 多线路项目至少需要两条真实线路；单线路结果只能证明单线路路径。
5. E911 只能使用运营商提供的非紧急验证流程，不拨打真实紧急号码。

## 相关现行说明

- 架构总览（线路模型、路由隔离、profile 选择）：`docs/ARCHITECTURE.md`
- REGISTER 三态字段契约：`docs/IMS_REGISTER_TRISTATE_SCHEMA.md`
- 410 基带崩溃分析与现场恢复：`docs/QCM410_BAM_DMUX_MODEM_CRASH.md`
- UE 隔离（netns/veth）迁移设计：`docs/ue-isolation-migration.md`
- eSIM MEP 预留接口设计：`docs/ESIM_MEP_INTERFACE_PLAN.md`
- 用户入口和能力概览：项目根目录 `README.md`
- 手动安装与升级：`docs/INSTALL.md`
- 运行环境与 systemd：`docs/ENVIRONMENT.md`
- 开发、构建和测试：`docs/DEVELOPER.md`
- API 调试集合：`bruno-api/README.md`
- carrier catalog 来源和限制：`docs/CARRIER_PROFILES.md`
- 用户可见版本记录：`docs/CHANGELOG.md`
