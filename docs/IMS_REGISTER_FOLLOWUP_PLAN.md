# IMS REGISTER 后续开发与验收清单

> 建立日期：2026-08-28<br>
> 项目：SimAdmin<br>
> 分支：`feat/ims-register-423-negotiation`
> 基线提交：`0444d58 feat(ims): complete register status-code fallback and terminal abandonment`

## 1. 文档用途

本文档用于接续此前的 IMS REGISTER、运营商配置投影和续期稳定性改造。

维护规则：

- `[ ]` 表示尚未完成或尚未验证。
- `[x]` 只有在代码完成且对应验收通过后才能勾选。
- 每次完成一项时，在该项下补充日期、提交号（未提交时明确写“未提交”）、测试命令和实机证据。
- “已写入工作树”不等于“已完成”：没有完成审阅和回归的代码仍保留在待验收区。
- 不要改动或清理当前未跟踪的抓包、发行包和分析目录，除非另有明确任务。

## 2. 问题背景与当前结论

现场现象是两个 IMS profile 都能收到 REGISTER `200 OK`，但其中一个 profile 的被叫会直接进入语音信箱。日志还显示周期性 refresh 失败并触发接入重建。

已定位的直接软件问题：REGISTER、NOTIFY、MESSAGE、MWI 等共享同一条 SIP 信令通道；旧实现会把 refresh 等待期间收到的任意 SIP 帧都当成 REGISTER 响应解析。收到请求帧时产生 `sip_status_line_invalid`，导致已经可用的 IMS registration 被错误拆除。

当前修复方向：

1. 用 Call-ID、CSeq 数字和 `REGISTER` 方法识别当前 REGISTER 客户端事务。
2. 非本事务帧暂存并交还会话循环，不让 REGISTER 事务误消费。
3. refresh 失败时先尝试兼容性降级，不立即重建整个接入腿。
4. 运营商数据库中的显式 `omit` 必须压过代码默认值。
5. PANI/CNI 必须最终来自真实接入上下文，而不是静态 RAT 或全零 cell-id。

## 3. 已写入当前工作树的功能

以下内容已经写入源码；代码层和自动化回归已按本轮记录更新，真实硬件/运营商网络验收仍保留在待办区。

- [x] REGISTER 事务键包含 Call-ID、CSeq 数字和 REGISTER 方法。
  - 位置：`backend/src/connectivity/core/register.rs`
- [x] REGISTER 等待循环跳过 provisional response，并有数量上限。
- [x] 非当前 REGISTER 事务帧进入 requeue，而不是当作状态行解析。
- [x] VoLTE、VoWiFi TCP、VoWiFi UDP channel 支持 fresh read 与 requeue。
  - 位置：
    - `backend/src/connectivity/core/access.rs`
    - `backend/src/connectivity/modems/ims/volte/channel.rs`
    - `backend/src/connectivity/modems/ims/vowifi/channel.rs`
- [x] VoLTE refresh 增加有限候选阶梯：当前形态、去掉接入网络信息、再去掉 Route。
  - 位置：`backend/src/connectivity/modems/ims/volte/live.rs`
- [x] VoLTE PANI 发送由 profile 三态策略控制，不再由 MMTEL 特征隐式强制开启。
- [x] VoLTE CNI 增加 profile 开关和报文生成路径。
  - 位置：`backend/src/connectivity/modems/ims/volte/sip.rs`
- [x] carrier catalog 支持布尔字段使用字符串 `omit` 映射为显式 false。
- [x] `security_agreement: "omit"` 映射为 `sec_agree_mode = "disabled"`。
  - 位置：`backend/src/connectivity/modems/ims/vowifi/carrier_catalog_v7.rs`
- [x] MMTEL Contact feature tag 的通用补全框架已存在，不需要另起一套实现。

已知已经通过的定向测试：

- [x] `register::tests`：23/23（2026-08-29 统一回归）。
- [x] `volte::sip::tests`：36/36（2026-08-29 统一回归）。
- [x] `volte::live::tests`：53/53（2026-08-29 统一回归）。
- [x] `carrier_catalog::v7::tests`：12/12（2026-08-29 统一回归）。

补充：2026-08-29 已完成统一回归；完整测试和仍未完成的真实环境验收见第 4 节及第 14.7/14.8 节。

## 4. 当前改动稳定化：代码审阅与回归

### 4.1 完整代码审阅

- [x] REGISTER transaction filtering 的超时语义已统一到单次 deadline；无关帧不会重新延长完整等待窗口。
  - 已确认 provisional response、无关帧分别计数，并拒绝缺失 Call-ID/CSeq 的错误匹配。
  - 完成日期：2026-08-29
  - 自动测试：`cargo test --bin simadmin register::tests -- --test-threads=1`（23 passed; 0 failed）。
- [x] requeue 的所有权和顺序已审阅。
  - FIFO 顺序、TCP/UDP/protected UDP channel 的 `into_parts` 保留，以及 channel 转换后的帧回交均有覆盖。
  - 完成日期：2026-08-29
  - 自动测试：`volte::channel::tests`（4 passed; 0 failed）、`vowifi::channel::tests`（6 passed; 0 failed; 1 ignored）。
- [x] requeue 队列具备帧数和总字节数上限，避免跨事务无限累积。
  - 当前 channel requeue 上限为 64 帧 / 4 MiB；超限按实现策略丢弃并记录诊断。
  - 完成日期：2026-08-29
- [x] refresh CSeq、Call-ID、认证 nonce 和 Security-Verify 生命周期已审阅。
  - 423、401/407、421/494 及兼容性候选切换后，事务键和安全状态不会跨候选污染。
  - 完成日期：2026-08-29
  - 自动测试：`volte::sip::tests`（36 passed; 0 failed）、`volte::live::tests`（53 passed; 0 failed）。
- [x] teardown/unregister 路径已复用事务过滤，避免注销事务误消费其他 SIP 帧。
  - 完成日期：2026-08-29
- [ ] VoLTE 与 VoWiFi 对相同 profile 字段的解释完全统一。
  - 仍需第 7 节的 schema、`omit` 保留和最终 SIP 报文端到端工作；当前只完成了相关代码路径的局部修复。

### 4.2 编译、格式与回归

- [ ] 全局运行 `cargo fmt --all -- --check`。
  - 当前工作树包含大量既有跨平台和换行修改，本轮仅做定向 rustfmt 检查，避免全局格式化引入无关变更。
- [x] 运行 `git diff --check`，无空白错误。
  - 完成日期：2026-08-29
- [x] WSL Debian 运行 `cargo check --bin simadmin`。
  - 完成日期：2026-08-29；存在既有 dead-code warnings，无编译错误。
- [x] 重新运行定向测试。
  - `register::tests`：23 passed; 0 failed。
  - `volte::sip::tests`：36 passed; 0 failed。
  - `volte::live::tests`：53 passed; 0 failed。
  - `carrier_catalog::v7::tests`：12 passed; 0 failed。
  - `volte::channel::tests`：4 passed; 0 failed。
  - `core::access::tests`：3 passed; 0 failed。
  - `vowifi::channel::tests`：6 passed; 0 failed; 1 ignored。
  - 完成日期：2026-08-29
- [x] 已排查 `vowifi::channel::tests` 在 WSL 中的长时间运行问题。
  - oversized fragmented UDP loopback 测试已明确标记 ignored；原因是 WSL2 loopback 在超过 MTU 时不投递该数据报，不是代码死锁。
  - 完成日期：2026-08-29
- [x] 运行完整后端测试：`cargo test --bin simadmin -- --test-threads=1`。
  - 结果：1359 passed; 0 failed; 3 ignored（共 1362 个测试）。
  - 完成日期：2026-08-29；含第 7 节新增的 9 个测试和第 14.7 节新增的 8 个 HTTP 集成测试。
  - 前端 `pnpm type-check`、`pnpm lint`、`pnpm build:full` 均通过（2026-08-29）。
- [x] 在 Debian/aarch64 目标环境完成编译或 CI 构建。
  - GitHub Actions Run `33236459920` 的 ARM64 job 成功，生成 `aarch64-unknown-linux-musl` 制品（2026-08-29）。
- [ ] Windows 原生编译仍受已知 `libc::IFF_UP` 平台问题影响；本项不作为本轮 IMS 修复的失败依据。
## 5. 真实接入网络上下文

> **2026-08-29 复核修正。** 本节原先整节标为未实现，但代码复核证明 ModemManager + LTE/NR 这条链路已经完整落地。原描述“当前 CNI/PANI 某些路径仍使用静态模板或全零 cell-id”对于 dynamic 路径已不成立：`core/access_network.rs` 模块文档明确写了 “it never manufactures a PLMN/TAC/cell-id placeholder”。
>
> 本次只做代码复核，未新增实现，也未做实机验收。下面逐项标注证据位置，未验证的项保持 `[ ]`。

