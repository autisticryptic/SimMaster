# SimAdmin VoLTE 改进记录与待办事项

> 创建时间：2026-07-22
> 范围：参照 1.6/1.7 逆向文档对 VoLTE 数据面、连接可观测性、配置架构、数据代理、IP 家族策略的全面改进。

---

## 最新校正与本地基线（2026-07-27）

> 本节是当前结论；若与后文 2026-07-26 及更早记录冲突，以本节和
> `VOLTE_真机实测结论_QMI端点能力.md` 为准。

### QMI 端点职责校正

- 备用端点 `/dev/wwan0qmi1` 配合 `raw-ip + no-qos-header` 可以单次建立 WDS，
  但后续进程无法继续使用已分配 CID，会出现 `Transaction timed out`。
- IMS 需要在同一 CID 上连续完成 `set-ip-family -> start-network ->
  get-current-settings`，最后一步还负责取得网络通过 PCO 下发的 P-CSCF。
- 因此完整原生 IMS 流应使用主端点 `/dev/wwan0qmi0`，并通过
  `qmi-proxy` 与 ModemManager 共享；备用端点保留给可单次启动的普通数据会话。
- 原生 IMS 激活仍未在当前参考设备上完成安全真机验证，默认保持关闭，只能通过
  `SIMADMIN_VOLTE_NATIVE_IMS_BEARER=1` 显式启用。默认生产路径仍由
  ModemManager 建立 IMS bearer。
- 禁止发送 `--wds-bind-data-port` 或 `--wds-bind-mux-data-port`；参考固件不支持，
  且实测可能导致 endpoint hangup、基带子系统重启甚至整机重启。

### 本轮完成

- 补齐 `readiness` 模块注册，beta2 的 `/run/qmi_auto_activate.ready` 建议性等待逻辑
  已进入实际编译模块树。
- 原生 QMI bearer 每 30 秒通过其原 WDS CID 查询 packet-service 状态：明确断线
  立即重建；普通查询错误连续 3 次后重建；基带卡死特征立即停止继续尝试。
- 原生 bearer 清理继续由 WDS handle 停止会话并释放 CID，不会错误交给 mmcli。
- 修复前端 VoLTE 尝试记录类型和每线路“解除小区锁定”的 `line_id` 传递。

### 验证基线

- 后端：`cargo test --locked`，692 passed，0 failed。
- VoLTE 定向：155 passed，0 failed。
- cellular/QMI 定向：89 passed，0 failed。
- 前端：`pnpm run build:full` 通过。

---

## 零、根因定位与突破（2026-07-26，真机验证成功）

> ⚠️ **此节推翻了本文档第三节此前的全部推测**（固件 DHCP bug / SIM FDN / 号码未开通 VoLTE 均**不是**主因）。

### 真正的根因：QMI 端点争用

ModemManager 独占主 QMI 端口 `/dev/wwan0qmi0` 跑普通移动数据。在**同一端口**上再建 IMS 数据会话 → 报 `interface-in-use-config-match`，或在激活 IMS PDP context 时**崩基带**。

### 解法：给 IMS 独立的 QMI 端点

内核自带 `rpmsg_wwan_ctrl` 的 ID 表只认三个通道：

```c
{ "DATA5_CNTL", WWAN_PORT_QMI }  // -> wwan0qmi0（ModemManager 在用）
{ "DATA4",      WWAN_PORT_AT  }  // -> wwan0at1
{ "DATA1",      WWAN_PORT_AT  }  // -> wwan0at0
```

用 `driver_override` 强绑 `DATA6_CNTL` **不行**：驱动匹配不到，`driver_data=0`（`WWAN_PORT_UNKNOWN`），内核把它发布成 `wwan0at2`（`type=AT`）——半通状态（CTL 能查、WDS 分配报 `endpoint hangup`、`start-network` 崩基带）。

