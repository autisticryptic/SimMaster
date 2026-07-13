# SimAdmin 多路接入 IMS / SIP Trunk 项目规划

> 本文档汇总截至目前所有分析与讨论共识，作为后续开发的总蓝图。
>
> **文档性质**：规划 + 已知资产清单 + 路线图 + TodoList。所有对二次开发 VoLTE 二进制的还原均为
> **基于字符串/行号锚点的行为级静态推断（clean-room）**，非逐字节反编译；最终实现基于公开
> 3GPP / RFC 规范独立编写，以符合 GPLv3 与上游 clean-room 风格。

---

## 一、项目定位（Vision）

把 SimAdmin 从"单设备 SIM 管理面板"升级为一个 **多路接入的 IMS 终端 + SIP Trunk 网关**：

- 对**运营商侧**：用 SIM 的号码，作为一个可注册的 IMS 终端（VoWiFi / VoLTE）或传统 CS 终端。
- 对**内网侧**：暴露一条标准 **SIP trunk / endpoint**，可对接 FreePBX / Asterisk / Linphone 等外部 UA。
- 设备本身**不放音**（目标硬件高通 410 随身 WiFi 无 mic/speaker/codec/PCM），因此语音一律走
  **网关 / RTP relay 模式**：真正说话的是内网的软电话，设备只做信令 + 媒体中继。

```
        Linphone / 软电话 / 话机
                 │ SIP + RTP (标准, 可 opus/G711)
        FreePBX / Asterisk (PBX，可选)
                 │ SIP + RTP (标准)
   ┌──────────── SimAdmin (SIP Trunk 网关) ───────────┐
   │  对外：一条标准 SIP endpoint (per SIM)             │
   │  对内：多路接入编排 (VoWiFi > VoLTE > CS，可配置)    │
   │                                                  │
   │   接入腿：                                        │
   │   ├─ VoWiFi 腿：WiFi → ePDG → IMS (自研 IKEv2/ESP) │
   │   ├─ VoLTE 腿：LTE  → IMS APN → IMS (内核 ip xfrm) │
   │   └─ CS   腿：基带直连基站 (ModemManager)          │
   └──────────────────────────────────────────────────┘
                 │ QMI / AT / D-Bus
            高通 410 / EC20 + SIM
```

---

## 二、核心架构认知

### 2.1 三条接入腿，两张网

| 接入腿 | 承载 | 保护方式 | 信令栈 | 适用能力 |
|--------|------|----------|--------|----------|
| **VoWiFi** | WiFi / IP → ePDG | 自研用户态 IKEv2 / ESP | IMS SIP | SMS / 语音 / 视频 |
| **VoLTE** | LTE 基带 → IMS APN bearer | 内核 `ip xfrm` (IPsec) | IMS SIP | SMS / 语音 / 视频 |
| **CS (传统)** | 基带直连基站 | 无（运营商网络内） | 基带控制信道 | SMS / 语音* |

关键洞察：**VoWiFi 与 VoLTE 共享同一套 IMS/SIP 上层**（REGISTER、Digest-AKA、
SMS-over-IMS 的 `MESSAGE`、Voice-over-IMS 的 `INVITE`、Video 的 SDP video m-line）。
两者差异**只在"如何建立受保护的 SIP 通道"**：

- VoWiFi：用户态 IKEv2 协商 + ESP 封装（原版 `vowifi/` 已实现）。
- VoLTE：让 ModemManager 建 IMS APN bearer + 用 `ip xfrm` 把 SA 灌进内核（本次重构）。

CS 是**完全不同的另一套**（不是 IP、不是 SIP，走基带）。

### 2.2 分层架构目标

```
┌──────────────────────────────────────────────────────┐
│  应用层：短信服务 / 语音服务 / 视频服务  (传输无关 API)      │
├──────────────────────────────────────────────────────┤
│  编排层 Orchestrator (按能力分别配置)                      │
│  - 选路(用户自定义优先级)   - 就绪监测                       │
│  - 故障回退               - 活跃监听者选举 + 收信去重          │
├───────────────────────────┬──────────────────────────┤
│  IMS 接入层 (共享 SIP/注册/AKA/SDP)  │   CS 接入层            │
│ ┌───────────┐ ┌───────────┐  │  (ModemManager)      │
│ │ VoWiFi 腿  │ │ VoLTE 腿   │  │  传统 SMS / CS 语音   │
│ │ IKEv2/ESP │ │ 内核 xfrm  │  │                      │
│ └───────────┘ └───────────┘  │                      │
├───────────────────────────┴──────────────────────────┤
│  对外接入层：SIP Trunk / (可选)内嵌 Asterisk + RTP relay    │
└──────────────────────────────────────────────────────┘
```

