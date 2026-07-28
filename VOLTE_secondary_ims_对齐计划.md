# VoLTE 修复计划：IMS 承载改走 DATA6 单发 + AT+CGCONTRDP 取 P-CSCF（对齐 beta2）

## 根因（已由 IDA + 你的实测日志双向确认）

当前代码把 IMS WDS 承载跑在**主口 `/dev/wwan0qmi0` + qmi-proxy**，目的是复用 CID
以便 `--wds-start-network → --wds-get-current-settings` 读回 P-CSCF。这个前提是错的。

beta2 二进制里三条决定性字符串：

1. `Native VoLTE secondary QMI IMS WDS bearer started`（`volte.rs:1976`）
   —— IMS 承载跑在**DATA6 辅助端点**，单发一条 start-network。
2. `--wds-get-current-settings` **只出现在 `secondary_qmi_data.rs`**（纯数据路径），
   IMS 路径根本不调它。
3. IMS 路径的 P-CSCF/IP 来自 **`AT+CGCONTRDP`**：
   `Native VoLTE P-CSCF candidates discovered from active IMS bearer`
   （`volte.rs:3671/3678`），配套错误码 `volte_cgcontrdp_ipv4_missing` /
   `volte_cgcontrdp_ipv6_missing` / `volte_cgcontrdp_gateway_missing`
   （`volte.rs:3779-3838`）—— 说明 beta2 从 CGCONTRDP 里同时取**地址、网关、DNS、P-CSCF**。

你的实测日志印证同一分工在本设备成立：
- 辅助端点单发 start-network **成功**：
  `Secondary DATA QMI bearer activated device=/dev/wwan0qmi1 interface=wwan0 family=4`
- 主口 IMS 尝试**失败**（两个 family 都失败）：
  `qmi_wds_start_failed:verbose call end reason (2,201): [internal] error`
  —— `(2,201)` 是 QMI internal call-end reason，不是家族协商，且 `pcscf_count=0`。

即：主口已被 ModemManager 自己的 bearer/CID 占用，再在主口起 IMS 会话冲突；
而 DATA6 单发是通的。这推翻了旧“breakthrough”记忆里“IMS 必须走主口”的结论
——那条结论仅因假设“需要多步 WDS 流程读 P-CSCF”而成立，而 beta2 从 AT 取 P-CSCF，
根本不需要多步流程。

## 目标

按 beta2 完整对齐（用户已拍板 “Secondary + AT P-CSCF”，且**设为默认**，移除主口专用要求）：
IMS 承载在 DATA6 辅助端点单发 start-network，P-CSCF/IP 从 `AT+CGCONTRDP` 读取。

## 现状可复用的组件

- `cellular/secondary_qmi.rs::start_ims_session`（line 750）—— **已存在**辅助端点单发
  start-network 实现，含 `parse_packet_data_handle` / `parse_call_end_reason`。
  目前只被数据路径用，IMS 路径没用它。
- `cellular/secondary_qmi.rs::ensure_endpoint`（line 529）—— 绑定/探测 DATA6 端点。
- `access/volte/pcscf.rs::parse_cgcontrdp_pcscf`（line 485）—— 已能从 CGCONTRDP 取
  P-CSCF（字段 7/8）。需扩展出地址/网关/DNS 字段（3/4/5/6）。
- `access/volte/pcscf.rs::discover_pcscf_via_active_at_context`（line 348）—— 已是
  “bearer 起来后读 active context CGCONTRDP” 的模式，正是 beta2 的取法。
- `access/volte/native_bearer.rs::to_bearer_connection` —— 把会话映射成
  `BearerConnection` 的契约不变，下游 SIP/IPsec/路由完全不用改。

## 改动方案

### 1. IMS 承载端点：主口 → DATA6 辅助端点（`native_bearer.rs` + `live.rs`）

- `establish_native_ims_bearer` 从 `WdsEndpoint::primary_via_proxy(primary_device)`
  改为解析并 `ensure_endpoint` 出本 baseband 的 DATA6 辅助端点（走
  `secondary_qmi::ensure_endpoint` / `discover_spare_qmi_ports`，按 baseband 配对，
  遵守多卡 per-line 约束——不取第一个 modem）。