**自编译 `rpmsg_wwan_ctrl_multi.ko`** 把备用通道以正确的 `WWAN_PORT_QMI` 注册 → 得到 `wwan0qmi1`/`wwan0qmi2`，`type=QMI` 真端口。

### 真机验证结果（192.168.100.13，Maxis 50212）

```
qmicli -d /dev/wwan0qmi1 --device-open-qmi \
  --device-open-net='net-raw-ip|net-no-qos-header' \
  --client-no-release-cid --wds-start-network='apn=ims,ip-type=4'

→ Network started, Packet data handle: 3263198272        ✅
→ IPv4 10.129.39.207 / mask 255.255.255.224 / gw .208    ✅
→ DNS 172.17.163.218, 172.17.167.218, MTU 1500           ✅
→ 基带全程存活（未崩溃）                                    ✅

ip-type=6 → CallFailed, verbose reason: [3gpp] ipv4-only-allowed
```

### 三个关键实现细节（少一个就不成）

1. **必须带 `--device-open-net='net-raw-ip|net-no-qos-header'`**。不带时 WDS CID 分配报 `endpoint hangup`；带上就稳定。`--wda-get-data-format` 证实端点是 `raw-ip` + QoS header no。
2. **ID 表要短**。曾一次注册 DATA6..DATA40（14 条）→ 暴露 14 个 QMI 口冲击 ModemManager 枚举。收敛到 DATA6+DATA7。
3. **加载模块后 MM 会短暂 `No modems were found`，约 15s 自动恢复**（新端口 attach 触发重扫），不是崩溃。

### 交付物

```
SimAdmin/kernel/rpmsg_wwan_ctrl_multi/
  ├── rpmsg_wwan_ctrl_multi.c   内核模块源码（含完整原理注释，可自行扩展通道）
  ├── Makefile                   本地/交叉编译 + install + load
  └── README.md                  为什么需要、怎么编、怎么验证、怎么扩展、多基带说明

SimAdmin/backend/src/cellular/secondary_qmi.rs
  端点发现 + 多基带配对 + 能力探测 + IMS 会话 start/stop/settings

SimAdmin/deploy/
  ├── install.sh                              一键安装（含内核模块/udev/systemd）
  └── system/simadmin-secondary-qmi.service   开机在 MM 之前初始化端点
      system/99-simadmin-secondary-qmi.rules  让 MM 忽略 IMS 端点
```

### 设备编译环境

Debian 13 trixie，内核 `6.17.0-rc6-lkiuyu-compile+`，头文件在 `/usr/src/linux-headers-<ver>`（Makefile/scripts/Module.symvers 齐全），`CONFIG_MODULE_SIG_FORCE` 未开 → 自编译 `.ko` 可直接加载。设备原本无 gcc/make，已装（`apt-get install -y --no-install-recommends gcc make`）。编译目录 `/root/ko_build`。

**主机侧交叉编译**：用 `cargo zigbuild --release --target aarch64-unknown-linux-musl`（zig + cargo-zigbuild 已装在 `D:\Program\Dev\Languages\Rust\cargo\bin`）。普通 `cargo build --target` 会因 `ring` 需要 aarch64 C 编译器而失败。

---

## 一、已完成项（可编译验证，599 个测试全绿）

### 0. 纯 ModemManager 主导路线（2026-07-22 新增，贴合 beta2）

**背景：** 另一 AI 诊断当前版本报 `QMI protocol error(14): CallFailed - interface-in-use-config-match`——ModemManager 已持有 `/dev/wwan0qmi0` 的 WDS 会话管理普通数据，而 `data_path.rs` 的 IPv6 WDS 预热又用 `qmicli --wds-start-network` 在同一端口抢第二个 WDS 会话，被 modem 拒绝。

