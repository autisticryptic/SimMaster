# VoLTE 短信模块逆向重建 —— 实施记录

> 本文档记录将 `SimAdmin-VoLTE`（v1.1.6-dev18 编译成品）中的 VoLTE(IMS over LTE) 短信能力，
> 通过 clean-room 方式重建到 `SimAdmin`（1.1.3）项目的完整过程与产物。
>
> **落地项目**：`SimAdmin`（1.1.3，含 vowifi/voice）
> **新增模块**：`backend/src/volte/`
> **依赖复用**：`vowifi::qmi_uim`（SIM AKA 运算）、`vowifi::sms`（3GPP SMS 编解码）
> **验证状态**：全量编译通过 + 357 测试全绿（含 62 新增 volte 单测）+ volte 模块 clippy 零告警

---

## 一、信息来源与合规声明

本次重建基于三类**合法信息源**，不含对第三方二进制的反汇编抄写：

1. **公开 3GPP / IETF 规范**：TS 24.229(IMS SIP)、TS 24.341(SMS over IP)、TS 24.011(RP/CP)、
   TS 23.040(TPDU)、TS 23.003(标识格式)、TS 33.203(IMS 接入安全)、RFC 3261(SIP)、
   RFC 3310(Digest AKAv1)、RFC 4169(AKAv2)、RFC 2617/2104(Digest/HMAC)。
2. **二进制暴露的可观测事实**：未 strip 的 `src/volte.rs:行号` panic location、明文字符串
   （SIP 头、AT 命令、`ip xfrm` 参数、错误码枚举）、前端 JS 的 stage 状态机与 API 路径。
   这些属于**接口与行为观测**，非源码。
3. **上游 `SimAdmin` 自身的干净室实现**：可复用的 `vowifi/qmi_uim.rs`、`vowifi/sms.rs`。

所有类型/函数名基于 3GPP 术语和 SimAdmin 既有风格自拟，不照搬二进制符号。报文格式依据公开
RFC/3GPP 规范实现，不逐字节复制观测报文。观测到的错误码字符串仅作行为对齐的参考清单。

---

## 二、逆向证据采集

### 2.1 采集方法

| 来源 | 方法 | 产出 |
|------|------|------|
| ELF 二进制 (`simadmin`, 8.3MB, aarch64) | MinGW `strings` 提取 | ~9000 行字符串 |
| 前端 `volteStatus-CbUvYYVW.js` | 直接读取 + 解析 | stage/phase/字段/错误映射完整契约 |
| `src/volte.rs:行号` 锚点 | 聚类还原 | 92 个锚点（761→5646，推断单文件约 5600 行） |

### 2.2 前端契约（硬约束，逐字保留）

- **stage（12 值）**：`disabled / starting / identity / identity_aka / radio / pcscf / modem /
  bearer / register_ipsec / register_udp / registered / stopping`
- **phase（5 值）**：`disabled / starting / registered / degraded / stopping`
- **registration_mode**：`ipsec / udp / ""`
- **control 响应字段**：`phase, stage, registration_mode, pcscf, session_started_at,
  registered_at, last_rx_at, last_tx_at, last_error, last_failure_at, next_retry_at,
  sent_count, received_count, duplicate_count, reconnect_count, data_path_mode`
- **前端 `enabled` 语义**：`feature_enabled && sms_enabled`
- **5 步进度条**：switch → usim → bearer → register → sms

### 2.3 关键技术锚点（二进制字符串证据）

- **SIP 头**：`P-Access-Network-Info: 3GPP-E-UTRAN-FDD`、`Accept-Contact: *;+g.3gpp.smsip`、
  `P-Preferred-Service: urn:urn-7:3gpp-service.ims.icsi.sms`、`User-Agent: SimAdmin VoLTE`、
  `Content-Type: application/vnd.3gpp.sms`、Contact 的 `+g.3gpp.accesstype="3GPP-E-UTRAN-FDD";
  +g.3gpp.smsip;expires=3600`、`Supported: path, gruu, sec-agree`、`Require/Proxy-Require: sec-agree`
- **IPsec (`ip xfrm`)**：`proto esp spi auth-trunc hmac(md5) enc`、`alg=hmac-md5-96;ealg=null`、
  `spi-c/spi-s/port-c/port-s`、`mode transport`、`Native VoLTE IPsec xfrm installed`
- **鉴权**：`AKAv1-MD5`、`AKAv2-MD5`、`http-digest-akav2-password`、`AKA returned AUTS, requesting resync`
- **IMS bearer**：`--wds-start-network=apn=ims,3gpp-profile=`、`SIMADMIN_MM_IMS_BEARER`、
  `/org/freedesktop/ModemManager1/Bearer/`
- **身份**：`AT+CIMI`、`--uim-get-card-status`、`ims.mnc<MNC>.mcc<MCC>.3gppnetwork.org`
- **协同**：`SMS listener paused while VoLTE IMS SMS path is registered`
- **降级**：`IPsec registration failed, falling back to plain UDP SIP`
- **配置**：`struct VolteConfig { feature_enabled, sms_enabled }`