### 2.3 目标硬件对能力的约束（务必牢记）

| 能力 | VoWiFi | VoLTE | CS | 备注 |
|------|--------|-------|-----|------|
| 收发短信 | ✅ | ✅ | ✅ | 三层皆可，CS 最可靠 |
| 语音（本地放音） | ❌ | ❌ | ❌ | **高通410 无音频硬件** |
| 语音（网关/relay） | ✅ | ✅ | ❌ | CS 音频走基带 PCM，无法 relay |
| 视频 ViLTE（网关/relay） | ✅ | ✅ | ❌ | 同语音，纯 relay |

结论：**语音/视频一律网关模式；CS 语音在目标设备上放弃**（EC20 等带 USB-Audio/PCM 的
设备才有可能，作为未来可选项保留接口）。

---

## 三、可配置策略模型（本次讨论新增共识）

### 3.1 能力与接入层解耦，各自独立配置

短信 / 语音 / 视频**各自**拥有：
1. **分层启用开关**（VoWiFi / VoLTE / CS 各一个 on/off）
2. **用户自定义优先级顺序**（可任意排列，不硬编码）

拟新增配置结构（挂到 `AppConfig`，仿 `VowifiConfig` 的 `#[serde(default)]` 模式）：

```rust
/// 单条接入腿的启用状态
pub struct AccessLegToggles {
    pub vowifi_enabled: bool,
    pub volte_enabled: bool,
    pub cs_enabled: bool,
}

/// 某项能力的接入策略：启用开关 + 有序优先级
pub struct AccessPolicy {
    pub toggles: AccessLegToggles,
    /// 优先级顺序，如 ["vowifi","volte","cs"]，编排层按此顺序尝试/选活跃监听者
    pub priority: Vec<AccessLeg>,   // AccessLeg = Vowifi | Volte | Cs
}

/// 顶层：短信 / 语音 / 视频 各一份，互不影响
pub struct ImsServicePolicy {
    pub sms:   AccessPolicy,   // 默认 priority = [vowifi, volte, cs]
    pub voice: AccessPolicy,   // 默认 priority = [vowifi, volte]（cs 默认关，硬件不支持）
    pub video: AccessPolicy,   // 默认 priority = [vowifi, volte]
}
```

### 3.2 收信去重与"活跃监听者选举"

**问题**：收信要求某条腿处于注册/监听态；若 VoWiFi-IMS 与 VoLTE-IMS 同时注册同一号码
会**重复收信**。（VoLTE 二进制里已见作者处理：`SMS listener paused while VoLTE IMS SMS
path is registered` —— 让 CS 监听器与 IMS 路径互斥。）

**方案（双保险）**：

1. **活跃监听者选举**：编排层按用户配置的优先级，选**当前就绪的最高优先级腿**作为唯一
   "活跃监听者"，其余腿的收信监听置为暂停；活跃腿掉线→自动切换到下一条并重新注册。
2. **收信去重兜底**：即便瞬时并发，落库前按 **`(发件号码 + SCTS 时间戳 + 内容/UDH 指纹)`**
   判重（上游 `sms.rs::MtSmsDeliver::is_duplicate_delivery` 已有基础逻辑，DB 层加去重标记）。

---

## 四、已有可复用资产清单（重要）

> 均来自上游 `SimAdmin/backend/src/`，本次调研确认为**真实可用**（非 dry-run）的部分已标注。

### 4.1 SIM 卡 AKA 运算 —— `vowifi/qmi_uim.rs` ⭐（最关键，直接可用）

```rust
pub const USIM_AID_PREFIX: &[u8] = &[0xa0,0x00,0x00,0x87,0x10,0x02];

pub struct UsimAkaApduResult { pub res: Vec<u8>, pub ck: Vec<u8>, pub ik: Vec<u8>, pub auts: Option<Vec<u8>> }

// 一站式：连 qmi-proxy → 开逻辑通道 → 发 USIM AUTHENTICATE → 解析 → 清理（含重试）
pub fn execute_usim_authenticate_via_proxy_reason_with_retry(
    proxy_socket:&str, device_path:&str, slot:u8, aid:&[u8],
    rand:&[u8], autn:&[u8], attempts:usize, timeout:Duration, retry_delay:Duration
) -> Result<UsimAkaApduResult, &'static str>;
```
输入 RAND/AUTN，输出 RES/CK/IK/AUTS。**IMS Digest-AKA、EAP-AKA 都靠它**。仅 `#[cfg(unix)]`。