**改动文件：**
- `backend/src/access/volte/data_path.rs`：新增 `ProbeResult::Disabled`；新增 `WDS_PREFLIGHT_ENV = "SIMADMIN_VOLTE_WDS_PREFLIGHT"` 环境开关；`probe_ims_ipv6` **默认直接返回 `Disabled`**（不开预热），仅当显式 `SIMADMIN_VOLTE_WDS_PREFLIGHT=1` 才走原 WDS 预热逻辑
- `backend/src/access/volte/live.rs`：`connect_inner` 的 `Ipv6Preflight` 阶段把 `Disabled` 与 `NoMuxEndpoint` 同等处理（记为 skipped，日志 `WDS preflight disabled; using ModemManager-managed bearer`）

**效果：** 默认连接路径是纯 ModemManager（`mmcli --create-bearer=apn=ims`），不再直接抢 QMI 端口，消除 `interface-in-use-config-match` 冲突。这正是 beta2 的主路线（`managed_mm_data.rs`）思路。兼容性也更好——ModemManager 自动处理 QMI/MBIM/AT 各厂商差异。

**已实机验证（2026-07-22，设备 10.0.0.116）：** 日志确认 `VoLTE IMS IPv6 WDS preflight disabled; using ModemManager-managed bearer`，纯 MM 路线生效，不再有 QMI 抢占错误。✅

---

### 0b. 默认 IP 家族顺序改为 IPv4First（2026-07-22 新增）

**改动文件：**
- `backend/src/infra/config.rs`：`VolteIpFamilyPreference` 的 `#[default]` 从 `Ipv6First` 移到 `Ipv4First`；更新枚举文档注释；更新两处断言默认值的测试（`volte_ip_family_preference_round_trips`、`per_line_volte_connection_is_independent_and_persists`）

**效果：** 符合用户要求的族选择逻辑——默认双栈（ipv4v6）请求；网络**明确**要求 v4-only/v6-only 时直接切换对应族（`FailureClass::NetworkForcedIpv4/Ipv6`，逻辑已在 P2 就位）；**不明确时先 IPv4 再 IPv6**（原来默认 Ipv6First 是先 v6，与要求相反）。

**需要实机验证的点：** 双栈失败且网络无明确族要求时，回落顺序为 v4→v6。

---

## 一、已完成项（可编译验证，583 个测试全绿）

### 1. Goal A — 删除 VoLTE 全局连接闸

**改动文件：**
- `backend/src/access/volte/live.rs:475-479`：删除 `if !config.connection_enabled { return Err(RUNTIME_NOT_RUNNING) }` 全局闸
- `backend/src/api/handlers.rs:3133` / `6150-6151`：删除调用方硬塞 `connection_enabled = true`

**效果：** 功能判断唯一权威为 `profile.enabled && profile.volte_connection_enabled`，全局 `feature_enabled`/`sms_enabled`/`connection_enabled` 仅保留作兼容序列化。

**需要实机验证的点：** 无（纯配置逻辑，编译测试即可覆盖）。

---

### 2. P1 — 连接尝试结构化字段

**改动文件：**
- `backend/src/access/volte/runtime.rs`：`VolteConnectionAttempt` 增加 `at_cid`/`qmi_device`/`bearer_path`/`interface`/`pcscf` 5 个结构化字段；`VolteSnapshot` 增加 `at_cid`/`bearer_path`；`record_attempt` 从快照自动捕获上下文
- `backend/src/access/volte/live.rs`：CID 确定后写 `state.at_cid`，bearer 连上后写 `state.bearer_path`
- `frontend/src/api/contracts.ts`：`VolteConnectionAttempt` 接口加 5 个可选字段
- `frontend/src/pages/sim/LineDetailsDialog.tsx`：attempt 行增加结构化元信息一行

**效果：** Web UI 连接尝试历史中，每一步的 AT CID、QMI 设备路径、bearer 路径、网卡名、P-CSCF 地址作为独立字段展示，不再需要解析 detail 自由文本。

