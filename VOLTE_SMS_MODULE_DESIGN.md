# VoLTE 短信模块 —— 行为级重构详细设计文档

> 目标：在 `SimAdmin` 后端新增一个 `volte` 模块，忠实复现第三方 VoLTE 二次开发版
> （`SimAdmin-VoLTE`，v1.1.6-dev18，commit `05ea96a`，`aarch64-unknown-linux-musl`）
> 中「通过 VoLTE(IMS over LTE) 收发短信」的能力，并使其可提交回 GPLv3 上游。
>
> **本文件是设计蓝图，不含实现代码。** 实现分 6 个阶段推进，每阶段独立编译 + 单元测试。

---

## 0. 方法论与合规声明

### 0.1 信息来源

本设计基于三类**合法信息源**，不含对第三方二进制的反汇编抄写：

1. **公开 3GPP / IETF 规范**：TS 24.229(IMS)、TS 24.341(SMS over IP)、TS 24.011
   (RP/CP 协议)、TS 23.040(TPDU)、RFC 3261(SIP)、RFC 3310(HTTP Digest AKA)、
   RFC 4867(AMR，语音用，本模块不涉及)。
2. **该二进制暴露的可观测事实**：未被 strip 的 panic location（`src/volte.rs:行号`）、
   明文字符串（SIP 头、AT 命令、`ip xfrm` 参数、错误码枚举）、前端 JS 里的 stage
   状态机与 API 路径。这些属于**接口与行为观测**，非源码。
3. **上游 `SimAdmin-main` 自身的干净室实现**：可复用的 `sms.rs`/`qmi_uim.rs`/
   `ims.rs`/`config.rs`。

### 0.2 干净室原则

上游 `vowifi/mod.rs` 明确声明「只用 SimAdmin 自有命名，不基于任何第三方二进制或私有
预设命名」。本模块**严格沿用**该原则：

- 所有类型/函数名基于 3GPP 术语和 SimAdmin 既有风格自拟，不照搬二进制符号。
- 报文格式依据公开 RFC/3GPP 规范实现，不逐字节复制观测到的报文。
- 观测到的错误码字符串（如 `volte_imsi_missing`）**仅作为行为对齐的参考清单**，
  重实现时用语义等价的 SimAdmin 风格命名。

### 0.3 可验证性边界（务必知悉）

| 层次 | 能否离线验证 | 验证方式 |
|------|-------------|---------|
| TPDU/RP-DATA 编解码 | ✅ 能 | 单元测试（复用 sms.rs 已有测试模式） |
| SIP 报文构造/解析 | ✅ 能 | 单元测试（往返 + 固定样例断言） |
| Digest-AKA 摘要计算 | ✅ 能 | 单元测试（RFC 3310 测试向量） |
| `ip xfrm` 命令拼装 | ✅ 能 | 单元测试（断言参数序列） |
| 状态机流转 | ✅ 能 | 单元测试（dry-run 全流程） |
| **真机收到运营商短信** | ❌ 不能 | **仅你在目标设备上真机验证** |
| 特定运营商报文怪癖 / 定时器精确值 | ❌ 不能 | **真机抓包补齐** |

**我能交付「离线正确、能编译、带测试」的实现；真机行为等价性需要你在设备上验证并反馈。**

---

## 1. 目标能力与非目标

### 1.1 目标（本次重构范围）

- VoLTE(IMS over LTE) 路线的**短信收发**：
  - **MT（接收）**：监听 P-CSCF 下发的 SIP `MESSAGE`，解析 3GPP SMS，回 RP-ACK，
    支持长短信拼接、去重、落库。
  - **MO（发送）**：构造 SIP `MESSAGE` 承载 SMS-SUBMIT，处理响应与 RP-ACK。
- IMS 注册：REGISTER → 401/Digest-AKA 挑战 → 鉴权 REGISTER → 200 OK，
  支持 **IPsec 优先、UDP 降级**双模。
- IMS 承载：借 ModemManager 建立 IMS APN bearer，发现 P-CSCF。
- IPsec：用内核 `ip xfrm` 建立 SIP 信令安全关联。
- 配置开关、运行状态、与原 SMS 监听器的协同、DB 持久化。
- API：`GET /api/volte/control`、`POST /api/volte/feature`。

### 1.2 非目标（本次不做）