### 5.1 统一数据模型

- [x] 定义可跨 VoLTE、VoWiFi、VoNR 使用的运行时接入上下文。
  - 位置：`backend/src/connectivity/core/access_network.rs`（686 行）
  - `ServingAccessSnapshot`：采集侧原始观测，含 MCC/MNC、technology、cell id、TAC、serving band、来源。
  - `ImsAccessNetworkContext`：解析后的上下文，`age()` 提供采集时间差。
  - `AccessNetworkSource` 标注数据来源可信度（含 `ModemManager`、`TestFixture`）。
  - RAT 建模为 `ImsAccessType`：`EutranFdd`、`EutranTdd`、`NrFdd`、`NrTdd`、`Iwlan`。
- [x] 明确“数据未知”语义。
  - `ServingAccessSnapshot::new()` 返回 `Option`，字段不完整时直接构造失败，不产生上下文。
  - `AccessIdentityPolicy` 四态决定未知时的行为：`Omit` / `Static` / `DynamicIfKnown` / `RequiredDynamic`。
  - `AccessIdentityResolution::required_dynamic_missing()` 让 `RequiredDynamic` 在数据缺失时可被上层识别。
  - 测试：`invalid_or_missing_identity_is_omitted`、`access_identity_policy_distinguishes_dynamic_static_and_missing_required`。
- [x] 为每条线路隔离运行时接入上下文，未使用进程级共享可变全局状态。
  - `ImsAccessNetworkRuntime` 实例挂在 `LineRuntime.ims_access_network` 上（`services/line_registry.rs`），每条线路一份。
  - 测试：`per_line_runtime_does_not_share_snapshots`。

### 5.2 从 modem 获取真实数据

- [x] 从 ModemManager 读取 serving system、RAT、注册 PLMN、TAC 和 cell identity。
  - 位置：`backend/src/connectivity/modems/ims/access_network.rs` 的 `serving_access_snapshot()`。
  - 并发查询 network info 与 cells data；注册状态不属于 `registered`/`roaming`/`attached` 时返回 `access_network_not_registered:<status>`。
  - serving cell 字段为 0 时回退到 cells 列表里标记 `is_serving` 的那一项。
- [x] 将数据按 `line_id` 注入 IMS access leg，不由 SIP builder 主动访问全局 modem。
  - `refresh_ims_access_network()`（`line_registry.rs:862`）按 line 发布；SIP builder 只接收 `Option<&ImsAccessNetworkContext>` 参数。
  - reader 类型线路和不存在的 modem 直接 `clear("access_network_unavailable_for_line_kind")`。
- [x] 过期处理。
  - `ImsAccessNetworkRuntime::context()` 走 `context_with_max_age()`，TTL 为 `DEFAULT_IMS_ACCESS_NETWORK_MAX_AGE = 30s`；超龄上下文不会被当成当前小区。
  - 两个 VoLTE 消费点（`volte/live.rs:2227`、`:3893`）都走 `.context()`，因此都受 TTL 约束。
- [x] 对无服务、字段缺失和查询失败提供可观察错误，而不是 panic。
  - 确认的无服务/不完整观测立即 `clear(reason)`；瞬时查询失败只 `record_refresh_error(reason)`，旧上下文保留到 TTL 到期。
  - 错误串区分 `access_network_modem_path_invalid`、`access_network_network_query_failed`、`access_network_cell_query_failed`、`access_network_not_registered`、`access_network_snapshot_incomplete`（后者列出 tech/plmn/cell/tac 各自 present 还是 missing）。
  - 状态可读：`AccessNetworkRuntimeStatus`，经 `line_registry.rs:268` 暴露。
- [x] 从 QMI 读取小区标识。**（本轮复核修正：QMI 其实是首选来源，不是"未接入"）**
  - 完成日期：2026-08-29（复核，未改代码）
  - `serving_access_snapshot()` 取小区数据走 `get_cells_data_for_modem()`（`hardware/cellular/modem_manager.rs:3653`），该函数**先试 QMI**：`get_cells_data_qmicli()` 执行 `qmicli -p -d <dev> --nas-get-cell-location-info`，由 `parse_qmicli_cell_location_output()` 解析；只有 QMI 返回空 cells 时才回落到 ModemManager 的 `GetCellInfo`，再回落到 `mmcli`。
  - 也就是说 cell id / TAC / tech / band 这组"绝不能伪造"的字段，主来源已经是 QMI。
  - 注：本项目的 QMI 访问方式是解析 `qmicli` 文本输出，不是自建 QMI 协议封帧，与 `qmi_wds.rs` 的结构一致。
- [x] 从 QMI 读取注册 PLMN 与注册状态。
  - 完成日期：2026-08-29；提交 `726823d`
  - 新增 `parse_qmicli_serving_system_output()`、`get_serving_system_qmicli()`、`QmiServingSystem`，位置 `backend/src/hardware/cellular/modem_manager.rs`。
  - 接线点：`get_network_info_for_modem()` 仅在 ModemManager 未给出 operator code 时才走 QMI；失败只记 debug 日志，调用方仍拿到原本的 ModemManager 视图。
  - **夹具是从 410 抓的真实输出**（`qmicli -p -d /dev/wwan0qmi0 --nas-get-serving-system`），不是凭记忆写的。抓取时先发现 410 根本没有 `/dev/cdc-wdm*`，真实节点是 `/dev/wwan0qmi0`。
  - 该输出里有两个必须避开的陷阱，都已写成断言：
    1. **MCC/MNC 出现两次**——`Current PLMN:` 和 `Full operator code info:` 各一份。逐行匹配 `MCC:` 会取到后出现的那个，所以搜索限定在第一个 block 内。测试 `serving_plmn_comes_from_the_current_plmn_block_only` 把第二个 block 改成 999/99 来证明作用域生效。
    2. **有两个区域码**——`3GPP location area code: '65534'`（0xFFFE，2G/3G 的 LAC "不适用"哨兵值）与 `LTE tracking area code: '15102'`（真正的 TAC）。取错会把哨兵值写进 PANI，比不发更糟。已用同设备的 `--nas-get-cell-location-info` 交叉验证：该命令报 TAC 15102、cell 55281991，与 serving-system 的 LTE 字段一致，与 LAC 不一致。
  - 归属说明：解析器放在 `hardware/cellular/` 而不是 `hardware/devices/qcm410/`。输出格式来自 libqmi，任何 QMI modem 都相同；410 专属的只是控制节点名，而 `qmi_control_device()` 已经处理。这与 `cellular/mod.rs` 文档描述的分界一致。
  - 测试：`parses_real_qmi_serving_system_output`、`serving_plmn_comes_from_the_current_plmn_block_only`、`unregistered_serving_system_yields_no_identity`、`roaming_status_is_read_from_the_scalar_field`。
  - **实机注意**：410 上 ModemManager 报得出 `operator id: 50212`，所以这条 fallback 实际不会触发。它补的是健壮性，不是当前的实际缺口。
- [ ] 明确 QCM410 当前固件能稳定提供哪些字段，以及字段刷新事件。
  - 需要实机采样，尚未做。
- [x] 切换后的上下文刷新有界，且不会使用过期数据。
  - 完成日期：2026-08-29（本轮只做复核，未改代码）
  - **实际数字修正。** 本节此前描述为"最长可能落后一个 refresh 周期"，含义不清。真实数字：`main.rs:1118` 无条件 spawn 的刷新任务周期为 **10 秒**（`interval(Duration::from_secs(10))`，`MissedTickBehavior::Delay`），启动时另有一次 `line_registry.refresh()`（`main.rs:911`）。每轮 refresh 都对新增和既有线路调用 `refresh_ims_access_network()`（`line_registry.rs:789`、`:795`）。
  - 因此小区重选/漫游/RAT 切换后，接入上下文最迟约 10 秒被重新发布；而 `context()` 的 TTL 是 30 秒，所以即使连续两轮 refresh 失败，过期上下文也不会被当作当前小区使用——两个数字是 10s 采集 / 30s 兜底，而不是无界滞后。
  - 结论：轮询周期远小于 TTL，行为是有界且安全的。
- [ ] 是否需要事件驱动的刷新（ModemManager 信号触发，而非 10 秒轮询）。
  - 纯优化项，不是正确性缺口：可把最坏延迟从约 10 秒降到信号到达即刷新。是否值得引入信号订阅的复杂度需要产品决策。