**需要实机验证的点：**
- 连接成功时，`at_cid`（应为 2）、`qmi_device`（应为 `/dev/wwan0qmi0`）、`bearer_path`（应为 `/org/freedesktop/ModemManager1/Bearer/N`）、`interface`（应为 `wwan0`）、`pcscf`（P-CSCF IP）是否都正确填充
- 连接失败时，是否能从 attempt 历史中准确定位失败步骤

---

### 3. Goal B — 数据连接仅代理出口，不做系统默认路由

**改动文件：**
- `backend/src/cellular/modem_manager.rs`：`set_data_connection_inner` 的 `isolated=false` 改为 `true`（所有 mmcli bearer 一律 `never-default + ignore-auto-dns`）；删除 `init_data_connection` 函数；watchdog 的数据激活分支改为跳过（打日志不拨号）
- `backend/src/main.rs`：移除开机全局自动拨号块

**效果：** 启用"数据连接与代理出口"后，该 SIM 卡的数据流量只能通过对应线路的 HTTP/SOCKS5 代理出口（`SO_BINDTODEVICE`），不会接管系统默认路由或 DNS，设备自身的出站流量（OTA/通知/DDNS 等）仍走 LAN/WiFi。

**需要实机验证的点：**
- 启用某线路数据后，`ip route show` 确认该卡的 bearer 接口没有成为默认路由（`default dev wwan0 ...` 应不存在）
- 代理软件（curl/浏览器 + SOCKS5 代理）的请求确实从 wwan0 出站（`tcpdump -i wwan0` 可见）
- 未走代理的流量（ping 8.8.8.8 without proxy）仍走 LAN 出
- 关闭数据连接后，`ip route` / `ip addr` 确认路由条目清理干净

---

### 4. P2 — IP 家族策略统一为 ImsConnectionPlan

**新增文件：**
- `backend/src/access/volte/plan.rs`：`IpFamily`（Ipv4/Ipv6）、`IpType`（Ipv4v6/Ipv4/Ipv6）及三套词汇转换器（config/MM/AT）、`FailureClass` 枚举（NetworkForcedIpv6/v4、PrefixUnavailable、FamilyUnsupported、PcscfFailed、Other）、`ImsConnectionPlan::from_preference()`

**改动文件：**
- `pcscf.rs`：`ordered_local_addrs` 和 `discover_pcscf_via_at_with_context` 改为接受 `&ImsConnectionPlan`，删除 `ordered_pdp_types`
- `bearer.rs`：`ensure_ims_bearer_observed` 增加 `plan` 参数，VecDeque 回落按 plan 顺序（而非硬编码 v4→v6），删除 `required_ip_type_after_failure`/`fallback_ip_types`
- `live.rs`：`connect_inner` 顶部统一建 `ImsConnectionPlan`，删除 `should_try_next_family`/`should_retry_bearer_after_at_context_cleanup`，全部用 `FailureClass` 替代

**效果：** 四处原本各自为政的族选择逻辑（AT 探测顺序、bearer 回落、预热、SIP循环）现在从同一个 plan 对象派生，bearer 回落也尊重 `ip_family_preference` 配置（原来始终 v4-first）。

**需要实机验证的点：**
- 设置 `ip_family_preference: ipv6_first`，确认 AT 探测顺序为 IPV4V6→IPV6→IP，bearer 回落顺序为 v6→v4（不再总是 v4→v6）
- `ip_family_preference: ipv4_first` 时回落顺序反转为 v4→v6
- 网络强制 IPv6 only（`Ipv6OnlyAllowed`）时，能直接跳过 ipv4 尝试
- `ip_family_preference: ipv4_only` + IPv6-only 网络：能产生 `volte_runtime_ims_family_unsupported` 错误码而非循环重试

---

## 二、待完成项（代码可写，实机验证条件不具备）

> ⚠️ 当前设备状态：`mmcli -L` 返回 `No modems were found`，设备基带未枚举出。以下两项在代码层面可以编写并编译测试，但真实效果需等基带重新出现后验证。