---

## 三、模块结构与产物

新增 `backend/src/volte/`（与 `vowifi/` 平级、独立）：

```
backend/src/volte/
├── mod.rs          // 模块声明 + clean-room 声明 + 再导出
├── errors.rs       // VolteError + 错误码族（对齐前端 last_error 子串契约）
├── runtime.rs      // stage/phase 状态机 + 快照 + VolteRuntime 句柄
├── sip.rs          // REGISTER/MESSAGE/RP-ACK 构造 + 响应解析 + TCP 粘包切帧
├── digest_aka.rs   // AKAv1/v2-MD5 + HMAC-MD5 + nonce 解码 + Digest 挑战解析
├── identity.rs     // IMPI/IMPU 推导(TS 23.003) + USIM AID + AKA 运算封装
├── ipsec.rs        // ip xfrm SA/策略命令拼装 + SPI/端口生成
├── bearer.rs       // ModemManager IMS bearer 管理 + APN/路径解析
├── pcscf.rs        // P-CSCF 发现 + IPv6/IPv4 设置解析
└── sms.rs          // MT 多段拼接/去重/RP-ACK + MO 提交构造
```

### 3.1 各文件职责

| 文件 | 职责 | 关键实现点 |
|------|------|-----------|
| `errors.rs` | 统一错误类型 + 错误码族 | `Display` 渲染为 `code` 或 `code:detail`；保留前端匹配的子串（`volte_imsi_missing` 等） |
| `runtime.rs` | 运行态状态机 | 12 stage / 5 phase / registration_mode 字符串**逐字对齐** `volteStatus.js`；`VolteRuntime` 三字段句柄（snapshot + advance_lock + generation），`reset_runtime` 用 generation 计数作取消令牌 |
| `sip.rs` | SIP 报文层 | `build_register`/`build_sms_message`/`build_rp_ack`；`parse_status`/`complete_frame_len`(Content-Length 粘包)/`header_values`/`sip_header_uri`；VoLTE 用 `SIP/2.0/UDP` 或 `TCP`，PANI=`3GPP-E-UTRAN-FDD` |
| `digest_aka.rs` | IMS 鉴权 | `aka_digest_password`(AKAv1=RES / AKAv2=base64(HMAC-MD5(RES‖IK‖CK)))、`compute_aka_response`(HA1/HA2/qop)、`decode_aka_nonce`(hex/base64→RAND‖AUTN)、`parse_digest_challenge`、`build_authorization_header`、resync 头 |
| `identity.rs` | 身份推导 | `home_domain`(MNC 补 3 位)、`derive_identity`(IMPI/IMPU)、`resolve_usim_aid`(校验+兜底)、`run_usim_aka`(复用 `vowifi::qmi_uim`) |
| `ipsec.rs` | 内核 IPsec | `build_install_plan`(双向 SA + 策略)、`hmac-md5-96`/`ealg=null`/`mode transport`、SPI/端口随机生成；命令拼装可测，实际 `ip xfrm` 执行隔离在 `#[cfg(unix)]` |
| `bearer.rs` | IMS 承载 | APN 配置、bearer 路径解析、漫游策略；device IO 隔离 |
| `pcscf.rs` | P-CSCF 发现 | IPv6/IPv4 地址/网关/DNS 解析；P-CSCF 地址族匹配校验 |
| `sms.rs` | 短信编排 | `MtReassembler`(多段乱序拼接)、去重 marker(`volte-mt:` / transport=`volte_ims`)、`build_rp_ack_body`、`build_mo_submission`(复用 `vowifi::sms`) |

### 3.2 主程序接线

| 文件 | 改动 |
|------|------|
| `config.rs` | `VolteConfig`(feature_enabled/sms_enabled/connection_enabled/auto-restore 三元组) + `AppConfig.volte` + `get_volte_config`/`set_volte_feature_enabled`/`set_volte_connection_enabled`(门禁联动) |
| `state.rs` | `use volte::runtime::VolteRuntime` + `volte_runtime`/`volte_connect_lock` 字段 + `AppState::new` 入参 + `FromRef` 实现 |
| `handlers.rs` | `VolteControlToggleRequest`/`VolteControlResponse` + `get_volte_control_handler`/`set_volte_feature_handler` |
| `main.rs` | `mod volte;` + 构造 `VolteRuntime` + 传入 `AppState::new` + 注册 `/api/volte/control` `/api/volte/feature` 路由 |
| `sms_listener.rs` | `modem_sms_paused_for_ims`：IMS(VoWiFi 或 VoLTE) 活跃时暂停 CS 监听，避免重复收信 |

### 3.3 对外 API

```
GET  /api/volte/control    # 查 VoLTE 运行状态（config + 运行快照）
POST /api/volte/feature    # 开关 VoLTE（body: {enabled: bool}）
```

---

## 四、复用策略

VoLTE 与 VoWiFi 在 SIM/短信底层能力上共享，直接复用（传输无关）：