### 4.2 3GPP 短信编解码 —— `vowifi/sms.rs` ⭐

**直接 pub 可用**：
```rust
pub fn build_single_part_mo_submission(recipient:&str, text:&str, service_center:&str)
    -> Result<MoSmsSubmission, SmsEncodingError>;      // 单段 MO：得到 RP-DATA body
pub fn parse_mt_rp_data(body:&[u8]) -> Result<MtSmsDeliver, SmsEncodingError>;  // MT 解析(含UDH)
pub fn classify_rp_ack(body:&[u8], expected_reference:u8) -> RpduAckState;
pub fn build_network_rp_ack(reference:u8) -> Vec<u8>;   // 收信后回给网络的 RP-ACK
```
```rust
pub struct MoSmsSubmission { pub body:Vec<u8>, pub rp_message_reference:u8, pub part_index:u8, pub part_count:u8, /*…*/ }
pub struct MtSmsDeliver { pub originator:String, pub text:String, pub service_center_timestamp:String,
                          pub segment_reference:Option<u16>, pub segment_sequence:u8, pub segment_total:u8, /*…*/ }
impl MtSmsDeliver { pub fn is_duplicate_delivery(&self, other:&Self) -> bool; }  // 去重基础
```
**私有但价值高，需提升为 pub 或复制**：`build_sms_submit_tpdu` / `parse_sms_deliver_tpdu` /
GSM7 打包解包(`encode/decode_gsm7_*`) / UCS2 / `parse_user_data_header`(UDH) /
号码 BCD 编解码(`encode/decode_address_value`) / GSM7 基础表+扩展表。
**缺口**：`build_single_part_mo_submission` 只支持单段；**MO 长短信分段(>160 GSM7 / >70 UCS2)需自写**。

### 4.3 SIP 响应解析 —— `vowifi/ims.rs`

```rust
pub fn parse_sip_response(response:&str, expected_realm:&str) -> Result<SipResponseSummary, ImsRegisterError>;
```
可解析状态行 + `WWW-Authenticate`/`Proxy-Authenticate`(Digest)/`Security-Server`/`Security-Verify`/
`Expires`/`Service-Route`/`P-Associated-URI` 等。**局限**：出于隐私它只提取"是否存在/是否匹配"
元数据，**丢弃真实 nonce/challenge**（`values_redacted:true`）。做真正 Digest-AKA 需**改造保留真值**。
私有 helper 值得复制：`split_header_values`(正确处理引号内逗号)、`parse_digest_params`、
`parse_security_server_offer`(解析 `alg/ealg/prot/mod/spi-c/spi-s/port-c/port-s`)。
**缺口**：本文件**不构造真实 SIP 请求字节**，只产出 summary 元数据；真实 REGISTER/MESSAGE/INVITE
报文构造需自写。`build_aka_digest_public_state` 只是打码占位，**真实 AKAv1-MD5 计算需自写**。

### 4.4 EAP-AKA（仅 VoWiFi/IKEv2 内层用）—— `vowifi/eap_aka.rs`
真实实现 RFC 4187：`parse_challenge` / `build_challenge_response(challenge, identity, &UsimAkaApduResult)` /
`build_sync_failure_response`。示范了如何把 SIM 的 AKA 结果喂进上层。VoLTE 走 SIP Digest-AKA 时用不到，
但**共享 IMS 核心时**它是 VoWiFi 腿的鉴权部件。

### 4.5 配置持久化 —— `config.rs`
```rust
pub struct VowifiConfig { pub feature_enabled:bool, pub connection_enabled:bool,
    pub auto_restore_initial_delay_secs:u64, pub auto_restore_attempts:u8, pub auto_restore_retry_delay_secs:u64 }
pub struct AppConfig { /*…*/ pub vowifi: VowifiConfig }          // 新配置挂这里
pub struct ConfigManager { /* Arc<RwLock<AppConfig>> + 原子落盘 */ }
// 门禁联动范例：set_vowifi_connection_enabled 在 feature 未开时拒绝；关 feature 连带关 connection
```
**照搬此模式**新增 `ImsServicePolicy` / `AccessPolicy`，全字段 `#[serde(default)]` 保证向后兼容。