### 5.3 动态生成 PANI/CNI

- [x] 用真实运行时上下文生成 VoLTE `P-Access-Network-Info`。
  - `ImsAccessNetworkContext::cellular_access_info()` 按 TS 24.229 表 7.2A.4-1 的固定宽度输出。
  - 测试：`lte_context_formats_real_tac_and_eci_at_fixed_widths`、`modem_snapshot_uses_serving_band_before_profile_hint`。
- [x] LTE 与 NR 分别使用正确的标识宽度。
  - LTE：16-bit TAC + 28-bit E-UTRAN cell id。
  - NR：24-bit TAC + 36-bit NR cell identity。
  - 测试：`nr_snapshot_preserves_complete_36_bit_nci`。
- [x] WLAN 接入类型与蜂窝辅助信息分别建模，未复用 LTE 字符串。
  - `ImsAccessType::Iwlan` 的 `cellular_identity_widths()` 返回 `None`，结构上无法为 WLAN 生成蜂窝小区标识。
- [x] 为 profile 提供明确策略：`omit`、`static`、`dynamic-if-known`、`required-dynamic`。
  - `AccessIdentityPolicy` 四态，`resolve_access_identity()` 统一入口，PANI 与 CNI 各自独立持有策略（`pani_identity_policy` / `cni_identity_policy`）。
- [x] 已无 `{PLMN}0000000` 占位生成逻辑。
  - `core/access_network.rs` 模块文档明确声明不制造占位符；全仓库搜索 `0000000` 只剩 IMSI/nonce 测试夹具，与 cell-id 无关。
- [x] 头部值注入防护。
  - `sanitize_header_value()`、`access_type_token()` 拒绝注入；测试 `header_value_and_contact_token_reject_injection`。
- [ ] 用真实运行时上下文生成 VoLTE `Cellular-Network-Info`。
  - CNI 有独立的 `cni_identity_policy` 和 `enable_cellular_network_info` 开关，且已被第 7 节的端到端测试覆盖"不发"的情形；但"发"的情形只有 `volte::sip::tests` 里基于测试夹具上下文的断言，尚未确认真实 ModemManager 快照driven 的 CNI 输出。
- [ ] home/visited network 区分的单元测试。
  - 现有测试覆盖 FDD/TDD 和 LTE/NR，未覆盖 home 与 visited 的差异。
- [x] 实机确认动态接入上下文在真实 LTE 线路上可用（2026-08-29，v1.1.5 / commit `726823d`）。
  - 设备 API 返回的线路接入状态：
    ```json
    {"available": true, "stale": false, "technology": "lte",
     "serving_plmn": "50212", "age_seconds": 0, "last_error": null}
    ```
  - 日志确认刷新周期真实生效（每 10 秒一轮）：
    ```text
    DEBUG Refreshed per-line IMS serving access context
          line_id=line-50ad5391cd09c09936f1081bd479139c technology=lte
    ```
  - 即 §5.1/§5.2 的数据模型、采集、按线路注入、TTL 与错误可观察性在实机跑通，`serving_plmn` 是真实值而非模板。
- [x] 澄清一处此前的误读：410 REGISTER 日志里的 `pani_format="profile_default"` 来自 **VoWiFi** 链路（`vowifi/live.rs`），其 `LivePaniFormat` 只有 `ProfileDefault`/`PlainWifi`/`Omit` 三态，本就没有"动态"选项——对 WLAN 接入而言用 profile 字符串是正确的，不该带蜂窝小区 ID。VoLTE 的动态 PANI 是另一条路径（`volte/sip.rs` 的 `resolve_access_identity`），派生 profile 已经是 `DynamicIfKnown`（`profiles.rs:1099`：`LteEpc => DynamicIfKnown`，`WifiEpdg => Static`），CNI 则相反（`LteEpc => Omit`，`WifiEpdg => DynamicIfKnown`）。两条链路的策略都符合设计，无需修改。
- [ ] 实机抓包确认 VoLTE REGISTER 报文里的 PANI 内容与网络侧观测一致。
  - 上面证明了上下文可用且策略正确，但没有抓到实际报文比对。设备上没有 `tcpdump`，需要先安装。

## 6. 尚未完成：VoWiFi refresh 对等性

共享 REGISTER driver 已提供事务过滤；本轮已完成 VoWiFi refresh 的代码层 requeue 和失败阈值修复，但仍未完成真实 ePDG/IKE 网络验收，也没有直接复制 VoLTE 的候选阶梯。

- [x] 已定位 VoWiFi registration refresh 的全部入口和定时器。
  - 完成日期：2026-08-29
- [x] refresh 等待期间的 NOTIFY、MESSAGE、OPTIONS、INVITE 会被 requeue，并在 refresh 后交还会话循环处理。
  - 完成日期：2026-08-29
  - 自动测试：MWI NOTIFY、inbound MESSAGE/INVITE 及 refresh 相关 live tests 均已覆盖。
- [x] requeue 帧在 TCP/UDP channel 转换、IPsec 重协商和 `into_parts` 后不丢失。
  - 完成日期：2026-08-29
  - 自动测试：`vowifi::channel::tests`（6 passed; 0 failed; 1 ignored）。
- [ ] 评估是否需要与 VoLTE 相同的有限候选阶梯。
  - 不要直接复制 VoLTE 顺序；需要根据 VoWiFi profile 字段定义安全候选，并确保已认证或已建立安全关联后不回退到违反运营商策略的不安全形态。
- [x] 已增加 VoWiFi refresh 与 MWI NOTIFY 同时到达的回归测试。
  - 完成日期：2026-08-29
- [x] 已增加 refresh 与 inbound MESSAGE/INVITE 同时到达的回归测试。
  - 完成日期：2026-08-29
- [x] 已增加 refresh 连续失败后仅在合理阈值触发 ePDG/IKE 重建的测试。
  - 完成日期：2026-08-29
- [ ] 在真实 ePDG/IKE、运营商 WLAN 和终端环境完成网络验收。
## 7. 尚未完成：运营商配置与 `omit` 全链路

catalog v7 投影已支持若干 REGISTER 字段的字符串 `omit`，但还需要确认从数据库到最终 SIP 报文的端到端一致性。

- [x] 列出所有支持三态的字段及其最终行为，形成唯一 schema 文档。
  - 位置：`docs/IMS_REGISTER_TRISTATE_SCHEMA.md`（2026-08-29）
  - 内容：三态定义、取值解析规则、11 个字段的 baseline 与报文影响、`security_agreement`/`sec_agree_mode` 语义、四层链路图、回归测试对应表。
- [x] 检查 profile record 的 JSON 往返是否保留 `omit`。
  - 完成日期：2026-08-29
  - 风险确认：`include_pani_initial`、`include_pani_authenticated`、`include_p_preferred_identity`、`always_add_sip_instance` 四个字段带 `#[serde(default = "default_true")]`，任何丢字段的中间层都会把 `false` 翻回 `true`。
  - 测试：`profile_record::tests::omitted_register_switches_survive_a_json_round_trip`（九个开关 + `sec_agree_mode` + 机制列表 + 整体相等）。
- [x] 检查 profile store 的加载路径是否保留 `omit`。
  - 完成日期：2026-08-29
  - 机制确认：store 经 `custom_records()` → `CarrierProfileRecord::from_database_json()` 加载。该函数先反序列化，再把**原始 JSON** 交给 `normalize_legacy_database_record()`，因此能区分"字段缺失"和"运营商写了 false"。源码注释已明确：missing 才归一化，authored `false`/`disabled`/`omit` 优先级更高且必须原样保留。
  - 测试：`profile_record::tests::stored_omit_survives_the_database_load_path`（九个开关全部 + 整体记录相等；既有 `database_migration_preserves_explicit_optional_header_disables` 已覆盖 legacy schema 路径的五个开关，本测试覆盖当前 schema 路径并补齐四个 PANI/Route/P-Preferred-Identity 开关）。
- [x] 明确 legacy 行无法表达 `omit` 的既有限制，并用测试固定。
  - 完成日期：2026-08-29
  - 早于某个开关存在的数据库行，缺该字段时不能被读成 `omit`：`always_add_sip_instance` 缺失归一化为 `true`（baseline），`enable_cellular_network_info` 缺失归一化为 `false`（CNI 可能泄露服务小区信息，绝不为旧行合成）。这是刻意的不对称，不是回归。
  - 测试：`profile_record::tests::a_legacy_row_missing_a_switch_is_not_read_as_an_omit`。