---

### P3 — 真正支持 ISIM 身份读取

**当前状态：** ISIM AID 已被识别并打标签（`DeviceIdentity.isim_aid` 作为状态展示），但身份永远从 IMSI 推导（`derive_identity`），从不 SELECT ISIM 读 EF。

**待实现内容：**

1. **`backend/src/access/vowifi/qmi_uim.rs` 新增 APDU 构造器：**
   ```rust
   // SELECT EF（SELECT BY FILE ID）
   pub fn build_select_ef_apdu(channel: u8, ef_id: u16) -> Vec<u8> {
       // CLA=0x0C+channel INS=0xA4 P1=0x08(by file id) P2=0x04(return FCP) Lc=0x02 Data=ef_id
   }
   // READ BINARY
   pub fn build_read_binary_apdu(channel: u8, offset: u16, length: u8) -> Vec<u8> {
       // CLA=0x0C+channel INS=0xB0 P1=offset_high P2=offset_low Le=length
   }
   // 从 SELECT FCP 响应解析文件大小（tag 0x80 或 0x82+0x81）
   pub fn parse_ef_length(fcp: &[u8]) -> Option<u16>
   // 解析 TLV 结构（tag: u8, data: &[u8]）→ 提取 tag 对应的值
   pub fn find_tlv_value<'a>(data: &'a [u8], tag: u8) -> Option<&'a [u8]>
   ```

2. **`backend/src/access/volte/isim.rs` 新模块：**
   ```rust
   pub struct IsimIdentity {
       pub impi: Option<String>,          // EF_IMPI 6F02, TLV tag 0x80
       pub impus: Vec<String>,            // EF_IMPU 6F04, 多条记录每条 TLV tag 0x80 in A0
       pub home_domain: Option<String>,   // EF_DOMAIN 6F03, TLV tag 0x80
       pub ist: Option<Vec<u8>>,          // EF_IST 6F07, 原始字节（服务表）
   }

   // 打开 ISIM 逻辑通道 → 读四个 EF → 关闭通道
   // 任何一步失败 → 返回部分结果或 Err，由调用方决定回退策略
   pub fn probe_isim_identity(
       proxy_socket: &str,
       device_path: &str,
       slot: u8,
       isim_aid: &[u8],
       timeout: Duration,
   ) -> Result<IsimIdentity, String>
   ```

3. **`backend/src/access/volte/live.rs::load_device_identity` 更新：**
   - 当 `applications.isim_aid.is_some()` 时，尝试调用 `probe_isim_identity`
   - 成功时用 ISIM 数据构建 `ImsIdentity`（IMPI → `private_user`，首个 IMPU → `public_uri`，DOMAIN → `home_domain`）
   - 失败时回落到现有 `derive_identity(imsi, mcc, mnc)`，`source` 字段设为 `"isim_fallback_read_failed"`

**实机验证需求：**
- [ ] 设备基带枚举出（`mmcli -L` 有输出）
- [ ] SIM 卡有 ISIM 应用（`qmicli -d /dev/wwan0qmi0 --device-open-proxy --uim-get-card-status` 输出含 `application type: 'isim'`）
- [ ] 验证 EF_IMPI 读取正确（应为 `<number>@<carrier-domain>`，如 `8613812345678@ims.46011.3gppnetwork.org`）
- [ ] 验证 EF_IMPU 读取正确（应包含 `tel:+86138xxxxx` 或 `sip:+86138xxxxx@...`）
- [ ] 验证 ISIM 身份读取失败时能正确回落 IMSI 推导，不影响 VoLTE 连接
- [ ] 在 Web UI 的 VoLTE 详情页确认 "身份来源" 字段变为 `isim` 而非 `imsi_fallback_isim_detected`

---

### P5 — 消除全局基带路径