| 复用符号 | 来源 | 用途 |
|---------|------|------|
| `execute_usim_authenticate_via_proxy_reason_with_retry` → `UsimAkaApduResult{res,ck,ik,auts}` | `vowifi/qmi_uim.rs` | SIM AKA 运算（RAND/AUTN→RES/CK/IK/AUTS） |
| `USIM_AID_PREFIX` | `vowifi/qmi_uim.rs` | USIM AID 校验/兜底 |
| `build_single_part_mo_submission` → `MoSmsSubmission` | `vowifi/sms.rs` | MO 单段 SMS-SUBMIT 编码 |
| `parse_mt_rp_data` → `MtSmsDeliver` | `vowifi/sms.rs` | MT RP-DATA 解码 |
| `build_network_rp_ack` / `classify_rp_ack` | `vowifi/sms.rs` | RP-ACK 构造/分类 |
| `db::insert_sms_at_with_transport` / `sms_id_by_pdu` | `db.rs` | 短信落库（transport=`volte_ims`）+ 去重 |

---

## 五、分阶段实施与验证

每阶段独立编译 + 单元测试，逐阶段验证。

| 阶段 | 内容 | 累计 volte 单测 |
|------|------|:---:|
| 1 | 模块骨架 + config + handlers + 路由 + state 接线 | 8 |
| 2 | `sip.rs` 报文构造/解析/粘包 | 18 |
| 3 | `digest_aka.rs` + `identity.rs` | 38 |
| 4 | `ipsec.rs` + `bearer.rs` + `pcscf.rs` | 55 |
| 5 | `sms.rs` MT 收短信（拼接/去重/RP-ACK） | 62 |
| 6 | sms_listener 协同 + 全量收尾 | 62 |

### 5.1 最终验证结果

- **全量编译通过**（`stable-x86_64-pc-windows-gnu`，rustc 1.97.0）
- **357 个测试全绿**（62 新增 volte + 295 既有），无回归
- **volte 模块 clippy 零告警**

### 5.2 黄金校验点

以下单测提供了字节级正确性铁证：

- **RFC 2104 HMAC-MD5 测试向量**：`key=0x0b*16, data="Hi There"` →
  `9294727a3638bb1c13f48ef8158bfc9d`；`key="Jefe"` → `750c783e6ab0b503eaa86e310a5db738`
- **RFC 2617 Digest 测试向量**：`Mufasa/testrealm@host.com/Circle Of Life` →
  `670fd8c2df070c60b045671b8b24ff02`（无 qop）/ `6629fae49393a05397450978507c4ef1`（qop=auth）
- **SIP 粘包切帧**：两条消息合并在一次 TCP 读取中，按 Content-Length 精确切分
- **多段乱序拼接**：段 2 先到、段 1 后到，仍按序拼成 `Hello World`

---

## 六、可验证性边界（诚实声明）

### 6.1 离线已完成并验证

报文构造/解析、Digest-AKA 计算（对照规范向量）、TPDU/RP-DATA 编解码（复用）、`ip xfrm`
命令拼装、多段拼接/去重逻辑、状态机字符串契约、配置联动。**这些纯逻辑均已单测覆盖，
Windows CI 可全量验证。**

### 6.2 需真机验证（尚未完成）

1. **运行时驱动状态机未接线**：各模块的纯逻辑已就位并单测，但把它们串成
   `identity→bearer→pcscf→ipsec→register→listen` 的完整异步驱动循环（对应 `volte.rs` 的
   runtime 主循环）尚未实现——它几乎全是真机 IO，离线无法验证，容易凭空猜错定时器/重试细节。
2. **真机字节级细节**：真实运营商 nonce 编码怪癖、xfrm SA 与 socket 端口绑定、特定 P-CSCF 的
   头顺序敏感性、IPv6 依赖，必须真机抓包（SIP/RTP）比对迭代。

### 6.3 真机验证步骤建议

1. 部署到目标 aarch64 设备。
2. 开启 VoLTE feature，观察 `/api/volte/control` 的 stage 推进。
3. 用另一手机发短信到该 SIM，看 MT 是否落库 + UI 显示。
4. 从 UI 发短信，看 MO 是否送达。
5. 失败时抓 `journalctl` 日志 + `tcpdump` P-CSCF 交互，反馈迭代。

---

## 七、后续工作（下一步）

- **阶段 7（未做）**：运行时驱动状态机（纯 IO 编排）——把 identity/bearer/pcscf/ipsec/
  register/sms 各组件按 stage 顺序串成异步驱动循环，含 IPsec 优先/UDP 降级、REGISTER 刷新、
  MT 监听、自动重连。
- **与 vowifi 共享层抽离**：见主扩展文档《SimAdmin_扩展开发文档_多路径语音短信Trunk.md》
  的阶段 A（抽取共享 `ims/` 核心），VoLTE 与 VoWiFi 的 SIP/AKA 逻辑可进一步收敛为同一份。

---

*本记录基于公开规范与合法行为观测编写，遵循上游 clean-room 原则，供 GPLv3 衍生开发之用。*