- [x] 检查 SIM override 路径。
  - 完成日期：2026-08-29
  - 结论是结构性的，不只是"当前没实现"：`ImsAccessOverride` 的 13 个字段全部是寻址（`profile_id`、`apn`、`domain`、`realm`、`registrar`、`pcscf`、`epdg_host`、`epdg_port`、`ip_stack`）、DNS（`dns`）和 IMSI 伪装（`spoof_imsi`、`custom_imsi`），不含任何 REGISTER 开关。
  - override 解析产物是 `EffectiveImsProfile`，该结构只有 8 个字段且完全不含 register policy；`effective_register_target()` 进一步只取 domain/realm/registrar。
  - 报文构造时 header 策略来自 `&CarrierProfile`，寻址来自独立的 `RegisterTarget` 参数。所以 override 能改"发到哪里"，无法改"带哪些头"。
  - 测试：`volte::live::tests::a_sim_override_cannot_resurrect_an_omitted_register_header`。测试里 override 填了 domain/realm/registrar/pcscf 并**先断言这些覆写确实生效**，再断言六个被 omit 的头仍然不存在——避免 override 被忽略时测试空转。
- [x] 检查配置导入导出路径。
  - 完成日期：2026-08-29
  - 配置导入导出是 CLI 路径，不是 HTTP：`simadmin config export` / `config import`，实现在 `backend/src/platform/config_maintenance.rs` 的 `export_json()` / `import_json()`。
  - **这九个开关不在导出范围内。** `CONFIG_TABLES` 只有 `config_line_profiles`、`config_modem_slots`、`config_standalone_sim_slots`、`config_documents`，restore 时额外带 `ims_sim_overrides`。存放 REGISTER 开关的 `custom_carrier_profiles` 表根本不参与 JSON 导出，所以导出导入无法丢掉这些字段。
  - 二进制 restore 路径（`restore_config_tables()`）用 `INSERT INTO {table} SELECT * FROM restore_source.{table}` 原样整表复制，`record_json` 逐字节保留。
  - 导出确实包含 SIM override，但如上一项所述，`ImsAccessOverride` 不含 REGISTER 开关。
- [x] 检查 HTTP 写入路径，并记录发现的暴露面。
  - 完成日期：2026-08-29
  - `PUT /api/vowifi/carrier-profiles`（`handlers.rs:8948`）直接以 `Json<CarrierProfileRecord>` 反序列化后交给 `ProfileStore::upsert()`。这条路径**拿不到原始 JSON**，因此和 `from_database_json()` 不同，无法区分"字段缺失"和"运营商写了 false"。
  - 后果：body 里省掉 `include_pani_initial`、`include_pani_authenticated`、`include_p_preferred_identity`、`always_add_sip_instance` 时，serde 默认值把这四个开关翻回 `true`，运营商的 omit 被静默取消。`include_route_header` 和 `enable_cellular_network_info` 恰好因为默认值就是 `false` 而幸存，但这是巧合，不是 presence 判断。
  - 当前实际风险有限：前端 `saveVowifiCarrierProfile(record: CarrierProfileRecord)` 发送完整的强类型记录，`CarrierProfileRecord` 是非 Partial 接口，所以自家 UI 不会触发。暴露面主要是第三方或手工调用 API 的 read-modify-write。
  - 测试：`profile_record::tests::a_partial_api_body_silently_reenables_default_true_switches`，断言的是**当前行为**，让暴露面可见并可回归。
- [x] 修复 HTTP 部分 body 的暴露面。
  - 完成日期：2026-08-29
  - handler 改收 `Json<serde_json::Value>`，经新增的 `CarrierProfileRecord::from_api_value()` 解析。body 缺少八个三态开关中任意一个即返回 400 `carrier_profile_register_switch_missing:<逗号分隔的字段名>`；缺整个 register 段落单独报 `carrier_profile_register_section_missing`，不混淆成"缺开关"。
  - 位置：`backend/src/api/handlers.rs` 的 `upsert_vowifi_carrier_profile_handler`，`backend/src/connectivity/modems/ims/vowifi/profile_record.rs` 的 `from_api_value()` / `from_api_json()` / `REQUIRED_REGISTER_SWITCHES`。
  - 理由与第 7 节第 6 项一致：PUT 语义是整体替换，省字段属于调用方编写错误，宁可明确报错也不要让 serde 默认值替运营商做决定。错误一次列出全部缺失字段，调用方一个来回就能改对。
  - **自家 UI 不受影响**：`contracts.ts` 的 `RegisterPolicyRecord` 里这八个字段全是非可选 `boolean`，前端在类型层就无法构造部分记录。前端 `pnpm type-check`、`pnpm lint`、`pnpm build:full` 均通过。
  - 测试：`profile_record::tests::the_api_parser_refuses_a_body_missing_register_switches`（逐字段删除各测一次、多字段一起缺时一并报出、缺整段单独报、完整 body 仍被接受）；`http_router_tests::a_partial_put_body_is_refused_by_the_live_endpoint`（真实端点断言拒绝 + 完整 body 仍接受 + `omit` 确实存活到存储状态）。
  - 原先刻意断言旧行为的两个测试已翻转而非删除：serde 层那个改名为 `plain_deserialization_of_a_partial_body_reenables_default_true_switches`，保留"为什么需要 `from_api_value`"的证据。
- [ ] 附带发现（不属于第 7 节，未处理）：前端 `current.ts:1172` 调用 `/vowifi/carrier-profiles/import`，但 `main.rs` 没有注册该路由，`aosp_apns`/`aosp_carrier_config`/`ipcc` 三种导入格式在后端没有实现。`contracts.ts` 的类型定义领先于实现。
- [ ] 检查以下字段的端到端测试：
  - `security_agreement`
  - `include_pani_initial`
  - `include_pani_authenticated`
  - `include_route_header`
  - `include_p_preferred_identity`
  - `always_add_sip_instance`
  - `enable_cellular_network_info`
  - `require_sec_agree_headers`
  - `proxy_require_sec_agree_headers`
- [x] 明确 `security_client_mechanisms` 与 `sec_agree_mode=disabled` 的关系。
  - 完成日期：2026-08-29；已写入 schema 文档第 3.1 节。
  - 实现确认：机制列表保留在数据里用于往返；`vowifi/live.rs` 用 `sec_agree_mode != "disabled"` 同时门控 `security_client` 和 `security_verify`，所以列表非空不等于会发 offer。此行为本来就正确，本轮只是补上文档和断言。
- [x] 增加最终 SIP 报文断言，不只验证中间 `RegisterPolicyRecord`。
  - 完成日期：2026-08-29
  - 测试：`volte::live::tests::omitted_register_switches_are_absent_from_the_built_request`。
  - 覆盖：从 record 经 `intern()` → `register_variants()` → `build_register_from_profile_with_target_visited_and_access()` 生成真实报文字节；对候选阶梯每个 variant × Initial/Authenticated/Refresh 三个阶段，断言 `P-Access-Network-Info`、`Cellular-Network-Info`、`P-Preferred-Identity`、`Route`、`Security-Client`、`Security-Verify` 均不存在，`Require`/`Proxy-Require` 不含 `sec-agree`，`Contact` 不含 `+sip.instance`。
  - 测试里刻意填入 P-CSCF 地址，避免 `Route` 断言空转。
  - 附带结论：`include_route_header` 在 VoLTE 侧本来就生效（`volte/sip.rs` 的 `policy.include_route_header`，由 `register_variants()` 从 profile 拷入），不存在 VoLTE 忽略该字段的问题，无需改三态。
- [x] 为错误类型值增加验证和诊断，避免无提示回落到默认行为。
  - 完成日期：2026-08-29
  - 缺陷：`bool_at_or_omit()` 旧实现对无法识别的值返回 `None`，等于把决定权交回调用方 baseline。于是 bundle 写 `"no"`、`"disabled"`、`1` 这类值时，默认值为 `true` 的头会被静默打开，且发生在注册路径上。
  - 修复：返回类型改为 `Result<Option<bool>, String>`，非法值返回 `carrier_catalog_register_bool_invalid:<pointer>:<value>` 并拒绝整行 profile；九个调用点全部用 `?` 传播。
  - 位置：`backend/src/connectivity/modems/ims/vowifi/carrier_catalog_v7.rs`
  - 测试：`wrongly_typed_register_switch_is_rejected_instead_of_defaulting`（`"no"`/`"yes"`/`"disabled"`/`1` 四种，断言错误里含 pointer 和原值）、`legal_register_switch_spellings_are_all_still_accepted`（`true`/`false`/`"true"`/`"false"`/`"omit"`/`"OMIT"` 六种仍可用，确保收紧不误伤既有 bundle）。
  - `access_identity_policy_at()` 本来就对非法值报错，无需修改。

