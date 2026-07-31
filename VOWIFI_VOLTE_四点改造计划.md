# VoWiFi / VoLTE 四点改造计划

> 目标目录：`SimAdmin`（HEAD=`d5e0dbd`，工作区干净，可编译）
> 编写日期：2026-07-29

本文件是四项修改的完整落地方案，供评审。写码前请确认本计划。

---

## 背景约定（本轮四点已确认的口径）

- **改动目录**：`SimAdmin`。
- **WiFi Calling「指定运营商 profile」下拉取值**：只列**数据库**里的 profile + 一个空位。
  - 空位 = 按 IMSI 自动计算连接域名（现有行为）。
  - 选中某条 = 强制用该 profile 连接。
- **运行时解析优先级**（这条线路要用哪个运营商 profile）：
  1. 该线路**显式钉选**的数据库 profile（`profile_id`）→
  2. 按 IMSI 从**数据库**匹配 →
  3. 按 IMSI **动态计算**（3GPP 标准派生）。
- **数据库何时新增 profile**：
  - SIM 通过**动态生成的 profile 成功连上 VoWiFi 后**自动落库；
  - 或用户在「运营商 Profile」页手动写入。
  - （「从 iOS/Android 基带提取 profile」为**待定**，单列 `docs/待定_基带Profile提取调研.md`。）
- **第四点（数据库字段多、运行时用得少）**：先给清单，再直接补全高价值字段的运行时接线。

---

## 现状事实（调研确认）

### VoLTE 地址族
- `VolteConfig.ip_family_preference: VolteIpFamilyPreference`（枚举：`ipv4_first`/`ipv6_first`/`ipv4_only`/`ipv6_only`），位于 `backend/src/platform/config.rs:2550/2586`。
- **是全局配置**（`GlobalConfig.volte`），**目前没有任何 API/网页可以编辑它**——只有 feature/connection/voice 三个开关。
- 消费唯一漏斗：`ImsConnectionPlan::from_preference()`（`volte/plan.rs:195`）。它把偏好翻译成：
  - `bearer_attempts`（总是 dual-stack 优先，再单族回退）
  - `pcscf_order`（单族探测/SIP 本地地址顺序）
- 连接入口 `connect_live_for_line(config: &VolteConfig, ...)`（`volte/live.rs:491`）取 `config.ip_family_preference`。**两个调用点**（`handlers.rs:3403`、`handlers.rs:6664`）都同时持有全局 `config = get_volte_config()` 和**每线路** `profile = get_line_profile(line_id)`。

> 结论：把族偏好做成 per-line 是干净的——调用点已有 `profile`，只需新增 per-line 字段并在 `connect_live_for_line` 传参处优先取它。

### WiFi Calling per-line 配置
- `LineVowifiConfig`（`config.rs:2814`）：`enabled` / `proxy_mode` / `proxy_endpoint` / `dns_server` / `epdg_host` / `epdg_port`。
- `LiveNetworkOverrides`（`live.rs:124`）：`epdg_host` / `epdg_port` / `dns_server` / `proxy`。
- ePDG override 消费点：`live_epdg_settings()`（`live.rs:208`）、`resolve_live_epdg()`（`live.rs:249`）——override 优先，否则用 profile 的。
- 运营商 profile 解析：`resolve_by_imsi()`（`profiles.rs:922`）优先级 = 数据库 → 内置 → 动态派生。**当前没有「按 line 钉选 profile」的能力**。
- 落库接口：`ProfileStore::save(record, source)`（`profile_store.rs:173`）会校验 + 写库 + republish overlay。

### 前端
- WiFi Calling 对话框：`frontend/src/pages/sim/VowifiLineDialog.tsx`（有自定义 ePDG profile 下拉、自定义 ePDG 主机、ePDG IKE 端口、专用 DNS、代理模式、代理端点）。
- 契约：`frontend/src/api/contracts.ts:814` `LineVowifiConfig`。
- 运营商 profile 列表 API：`api.listVowifiCarrierProfiles()`（`current.ts:1048`）。
- **前端目前没有 VoLTE 族偏好的编辑器**（只有只读展示 `current_ip_family`）。

---

## 第四点：数据库 profile 字段 vs 运行时实际消费（清单）

数据库记录 `CarrierProfileRecord` 存 8 大类。逐字段扫过整个 vowifi 运行时（排除展示层 diagnostics、存储转换层 profile_record）：

### 运行时真正消费（连接行为受影响）
- **epdg**：`host` `port` `ip_stack` `apn` `dns_servers`
- **ikev2**：`ike_proposals` `esp_proposals` `nat_keepalive_seconds` `dpd_interval_seconds` `reauth_interval_seconds` `aka_challenge_mode` `include_epdg_idr`
- **ims**：`domain` `realm` `registrar` `pcscf` `transport` `local_port` `user_agent` `identity_source`
- **ims.register**：`supported_header` `expires_seconds` `use_plain_digest_placeholder` `strict_security_server_offer` `sec_agree_mode` `require_sec_agree_headers` `access_network_info` `include_pani_authenticated` `security_client_mechanisms` `live_header_variant_set` `enable_initial_reject_fallback`
- **sms**：`receiver_transport`
- **voice**：`vowifi_enabled` `carrier_fallback_enabled` `preferred_codecs` `amr_octet_align` `ptime_ms`
- **identity**：`device_model_hint`