- VoLTE **语音通话**（只做短信；语音是另一条线）。
- 替换或修改原版 `vowifi/`（VoWiFi/ePDG 路线）——两者并存、互不干扰。
- 非 QMI 调制解调器的适配（沿用上游 qmi-proxy 假设）。

---

## 2. 架构总览

```
                 ┌────────────────────────────────────────────┐
                 │            HTTP API (axum)                  │
                 │  GET /api/volte/control                     │
                 │  POST /api/volte/feature {enabled}          │
                 └───────────────┬────────────────────────────┘
                                 │
                 ┌───────────────▼────────────────────────────┐
                 │        volte::runtime (编排/状态机)          │
                 │  stage: disabled→starting→identity→          │
                 │   identity_aka→radio→pcscf→modem→bearer→      │
                 │   register_ipsec/register_udp→registered      │
                 └───┬───────┬───────┬───────┬───────┬──────────┘
                     │       │       │       │       │
        ┌────────────▼┐ ┌────▼────┐ ┌▼──────┐│ ┌─────▼──────┐
        │ identity     │ │ bearer  │ │ ipsec ││ │ sip (信令)  │
        │ IMSI/USIM AID│ │ (MM/QMI)│ │(xfrm) ││ │ REGISTER/   │
        │ (读卡)        │ │ P-CSCF  │ │       ││ │ MESSAGE     │
        └──────┬───────┘ └─────────┘ └───────┘│ └──────┬──────┘
               │                               │        │
        ┌──────▼───────┐              ┌────────▼───┐ ┌──▼──────────┐
        │ qmi_uim (复用)│              │ digest-aka │ │ sms 编解码   │
        │ AKA 运算      │◄─────────────┤ (RFC3310)  │ │ (复用sms.rs) │
        │ RES/CK/IK     │   CK/IK/RES  └────────────┘ │ TPDU/RP/UDH │
        └──────────────┘                              └─────────────┘
                                 │
                 ┌───────────────▼────────────────────────────┐
                 │  db (SmsMessage, transport="volte_ims")     │
                 │  + 与 sms_listener 协同(注册后暂停MM监听)      │
                 └─────────────────────────────────────────────┘
```

### 2.1 与原版 VoWiFi 的技术路线对比

| 维度 | 原版 `vowifi/`(VoWiFi) | 本模块 `volte/`(VoLTE) |
|------|----------------------|----------------------|
| 接入网 | WiFi → ePDG | LTE 蜂窝基带 |
| 隧道/加密 | 自研用户态 IKEv2 + ESP(`ike_*`, `dataplane`) | **内核 IPsec (`ip xfrm`)** |
| IMS 承载 | 自研 TUN 网关 (`tun_gateway`) | **ModemManager IMS APN bearer** |
| IMS 鉴权 | EAP-AKA(IKEv2 内层) | **SIP Digest-AKA (RFC 3310)** |
| SIP 栈 | `vowifi/ims.rs`(dry-run) | `volte/sip.rs`(真实报文) |
| SMS 编解码 | `vowifi/sms.rs` | **复用** `vowifi/sms.rs` |
| SIM AKA 运算 | `vowifi/qmi_uim.rs` | **复用** `vowifi/qmi_uim.rs` |

**关键点**：VoLTE 路线把「隧道」和「承载」这两个原版最重的自研部分，分别外包给了
**Linux 内核**和**ModemManager**，自己只写上层 SIP + SMS 业务逻辑。这也是为什么它
能用相对少的代码实现。

---

## 3. 从二进制还原的 `volte.rs` 函数地图

> 依据未被 strip 的 `src/volte.rs:行号` panic location 锚点聚类还原。行号为原二进制
> 位置，仅用于推断**功能分布与调用顺序**，非我方实现的行号。

