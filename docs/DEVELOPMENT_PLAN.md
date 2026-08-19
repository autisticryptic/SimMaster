# SimAdmin 未完成开发计划

> 状态：2026-08-19 整理版。本文是本仓库唯一的后续开发与验收计划，只记录尚未完成、尚未通过外部验收或仍需收口的事项。
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
- [x] 完整 `cargo test --workspace --no-fail-fast`：1041 passed、1 ignored（需要外部 Asterisk/Linphone）、0 failed。
- [x] 清理默认 Linux binary 的 dead-code / unused warning；未接线的 E911 provider 仍以明确待办保留，不用 warning 掩盖状态。
- [x] 增加前端 `pnpm lint`、`pnpm type-check`、`pnpm build` 的 CI 检查，并固定非交互依赖安装方式（`.github/workflows/frontend-checks.yml`）。

前端显示层已完成一轮与线路隔离契约的收口：Dashboard 分别显示设备、IMS 和 Trunk 状态；线路选择器标明读卡器、离线和槽位冲突；VoWiFi `scaffold_only` 状态明确显示为“未接线”。这些显示调整不代表对应能力已经通过真实硬件或运营商验收。

## P1：VoWiFi 业务闭环

### 语音

- [ ] 使用真实运营商完成 VoWiFi 外呼接通、被叫接听、拒接、未接记录和挂断验收（代码路径与本地模拟已覆盖，仍缺运营商实测）。
- [x] 代码层覆盖 VoWiFi/VoLTE early media、180 provisional、带 offer 的 answer、CANCEL、超时、媒体方向和 re-INVITE 路由；[ ] 真实运营商矩阵仍待执行。
- [x] 本地协议测试覆盖双向 RTP、SIP INFO DTMF、telephone-event、媒体方向、hold/resume 和资源清理；[ ] 真实 VoWiFi 运营商验收仍待执行。
- [ ] 将普通号码测试结果按线路、access、codec、SIP 状态、RTP 计数和脱敏 trace 独立记录（需要授权的真实测试号码）。

2026-08-19 已完成一轮 QCM410/50212 飞行模式实测：VoWiFi IMS、trunk、7201 绑定、`100/183` 和失败资源清理通过；运营商以 `480 Release Call received from CAP` 在接通前释放，故 RTP、DTMF、hold 和视频仍未验收。代码已补齐 trunk/API 呼叫的统一 `Started` 生命周期事件和历史记录竞态测试。完整证据见 [VoWiFi 通话实机测试记录](./VOWIFI_CALL_TEST_2026-08-19.md)。

### 视频

- [ ] 完成 VoWiFi H.264 SDP、视频 RTP relay、音频/视频 re-INVITE 的真实运营商互操作。
- [ ] 完成 Asterisk/Linphone 与 VoWiFi 的视频矩阵，包括音频→视频、视频→音频、拒绝升级、保持/恢复和双线路并发。
- [ ] 明确 RTCP、RTCP-mux、视频 payload type 和 codec policy 的支持范围；不支持的能力必须显示为 unsupported。

## P1：多线路和 SIM 身份隔离

至少需要两条真实线路，最好两张相同 PLMN 和两张不同 PLMN 的 SIM，逐项验收：

代码侧审计记录（不替代下面的实机验收）：VoWiFi TUN、IMS REGISTER、安全协商、XCAP、operator session 和 SIM device runtime 均以 `line_id` 为键；VoLTE/VoWiFi/Trunk、数据、漫游、飞行模式、视频、语音/SMS 路径以及 lpac reader 参数均已下沉到 `LineProfileConfig`。基带 `line_id` 只由稳定物理槽锚点和 UIM slot 生成，同槽换卡不再生成新线路；旧版“物理槽 + ICCID”ID会迁移线路配置、自动化目标、通知线路筛选和累计流量。SIM IMS 覆写仍独立使用 ICCID 或 EID + profile ICCID，不会误变成基带级配置。其中 lpac 旧全局 reader 参数只允许单线路迁移，并有双线路隔离测试。

- [ ] 两条线路同时建立 VoLTE bearer 和 IMS REGISTER，分别使用自己的 QMI、netdev、P-CSCF、路由表和 runtime。
- [ ] 两条线路同时建立 VoWiFi TUN/ePDG/IKE/ESP/REGISTER，TUN、代理、DNS、route 和 operator session 不互相覆盖。
- [ ] 相同 PLMN 的两张 SIM 使用不同 effective profile、IMEI、E911、UT、MWI 和 trunk 配置时不串值。
- [ ] 独立读卡器换卡后按 ICCID/EID + eSIM profile ICCID 重新选择覆写；不能使用 reader `line_id` 误绑定旧 SIM。
- [ ] 在两条真实线路上验证 SIM 从 modem A 移到 modem B、eSIM profile 切换、拔卡再插回和 modem 编号变化后，物理线路配置保持在槽位、SIM 覆写跟随 ICCID/EID + profile ICCID。
- [ ] 一条线路停止、断网、认证失败或 bearer 重建时，另一条线路的 REGISTER、通话、RTP relay 和历史记录不受影响。
- [ ] 两条线路同时使用 Asterisk trunk，验证 AOR、auth username、local port、Call-ID、RTP socket 和 incoming/outgoing binding 不冲突。

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

- [ ] 完成 `detect_device_kind()` 的真实 sysfs/DT/udev capability 探测；未知设备不得写 QCM410 DATA6 udev 规则或绑定 secondary QMI。
- [ ] 将 QCM410 `ImsBearerTransport` 通过 provider/capability 注入 runtime；generic ModemManager 路径不能依赖 QCM410 类型。
- [x] 已删除未实现的 `DataTransport`、`VoiceTransport`、`SmsTransport`、`RegistrationTransport` stub；保留实际 `ImsBearerTransport` capability seam。
- [ ] 为 EC20/EC25/EG25 与 USB SIM reader 完成真实设备验收；本轮只完成静态线路隔离审阅。
- [ ] 只有找到真实双向音频数据面后，才实现 CS trunk；仅有 ModemManager 呼叫控制不能标记为 CS trunk ready。
- [ ] 验证 QCM410 数据与 IMS bearer 并发时的 slot allocator、baseband wedge guard、恢复和 modem 重启行为。

## P2：媒体和 codec 能力

- [ ] 明确 codec 支持矩阵；`trunk.codec_allow` 必须真正参与 SDP offer/answer，而不是只保存配置。
- [ ] EVS 当前只有 SDP/model 基础；若要宣称 EVS 可用，必须提供编解码、转码、jitter buffer 或明确交由 Asterisk/外部媒体后端处理并完成实测。
- [ ] 补齐 RTP/RTCP 配对、RTCP-mux、丢包、乱序、端口重启和长通话媒体指标验收。
- [ ] 完成 hold/resume、双通话、媒体方向、失败 re-INVITE 保留原 relay 和资源回收的 VoLTE/VoWiFi 矩阵。

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

- 用户入口和能力概览：项目根目录 `README.md`
- 手动安装与升级：`docs/INSTALL.md`
- 运行环境与 systemd：`docs/ENVIRONMENT.md`
- 开发、构建和测试：`docs/DEVELOPER.md`
- API 调试集合：`bruno-api/README.md`
- carrier catalog 来源和限制：`docs/CARRIER_PROFILES.md`
- 用户可见版本记录：`docs/CHANGELOG.md`