## 8. 尚未实现：VoNR/5G IMS 实际链路

当前工程只有 LTE/NR 通用数据模型基础，不代表已经支持 VoNR。

- [ ] 实现 NR SA IMS PDU session/bearer 建立。
- [ ] 实现 5GS QoS flow、QFI 和语音媒体承载映射。
- [ ] 从 modem/provider 获取 NR serving cell、NCI、TAC、注册域与 IMS capability。
- [ ] 生成符合 NR 接入的 PANI/CNI，禁止仍标记为 E-UTRAN。
- [ ] 处理 EPS fallback、RAT handover 和 registration continuity。
- [ ] 实现并验证 VoNR capability 探测；不能仅凭设备支持 5G 就报告 VoNR ready。
- [ ] 增加 NR SA 注册、主叫、被叫、双向 RTP、DTMF、BYE、短信及回落场景测试。
- [ ] 在支持 NR IMS 的真实硬件和运营商网络上验收。

相关现有说明：`docs/ue-isolation-migration.md` 的 5G IMS 章节。

## 9. 实机验收矩阵

### 9.1 REGISTER 与 refresh

- [ ] 初始 REGISTER：无认证直返 2xx。
- [ ] 401/407 AKA challenge 后成功。
- [ ] 423 Min-Expires 协商后成功。
- [ ] 421/494 sec-agree 升级后成功。
- [ ] refresh 等待期间收到 MWI NOTIFY，不掉注册且 NOTIFY 最终被处理。
- [ ] refresh 等待期间收到 SMS MESSAGE，不掉注册且短信最终被处理。
- [ ] refresh 等待期间收到 INVITE，不掉注册且被叫正常振铃。
- [ ] refresh 第一候选失败、降级候选成功时，不重建 bearer/ePDG。
- [ ] 所有有限候选失败后才重建 access，并有明确诊断。

### 9.2 Profile 兼容性

- [ ] 完整 MMTEL feature tags + 动态 PANI/CNI。
- [ ] SMS-only Contact。
- [ ] 显式 omit PANI。
- [ ] 显式 omit CNI。
- [ ] sec-agree auto、required、disabled/omit。
- [ ] 有 Route 与无 Route。
- [ ] roaming visited network identity。
- [ ] 至少两个不同运营商 profile，避免为单一 Maxis 行为过拟合。

### 9.3 业务能力

- [ ] VoLTE 主叫。
- [ ] VoLTE 被叫，不进入语音信箱。
- [ ] 双向 RTP 与静音恢复。
- [ ] SMS over IMS 收发。
- [ ] MWI SUBSCRIBE/NOTIFY。
- [ ] ViLTE capability 与视频媒体协商（若当前版本宣称支持）。
- [ ] VoWiFi 主叫、被叫、切换和 refresh。

## 10. 可观察性与诊断

- [ ] REGISTER 日志加入事务键的脱敏摘要：Call-ID hash、CSeq、候选 label。
- [ ] 记录跳过帧的类型、方法、Call-ID 是否匹配，但禁止输出认证 nonce、Authorization 和完整用户身份。
- [ ] 区分以下失败：传输超时、无关帧洪泛、provisional 洪泛、状态行非法、事务键不匹配、认证拒绝、策略拒绝。
- [ ] 记录实际 PANI/CNI 来源：dynamic、static profile、compatibility fallback 或 omitted。
- [ ] refresh 降级成功时记录被移除的头，不记录完整 SIP 报文中的敏感字段。
- [ ] 为每条线路分别统计 refresh 成功率和 access rebuild 次数。

## 11. 当前工作树注意事项

截至本轮复核（2026-08-29），工作树包含大量未提交修改和以下未跟踪内容：

- `.ci-dl/`
- `.ci-patch.txt`
- `7733.zip`
- `r6-arm64.tar.gz`
- `r6/`
- `r7/`
- `simadmin_1.1.7-beta8.tar.gz`

这些内容可能是参考包、构建产物或逆向分析材料。后续开发不得默认删除、移动、格式化或加入提交。

当前修改跨越 IMS、platform config、数据库和文档。提交前应按逻辑拆分，至少建议：

1. REGISTER transaction filtering 与 channel requeue。
2. refresh 候选阶梯。
3. PANI/CNI runtime policy。
4. catalog v7 `omit` 投影。
5. 配置存储/迁移与文档。

## 12. 每项完成记录模板

复制以下模板到已完成项下：

```text
完成日期：YYYY-MM-DD
提交：<commit hash>
实现摘要：<一句话>
自动测试：<命令和结果>
实机环境：<设备/固件/运营商/接入方式>
实机结果：<REGISTER/主叫/被叫/RTP/SMS/MWI>
遗留风险：<没有则写“无已知风险”>
```

## 13. 完成定义

本计划只有同时满足下列条件才可整体标记完成：

- [ ] 第 4 至第 10 节的适用项目全部完成或明确标记为延期并说明原因。
- [ ] 定向测试和完整后端测试通过。
- [ ] Debian/aarch64 目标构建通过。
- [ ] 至少两个运营商 profile 完成 REGISTER refresh、主叫和被叫实机验收。
- [ ] 不再使用未标记的伪造 cell-id。
- [ ] `omit` 从数据库到最终 SIP 报文有端到端测试。
- [ ] 文档、配置 schema 和诊断字段与实现一致。


## 14. 第二阶段：每线路 VoLTE Profile 编排

### 14.1 目标与边界

本阶段把 VoLTE 的“连接重试”从重复使用同一个 carrier profile，改造成每条物理基带/读卡器独立保存的三个 profile 候选槽位。该策略属于 `LineProfileConfig`，不做成全局设置；SIM 更换后仍由该物理线路保留自己的编排规则。

必须区分两层尝试：

1. **Profile 候选尝试**：完整切换用户数据库、下载 catalog、3GPP 派生 profile；这是本阶段新增的外层三次尝试。
2. **单个 Profile 内部的 REGISTER 状态机**：401/407 AKA、423 Min-Expires、421/494 sec-agree，以及有限的 REGISTER 报文形态兼容候选；这些仍在单个 profile 尝试内部执行，不能与外层 profile 候选混为一谈。

### 14.2 持久化模型

- [x] 在 `LineProfileConfig` 新增 `volte_profile_selection`，并保证旧配置反序列化时自动得到默认值。
- [x] 每条线路固定保存三个有序候选槽位，默认顺序为：
  1. `database`：用户自行创建或导入到主应用数据库的 profile。
  2. `carrier_catalog`：下载并只读使用的 carrier catalog profile。
  3. `derived`：根据 SIM IMSI/Home PLMN 自动派生的 3GPP profile。
- [x] 每个候选保存 `{ source, profile_id? }`，不能只保存裸 `profile_id`，避免用户数据库和下载 catalog 同名时产生歧义。
- [x] `profile_id = null` 表示在该来源内按 IMSI/Home PLMN 自动选择；非空表示严格选择该来源中的指定 profile。
- [x] `derived` 候选禁止设置 `profile_id`。
- [x] 三个槽位允许用户重新排序；同一来源可重复出现，以支持测试不同显式 profile。
- [x] 旧 `SimOverride.ims_volte.profile_id` 保留兼容：线路候选未显式指定 ID 时，只在与旧 profile 实际来源一致的槽位中作为来源内 pin 使用；线路显式 ID 优先。
- [x] ConfigManager 增加 get/set/validate，并用测试证明线路 A 的修改不会影响线路 B 或独立读卡器线路。

### 14.3 来源解析和兜底规则

- [x] `database` 自动模式只搜索用户数据库，不得被下载 catalog 的同名 profile 覆盖。
- [x] `carrier_catalog` 自动模式只搜索下载 catalog，并要求 LTE/EPC 投影可用。
- [x] 显式指定 profile 时严格限制来源，用户数据库和下载 catalog 的同名 ID 必须可分别选择。
- [x] `derived` 始终根据当前 SIM 的 Home PLMN 派生，不从数据库读取。
- [x] 当 `database` 或 `carrier_catalog` 槽位没有可用 profile（来源不存在、无匹配项、显式项已删除或无 LTE 投影）时，该槽位改用派生 profile，并在运行时记录原请求来源和兜底原因。
- [x] 按用户要求保留三个逻辑槽位：若用户数据库和下载 catalog 都不存在，则三个槽位都可解析为同一个派生 profile，并实际执行三次；不得因去重而静默缩减为一次。
- [x] 派生也失败时返回可诊断错误，错误中只包含来源、槽位和脱敏后的 PLMN/IMSI 前缀，不输出完整 IMSI、AKA 数据或鉴权头。