| 原行号区间 | 推断职责 | 对应本设计模块 |
|-----------|---------|--------------|
| ~761 | IPsec 上下文清理（`xfrm policy/state flush`） | `ipsec.rs` |
| ~1366–1375 | IMS data-path probe（探测 P-CSCF 可达性） | `bearer.rs` |
| ~1581–1648 | xfrm 安装、SIP 头常量、bearer 就绪、共享 wwan0 数据激活 | `ipsec.rs`/`bearer.rs` |
| ~1791–1852 | 删除陈旧 IMS bearer、P-CSCF 发现前清理、bearer 连接 | `bearer.rs` |
| ~1965–2018 | bearer 按漫游策略重建、连接重试 | `bearer.rs` |
| ~2229 | IPsec 注册失败 → 降级 UDP | `runtime.rs` |
| ~2443–2487 | 注册成功/监听/REGISTER 刷新 | `runtime.rs` |
| ~2570–2727 | **IPsec 运行时**：MO 准备/发送、MT 接收/RP-ACK | `runtime.rs`/`sms_flow.rs` |
| ~2747–2946 | IPsec MO SMS 多变体、MT 接收/去重/解析、非-MESSAGE 应答 | `sms_flow.rs` |
| ~3050–3093 | Digest 挑战解析（realm/nonce/qop/opaque） | `digest_aka.rs` |
| ~3199–3270 | RP-ACK 重传确认、Security-Server 解析 | `sms_flow.rs`/`sip.rs` |
| ~3340–3387 | AKA 材料、USIM AID 发现、SIP 头 UTF-8 校验 | `digest_aka.rs`/`identity.rs` |
| ~3597–3722 | **MO SMS 多变体发送**(IPv4/IPv6)、SIP 响应 | `sms_flow.rs` |
| ~4414 | 注册态字段序列化(associated_uri/service_route) | `runtime.rs` |
| ~5374–5418 | MT 多段拼接缓存、段序/段数 | `sms_flow.rs` |
| ~5605–5646 | MT 落库、去重标记、随机端口/SPI 生成 | `sms_flow.rs`/`ipsec.rs` |

### 3.1 观测到的关键常量/字符串（作为行为参考，实现时重新命名）

**SIP 头（MESSAGE 请求相关）**：
```
P-Access-Network-Info: 3GPP-E-UTRAN-FDD
Accept-Contact: *;+g.3gpp.smsip
P-Preferred-Service: urn:urn-7:3gpp-service.ims.icsi.sms
User-Agent: SimAdmin VoLTE
Content-Type: application/vnd.3gpp.sms
Accept: application/vnd.3gpp.sms
```

**IMS 注册 Contact 相关**：
```
;+g.3gpp.accesstype="3GPP-E-UTRAN-FDD";+g.3gpp.smsip;expires=3600
```

**IPsec (`ip xfrm`) 参数**：
```
xfrm state ... proto esp spi <spi> ... auth-trunc hmac(md5) ... enc <null> ...
alg=hmac-md5-96;ealg=null
sport/dport, src/dst, mode transport
sec-agree 头字段: mechanism, security-server, spi-c, spi-s, port-c, port-s
```

**AT / QMI / 命令**：
```
AT+CIMI                         # 读 IMSI
--uim-get-card-status           # 读 USIM AID
--wds-start-network=apn=ims,3gpp-profile=<n>
mmcli / qmicli / ip / ifconfig
SIMADMIN_MM_IMS_BEARER          # 环境变量指定 bearer 路径
/org/freedesktop/ModemManager1/Bearer/<n>
```

**Digest-AKA**：
```
AKAv1-MD5 / AKAv2-MD5 / MD5
http-digest-akav2-password
realm / nonce / qop / opaque / Proxy-Authorization / WWW-Authenticate
```

**观测到的错误码族**（重实现时用 SimAdmin 风格重命名，此处仅列语义）：
`imsi_missing`, `smsc_missing`, `phone_uri_invalid`, `hex_invalid`,
`random_spi_invalid`, `sip_status_invalid`, `sip_not_utf8`, `sip_header_missing`,
`ipv6_gateway_missing`, `pcscf_family_mismatch`, `ipsec_ik_invalid`,
`ipsec_requires_ipv6`, `digest_*_missing`, `digest_qop_unsupported`,
`aka_material_invalid`, `aka_res_empty`, `usim_aid_not_usim`,
`runtime_mm_bearer_*`, `data_path_probe_*`, `register_auth_unexpected_status`,
`sms_message_all_variants_failed` …

---

## 4. 前端契约（必须严格对齐，否则现有 UI 不显示）

前端 `volteStatus.js` 已固定以下枚举与字段，重构必须产出**完全一致**的字符串值。

### 4.1 stage（节点，`b()` 函数映射）