### 存了但运行时**完全没消费**（死字段）
- **ims.register**：`contact_mode` `contact_param_order` `temporary_status_codes` `forbidden_status_codes` `initial_reject_fallback_status_codes` `temporary_retry_seconds`
- **ims**：`tcp_keepalive_seconds` `options_ping_interval_seconds`（有默认常量，未接线）
- **sms**：`smsc_auth_required`
- **voice**：`sip_endpoint_exposed`
- **identity**：`spoof_imei` `device_identity_enabled` `device_identity_imei`
- **e911**：整块（`enabled` `provider` `entitlement_url` `websheet_host_policy`）——只有校验，无运行时行为

### 本轮建议补全接线的「高价值」字段（按对真机连通性的影响排序）
1. `ims.register.contact_mode` + `contact_param_order` —— 决定 REGISTER 的 Contact 头形态，部分运营商严格校验。
2. `ims.register.temporary_status_codes` / `forbidden_status_codes` / `temporary_retry_seconds` —— 注册被拒后「重试还是放弃 / 多久重试」，现在硬编码常量，profile 改了不生效。
3. `ims.register.initial_reject_fallback_status_codes` —— 配合已消费的 `enable_initial_reject_fallback`，触发码目前写死。
4. `ims.tcp_keepalive_seconds` / `options_ping_interval_seconds` —— 注册保活，影响 NAT 掉线。

> `e911.*` / `spoof_imei` / `device_identity_*` / `sip_endpoint_exposed` / `smsc_auth_required` 依赖尚未实现的大功能或行为面很小，本轮**只列清单不接线**。

---

## 实施方案

### ① VoLTE 地址族：单选枚举 → 每线路「可勾选 + 有序」

**后端**
- 新增每线路字段（`LineProfileConfig`，`config.rs`）：
  ```rust
  /// 有序的 IMS 地址族尝试顺序；`None` 继承全局默认（`VolteConfig.ip_family_preference`）。
  /// 元素取值 ipv4 / ipv6，顺序即单族回退顺序；只含一个元素等价「仅该族」。
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub volte_ip_families: Option<Vec<VolteIpFamily>>,
  ```
  其中新增轻量枚举 `VolteIpFamily { Ipv4, Ipv6 }`（snake_case 序列化）。
- `plan.rs` 新增 `ImsConnectionPlan::from_families(&[VolteIpFamily])`，与现有 `from_preference` 并存：
  - `[v4, v6]` → dual-stack, ipv4, ipv6（== `Ipv4First`）
  - `[v6, v4]` → dual-stack, ipv6, ipv4（== `Ipv6First`）
  - `[v4]` → 仅 ipv4（== `Ipv4Only`）
  - `[v6]` → 仅 ipv6（== `Ipv6Only`）
  - 空 → 回退默认 `[v4, v6]`
  （即有序列表是现有 4 枚举值的严格超集，语义不变。）
- `connect_live_for_line` 传参处（`live.rs:532` 附近，两个 handler 调用点已同时持有 `profile`）：
  优先用 `profile.volte_ip_families` 构造 plan，`None` 时回退 `config.ip_family_preference`。
- 保留 `VolteIpFamilyPreference` 与 `from_preference` 不删，旧全局配置继续可反序列化（兼容）。

**新增 API**
- `PUT /api/volte/lines/{line_id}/ip-families`（写 `Option<Vec<VolteIpFamily>>`），返回该线路最新 VoLTE 控制响应。
- `VolteLineControlResponse.profile` 已含 `LineProfileConfig`，前端读现值即可。

**前端**
- 在 per-line VoLTE 卡片（`LineDetailsDialog` 的 `volte` tab / 或 `ModemLinesPanel` 的 VoLTE 区）新增编辑器：
  - 两个可勾选项（IPv4 / IPv6）；
  - 勾选后可上下调序（决定回退顺序）；
  - 一个「跟随默认」态（对应 `None`）。
- `contracts.ts` 加 `volte_ip_families?: ('ipv4'|'ipv6')[] | null`；`current.ts` 加 `setVolteLineIpFamilies()`。

### ② WiFi Calling：删自定义 ePDG 主机 + IKE 端口

**后端**
- `LineVowifiConfig` 删 `epdg_host`、`epdg_port`（含 `Default`、`default_vowifi_epdg_port` 若无其它引用则一并清理）。
- `LiveNetworkOverrides` 删 `epdg_host`、`epdg_port`。
- `build_live_network_overrides`、`live_epdg_settings`、`resolve_live_epdg` 去掉 override 分支，ePDG 主机/端口直接取 profile。
- `validate_line_vowifi_config`（`config.rs:3080`）删对应校验。