### 14.4 运行时与重试行为

- [x] VoLTE 自动恢复批次最多按三个候选槽位执行，默认依次为用户数据库、下载 catalog、派生配置，不再无差别重复同一个 profile。
- [x] 每次候选切换前清理上一 profile 的 bearer/profile lease、P-CSCF、REGISTER 对话和临时安全关联，禁止跨 profile 复用 nonce、Route、Security-Verify 或 Call-ID。
- [x] Baseband-wedged 等不安全重试错误仍立即中止整个批次，不能为了尝试下一个 profile 继续冲击基带。
- [x] 运行时状态增加：当前候选索引、请求来源、请求 profile ID、实际 profile ID、实际来源、是否派生兜底、失败原因和每槽位结果。
- [x] 成功后保留实际生效 profile 的 ID/来源，下一次全新恢复批次仍从用户配置的第一个槽位开始。
- [x] 手动“重试”启动一个新的三槽位批次；REGISTER refresh 失败后的 access rebuild 也启动新的三槽位批次。
- [x] 非恢复路径（例如发送短信时发现 VoLTE 未连接）必须复用同一候选编排入口，不能绕过策略只尝试旧的自动匹配。

### 14.5 后端 API

- [x] 新增 `GET /api/volte/lines/{line_id}/profile-selection`，返回线路策略、可选 profile 列表和当前运行时解析结果。
- [x] 新增 `PUT /api/volte/lines/{line_id}/profile-selection`，保存三个有序候选并做来源/ID 校验。
- [x] 可选 profile 列表必须按来源分开返回，并至少包含 `profile_id`、PLMN、品牌/名称、来源、LTE ready 状态和 catalog release/source。
- [x] API 不得把下载 catalog profile 写入用户数据库；选择只保存引用。
- [x] 保存正在连接或已注册线路的策略时，先持久化，再安全断开旧会话并启动新批次；离线线路只保存配置。
- [x] API 错误码覆盖：无效线路、候选数错误、derived 带 ID、来源不支持、显式 profile 不存在、显式 profile 无 LTE 投影。

### 14.6 前端

- [x] 在 IMS 页面每条线路的“VoLTE / IMS 注册”旁新增“配置”按钮，交互位置和 VoWiFi、Trunk 保持一致。
- [x] 新增 VoLTE Profile 配置对话框，清楚显示“这是每条基带/读卡器独立设置，不是全局设置”。
- [x] 对话框提供三个可排序槽位；每槽位可选择来源，并在用户数据库/下载 catalog 来源下选择“自动匹配”或指定 profile。
- [x] 数据库或 catalog 不存在时显示“本槽位将使用派生配置兜底”，但仍允许保存。
- [x] 显示旧 SIM profile pin 的兼容状态，以及线路显式选择对它的覆盖关系。
- [x] 显示当前实际生效 profile、来源、候选槽位、兜底原因和最近三个候选结果。
- [x] 保存后刷新该线路，不影响其他线路卡片的状态和表单。

### 14.7 自动测试

- [x] 配置默认值和旧 JSON 迁移测试。
- [x] 每线路/读卡器隔离和持久化测试。
- [x] 三槽位顺序、重复来源、显式 ID 和 derived 禁止 ID 的验证测试。
- [x] 用户数据库与 catalog 同名 ID 的来源隔离测试。
- [x] 用户数据库自动匹配、catalog 自动匹配、显式匹配和派生匹配测试。
- [x] 用户数据库缺失、catalog 缺失、两者都缺失时的逐槽位派生兜底测试；两者都缺失时断言实际保留三次派生尝试。
- [x] 第一槽位失败第二槽位成功、前两槽位失败第三槽位成功、三槽位全部失败测试。
- [x] Baseband-wedged 批次立即中止。
  - 测试：`api::handlers::tests::volte_profile_batch_aborts_on_baseband_wedge_or_generation_change`。
- [x] 候选切换的逻辑运行时状态清理。
  - 测试：`connectivity::modems::ims::volte::live::tests::profile_switch_aborts_listener_and_releases_line_scoped_registration`、`connectivity::modems::ims::volte::runtime::tests::profile_switch_clears_session_ownership_without_cancelling_the_batch`。
- [x] 手动 retry 重置到槽位 1。
  - 测试：`connectivity::modems::ims::volte::runtime::tests::a_new_manual_retry_batch_restarts_at_slot_one`。
- [ ] 真实 bearer/QMI endpoint、AT CID、xfrm/IPsec、P-CSCF reporting 和 IMS profile lease 释放的集成测试。
- [x] API helper 的 GET/PUT payload、校验、离线保存、在线重启和跨线路隔离。
  - 测试：`api::handlers::tests::volte_profile_selection_*`。
- [x] HTTP/AppState API 集成测试基础设施与首批用例。
  - 完成日期：2026-08-29
  - 前置改造：把 `main()` 里内联的 733 行 router 抽成 `build_router(app_state, cors) -> Router`，逐行核对与 HEAD 一致，161 条路由数量不变。这是此前"测试无法构造真实 router"的唯一障碍。
  - 新增 `main.rs` 的 `http_router_tests` 模块，构造**仓库首个 `main()` 之外的 `AppState`**：十五个依赖全部按 `main` 的方式真实构造（database、overrides、config manager、carrier catalog、line registry、eSIM supervisor、notification sender、diagnostic log、event bus、event emitter、SMS resync、DDNS、E911、shutdown channel），落在用完即删的临时文件上，没有任何 stub。
  - 请求走真实 ephemeral 端口的 TCP socket，不用 `tower::ServiceExt::oneshot`（`tower` 只是传递依赖），因此同时覆盖 `axum::serve` 和 router 之外的 layer。
  - 四个用例：`/api/health` 免认证可达；四个不同子系统的受保护路由被 auth layer 拒绝；`spa_fallback` 两个分支被区分；受保护路由的 preflight 成功且带 CORS 头。
  - 两处刻意设计：受保护路由除断言 401 外还断言错误体，因为端点改名后会返回 404，只断言状态码会因错误原因通过——写这批测试时正是这条断言抓出了三个不存在的路径；SPA 用例断言响应体而非状态码，因为测试检出没有前端产物，两个分支都会 404。
  - D-Bus 处理：`Connection::system()` 认 `DBUS_SYSTEM_BUS_ADDRESS`，所以 session bus 可以顶替：
    ```bash
    dbus-run-session -- env DBUS_SYSTEM_BUS_ADDRESS="$DBUS_SESSION_BUS_ADDRESS" \
      cargo test --bin simadmin http_router
    ```
    无 bus 时每个用例带警告跳过而不是失败，保证无 D-Bus 机器上 `cargo test` 仍为绿。
- [x] 认证闭环的 HTTP 覆盖。
  - 完成日期：2026-08-29
  - 测试：`http_router_tests::a_session_from_login_opens_the_protected_routes`。
  - 覆盖：全新安装先拒绝 → `/api/auth/setup` 设置管理员密码 → `/api/auth/login` 下发 `simadmin_session` cookie → 该 cookie 打开受保护路由 → **伪造 cookie 仍被拒绝**。最后一条是必要的：没有它，正向断言对任意字符串都会通过。
  - 新增 helper：`authenticate()` 返回 session cookie，`post_json()`、`put_json()`、`get_with_cookie()`。cookie 手工回放，因为 `reqwest` 的 cookie store 需要本仓库未启用的 `cookies` feature。后续所有需要登录的端点用例都可以从 `authenticate()` 起步。
- [x] `omit` 取消暴露面在端点层的证据。
  - 完成日期：2026-08-29
  - 测试：`http_router_tests::a_partial_put_body_cancels_an_omit_through_the_live_endpoint`。
  - PUT 一个 `always_add_sip_instance` 显式为 false 但字段被删掉的 body，再经 `/resolve` 读回存储投影，断言该开关又变成了 `true`。这把第 7 节的 serde 层证据提升到了真实客户端会遇到的边界。
  - 该测试**刻意断言当前行为**：handler 改成 presence-aware 后它必须失败，失败信息里写明了这一点，以免过期测试对两种行为都点头。
  - 用 `/resolve` 而不是 list 端点，因为 list 只返回摘要，看不到开关。