| stage 值 | 前端文案 | 含义 |
|---------|---------|------|
| `disabled` | 未启动 | 功能关闭 |
| `starting` | 准备启动 | 初始化 |
| `identity` | 读取 USIM | 读 IMSI |
| `identity_aka` | 读取鉴权材料 | 准备 AKA |
| `radio` | 等待 LTE | 等 LTE 注册 |
| `pcscf` | 发现 P-CSCF | 解析 P-CSCF 地址 |
| `modem` | 等待 ModemManager | 等 MM 就绪 |
| `bearer` | 建立 IMS bearer | 建 IMS APN 承载 |
| `register_ipsec` | IMS 注册（IPsec） | IPsec 保护下注册 |
| `register_udp` | IMS 注册（UDP） | 降级明文 UDP 注册 |
| `registered` | 短信已接管 | 注册成功，接管短信 |
| `stopping` | 正在停止 | 关闭中 |

### 4.2 phase（阶段，`g()` 函数映射）

| phase 值 | 前端文案 |
|---------|---------|
| `disabled` | 未启动 |
| `starting` | 准备启动 |
| `registered` | 已注册 |
| `degraded` | 等待恢复 |
| `stopping` | 正在停止 |

### 4.3 registration_mode（`v()` 函数）

`ipsec` → "IPsec"；`udp` → "UDP"。

### 4.4 control 响应字段（`volteStatus.js` + volte.rs:~1366 序列化字段还原）

```jsonc
{
  "enabled": true,                  // 或 feature_enabled && sms_enabled
  "feature_enabled": true,
  "sms_enabled": true,
  "phase": "registered",
  "stage": "registered",
  "registration_mode": "ipsec",     // ipsec | udp | ""
  "pcscf": "…",                     // P-CSCF 地址(可脱敏)
  "session_started_at": "…",
  "registered_at": "…",
  "last_rx_at": "…",
  "last_tx_at": "…",
  "last_error": "…",
  "last_failure_at": "…",
  "next_retry_at": "…",
  "sent_count": 0,
  "received_count": 0,
  "duplicate_count": 0,
  "reconnect_count": 0,
  "data_path_mode": "…",
  "data_path_probe": { … }
}
```

### 4.5 错误文案映射（`h()` 函数，last_error 子串匹配）

前端靠 `last_error` 里的**子串**匹配给出中文提示。为让 UI 正确显示，本模块产出的
`last_error` 应包含前端能识别的关键子串（下表左列）。这属于**接口契约**，需保留：

| last_error 含子串 | 前端提示 |
|------------------|---------|
| `volte_imsi_missing` | 未读取到 IMSI |
| `volte_at_` | AT 通道未就绪 |
| `volte_runtime_mm_bearer_roaming_forbidden` / `roaming not allowed` | IMS bearer 禁止漫游 |
| `PdpAuthFailure` / `PDP authentication failure` | IMS PDP 鉴权失败 |
| `volte_dependency_missing:ip` / `volte_command_spawn_failed:ip:...` | 系统缺少 ip 命令 |
| `volte_runtime_mm_modem_*` / `couldn't find modem` | ModemManager 尚未就绪 |
| `volte_runtime_mm_bearer` / `volte_runtime_health_bearer` | IMS bearer 未就绪 |

> **合规注**：这些子串是**前端已发布的接口契约**（前端 JS 是 GPL 产物的一部分，可读），
> 保留它们是为了兼容既有 UI，属于互操作性需要，不构成对二进制的抄写。

---

## 5. 模块文件结构设计

在 `backend/src/` 下新建 `volte/` 目录（与 `vowifi/` 平级、独立）：

```
backend/src/volte/
├── mod.rs          // 模块声明 + 顶层文档 + 干净室声明
├── identity.rs     // IMSI 读取 + USIM AID 发现(封装/复用 qmi_uim)
├── digest_aka.rs   // RFC 3310 Digest-AKA 摘要计算(用 CK/IK/RES)
├── sip.rs          // SIP 请求构造 + 响应解析(REGISTER/MESSAGE)
├── ipsec.rs        // ip xfrm SA/policy 命令拼装 + SPI/端口生成
├── bearer.rs       // ModemManager IMS bearer 管理 + P-CSCF 发现 + data-path probe
├── sms_flow.rs     // MO/MT 短信编排(复用 vowifi::sms 编解码 + RP-ACK + 拼接去重)
├── runtime.rs      // 状态机 + 编排 + 公开状态快照 + 与sms_listener协同
└── errors.rs       // VolteError 统一错误类型(语义对齐前端子串契约)
```