### 4.6 短信持久化 —— `db.rs`
```rust
pub struct SmsMessage { pub id:i64, pub direction:String, pub phone_number:String, pub content:String,
    pub timestamp:String, pub status:String, pub pdu:Option<String>, pub transport:String /* "modem"/"vowifi_ims"/… */ }
pub struct SmsStats { /*…*/ }
```
`transport` 字段已支持区分来源；VoLTE 加 `"volte_ims"`，去重逻辑在此表做。

### 4.7 语音信令（已写好，待挪用）—— `SimAdmin/backend/src/vowifi/voice.rs`
上一轮已验证：呼叫状态机、SDP 协商、RTP/AMR 编解码、INVITE/ACK 构建、双腿选路，**10 个单测全过**。
预留 seam：`AudioSource`/`AudioSink`、`CarrierVoiceLeg`、`SipEndpointBridge`、`select_voice_leg`。
**语音阶段可直接复用其信令层**；RTP relay 循环需填实。

### 4.8 HTTP / 路由模式 —— `main.rs` / `handlers.rs` / `models.rs`
路由风格：`.route("/api/xxx", get(h).post(h2).options(options_handler))`。
已有 `SendSmsRequest{phone_number,content}`、`MakeCallRequest{phone_number}`、
`ImsStatusResponse{registered,voice_capable,sms_capable}` 等 DTO 可复用/扩展。

---

## 五、从二进制还原的 VoLTE 函数地图（`src/volte.rs`，约 5600+ 行）

基于未被 strip 的 `src/volte.rs:行号` 锚点聚类（行为级推断）：

| 行号区间 | 推断职责 |
|----------|----------|
| ~760 | IPsec 上下文清理（`ip xfrm policy/state flush`） |
| ~1360–1650 | IMS data-path probe、bearer 就绪判定、SIP 头常量、xfrm 安装 |
| ~1580–1850 | IMS bearer 连接 / 删除陈旧 bearer / P-CSCF 发现前清理 |
| ~1960–2020 | bearer 重建（匹配漫游策略）、连接重试 |
| ~2220–2500 | **IPsec 注册链路**：REGISTER→401→AKA→200 OK over IPsec；失败降级 UDP |
| ~2440–2500 | 注册成功、监听、REGISTER 刷新 |
| ~2570–2950 | **IPsec 运行时**：MO SMS 准备/发送、MT SMS 接收/解析/去重、RP-ACK、非-MESSAGE 请求应答 |
| ~3050–3400 | **Digest 鉴权**：realm/nonce/qop/opaque 解析、AKAv1/v2-MD5、Security-Server 解析、USIM AID 发现 |
| ~3590–3720 | **MO SMS 多变体发送**（IPv4/IPv6 多次尝试）、SIP 响应处理 |
| ~4410 | 注册态字段序列化（associated_uri/service_route/feature_caps/security_verify） |
| ~5370–5650 | **MT SMS 落库**：去重标记、多段拼接缓存、随机端口/SPI 生成 |

### 关键固定字符串（报文可直接对照规范复刻）
```
P-Preferred-Service: urn:urn-7:3gpp-service.ims.icsi.sms
Accept-Contact: *;+g.3gpp.smsip
P-Access-Network-Info: 3GPP-E-UTRAN-FDD
User-Agent: SimAdmin VoLTE
Content-Type: application/vnd.3gpp.sms
Accept: application/vnd.3gpp.sms
Contact 参数: ;+g.3gpp.accesstype="3GPP-E-UTRAN-FDD";+g.3gpp.smsip;expires=3600
```
### 外部命令（依赖目标设备系统工具，非开源库）
```
ip xfrm state/policy (…proto esp spi… alg=hmac-md5-96;ealg=null)   # 内核 IPsec
ip -6 / route / addr                                               # 缺失报 volte_dependency_missing:ip
mmcli  (ModemManager CLI)      qmicli (--uim-get-card-status / --wds-start-network=apn=ims,3gpp-profile=…)
D-Bus: org.freedesktop.ModemManager1[.Modem/.Bearer/.Messaging]
AT+CIMI / AT+CSCA? / AT+CRSM   (读 IMSI / SMSC / EF 文件)
```
### 前端阶段机（`volteStatus.js`，UI 契约，须对齐）
```
disabled → starting → identity(读USIM) → identity_aka(读鉴权材料) → radio(等LTE)
→ pcscf(发现P-CSCF) → modem(等ModemManager) → bearer(建IMS bearer)
→ register_ipsec / register_udp (IMS注册) → registered(短信已接管) → stopping
registration_mode ∈ { ipsec, udp }
```