**当前状态：** 约 30 个 `modem_manager.rs` 函数仍通过全局 `find_modem_path`（始终选第一个 modem），无 line 参数——APN、无线模式、频段锁、运营商扫描/选择、小区位置、信号、通话、legacy SMS 都走全局路。小区锁为纯内存全局态（`app.cell_lock`），未接硬件。VoWiFi 仍单基带。

**待实现内容：**

#### P5-a：清理 VoLTE 遗留全局 shim（小改，可立刻做）
- `handlers.rs:5520`（`set_volte_feature_handler`）和 `5559`（`set_volte_connection_handler`）是两个仅允许禁用连接的遗留全局入口，已有注释说明"物理 IMS 会话必须通过线路端点"。可安全降级为仅返回 `volte_line_endpoint_required`（enable 和 disable 都要求使用线路端点），或直接删除这两个全局 handler。

#### P5-b：小区锁（CellLockStore）改为 per-line（中改）
- `CellLockStore` 目前是 `app.cell_lock: Mutex<CellLockStore>`（单个全局实例）
- 改为 `app.cell_lock: Arc<Mutex<HashMap<String, CellLockStore>>>` 或纳入 `LineRuntime`
- Handler 添加 `Path(line_id): Path<String>` 参数

#### P5-c：射频/运营商 Handler 改为 per-line（大改）
以下 handler 需要加 `line_id` 参数，并从 `line_registry.get(line_id).binding().modem_path` 取 modem_path 传给 per-modem 孪生函数：

| Handler | 当前调用 | 改为 |
|---|---|---|
| `get_radio_mode_handler` | `get_radio_mode(&conn)` | `get_radio_mode_for_modem(&conn, modem_path)` |
| `set_radio_mode_handler` | `set_radio_mode(&conn, ...)` | `set_radio_mode_for_modem(...)` |
| `get_band_lock_handler` | `get_band_lock_status(&conn)` | per-modem 孪生（待补全） |
| `set_band_lock_handler` | `set_band_lock(&conn, ...)` | per-modem 孪生（待补全） |
| `get_cell_location_handler` | `get_cell_location(&conn)` | per-modem 孪生（待补全） |
| `get_network_operators` | `get_operators_list(&conn)` | per-modem 孪生（待补全） |
| `scan_network_operators` | `scan_operators(&conn)` | per-modem 孪生（待补全） |
| `register_operator_manual_handler` | `register_operator_manual(&conn, ...)` | per-modem 孪生（待补全） |
| `register_operator_auto_handler` | `register_operator_auto(&conn)` | per-modem 孪生（待补全） |
| `get_apn_list_handler` | `list_apn_contexts(&conn, ...)` | per-modem 孪生（待补全） |
| `set_apn_handler` | 通过 payload.context_path 定位 | 加 line_id 验证 |

**API 路由变更示意（frontend 同步）：**
```
# 原来
GET/POST /api/radio-mode
GET/POST /api/band-lock

# 改为
GET/POST /api/lines/:line_id/radio-mode
GET/POST /api/lines/:line_id/band-lock
GET/POST /api/lines/:line_id/cell-lock
GET     /api/lines/:line_id/network/operators
POST    /api/lines/:line_id/network/operators/scan
POST    /api/lines/:line_id/network/operators/register
```

#### P5-d：VoWiFi per-line 化（大改，依赖 P5-c 模式）
VoWiFi 目前是单全局运行时 (`app.vowifi_runtime`) + 全局 `current_sim_identity` + `DEFAULT_QMI_DEVICE`。改法参考 VoLTE 已有的 per-line 模式（`LineRuntime.volte`/`volte_live`），对 VoWiFi 做同等的 `VowifiRuntime`  per-line 化。工作量大，建议单独立项。

**实机验证需求（P5-b ~ P5-d）：**
- [ ] 多基带设备上，针对某一个 line_id 设置无线模式/频段锁，确认只影响对应基带
- [ ] 小区锁设置/清除后，重启 simadmin，确认 per-line 状态持久化（若改为写入 LineProfileConfig）
- [ ] 运营商扫描/手动注册，确认操作的是指定线路的 modem 而非第一个