复用（不复制）上游既有模块：
- `vowifi::sms`：TPDU/RP-DATA/GSM7/UCS2/UDH 编解码（需将部分私有函数提升为 `pub`，
  或在 `sms.rs` 增加 `pub` 门面函数——见 §7）。
- `vowifi::qmi_uim`：`execute_usim_authenticate_via_proxy_reason_with_retry` 做 AKA。
- `vowifi::ims::parse_sip_response` 的私有 helper（Digest 参数解析等）——评估是否提升复用。

改动的既有文件：
- `main.rs`：注册 2 条路由 + app state 挂 volte runtime。
- `handlers.rs`：新增 `get_volte_control_handler` / `set_volte_feature_handler`。
- `models.rs`：新增 `VolteControlResponse` / `SetVolteFeatureRequest`。
- `config.rs`：新增 `VolteConfig` + `ConfigManager` 的 getter/setter。
- `db.rs`：`SmsMessage.transport` 增加 `"volte_ims"` 取值（字段已存在，无需改表）。
- `sms_listener.rs`：VoLTE 注册成功后暂停 MM SMS 轮询（复用其已有 pause 机制）。
- `vowifi/sms.rs`：按需 `pub` 化编解码函数（加性改动，不改逻辑）。

---

## 6. 分阶段实现计划

每个阶段：独立编译通过 + 单元测试 + 在你机器上 `cargo test` 验证后再进入下一阶段。

### 阶段 1：模块骨架（低风险，先落地形状）
- 新建 `volte/mod.rs` + `errors.rs` + `runtime.rs`（仅状态机与快照，无真实 IO）。
- `VolteConfig`（仿 `VowifiConfig`：`feature_enabled` / `sms_enabled` /
  auto-restore 三元组）挂到 `AppConfig`，加 `#[serde(default)]`。
- `ConfigManager`：`get_volte_config` / `set_volte_feature_enabled`
  （关 feature 连带关 sms，仿 vowifi 联动）。
- `models.rs`：`VolteControlResponse` / `SetVolteFeatureRequest`。
- `handlers.rs`：两个 handler（先返回 runtime 快照 + 开关落库）。
- `main.rs`：注册 `/api/volte/control`、`/api/volte/feature`。
- 状态机：stage/phase 枚举（严格对齐 §4），`as_str()`，dry-run 全流程快照。
- **测试**：状态机流转、开关联动、control 响应字段序列化、dry-run 快照。
- **交付物**：能编译、能起服务、`/api/volte/control` 返回 `disabled` 态。

### 阶段 2：SIP 报文层（纯逻辑，完全离线可测）
- `sip.rs`：构造 REGISTER / MESSAGE 请求字节（含 §3.1 的头）；解析 SIP 响应
  （状态行 + Via/CSeq/Call-ID/WWW-Authenticate/Security-Server/Service-Route/
  P-Associated-URI/Expires）。评估复用/改造 `ims::parse_sip_response`
  （需保留真实 nonce/realm/qop/opaque，而非脱敏）。
- Call-ID/branch/tag/CSeq 生成器（随机 + 单调）。
- **测试**：请求构造固定样例断言、响应解析往返、401 挑战字段提取、
  多值头引号内逗号处理。

### 阶段 3：IMS 注册 + Digest-AKA（离线可测，AKA 运算复用）
- `digest_aka.rs`：RFC 3310 AKAv1-MD5 / AKAv2-MD5。输入 realm/nonce(含 RAND‖AUTN)/
  qop/uri + SIM 返回的 RES/CK/IK，产出 `response`/`cnonce`/`nc`。
  - nonce 解码：base64(RAND‖AUTN) → 拆出 RAND/AUTN 喂给 qmi_uim。
  - AUTS(同步失败) → 触发 resync REGISTER。
- `identity.rs`：读 IMSI（`AT+CIMI` 经 MM Modem.Command，或 QMI）、USIM AID 发现
  （`--uim-get-card-status`，失败用内置 AID 前缀兜底）。