- [x] VoLTE profile-selection 错误码矩阵的 HTTP 覆盖。
  - 完成日期：2026-08-29
  - 测试：`http_router_tests::profile_selection_put_reports_each_validation_error`、`profile_selection_get_answers_for_a_config_only_line`。
  - **无需硬件的关键点**：handler 的线路检查同时接受"config 里存在的线路"，所以用 `ConfigManager::reconcile_line_profiles(&["line-" + 32位hex])` 注册一条纯配置线路，就能越过线路检查、抵达全部校验错误。`TempState` 现在保留 `ConfigManager` 供测试注册。
  - PUT 侧断言五个错误码，全部经真实 HTTP：
    - 未知线路 → 404 `line_not_found`，且在存储任何策略之前拒绝（打错 line_id 不会留下无人读取的策略）
    - 槽位数不等于 3 → 400 `volte_profile_attempt_count_invalid`（不会静默补齐到三个）
    - `derived` 带 `profile_id` → 400 `volte_derived_profile_id_not_allowed`（derived 由 SIM 的 home PLMN 推导，指定 ID 是矛盾而非偏好）
    - 显式 ID 在指定来源中不存在 → 400 `volte_profile_not_found_in_source:<source>:<id>`，并断言错误里含来源名，避免另一来源的同名 ID 被当成命中
    - 无法识别的 source → 400（拒绝而非回落默认）
  - GET 侧：纯配置线路必须能返回三个槽位（对话框要在 modem 就位前就能打开），未知线路仍 404。
- [x] profile-selection 保存成功的 happy path，实机（2026-08-29，v1.1.5 / commit `726823d`）。
  - 在真实线路 `line-50ad5391cd09c09936f1081bd479139c` 上：GET 读到默认顺序 `[database, carrier_catalog, derived]`；PUT 改为 `[derived, database, carrier_catalog]` 返回 `status: ok`；重新 GET 读回新顺序，证明持久化；再 PUT 恢复默认顺序并再次读回确认。
  - 这补上了本地 HTTP 测试无法覆盖的部分——为在线线路保存会启动连接批次，需要真实线路。
- [x] 前端 TypeScript 类型检查、lint 和 production build。
  - 完成日期：2026-08-29；已执行 `pnpm type-check`、`pnpm lint`、`pnpm build:full`。
- [x] 浏览器级 VoLTE Profile 对话框交互测试（2026-08-29，提交 `af6b819`）。
  - 引入 Playwright（`@playwright/test` 1.62.1 + Chromium）作为前端第一个测试框架，此前 `frontend/package.json` 连 `test` script 都没有。
  - 位置：`frontend/e2e/volte-profile-dialog.spec.ts`、`frontend/playwright.config.ts`、`frontend/tsconfig.e2e.json`。
  - 运行方式：`E2E_PASSWORD=... pnpm test:e2e`。被测前端是**本地工作副本**（Vite 提供），其 dev server 已把 `/api` 代理到设备（`VITE_API_PROXY_TARGET`，默认 `http://192.168.100.13:3000`），所以改 UI 不必为测试重新部署前端。不设 `E2E_PASSWORD` 时全部跳过且退出码 0，无设备的机器上仍为绿。
  - 五个用例，全部对真实 410 通过：
    1. 对话框声明"不是全局设置"并显示三个槽位
    2. 排序按钮首尾禁用，且点击后槽位顺序真的交换（含第三槽位不受影响）
    3. 槽位来源切到派生后，该槽位的 Profile 选择器变为禁用
    4. 某来源没有 LTE-ready profile 时显示"将使用派生配置兜底"，且仍允许保存
    5. 保存按钮发出的 PUT 请求体被拦截检查，`attempts` 恰好三项
  - 新增 `data-testid="volte-profile-config"`（`ModemLinesPanel.tsx`）。一张线路卡上有四个写着"配置"的按钮（数据代理、VoLTE、VoWiFi、Trunk），侧边栏还有"基本配置"；按 label 匹配会点错并**真的执行**——写这批测试时它点到了数据代理的配置并在实机上触发了一次保存（事后核对 `config.sqlite3` 与部署前备份逐字节相同，是一次值未变的空保存，无实际影响）。
  - 两处选择器只能从运行中的应用学到、读源码不够：VoLTE 区块只在线路工作台的"IMS 与 Trunk" tab 下渲染，所以测试要先选线路再切 tab；每个槽位渲染两个 combobox（来源 + Profile 选择器），MUI 的关联方式让 `getByLabel` 取不到，所以来源按偶数下标读取。
  - e2e 代码是**被类型检查的**，不是排除在 lint 之外：`tsconfig.e2e.json` 从 `tsconfig.json` 引用，并加入 eslint 的 parser projects。`pnpm lint`、`pnpm type-check`、`pnpm build:full` 全部通过。

**本轮自动验证记录（2026-08-29）：**

- WSL Debian：`cargo test --bin simadmin -- --test-threads=1` 通过，结果为 `1338 passed; 0 failed; 3 ignored`（含新增 DNS 自定义端口测试）。
- WSL Debian：`cargo check --bin simadmin` 通过；仅有既有 dead-code warnings。
- 定向测试：`register::tests` 23、`volte::sip::tests` 36、`volte::live::tests` 53、`carrier_catalog::v7::tests` 12、`volte::channel::tests` 4、`core::access::tests` 3 均通过；`vowifi::channel::tests` 6 passed、1 ignored。
- `git diff --check` 通过。
- 前端：`pnpm type-check`、`pnpm lint`、`pnpm build:full` 均通过。
- 仍缺：真实 bearer/QMI/xfrm/P-CSCF/profile lease 资源释放集成测试、浏览器 E2E，以及第 14.8 节真实设备/网络验收。
- HTTP/AppState 集成测试已于 2026-08-29 完成基础设施加 8 个用例（router 抽取、认证闭环、SPA/API 分支、CORS preflight、`omit` 端点暴露面、profile-selection 错误码矩阵）。剩余的 HTTP 覆盖都属于需要真实线路的 happy path，归入第 14.8 节，不再作为第 14.7 节的缺口。

### 14.8 实机验收

- [ ] 用户数据库 profile 成功注册并完成主叫、被叫、双向 RTP、短信和 refresh。
- [ ] 下载 catalog profile 成功注册并完成同样业务矩阵。
- [ ] 前两来源不可用时派生 profile 兜底注册。
- [ ] 切换候选时抓包确认 Call-ID、CSeq 对话、安全关联和 P-CSCF 未跨 profile 污染。
- [ ] 两条不同基带线路配置不同顺序并同时运行，确认互不影响。
- [ ] 独立读卡器绑定线路保存/恢复自己的顺序，确认不读取其他基带的设置。


## 15. VoWiFi 自定义 DNS 恢复

- [x] 确认后端自定义 DNS 数据模型、API 和 ePDG 解析逻辑仍然存在，缺失点主要是 VoWiFi 配置对话框的前端控件。
- [x] 在 VoWiFi 线路配置页面恢复 SIM/线路级“自定义 ePDG DNS”输入项。
- [x] 支持填写多个 DNS 服务器，每行一个，并按用户填写顺序依次尝试。
- [x] 保存到该 SIM/线路的 `ims_vowifi.dns` 覆写字段，不影响其他线路。
- [x] 清空输入后保存为 `null`，回退到 carrier profile DNS，再回退到系统/内置 DNS。
- [x] 前端增加 IPv4、IPv6 及带端口 DNS 输入校验。
- [x] 后端增加 `ims_vowifi.dns` 校验，拒绝非法地址和端口 0。
- [x] 后端定向测试：`effective_profile::tests` 16 passed。
- [x] 后端 VoWiFi live 定向测试：69 passed。
- [x] 前端 `pnpm type-check`、`pnpm lint` 和 `pnpm build:full` 通过（2026-08-29）。
- [x] 410 实机确认 `ims_vowifi.dns` 可持久化，配置 `1.1.1.1`、`8.8.8.8` 后 ePDG 解析阶段达到 `epdg_ready`（2026-08-29）。
- [x] 410 实机故障注入确认运行时确实使用线路级 DNS：配置不可达的 `192.0.2.1` 后日志记录该地址并返回 ePDG DNS 超时（2026-08-29）。
- [x] 显式 DNS 全部失败时追加系统 resolver 兜底：`live_dns_attempts()` 始终在“线路 DNS -> profile DNS”之后加入 `None`，由系统 resolver 接管；新增单元测试 `live_dns_attempts_end_with_system_resolver_fallback` 通过（2026-08-29）。
  - 410 实机故障注入：临时将 ePDG host 设为 `localhost`、端口设为 `9`，仅配置不可达 `192.0.2.1`；日志记录自定义 DNS 超时但未产生最终 `epdg_dns_resolution_failed`，流程继续到 IKE 阶段，随后因测试端点失败而结束。该记录证明系统 resolver 兜底生效，但不代表运营商网络验收。