---

## 三、VoLTE 真机连接问题（硬件/运营商侧）

> 此问题与代码无关，是 MSM8916 固件 + 电信 46011 订阅侧的问题。

### 2026-07-22 真机复验（当前部署版本 = 纯 MM 路线）

手动在设备上复现 `mmcli --create-bearer=apn=ims` + `--connect`：

| 步骤 | 结果 |
|---|---|
| modem 0 整体状态 | `connected`、LTE、电信 46011、信号 100%、packet attached ✅ |
| 默认数据 bearer（CID1 ctnet, ipv4v6） | 正常，拿到 IPv4 `10.66.211.8` + IPv6 `240e:478:...` ✅ |
| CID2 IMS context | `+CGDCONT: 2,"IPV6","ims"`（IPV6 单栈存在） |
| `create-bearer apn=ims,ip-type=ipv4v6` | 创建成功 |
| `--connect`（第一次） | `MobileEquipment.Ipv6OnlyAllowed: IPv6 only allowed` |
| `--connect`（另一次） | `MobileEquipment.Unknown: internal error: error` |
| connect 之后 | **`mmcli`: `no modems found`——基带崩溃/复位** |

**关键结论：**
- 纯 ModemManager 路线（我们的改动）已生效，不再是 QMI 抢占问题。
- 但 IMS bearer 一 `--connect`，网络先返回 `Ipv6OnlyAllowed`（这张卡/网络的 IMS 要求 IPv6-only），**紧接着基带就崩溃复位**——正是 MSM8916 固件激活 IMS PDP context 时的老崩溃（`dhcp_client_mgr.c`）。
- 默认数据 CID1 双栈完全正常，唯独 IMS APN 的 bearer 激活会崩基带 → 定位在**固件对 IMS PDP context 的处理**，而非主机软件。
- **待逆向 beta2 确认**：beta2 在这台设备上能收短信，若纯 MM 主路线也会崩基带，则 beta2 成功很可能靠其 `secondary_qmi.rs` / DATA6 独立端点（自编译内核模块）隔离路线。逆向结果见下方"六、beta2 VoLTE 实现逆向"。

---

### 已确认的症状（2026-07-20 实测）

| 路径 | 结果 |
|---|---|
| AT `$QCPDPIMSCFGE=2,1,1,1` + `CGACT=1,2` | **崩基带** `dhcp_client_mgr.c:263` |
| `mmcli --create-bearer=apn=ims,ip-type=ipv4v6` | 返回 `Ipv6OnlyAllowed` |
| `mmcli --create-bearer=apn=ims,ip-type=ipv6` | 返回 `Unknown: prefix-unavailable` |
| `qmicli --wds-start-network=apn=ims,ip-type=6` | 崩基带 |
| `AT+CGCONTRDP=2` 读 P-CSCF | 返回空（无 P-CSCF） |
| 固件侧 IMS 注册状态 `AT+CIREG?` | 返回 Unknown error |

### 怀疑原因（按可能性排序）

1. **号码未在电信侧开通 VoLTE**（最可能）
   - 同款设备其他人能成功，说明代码没问题，是订阅侧差异
   - 验证方法：把本卡插入普通 VoLTE 手机 → 能否接通 HD 高清通话
   - 解决：拨打电信 1000，要求开通 VoLTE 服务（免费）

2. **FDN（固定拨号）拦截 IMS APN 激活**
   - 实测卡内 `enabled locks: fixed-dialing`，`sim-pin2: enabled-not-verified`（3次机会）
   - 不确定 FDN 是否拦 IMS PDP，取决于基带实现
   - 验证/解决：凭机主身份联系电信关闭 FDN 或取得 PIN2，然后 `AT+CLCK="FD",0,"<pin2>"`