- `runtime.rs`：注册子状态机 REGISTER→401→AKA→鉴权 REGISTER→200，装配私有身份
  (IMPI/IMPU)、Contact、Security-Client。
- **测试**：RFC 3310 测试向量验证 Digest 计算；nonce 拆分；resync 触发条件；
  IMPI/IMPU 从 IMSI 推导（`<IMSI>@ims.mnc<MNC>.mcc<MCC>.3gppnetwork.org`）。

### 阶段 4：IPsec (ip xfrm) + IMS bearer / P-CSCF（命令层可测，IO 需真机）
- `ipsec.rs`：拼装 `ip xfrm state/policy` add/flush 命令（transport 模式、
  hmac-md5-96、ealg null、双向 SA、sport/dport/spi-c/spi-s）；SPI/本地端口随机生成。
- `bearer.rs`：经 MM D-Bus/CLI 建立/复用/删除 IMS APN bearer；从 bearer 属性/PCO
  取 IPv6 地址、网关、P-CSCF；data-path probe（向 P-CSCF 发探测 SIP 看可达）。
- **测试**：xfrm 命令参数序列断言、SPI/端口范围校验、P-CSCF 地址族匹配、
  bearer 路径解析。**真机项**：实际 xfrm 安装、bearer 建立（你验证）。

### 阶段 5：MT 收短信（编解码复用，链路可单测）
- `sms_flow.rs` MT 侧：监听 SIP MESSAGE（IPsec/UDP 两路）→ `parse_mt_rp_data`
  （复用 sms.rs）→ 长短信拼接缓存（段序/段数/参考号）→ 去重（trace/时间戳/
  跨传输标识）→ 落库（transport=`volte_ims`）→ 回 RP-ACK（`build_network_rp_ack`）→
  200 OK 响应 MESSAGE。
- 与 `sms_listener` 协同：注册成功后暂停 MM 轮询，停止时恢复。
- **测试**：单段接收→ACK、多段乱序拼接、重复段忽略、非 MESSAGE 请求应答、
  RP-ACK 重传确认。**真机项**：真实收到运营商短信（你验证）。

### 阶段 6：MO 发短信 + 协同收尾
- `sms_flow.rs` MO 侧：`build_single_part_mo_submission`（复用）→ 装进 MESSAGE →
  多变体发送（IPv4/IPv6 尝试）→ 等 202/200 + RP-ACK → 落库/更新状态。
- MO 长短信分段（>160 GSM7 / >70 UCS2，加 UDH）——上游 sms.rs 只解不分，需自写。
- `/api/sms/send` 集成：VoLTE 已注册走 VoLTE，否则回退 MM（复用现有回退点）。
- **测试**：单段/多段 MO 构造、回退判定、状态更新。**真机项**：真实发出（你验证）。

---

## 7. 对上游既有代码的改动清单（加性、可编译器校验）

| 文件 | 改动 | 风险 |
|------|------|------|
| `vowifi/sms.rs` | 将 GSM7/UCS2/TPDU/地址/UDH 等私有 fn 提升为 `pub`（或加 pub 门面）；MO 分段新增 `pub fn build_multipart_mo_submissions` | 低（加性） |
| `config.rs` | 新增 `VolteConfig` + `AppConfig.volte` 字段 + 3 个 getter/setter | 低 |
| `models.rs` | 新增 2 个 DTO | 低 |
| `db.rs` | `transport` 增加 `"volte_ims"` 语义（无表结构变更） | 无 |
| `handlers.rs` | 新增 2 个 handler | 低 |
| `main.rs` | 注册 2 条路由 + app state 挂 runtime | 低 |
| `sms_listener.rs` | 复用既有 pause/resume，增加 VoLTE 注册态判断 | 中 |
| `backend/src/` | 新增 `volte/` 模块（8 个文件） | —— |
| `main.rs` / `lib` | `mod volte;` 声明 | 低 |

---

## 8. 关键技术难点与风险

1. **Digest-AKA 正确性**：nonce 是 base64(RAND‖AUTN[‖server-data])，拆分方式、
   AKAv2 的 password 派生（用 CK/IK 经 PRF）需严格按 RFC 3310/3GPP TS 33.203。
   → 用规范测试向量单测兜底；真机 401 二次挑战验证。