- [x] 第二次部署：commit `726823d` 已构建、校验并部署到 410（2026-08-29 深夜）。
  - 推送到 CI 触发分支 `feat/catalog-free-iphone-fallback`（`f44aac8..726823d`，快进），GitHub Actions 重新上传 v1.1.5 release 资产。
  - 制品 SHA-256 `2d2e32a0...9782a03` 与 `SHA256SUMS.txt` 一致；包内 `meta.json` 的 commit 为 `726823d`。
  - 部署前备份到 `/opt/simadmin-backup-20260829-234808`（`data.db`、`config.sqlite3`、`config.yaml`、`meta.json`、两个 carrier 数据库）。
  - **部署注意**：直接 `cp` 二进制会因 `simadmin-secondary-qmi.service` 也在运行同一可执行文件而报"文本文件忙"。必须同时停这两个 unit，并用 `mv` 替换。
  - 部署后：`/opt/simadmin/simadmin` 的 md5 与 `meta.json` 的 `binary_md5` 一致（`a7cdbb92...`），两个 service 均 active，`/api/health` 返回 `version: 1.1.5`，`data.db`/`config.sqlite3`/`config.yaml` 均保留且 `config.sqlite3` 与备份逐字节相同。
  - 调试用的临时 systemd drop-in 已删除，`RUST_LOG` 恢复为 unit 默认值；`/tmp` 下的制品和 staging 目录已清理。
- [x] v1.1.5 ARM64 制品已构建、校验并部署到 410（2026-08-29）。
  - GitHub Actions Run `33236459920` 成功；制品 `aarch64-unknown-linux-musl`，commit `f44aac8`，SHA-256 已与 `SHA256SUMS.txt` 一致。
  - `/api/health` 返回 `version: 1.1.5`；三个 systemd 服务/定时器均为 active；管理员密码登录验证成功（明文密码不写入文档）；既有 `data.db`、`config.sqlite3`、carrier 数据库和 `config.yaml` 均保留。
  - 恢复后的 `ims_vowifi.dns` 仍为 `1.1.1.1`、`8.8.8.8`，重新连接后 ePDG/IKE/Child SA/ESP readiness 均达到 true；当前运营商仍在 IMS REGISTER 阶段返回 SIP `400`，所以完整 VoWiFi 注册仍未完成。
- [x] 自定义 DNS 端口在运行时真正生效（2026-08-29，提交 `2f2606a`；实机验证见本节末）。
  - 问题：前端和 `parse_dns_server` 都支持 `1.1.1.1:5353`、`[IPv6]:5353`，但 live 层调用 `parse_dns_server(...).ip()` 丢弃端口，`epdg.rs` 又用 `SocketAddr::new(dns_server, 53)` 重建目标，导致自定义端口被静默改回 53。
  - 修复：DNS 候选类型由 `Vec<IpAddr>` 改为 `Vec<SocketAddr>`，贯穿 `live_dns_candidates()`、`live_dns_attempts()`、`resolve_epdg_with_dns_override()`、`resolve_epdg_via_socks5()` 和 `query_dns_records()`。
    - 位置：`backend/src/connectivity/modems/ims/vowifi/live.rs`、`backend/src/connectivity/modems/ims/vowifi/epdg.rs`
  - 系统 resolver 与公共 DNS 兜底显式构造为端口 53，行为不变。
  - `live_epdg_settings()` 对外仍返回 `Option<IpAddr>`，既有 API 形状不变。
  - SOCKS5 代理路径同样保留自定义端口：`query_dns_via_socks5()` 传入完整 `SocketAddr`，由 `encode_udp_datagram()` 写入 RFC 1928 §7 的 `DST.PORT`；既有测试 `encodes_ipv4_udp_request_header`、`encodes_ipv6_udp_request_header` 已覆盖任意端口编码。
  - 新增测试：`connectivity::modems::ims::vowifi::epdg::tests::custom_dns_query_uses_the_configured_udp_port`。
    - 用本地 UDP responder 绑定临时端口（断言不等于 53），同时应答 A 与 AAAA 查询，证明查询确实发到配置端口而不是 53。
  - 自动测试：`vowifi::epdg::tests` 4 passed；完整后端 `cargo test --bin simadmin -- --test-threads=1` 结果 `1338 passed; 0 failed; 3 ignored`。
  - 格式化：仅对本次修改的两个后端文件执行 `rustfmt`，`git diff --check` 无空白错误。
- [x] 前端帮助文本明确端口有效：输入项标签改为“自定义 ePDG DNS（地址或地址:端口）”，helper text 说明 `IPv4:端口` / `[IPv6]:端口`，省略端口默认 53（2026-08-29，未提交）。
  - 位置：`frontend/src/pages/sim/VowifiLineDialog.tsx`
  - 自动测试：`pnpm type-check`、`pnpm lint`、`pnpm build:full` 均通过。
- [x] 410 实机验证自定义 DNS 端口（2026-08-29，v1.1.5 / commit `726823d`）。
  - 步骤：把线路的 `ims_vowifi.dns` 设为不可达的 `192.0.2.1:5353`（非 53 端口），读回确认持久化为 `['192.0.2.1:5353']`，然后触发一次 VoWiFi 重连。
  - 设备日志（关键证据，端口是 **5353** 而不是 53）：
    ```text
    WARN ePDG resolution failed on this DNS server; trying the next candidate
         line_id=line-50ad5391cd09c09936f1081bd479139c
         dns_server=Some(192.0.2.1:5353) error=DNS resolution failed: fallback_timeout
    ```
  - 同时验证了兜底链路：自定义 DNS 超时后继续走下去，最终仍解析出真实 ePDG 地址
    `[202.75.146.42:500, 202.75.146.43:500, 202.75.146.41:500]` 并进入 IKE 阶段。
  - 修复前（commit `f44aac8` 及更早）这里会打印 `192.0.2.1:53`——端口被静默改回 53。
  - 测试后已把 DNS 恢复为 `['1.1.1.1', '8.8.8.8']`。
  - 说明：未用 tcpdump 抓包，设备上没有该工具；证据是运行时日志打印的目标 `SocketAddr`，它就是 `send_to()` 的实际目标。
- [ ] 真实设备/运营商网络完整验收：ePDG/IKE/Child SA/ESP 均已就绪，但 IMS REGISTER 未闭环。
  - **纠正一处此前写错的结论（2026-08-30）。** 本项原先写成"运营商阻塞在 `400/421`"，那是照抄更早 session 的说法，没有对照已有记录核实。实际上 **2026-08-22 这条线路曾拿到 `200 OK`**（修复提交 `97e982d`、`2818de0`、`dd4bb0f`，当时 SMS 与 Voice over IMS readiness 都验过）。所以这更可能是回归，不是运营商侧拒绝。把它记成"运营商阻塞"会让后续 session 放弃排查，这个错误说明必须留在文档里。
  - 当时确认过的 Maxis(50212) 四步握手：
    | 发送 | 运营商回 |
    |---|---|
    | Security-Client，无 Require | 421 Require: sec-agree |
    | + Require | 400（RFC 3329 §2.3 还要 Proxy-Require） |
    | + Proxy-Require | 400（TS 24.229 §5.1.1.2.2 还要空 AKA Authorization） |
    | + 空 AKA Authorization | 401 → AKA → **200 OK** |
  - **当前可疑点（尚未在设备上确认）**：410 的 REGISTER 日志里只出现两个 variant，且两者的 `initial_authorization` 都是 `"none"`：
    ```text
    register_variant="standard_3gpp_conservative"     initial_authorization="none"  sec_agree_headers_present=false
    register_variant="catalog_v7_sec_agree_required"  initial_authorization="none"  sec_agree_headers_present=true
    ```
    也就是上表第 4 步那个空 AKA Authorization 没有被发出。
  - 相关背景：`0444d58` 有意把 catalog 投影的 `initial_authorization` 默认值从 `"aka_empty"` 改成 `"none"`，理由是 TS 24.229 §5.1.1.2.2 要求首次 REGISTER 不带认证，某些运营商对预填的空 AKA 直接回 400；差异改由候选阶梯吸收。`vowifi/live.rs` 的阶梯里确实存在多个 `AkaEmpty*` 形态（约 1242–1395 行），但设备日志显示阶梯没有走到它们。
  - 下一步需要的证据（都要在设备上取）：
    1. 运营商现在实际回的状态码（是否仍是 421/400，还是别的）
    2. 阶梯为何停在前两个 variant——是提前放弃、还是这个 profile 的候选列表本就不含 `AkaEmpty*`
    3. 与 `f44aac8`（已知可注册的提交）做 A/B 对照
  - 本项不能仅因 `epdg_ready` 勾选。