3. **固件 DHCP 客户端 bug**（最难绕过）
   - 激活 IMS PDP context 时固件内部 DHCP 客户端崩溃
   - 所有激活路径（mmcli/qmicli/AT直接）都触发同一崩溃点
   - 如果 1、2 解决后仍崩，则是固件本身的问题，需考虑固件升级或改用纯固件 native VoLTE（`ATD`/`ATA` + PCM 音频，不走主机软 IMS 栈）

### 下一步验证顺序（不需改代码）

1. **立刻可做（无需 PIN2）：**
   - 把本卡插入普通支持 VoLTE 的手机，拨打测试
   - 或拨打电信 1000 确认号码 VoLTE 开通状态

2. **有了新 SIM 卡后：**
   - 换入已知开通 VoLTE 的电信卡（或联通/移动卡）测试
   - `mmcli --create-bearer=apn=ims,ip-type=ipv4v6` 是否成功建立 bearer 并拿到 P-CSCF
   - 如果成功，之前的代码改进（AT 探测非致命、IP 家族计划）就能完整验证

3. **连接成功后，验证代码层面的改进：**
   - 观察 Web UI 连接尝试历史：各字段（at_cid/qmi_device/bearer_path/interface/pcscf）是否正确填充
   - 观察 `ip_family_preference` 配置是否生效（v6first 与 v4first 切换后 bearer 尝试顺序是否变化）
   - 确认 AT 探测失败时能跳过、直接靠 bearer PCO 拿 P-CSCF（`"continuing_with_bearer_pco"` 日志）

---

## 四、快速参考：改动文件清单

```
backend/src/
  access/volte/
    mod.rs               ← 注册 plan 模块
    plan.rs              ← 新增：IP 家族连接计划（P2）
    bearer.rs            ← P2：接受 plan，fallback 按 preference
    data_path.rs         ← 修复：bam-dmux 无 MUX 端点时跳过预热
    errors.rs            ← 新增：RUNTIME_IMS_FAMILY_UNSUPPORTED
    identity.rs          ← 待扩展（P3 ISIM）
    isim.rs              ← 待新增（P3 ISIM EF 读取）
    live.rs              ← Goal A 闸删除 + P1 字段清零 + P2 plan 线程
    pcscf.rs             ← P2：接受 plan，删除 ordered_pdp_types
    runtime.rs           ← P1：attempt 结构化字段 + snapshot 新字段
  cellular/
    modem_manager.rs     ← Goal B：set_data_connection_inner isolated=true；
                           init_data_connection 删除；watchdog 不再全局拨号；
                           P5-c 待：射频/运营商 per-modem 孪生函数
    cell_lock_store.rs   ← P5-b 待：改为 per-line
  api/
    handlers.rs          ← Goal A：删 connection_enabled=true；
                           Goal B：已隐式（只改 modem_manager）；
                           P5-a 待：遗留全局 VoLTE handler 清理；
                           P5-b/c 待：cell_lock/射频 handler 加 line_id
  main.rs                ← Goal B：删除全局自动拨号块

frontend/src/
  api/contracts.ts       ← P1：VolteConnectionAttempt 新字段
  pages/sim/
    LineDetailsDialog.tsx ← P1：attempt 行显示结构化元信息
    ModemLinesPanel.tsx   ← P5-b/c 待：cell_lock/射频 控件加 line_id
```

---

## 五、参考资料

- `SimAdmin-VoLTE/VoLTE_1.6_vs_1.7_功能对比与迁移指南.md`
- `SimAdmin-VoLTE/VOLTE_深度逆向_含源码对比.md`
- `SimAdmin/VOLTE_1.7对照改进记录.md`（上一轮改进总结）
- Memory：`simadmin-vs-ltemanager-volte-architecture`（设备10.0.0.116 实测根因）
- Memory：`simadmin-1.6-vs-1.7-volte-diff`（两版架构差异）