2. **IPsec 与 SIP 端口绑定**：sec-agree 协商出的 spi-c/spi-s/port-c/port-s 必须与
   实际 xfrm SA 和 socket 绑定端口一致，否则内核丢包。→ 命令层单测 + 真机抓包。
3. **IPv6 依赖**：观测到 `ipsec_requires_ipv6`——IMS bearer 通常是 IPv6，P-CSCF 亦然。
   IPv4 环境可能只能走 UDP 降级。→ 双模实现，环境探测。
4. **ModemManager 版本差异**：bearer 属性/PCO 字段在不同 MM 版本暴露不同。
   → 多路径兜底（D-Bus + CLI）。
5. **与原 SMS 监听器竞态**：注册成功/失败/重连时的 pause/resume 时序。
   → 复用上游已验证的 pause 机制，加状态判断。
6. **真机不可及性**：我无法连真实网络，字节级怪癖靠你抓包反馈迭代。

---

## 9. 验证策略

- **每阶段**：`cargo test`（Windows + MinGW 环境，如上次 voice 模块）。
- **离线集成**：dry-run 快照走完整 stage 流程，断言字段与前端契约一致。
- **真机**（你执行）：
  1. 部署到目标 aarch64 设备。
  2. 开启 VoLTE feature，观察 `/api/volte/control` 的 stage 推进。
  3. 用另一手机发短信到该 SIM，看是否 MT 落库 + UI 显示。
  4. 从 UI 发短信，看 MO 是否送达。
  5. 失败时抓 `journalctl` 日志 + `tcpdump` P-CSCF 交互，反馈给我迭代。

---

## 10. 交付节奏建议

- 一次一个阶段，编译+测试通过并经你确认后再进下一阶段。
- 每阶段结束我给：改动文件清单、测试结果、真机验证指引（若有）。
- 全部完成后：整理成适合上游 PR 的提交（含模块文档、干净室声明、测试）。

---

## 附录 A：IMPI/IMPU/域名推导规则（3GPP TS 23.003）

```
IMPI  = <IMSI>@ims.mnc<MNC>.mcc<MCC>.3gppnetwork.org
IMPU  = sip:<IMSI>@ims.mnc<MNC>.mcc<MCC>.3gppnetwork.org
Home  = ims.mnc<MNC>.mcc<MCC>.3gppnetwork.org
（MNC 不足 3 位补前导 0；部分运营商用 ISIM 显式 IMPI，无 ISIM 时按上式从 USIM 推导）
```

## 附录 B：SMS-over-IP 报文栈（TS 24.341）

```
SIP MESSAGE
  Content-Type: application/vnd.3gpp.sms
  body = RP-DATA (TS 24.011)
           └── RPDU
                └── TPDU (TS 23.040): SMS-DELIVER(MT) / SMS-SUBMIT(MO)
                      └── User-Data (GSM7 / UCS2, 可含 UDH 长短信头)
应答: RP-ACK / RP-ERROR 亦封装在反向 SIP MESSAGE 中
```

## 附录 C：复用资产速查（来自 SimAdmin-main 调研）

| 能力 | 复用符号 | 位置 |
|------|---------|------|
| SIM AKA 运算 | `execute_usim_authenticate_via_proxy_reason_with_retry` → `UsimAkaApduResult{res,ck,ik,auts}` | `vowifi/qmi_uim.rs` |
| USIM AID 常量 | `USIM_AID_PREFIX` 等 | `vowifi/qmi_uim.rs` |
| MO 单段编码 | `build_single_part_mo_submission` → `MoSmsSubmission` | `vowifi/sms.rs` |
| MT 解码 | `parse_mt_rp_data` → `MtSmsDeliver` | `vowifi/sms.rs` |
| RP-ACK | `classify_rp_ack` / `build_network_rp_ack` | `vowifi/sms.rs` |
| SIP 响应解析 helper | `parse_sip_response`(+私有 helper，需改造保留真实值) | `vowifi/ims.rs` |
| 配置模式 | `VowifiConfig` / `ConfigManager` | `config.rs` |
| 短信持久化 | `db::SmsMessage`(transport 字段已支持) | `db.rs` |
| SMS 监听 pause | `sms_listener` 既有 pause 机制 | `sms_listener.rs` |

---

*本设计文档基于公开规范与合法行为观测编写，遵循上游干净室原则，供 GPLv3 上游贡献之用。*