---

## 六、拟新增模块骨架（写入 `SimAdmin/backend/src/`）

```
backend/src/volte/                 # 独立于 vowifi/，忠实复制 VoLTE-over-LTE 路线
├── mod.rs                         # pub mod 声明
├── config.rs                      # VolteConfig（feature/connection/auto-restore）
├── state.rs                       # 阶段枚举 + VolteRuntimePublicState（对齐前端字段）
├── sip.rs                         # 真实 SIP 请求构造(REGISTER/MESSAGE/INVITE/ACK) + 响应解析
├── digest_aka.rs                  # SIP Digest AKAv1/v2-MD5 计算(消费 qmi_uim 的 CK/IK/RES)
├── ipsec.rs                       # ip xfrm SA/policy 安装与清理、随机 SPI/端口
├── bearer.rs                      # ModemManager IMS APN bearer 建立/重建/探测、P-CSCF 发现
├── identity.rs                    # IMSI / IMPI / IMPU / SMSC 读取(qmicli/AT+CIMI/AT+CRSM)
├── sms_mt.rs                      # MT 收信：SIP MESSAGE→RP-DATA→拼接→去重→RP-ACK→落库
├── sms_mo.rs                      # MO 发信：文本→(分段)→RP-DATA→SIP MESSAGE→等响应
├── runtime.rs                     # 状态机编排：阶段推进、注册刷新、掉线重连、健康监测
└── live.rs                        # #[cfg(unix)] 真实网络/命令 IO 装配（Windows 下禁用桩）

backend/src/orchestrator/          # 跨接入腿编排（能力级）
├── mod.rs
├── policy.rs                      # AccessPolicy / ImsServicePolicy / 优先级解析
├── sms_router.rs                  # 三层 SMS 选路 + 活跃监听者选举 + 去重回退
├── voice_router.rs               # (阶段E) 语音选路（网关模式）
└── listener_election.rs           # 活跃监听者选举 + 腿健康状态汇总

backend/src/trunk/                 # (阶段D) 对外 SIP trunk / Asterisk 对接
├── mod.rs
├── endpoint.rs                    # 对外 SIP UAS/UAC（per SIM 一个 AOR）
├── rtp_relay.rs                   # RTP/RTCP 双向转发（不放本地音，纯中继）
└── bridge.rs                      # 内网腿(Linphone/Asterisk) ↔ 运营商腿(VoWiFi/VoLTE) 桥接
```

**离线可单测**（Windows+MinGW 即可验证）：`sip.rs` / `digest_aka.rs` / `sms_mt.rs` /
`sms_mo.rs` 的编解码与报文构造、`policy.rs` 选路逻辑、`ipsec.rs` 命令拼装（校验参数字符串）。
**仅真机可验**：真实注册/收发、`live.rs`、`bearer.rs`、RTP relay 联通。

---

## 七、开发路线图（Roadmap）

| 阶段 | 目标 | 交付物 | 可离线验证 | 需真机验证 |
|------|------|--------|:---:|:---:|
| **A. VoLTE SMS 核心** | 打通 IMS 地基 + VoLTE 收发短信 | `volte/` 全套 + 单测 | ✅ 编解码/报文/AKA-MD5 | ✅ 真实注册收发 |
| **B. 三层 SMS 编排** | 可配置优先级 + 活跃监听者 + 去重回退 | `orchestrator/` SMS 部分 | ✅ 选路/去重逻辑 | ✅ 切换/回退 |
| **C. 共享 IMS 核心** | VoWiFi/VoLTE 合并 SIP/注册/AKA 层 | 重构后的共享 `ims_core` | ✅ 单测回归 | ⚠️ 两腿回归 |
| **D. SIP Trunk 网关** | 对外 SIP endpoint + RTP relay + Asterisk 对接 | `trunk/` | ✅ SIP/SDP 解析 | ✅ Linphone 联通 |
| **E. 语音编排** | VoWiFi/VoLTE 语音腿（网关模式）+ 可配置优先级 | 挪用 `voice.rs` + `voice_router` | ✅ 信令/选路 | ✅ 真实通话 |
| **F. ViLTE 视频** | SDP video m-line + H.264 RTP relay | video 扩展 | ✅ SDP 协商 | ✅ 真实视频呼叫 |