**前端**（`VowifiLineDialog.tsx`）
- 删「自定义 ePDG 主机」「ePDG IKE 端口」两个输入框与相关校验。
- 保留：ePDG profile 选择器（改造见 ③）、专用 DNS、代理模式、代理端点。
- `contracts.ts` 的 `LineVowifiConfig` 删 `epdg_host`/`epdg_port`。
- 「保存为自定义 profile」按钮逻辑（现在依赖 `epdg_host`）改为基于所选 profile / 匹配结果（见 ③）。

### ③ WiFi Calling：新增「指定运营商 profile」

**后端**
- `LineVowifiConfig` 加：
  ```rust
  /// 把这条线路钉到指定数据库 profile。None = 按 IMSI 自动。
  /// 钉选的 id 若已不存在则回落到自动匹配，永不因删 profile 而卡死线路。
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub profile_id: Option<String>,
  ```
- 新增解析函数（`profiles.rs` 或 `live.rs`）：
  ```rust
  pub fn resolve_profile_for_line(line_id, imsi) -> Option<CarrierMatch> {
      // 1. 线路钉选（仅认数据库 profile；失效则继续）
      // 2. resolve_by_imsi(imsi)   ← 内含 数据库→内置→动态 的既有优先级
  }
  ```
  钉选只接受**数据库**来源的 profile（用 `ProfileStore` 查，命中才用；否则视为未钉选并回落）。
- 运行时匹配点（`live.rs:2098`、`live.rs:2662` 等 `resolve_by_imsi(identity.imsi)`）改调 `resolve_profile_for_line(line_id, imsi)`。
- `line_pinned_profile_id(line_id)` 辅助（从 per-line 网络 override 或直接从 config 读 `profile_id`）。

**前端**（`VowifiLineDialog.tsx`）
- 现有「自定义 ePDG profile」下拉改为「指定运营商 profile」：
  - 数据源换成 `api.listVowifiCarrierProfiles()`（数据库 profile）；
  - 顶部保留空位「（自动：按 SIM 卡 IMSI 匹配）」= `profile_id = null`；
  - 选中写 `draft.profile_id`。
- `contracts.ts` 加 `profile_id?: string | null`。

### ④ 高价值字段接线 —— 本轮不实现，留清单（2026-07-29 定）

**写码前对 `live.rs` 全量核查后修正结论**：这几组字段在运行时**零引用**——不是「有消费者但没连上」，而是「消费它们的功能本身尚不存在」。补全它们等于**从零建三个新功能**，且都有改坏当前已能连通真机的 REGISTER 流程的风险，因此本轮不做，保持成清单（数据库存字段、网页可编辑、运行时暂不消费）。

1. **`contact_mode` + `contact_param_order`** —— Contact 头现由 `build_contact_header` 固定结构生成，且已有一套成熟的**头部变体探测系统**（`LiveRegisterHeaderVariant` / `live_register_header_variants`，会轮询多种 Contact 形态找运营商接受的那个），profile 的这两个字段完全没参与。接线需重构 `build_contact_header` 并与变体系统协调，可能打乱现有可用的 REGISTER 探测。
2. **注册状态码策略**（`temporary_status_codes`/`forbidden_status_codes`/`initial_reject_fallback_status_codes`/`temporary_retry_seconds`）—— live 注册路径（`run_live_ims_register_until`）现为硬编码 `200 / 401|407 / 其它` 三分支，**没有「稍后重试」循环**。接线需在 live vowifi 注册路径新建一套状态码驱动的重试引擎。
3. **`tcp_keepalive_seconds` / `options_ping_interval_seconds`** —— live 路径**没有 SIP 保活 / OPTIONS ping 循环**，注册只校验一次。接线需新建每线路一个后台保活任务（定期 OPTIONS + 按需重注册）。

**后续触发条件**：等真机测过 ①②③、出现「某运营商卡在 Contact 形态 / 保活掉线 / 状态码重试」的具体现象后，再针对性建对应的那**一个**功能（有真机佐证，不盲猜）。

---

## 落地顺序与验证

1. **②+③ 一起做**（互相耦合：删自定义 ePDG 的同时上「指定 profile」）——后端结构 + 解析 + 前端对话框。✅ 已完成并验证。
2. **①** VoLTE 族 per-line —— 配置结构 + plan + 传参 + 新 API + 前端编辑器。✅ 已完成并验证。
3. **④** —— 见上，本轮不实现，留清单。

**每步验证**
- `cargo test`（新增：族列表→plan 映射、钉选优先级、override 删除后回退 profile、各补全字段的行为）。
- `cargo zigbuild --release --target aarch64-unknown-linux-musl`（交叉编译过）。
- 前端 `pnpm lint` + `vite build`。

**兼容性**
- 旧 config：`epdg_host`/`epdg_port` 反序列化时被忽略（serde 未知字段默认丢弃）；`profile_id`/`volte_ip_families` 缺省即 `None`，行为与现状一致。
- 旧全局 `ip_family_preference` 保留，per-line 为 `None` 时沿用。

---

## 待定（不在本轮实现）
- 从 iOS/Android 基带提取运营商 profile → 见 `docs/待定_基带Profile提取调研.md`。
- e911、IMEI 伪装、外呼 SIP 端点、smsc_auth 等死字段的运行时接线。