- 会话建立改用 `secondary_qmi::start_ims_session`（单发），**不再** allocate 保留 CID、
  **不再** `--wds-get-current-settings` 做 IMS 的 P-CSCF 来源。
- 保留“绝不发任何 `--wds-bind*`”硬约束与 `bind_arguments_are_never_emitted` 守卫。
- wedge 立即中止不回落 MM（保留现有 `FailureClass::BasebandWedged` 逻辑）。
- family 顺序对齐 beta2：先 `ip-type=6` 再 `ip-type=4`（beta2 预烘焙串
  `,3gpp-profile=1,ip-type=6` 在前）。但保留 plan 的 preference 覆盖能力；本设备
  Maxis 卡实测 v6 被拒会自动落 v4，安全。

### 2. `3gpp-profile` 值：从“AT 上下文 cid”改为对齐 beta2

- 日志显示当前 `request.profile_id` 来自保留的 AT 上下文 `cid=2`
  （`live.rs:686 request.profile_id = ims_profile.map(|p| u32::from(p.cid))`）。
- beta2 预烘焙串是 `,3gpp-profile=1`。把 WDS start 的 profile 固定按 beta2 走（1），
  或在 profile 探测拿不到时缺省 1；AT 上下文 cid 只用于 CGCONTRDP 读取，不再直接
  当 WDS profile。（`(2,201) internal` 有可能正是 profile=2 不匹配 WDS 侧 profile 表所致，
  次要嫌疑，一并修。）

### 3. P-CSCF/IP 来源：AT+CGCONTRDP 升为 IMS 主来源（`pcscf.rs` + `live.rs`）

- bearer 起来后，用 `AT+CGCONTRDP=<ims_cid>` 读取，作为 P-CSCF **主来源**
  （对齐 `volte.rs:3671` “discovered from active IMS bearer”）。当前 `live.rs` 把它当
  “PCO 空了才兜底”，需前移。
- 扩展 CGCONTRDP 解析：从字段 3（local addr+subnet）、4（gateway）、5/6（DNS）
  取全套 IP 配置，喂给 `NetdevConfig`/`to_bearer_connection`。新增/复用错误码对齐
  `volte_cgcontrdp_ipv4_missing`/`ipv6_missing`/`gateway_missing`。
- 辅助端点 `read_current_settings`（单发新进程、CID 丢失多半返回空）降级为
  best-effort 补充，不作为唯一来源。

### 4. netdev 解析

- 沿用 `qmi_netdev::resolve`（bam-dmux 起会话后逐 wwanN 探测），本设备日志已能解析到
  `interface=wwan0`。地址/网关改用 CGCONTRDP 的结果配置该 netdev。

### 5. data-slot 语义

- beta2 两种 allocation 都保留。默认 VoLTE-only 线路 = IMS 独占（现在改为 IMS 在 DATA6，
  主口留给 MM/数据）。`data_slot.rs` 的注释与 `ims_on_primary()` 判定需据实改写：
  IMS 不再在主口，`native_ims_bearer_required` 改为“有可用辅助端点即走 native 辅助路径”。

## 验证

- `cargo test --bin simadmin`（bin-only crate，不能 `--lib`）。基线 682 测试。
- 重点更新/新增单测：CGCONTRDP 全字段解析、family 顺序（v6→v4）、profile=1、
  辅助端点被选为 IMS 端点、wedge 仍中止不回落、`--wds-bind*` 守卫仍绿。
- 真机：`SIMADMIN_VOLTE_NATIVE_*` 默认开启后，确认 IMS start-network 走 DATA6 成功、
  CGCONTRDP 拿到 P-CSCF（`pcscf_count>0`）、REGISTER 成功。

## 环境坑（务必遵守，来自既有记忆）

- **Edit 工具会往文件中间注入 UTF-8 BOM** → 大改动优先 `Write` 整文件；改完用
  PowerShell `Get-Content`/`findstr`/hexdump 客观核实，别只信 Read/Grep（对大文件会失真）。
- 主口辅助端点 qmi-proxy 在 `/usr/libexec/qmi-proxy`，不在 PATH。
- 辅助端点必须带 `--device-open-net=net-raw-ip|net-no-qos-header`。
- 绝不发 `--wds-bind-data-port`/`--wds-bind-mux-data-port`（触发 SSR）。