依赖关系：A 是一切的地基（验证 AKA/IPsec/P-CSCF）；B 依赖 A（需要 VoLTE 腿就位）；
C 是可选优化（减少 VoWiFi/VoLTE 重复代码）；D 依赖 A（需一条 IMS 腿能收发）；
E 依赖 A+D（信令地基 + 对外网关）；F 依赖 E（视频是语音的超集）。

---

## 八、总 TodoList

### 阶段 A — VoLTE SMS 核心 【最高优先，进行中规划】
- [ ] A1. `volte/config.rs`：`VolteConfig`（仿 `VowifiConfig`），挂 `AppConfig`，`#[serde(default)]`
- [ ] A2. `volte/state.rs`：阶段枚举 + `VolteRuntimePublicState`，字段严格对齐前端 `volteStatus.js`
- [ ] A3. `volte/identity.rs`：读 IMSI/IMPI/IMPU/SMSC（qmicli / `AT+CIMI` / `AT+CRSM`）
- [ ] A4. `volte/sip.rs`：真实 SIP 请求构造（REGISTER/MESSAGE/ACK）+ 复用改造 `parse_sip_response`（保留真实 nonce）
- [ ] A5. `volte/digest_aka.rs`：SIP Digest AKAv1-MD5（消费 `qmi_uim` 的 RES/CK/IK；支持 AUTS 重同步）
- [ ] A6. `volte/bearer.rs`：ModemManager IMS APN bearer 建立/重建/探测 + P-CSCF 发现
- [ ] A7. `volte/ipsec.rs`：`ip xfrm` SA/policy 安装与清理、随机 SPI/端口、`hmac-md5-96;ealg=null`
- [ ] A8. `volte/sms_mt.rs`：MT 收信链路（SIP MESSAGE→RP-DATA→拼接→去重→RP-ACK→落库）
- [ ] A9. `volte/sms_mo.rs`：MO 发信链路（含 **MO 长短信分段** 自写 UDH）
- [ ] A10. `volte/runtime.rs`：状态机编排、注册刷新、掉线重连、健康监测、IPsec↔UDP 降级
- [ ] A11. `volte/live.rs`：`#[cfg(unix)]` 真实 IO 装配（Windows 桩）
- [ ] A12. DB：`SmsMessage.transport = "volte_ims"` + 去重标记落库
- [ ] A13. API：`GET /api/volte/control`、`POST /api/volte/feature`、`GET /api/ims/status` 接线
- [ ] A14. 单元测试：SIP 报文、TPDU/RP-DATA、GSM7/UCS2、Digest-AKA 向量、xfrm 命令拼装
- [ ] A15. Windows+MinGW 编译验证 + 全单测通过

### 阶段 B — 三层 SMS 编排器
- [ ] B1. `orchestrator/policy.rs`：`AccessLeg` / `AccessLegToggles` / `AccessPolicy` / `ImsServicePolicy`
- [ ] B2. 配置接线：短信策略持久化 + API（读/改优先级与分层开关）
- [ ] B3. `orchestrator/listener_election.rs`：活跃监听者选举（按优先级选就绪最高腿，唯一监听）
- [ ] B4. `orchestrator/sms_router.rs`：发信选路 + 失败回退（按用户顺序遍历）
- [ ] B5. 收信去重：`(号码+SCTS+内容/UDH 指纹)` 判重，跨腿统一入口
- [ ] B6. 与原 CS `sms_listener.rs` 协同：IMS 活跃时暂停 CS 监听，掉线时恢复
- [ ] B7. 单测：选路顺序、回退、去重、监听者切换

### 阶段 C — 共享 IMS 核心（可选优化）
- [ ] C1. 抽取 `ims_core`：REGISTER / Digest-AKA / SIP 事务 / SDP 协商（VoWiFi+VoLTE 共用）
- [ ] C2. 接入腿 trait：`ImsAccessLeg`（`establish_secure_channel` / `send` / `recv`）
- [ ] C3. VoWiFi 腿、VoLTE 腿分别实现 trait
- [ ] C4. 回归测试：原 VoWiFi 短信/语音单测不回退

### 阶段 D — SIP Trunk / Asterisk 网关
- [ ] D1. 决策：**内嵌轻量 SIP endpoint**（推荐，Rust 自实现 UAS/UAC）vs 内嵌完整 Asterisk（重）
- [ ] D2. `trunk/endpoint.rs`：对外 SIP AOR（per SIM），注册/鉴权内网 UA
- [ ] D3. `trunk/bridge.rs`：内网腿 ↔ 运营商 IMS 腿的 dialog 桥接（INVITE 双向映射）
- [ ] D4. `trunk/rtp_relay.rs`：RTP/RTCP 双向转发（NAT 处理、SSRC/端口映射）
- [ ] D5. FreePBX/Asterisk 对接文档 + Linphone 联通测试
- [ ] D6. 安全：对外 endpoint 必须有鉴权（不可照搬 SMS 端点的开放模式）

### 阶段 E — 语音编排（网关模式）
- [ ] E1. 挪用 `vowifi/voice.rs` 信令层到共享 `ims_core`（INVITE/SDP/呼叫状态机/AMR）
- [ ] E2. VoLTE 语音腿：在 VoLTE IMS 会话上发 INVITE（复用 A 的注册/IPsec）
- [ ] E3. `orchestrator/voice_router.rs`：语音独立优先级（默认 [vowifi,volte]，cs 关）
- [ ] E4. RTP relay 填实：与 `trunk/rtp_relay.rs` 打通，运营商腿 ↔ 内网软电话
- [ ] E5. AMR ↔ G.711/opus 转码策略（设备端 relay 不转码 / 交给 Asterisk 转）
- [ ] E6. 真机：Linphone 经网关拨打/接听运营商电话

### 阶段 F — ViLTE 视频通话
- [ ] F1. SDP 增加 video m-line（H.264，profile-level-id / packetization-mode）
- [ ] F2. 呼叫状态机扩展音视频双流
- [ ] F3. H.264 RTP relay（RFC 6184 分包转发，不转码）
- [ ] F4. `video` 独立优先级配置
- [ ] F5. 真机：ViLTE 视频呼叫经网关联通

### 横切事项（贯穿全程）
- [ ] X1. 敏感值策略：号码/IMSI/密钥/nonce/SDP 不进日志、不序列化对外
- [ ] X2. Clean-room 纪律：注释注明基于 3GPP/RFC 规范；不含第三方二进制的私有命名
- [ ] X3. GPLv3 合规：保留版权与许可证声明、标注修改内容与日期
- [ ] X4. 交叉编译到 `aarch64-unknown-linux-musl`（真机构建）
- [ ] X5. 每阶段：Windows 离线单测 → 真机验证 → 文档更新

---

## 九、可验证性与合规边界（务必知晓）

1. **离线可保证**：SIP 报文构造、TPDU/GSM7/UCS2 编解码、Digest-AKA 计算、`ip xfrm` 命令拼装、
   选路/去重逻辑 —— 均可写单元测试并在 Windows + MinGW 环境验证正确性。
2. **真机才可验**：真实 IMS 注册、真实收发短信/通话、`live.rs`、bearer 建立、RTP 联通 ——
   依赖真实 SIM / LTE / P-CSCF / 运营商网络，只能在目标设备验证。字节级细节（定时器毫秒、
   特定运营商报文怪癖）需真机抓包补齐。
3. **Clean-room / GPLv3**：所有代码基于公开 3GPP（TS 24.229 / 24.341 / 24.011 / 23.040）与
   RFC（3261 SIP / 3310 AKA / 4187 EAP-AKA / 3550 RTP / 6184 H.264）独立实现；不逐字节抄袭
   二次开发二进制，不使用其私有命名。成果以 GPLv3 回馈上游。
4. **安全红线**：对外 SIP endpoint 与 `/api/volte/*` 必须显式鉴权，不得照搬现有 SMS 端点的开放模式。

---

## 十、待你确认 / 后续决策点

- **D1（网关实现方式）**：推荐"Rust 自实现轻量 SIP endpoint"而非内嵌完整 Asterisk（后者体积/
  依赖对随身 WiFi 太重）。是否认可？
- **CS 语音**：目标设备无音频硬件，确认放弃（仅为 EC20 等设备保留 trait 接口）。
- **阶段 C（共享核心）时机**：先让 VoLTE 独立跑通（A/B）再合并，还是一开始就设计共享层？
  （建议先独立、B 之后再合并，降低早期风险。）
