# SimAdmin 多路径语音/短信 Trunk 网关 — 开发全程记录

> **文档性质**：开发历程整合文档（合并自 1 份主规划 v2 + D3b~D9 七个阶段总结 + 多卡化改造记录，共 9 份分散文档）。
> **整合日期**：2026-07-27
> **覆盖范围**：架构重构 → 共享 IMS 核心(A) → VoLTE SMS(B) → SMS 编排器(C) → 多卡底座(M) → SIP Trunk 网关(D3b~D9) → 语音(E) → ViLTE(F) → 通话中视频切换(G) → 真机验证记录
>
> **合规声明**：对 `SimAdmin-VoLTE` 二进制的描述均基于行为级静态分析（字符串锚点 + 前端 API），不含反编译源码。VoWiFi 回迁使用合法持有的旧版 GPLv3 源码。所有重构基于公开 3GPP/RFC 规范独立完成（clean-room），遵循 GPLv3。
>
> **最新稳定基线（2026-07-19，D9 + 物理槽位重构）**：稳定 HEAD 为 `dd4eb95`；Rust `563/563` 测试、`cargo fmt --all --check`、严格 Clippy、前端 ESLint/TypeScript/Vite 生产构建全部通过。上述仅代表代码与本地/离线验证；真实 IMS Bearer/REGISTER/Trunk 语音视频/DTMF/IVR/短信综合回归仍待新 SIM/真机复验。

---

## 目录

1. [项目定位与最终目标](#一项目定位与最终目标)
2. [版本谱系与基线校正](#二版本谱系与基线校正)
3. [统一架构与目录结构](#三统一架构与目录结构)
4. [阶段完成状态总览](#四阶段完成状态总览)
5. [架构重构 + 阶段 A：共享 IMS 核心](#五架构重构--阶段-a共享-ims-核心)
6. [阶段 B：VoLTE SMS 腿](#六阶段-bvolte-sms-腿)
7. [阶段 C：三层 SMS 编排器](#七阶段-c三层-sms-编排器)
8. [阶段 M：多基带/多 SIM 底座与物理槽位](#八阶段-m多基带多-sim-底座与物理槽位)
9. [阶段 D：每线路 SIP Trunk 网关（D3b~D9）](#九阶段-d每线路-sip-trunk-网关d3bd9)
10. [阶段 E：语音编排（网关模式）](#十阶段-e语音编排网关模式)
11. [阶段 F/G：ViLTE 视频与通话中切换](#十一阶段-fgvilte-视频与通话中切换)
12. [真机验证记录（高通 410）](#十二真机验证记录高通-410)
13. [关键约束、风险与工程规范](#十三关键约束风险与工程规范)
14. [Git 检查点与候选二进制清单](#十四git-检查点与候选二进制清单)
15. [剩余待办](#十五剩余待办)

---

## 一、项目定位与最终目标

把 SimAdmin 从"SIM 管理工具"升级为 **"SIM 多路径接入网关 / SIP Trunk"**：

1. **短信**支持三条接入路径，按用户可自定义优先级回退：VoWiFi / VoLTE / CS。
2. **语音**支持三条接入路径，同样可配置优先级；语音与短信优先级**独立设置**。
3. 每条路径（层级）可独立启用/禁用。
4. 新增 **VoLTE**（SMS/语音）与 **ViLTE**（视频通话）能力。
5. 对外提供标准 **SIP Trunk endpoint**，对接 FreePBX/Asterisk，桥接外部 Linphone 软电话拨打电话。

### 关键设备约束

目标设备之一是**高通 410 随身 WiFi**（MSM8916 精简型），**无音频硬件**（无 mic/speaker/codec/PCM）：

- **本地话机模式不可行**：设备无法本地采集/播放音频。
- **网关模式可行**：设备只做 SIP 信令 + RTP relay（转发 UDP 包），真正的音频端点是外部软电话或 PBX 后的话机。
- **CS 语音的音频无法软件 relay**：CS 通话音频走基带内部 PCM/模拟通路，无 IP 包可转发；无音频接口设备上 CS 语音这一层对语音网关无效。

> **结论**：项目定位为 SIP Trunk 网关，恰好化解"无音频硬件"矛盾——网关本就不该放音。VoLTE/ViLTE 媒体走 RTP over UDP（非基带 PCM），设备全程不解码任何一帧音视频即可转发。

---

## 二、版本谱系与基线校正

### 2.1 四个版本

| 版本 | 目录 | 版本号 | 形态 | vowifi/ | voice.rs | volte.rs |
|------|------|--------|------|:---:|:---:|:---:|
| 最新上游 | `SimAdmin-main` | 1.1.5 | 源码 | ✗ | ✗ | ✗ |
| 旧上游+VoWiFi | `SimAdmin-main-vowifi` | 1.1.3 | 源码 | ✓ | ✗ | ✗ |
| VoWiFi 语音版 | `SimAdmin`（工作树） | 1.1.3 | 源码 | ✓ | ✓ | ✗ |
| VoLTE 版 | `SimAdmin-VoLTE` | 1.1.6-dev18 | 仅二进制 | ✗ | ✗ | ✓（独立文件） |

### 2.2 基线校正（重要）

原规划以"迁移到 1.1.5"为前提（含阶段 0 VoWiFi 回迁手册）。但**实际二次开发是在 `SimAdmin`（1.1.3 + vowifi + voice）工作树上直接进行**，并未迁移到 1.1.5。因此：

- **"阶段 0 VoWiFi 回迁"在当前工作树中不适用**（vowifi 本就在树内）。
- 迁移到 1.1.5 作为一项**独立、尚未开始**的任务保留。
- 关键发现：**1.1.5 不是 1.1.3 删 vowifi**，而是独立演进分支（新增 Email/ServerChan3 通知、OTA 模板治理、eSIM 字段等）。若未来回迁，必须做三方合并（3-way merge），不能靠覆盖文件。

### 2.3 回迁高危点（未来迁移 1.1.5 时用）

- **db.rs 的 `SmsMessage.transport` 列索引同步**：加字段使结构体 7→8 字段，SELECT 与 `row.get(索引)` 须同步。改动面收敛在 `sms_message_from_row` + `get_sms_messages` + `get_sms_conversation` 三处。
- **handlers.rs 的 eSIM profile 切换钩子**：1.1.5 该 handler 已演进，植入前必须比对当前实现。
- **`NotificationRule.title_template`**：1.1.5 新增字段，构造处不补则编译失败。

---

## 三、统一架构与目录结构

### 3.1 分层架构

```
┌───────────────────────────────────────────────────────────┐
│  对外接口层：Web UI │ REST API │ SIP Trunk endpoint          │
├───────────────────────────────────────────────────────────┤
│  应用服务层（传输无关）：短信服务 │ 语音服务                    │
├───────────────────────────────────────────────────────────┤
│  编排层 Orchestrator：短信选路/语音选路（独立优先级）           │
│  就绪监测 · 故障回退 · 活跃监听者选举 · 收信去重                │
├──────────────────────────┬────────────────────────────────┤
│  IMS 接入层（共享 SIP/注册/AKA） │  CS 接入层（ModemManager）   │
│  ┌VoWiFi腿 IKEv2/ESP┐ ┌VoLTE腿 内核xfrm┐ │  传统 SMS/CS 语音   │
└──────────────────────────┴────────────────────────────────┘
              QMI / AT / D-Bus / USB-Audio · 基带 + SIM
```

**核心设计原则**：把"IMS 接入"抽象成契约，VoWiFi 腿与 VoLTE 腿各实现一份（差异仅在"如何建立受保护的 SIP 通道"），REGISTER / SMS-MESSAGE / Voice-INVITE / ViLTE 信令**只写一遍**，两条腿共用。

### 3.2 当前工作树实际目录（已重构）

```
SimAdmin/backend/src/
├── main.rs / state.rs          # 入口 + 全局 AppState（含多线路注册表）
├── ims/                        # 🟢 共享 IMS 核心
│   ├── context.rs / access.rs  #   中立上下文 + ImsChannel / AccessLeg
│   ├── register.rs             #   initial→401/407→AKA→authenticated→200 事务
│   ├── sip_message.rs          #   VoWiFi/VoLTE 共用 SIP builder
│   ├── digest_aka.rs           #   AKAv1/v2-MD5 + HMAC-MD5 + nonce 解码
│   ├── sip_frame.rs            #   SIP 组帧/解析原语（含粘包处理）
│   ├── sms_codec.rs            #   共享 RP/TPDU/GSM7/UCS2 编解码
│   └── voice.rs                #   共享呼叫状态机、SDP/RTP/AMR 编解码
├── access/
│   ├── line_registry.rs        #   🟢 每物理基带+SIM 的稳定线路身份与独立运行时
│   ├── vowifi/                 #   VoWiFi 腿（ike*/ims/sms/qmi_uim/live/voice... 31 文件）
│   └── volte/                  #   VoLTE 腿（identity/bearer/pcscf/ipsec/sip/sms/runtime/voice/rtp_relay/vilte）
├── trunk/                      # 🟡 每线路 Asterisk SIP Trunk
│   ├── dialog.rs / sip.rs / bridge.rs / driver.rs / runtime.rs
├── orchestrator/               # 🟢 sms_router/listener_election/dedup/voice_router
├── voice_services/             # ⚪ 旧筛选实现暂留回滚，不接入目标 Trunk 调用链
├── automation/ api/ cellular/ messaging/ notify/ network/ system/ sim/ infra/
```

**依赖方向单向**：`vowifi/ → ims/`、`volte/ → ims/`，两条腿互不依赖。共享层 `ims/` 无 IO（纯逻辑，Windows CI 可全量单测）。

### 3.3 接入腿抽象的关键决策：enum 分发而非 dyn trait

Rust 原生 `async fn in trait` 目前不是 dyn-compatible，无法 `Box<dyn ImsAccessLeg>`。腿的种类是封闭集合（VoWiFi/VoLTE 两种），故采用 **enum 分发**（`enum AccessLeg { VoWiFi(..), VoLTE(..) }` + match），零虚表开销、`async fn` 直接可用、无堆分配。两层抽象：`AccessLeg`（编排器视角）+ `ImsChannel`（信令层视角，只依赖"能收发 SIP 字节"）。

---

## 四、阶段完成状态总览

| 阶段 | 目标 | 状态 | 说明 |
|------|------|:---:|------|
| 目录/架构重构 | 领域化分层 + 共享核心抽离 | 🟢 完成 | 563 项测试全绿；已建 Git 检查点 |
| A. 共享 IMS 核心 | VoWiFi/VoLTE 合并 SIP/AKA/语音状态层 | 🟢 完成 | context/access/register/sip_message/sip_frame/digest_aka/sms_codec/voice |
| B. VoLTE SMS 腿 | 收发短信与真机 IO | 🟢 真实 MT/MO 通过 | REGISTER 200、Service-Route、短 MO、两段 GSM7 MO、MT、RP-ACK、入库、重传去重 |
| C. 三层 SMS 编排器 | 可配置优先级 + 活跃监听者 + 去重 | 🟢 后端+UI 完成 | 真机多腿收发验收延期 |
| M. 多基带/多 SIM 底座 | 每物理基带+SIM 独立线路 | 🟢 核心+物理槽位+UI 完成 | 真机复验物理锚点来源待办 |
| E. 语音 | VoLTE 语音编排 + 信令 | 🟢 双向 live+重协商接线 | 真实通话待 IMS 恢复 |
| D. SIP Trunk 网关 | 对外 endpoint + dialog + RTP relay | 🟡 D9 本地完整，真机待验 | REGISTER 200/3599s 历史通过 |
| F. ViLTE 视频 | SDP video + H.264 relay | 🟡 Trunk/live 接线，真机待验 | — |
| G. 通话中 VoLTE↔ViLTE 切换 | 活跃通话内加/撤视频 re-INVITE | 🟡 双向链路完成，真机待验 | — |
| D9. 恢复与可观测性 | 有界恢复、手动重试、Trunk 诊断 | 🟢 本地完成 | 五轮注册 + 三次 MM 恢复 |
| WIP. 每线路 VoWiFi/独立读卡器 | 每 SIM ePDG/DNS/代理 + 外置槽位 | 🟠 开发中未提交 | 非主线路独立 IKE/IMS 未拆分 |
| H. SimAdmin 语音筛选 | 黑白名单/验证码/语音信箱 | ⚪ 退出目标链 | 统一交 Asterisk，旧实现待删 |
| 网页接听 | 浏览器作 Asterisk WebRTC UA | 💤 最终 Todo | 只连 Asterisk WSS，与 SimAdmin 零耦合 |
| 迁移到 1.1.5 | 成果搬到最新上游 | 🔴 未开始 | 当前在 1.1.3 基线 |

`runtime_scope=per_line_config_legacy_primary_runtime`：配置与持久化已落地，但非主线路的独立 IKE/IMS live runtime 尚未拆分。

---

## 五、架构重构 + 阶段 A：共享 IMS 核心

### 重构成果
- 领域化分层：22 个平铺文件 → 2 根文件 + 11 领域目录。
- 接入腿分组：vowifi/volte 收进 `access/` 伞下（比规划的"顶层平级"更进一步）。
- 共享核心 `ims/` 抽出并被两腿复用；`backend/src/README.md` 源码结构说明。

### 阶段 A 决策修正
经核对 `live.rs`（6585 行）确认，VoWiFi 与 VoLTE 在 IMS 层高度重合（REGISTER 事务、Digest-AKA、SIP MESSAGE/RP-ACK 构造、响应解析/粘包全部一致），差异只在"受保护通道"。因此**先抽共享层再写 VoLTE**，比"VoLTE 独立重写再回头抽取"省 3000+ 行重复代码、少一次回归。旧版把这步列为"可选/后置"的结论作废（当时误判 `ims.rs` 是唯一 SIP 载体，实际真实报文在 `live.rs`）。

### 已落地
- `ims/context.rs`：中立 `ImsRegisterParams`/`ImsIdentity`/`ImsRoute`（取代对 `&CarrierProfile`/`ImsClientTcpRoute` 的直接依赖）。
- `ims/sip_message.rs`：共享 SIP builder；`ims/sip_frame.rs`：组帧/解析原语。
- `ims/digest_aka.rs`：AKAv1/v2-MD5 + hmac_md5 + nonce 解码，**RFC 2617/2104/3310 测试向量单测**。
- `ims/register.rs`：REGISTER 事务骨架；`ims/access.rs`：`trait ImsChannel` + `enum AccessLeg`。
- `ims/sms_codec.rs` / `ims/voice.rs`：短信编解码与呼叫状态机/SDP/RTP/AMR 进入共享层。
- **VoWiFi 全量回归通过**（10 voice 单测 + ims/live 单测），行为不变。

### `vowifi::qmi_uim` 跨腿复用
SIM 侧 AKA 运算（`execute_usim_authenticate_via_proxy_reason_with_retry`）已是 `pub`、逻辑传输无关，VoLTE 直接 `use crate::access::vowifi::qmi_uim`。返回 `UsimAkaApduResult { res, ck, ik, auts }`；USIM AID 前缀 `A0000000871002`。仅 `#[cfg(unix)]`，需 qmi-proxy + ModemManager 释放 QMI 端口。

---

## 六、阶段 B：VoLTE SMS 腿

### 从 VoLTE 二进制还原的行为规格

**运行阶段（对齐前端 volteStatus.js）**：
```
disabled → starting → identity(读USIM) → identity_aka(读鉴权材料)
    → radio(等LTE) → pcscf(发现P-CSCF) → modem(等ModemManager)
    → bearer(建立IMS bearer) → register_ipsec / register_udp → registered(短信已接管)
    → degraded / stopping
```

**技术锚点（二进制字符串证据）**：
- IPsec：`ip xfrm`、`alg=hmac-md5-96;ealg=null`、`Native VoLTE IPsec xfrm installed`。
- IMS bearer：`--wds-start-network=apn=ims,3gpp-profile=`、`SIMADMIN_MM_IMS_BEARER`、`/org/freedesktop/ModemManager1/Bearer/`。
- SIP 短信头：`Content-Type: application/vnd.3gpp.sms`、`P-Preferred-Service: urn:urn-7:3gpp-service.ims.icsi.sms`、`Accept-Contact: *;+g.3gpp.smsip`、`P-Access-Network-Info: 3GPP-E-UTRAN-FDD`、`User-Agent: SimAdmin VoLTE`。
- 鉴权：`AKAv1-MD5`、`AKAv2-MD5`、`http-digest-akav2-password`、`AKA returned AUTS, requesting resync`。
- 降级：`IPsec registration failed, falling back to plain UDP SIP`。
- 协同：`SMS listener paused while VoLTE IMS SMS path is registered`（CS 监听器与 IMS 路径互斥）。

### 已落地模块
- `access/volte/identity.rs`：IMSI（`AT+CIMI`）+ USIM AID（复用 qmi_uim）。
- `access/volte/bearer.rs`：ModemManager IMS bearer 建立/删除陈旧/重建。
- `access/volte/pcscf.rs`：P-CSCF 发现。
- `access/volte/ipsec.rs`：内核 `ip xfrm` SA/策略/清理 + 依赖检查。
- `access/volte/channel.rs`：UDP `ImsChannel`、`SO_BINDTODEVICE`、安全端口 rebind。
- `access/volte/runtime.rs`：状态机（stage 对齐前端契约）。
- `access/volte/sms.rs`：MT/MO 链路（复用 `parse_mt_rp_data`/`build_single_part_mo_submission`）。
- `access/volte/sip.rs` / `digest_aka.rs`：真实 SIP 报文 + Digest 适配器。
- `access/volte/live.rs`：真机 IO——bearer→P-CSCF→REGISTER→AKA→xfrm→200 装配。

### 固定双栈有界回退
默认 `ipv4v6`；网络明确只允许 IPv4/IPv6 时直达对应单栈，否则依次 IPv4→IPv6；每族最多一次、总计最多三次，失败 bearer 逐次删除；旧地址族配置只读兼容，无 API/UI 自定义入口（地址族自定义入口于 `9e11f67` 删除）。

### Qualcomm P-CSCF PCO（关键）
单靠 QMI `Get Current Settings` 会 `PCO=false`、P-CSCF 空。默认 CID 2 的必要 AT 流程：
```
AT+CGACT=0,<cid>
AT+CGDCONT=<cid>,"IPV6","ims"
AT$QCPDPIMSCFGE=<cid>,1,1,1    # 关键开关：让基带在 PCO/CGCONTRDP 返回 P-CSCF 主备
AT+CGACT=1,<cid>
AT+CGCONTRDP=<cid>
AT+CGACT=0,<cid>
AT$QCPDPIMSCFGE=<cid>,0,0,0    # 复位
AT+CGDCONT=<cid>,"IPV4V6",""   # 复位 context
```
每次连接的具体 P-CSCF 地址可能变化，故保存候选列表并按地址族筛选，不硬编码。

### 四端口与 XFRM 精确语义（真机校准）
一次成功记录到四个独立端口：本地随机 send、本地随机 receive、P-CSCF client（通常 9950）、P-CSCF send（通常 9900）。
- 出站 SA：`本地 send → P-CSCF port-s`，使用 P-CSCF `spi-s`。
- 入站 SA：`P-CSCF port-c → 本地 receive`，使用 UE `spi-s`。
- SA 只限定 IPv6 源/目的与 ESP/SPI，不在 state 的 `sel` 锁 UDP 端口；UDP 端口只放 policy selector。
- **`cipher_null` 加密密钥必须空字符串**（写 `0x` 会被内核 `RTNETLINK answers: Invalid argument` 拒绝）。
- 常见初版故障：把 P-CSCF 端口绑成本地端口 → 保护后 REGISTER 超时。

### 长短信分段
MO 长短信：GSM7 153 septets/段、UCS2 67 UTF-16 units/段、8-bit concatenation UDH。关键：UDH 后正文需 1 个 fill bit 对齐 GSM7 septet（`95f63ff` 修复点，否则手机端乱码）。

---

## 七、阶段 C：三层 SMS 编排器

### 收信去重与活跃监听者
- **发信**：临时选一条腿，失败换下一条。
- **收信**：同一时刻 IMS 侧只有**一条**腿作"活跃监听者"（按 `sms_path.priority` 取第一个 enabled 且 Registered 的 IMS 腿）。CS 监听器受 `cs_fallback_receiver` 控制：IMS 活跃时暂停 CS 或保留+强制去重。活跃腿掉线切下一优先级腿重注册。

### 已落地
- `config.rs` `SmsPathPolicy` + `PathLayerConfig` + `AccessPathKind` + `MidFlightDisablePolicy`（中途关线路：默认自动切换/可选反馈失败）+ `normalized()` 归一化。
- `orchestrator/sms_router.rs`：优先级发送 + 回退状态机（enum 分发无 dyn）。
- `orchestrator/listener_election.rs`：活跃监听者选举。
- DB 去重：独立 `sms_dedup` 指纹表 + `claim_sms_dedup`（`INSERT OR IGNORE` 竞态安全）+ `cleanup_sms_dedup`；`orchestrator/dedup.rs` 无明文指纹。每日清理任务（启动延迟 60s + 每 24h，保留天数 `dedup_retention_days` 默认 30）。
- `/api/sms/send` 按 `SmsPathPolicy.priority` 调 VoWiFi/VoLTE/CS，失败遵守 `MidFlightDisablePolicy`。
- 前端策略 UI + `GET/POST /api/sms/path-policy`。
- 可观测性：DB/API/通知模板/Web 气泡统一标记 `CS / VoLTE / VoWiFi`；CS 成功入库后再删 ModemManager 短信对象（防丢信）；短信历史默认上限 10,000（可配 100–100,000），按日删最旧。

VoLTE 单腿收发已通过；真实 VoWiFi/CS 回退与跨三腿重复投递待补测。

---

## 八、阶段 M：多基带/多 SIM 底座与物理槽位

### 目标
把 SimAdmin 从"单基带/单 SIM"改造为"多基带 + 多 SIM 每线路独立运行时"。每线路稳定 `line_id`（hardware + active SIM 的稳定散列，**不使用会在重启后变化的 ModemManager 编号**）。

### 已落地
- `access/line_registry.rs` + `LineRuntimeRegistry`：每线路独立 VoLTE runtime/live session/listener/连接锁。
- VoLTE modem/QMI/UIM 参数注入化，移除 live 层固定 modem 0/`/dev/wwan0qmi0`/UIM slot 1。
- `LineProfileConfig` 持久化、`/api/modems`、`/api/volte/lines*`、按 `line_id` 的 SMS 发送与数据库归属。
- 物理槽位优先使用 sysfs/udev 锚点并持久化顺序，支持 UIM slot、换卡后旧线路保留、配置迁移、冲突/降级可视化。
- SIM 卡页新增"基带线路"页签：在线/驻网/IMS/P-CSCF/QMI/UIM 状态 + 总开关 + 每线路 IMS 连接。

### 兼容边界
`POST /api/sms/send` 可选传 `line_id`；未传及旧设备/网络控制 API 继续用主线路。剩余"查找第一个基带"的通用控制接口列为后续逐项迁移。VoWiFi live/ePDG 仍是单实例兼容层（`runtime_scope=per_line_config_legacy_primary_runtime`）。

### per-line 设计原则（硬约束）
显式传了 `line_id` 但线路不存在/不在位 → **报错，不静默回落主线路**（静默回落正是"UI 说配置线路 N、实际写到线路 1"这类 bug 的根因）。新持久化配置放 `LineProfileConfig`，用 `Option<T>`（None=继承全局），保证旧配置和单卡设备行为不变。

---

## 九、阶段 D：每线路 SIP Trunk 网关（D3b~D9）

### 架构决策（2026-07-16 敲定）
1. **线路 B + 画法一**：SimAdmin 作 SIP Trunk 对接远程 Asterisk，Web 电话挂 Asterisk 后面（浏览器→WebRTC→Asterisk→SIP Trunk→SimAdmin→IMS→运营商）。转码（AMR↔Opus）与 WebRTC 终结全由 Asterisk 承担，SimAdmin 保持纯 RTP relay。**否决"设备自建 WebRTC 网关"变体**。
2. **Trunk 注册双模可配**：`static_peer`（IP 直连、被动应答，靠 `match_host` 认对端）与 `outbound_register`（主动 REGISTER + 定时刷新，NAT 友好），每线路 `TrunkProfile` 里选。
3. **来电策略归属 Asterisk**：运营商来电进 SimAdmin 后不做号码/内容筛选，按 `line_id` 直接桥接 Asterisk；黑白名单/验证码/营销/振铃组/语音信箱统一交 Asterisk。
4. **Web 软电话最终 Todo**：只连 Asterisk WSS，与 SimAdmin 零耦合，倾向 Cloudflare Pages/Workers。

### D3b — 配置层（2026-07-16）
- `TrunkRegistrationMode`（`StaticPeer` 默认 / `OutboundRegister`）+ `TrunkProfileConfig`（全字段 `#[serde(default)]`，默认惰性禁用）。
- 字段：`enabled`/`registration_mode`/`asterisk_host`/`asterisk_port`(5060)/`username`/`secret`/`context`/`extension`/`codec_allow`/`register_expiry_secs`(3600)/`match_host`；后续补 `local_port`。
- 挂进 `LineProfileConfig`（顺作者预留意图 "Trunk settings will extend this same profile later"）。
- `redacted()` 脱敏、`secret_set()` 提示位；**空 secret = 保留已存 secret**（支持前端脱敏回环）。
- `set_line_trunk_profile`/`set_line_trunk_enabled` 门禁校验（启用要求线路已启用 + host 非空；outbound 额外要求 username）。
- 安全修复：`build_volte_line_response` 与连接 handler 补 `.redacted()`，堵 `/api/volte/lines*` 泄漏。
- 3 个 handler + `/api/trunk/lines/{line_id}` 路由；7 个单测。Git `b675d32`。

### D4 — SIP UDP endpoint 与 REGISTER（2026-07-16）
- 每 `LineRuntime` 持有独立 `TrunkRuntime`，按 `line_id` 独立协调，不共享 SIP socket。
- SIP UDP endpoint：DNS/IPv4/IPv6 解析、connected UDP peer 校验、REGISTER 重传、CSeq 匹配、1xx 忽略。
- `static_peer` 不发 REGISTER 但开双向 socket + CRLF keepalive + 应答 OPTIONS。
- Digest MD5/MD5-sess，`qop=auth`/无 qop，401 `WWW-Authenticate`/407 `Proxy-Authenticate`/423 `Min-Expires`，85% 周期刷新。失败 5s 起步、上限 300s 指数退避。
- **Contact 稳定性**：所有启用 Trunk 必须配稳定唯一本地 SIP 端口（不再用随机端口），多线路按 5062/5064/5066... 分配。关闭时先发 `REGISTER Expires: 0` 注销。
- 密码只存持久化配置和内存，API/运行态/日志/文档均不输出明文。新增 `SIMADMIN_CONFIG_PATH` 供真机候选绕开正式 config。
- Git：`2b78d5f`/`47b57d4`/`d97f37f`/`6d10cae`/`d2debb3`/`3ed7628`（Contact 级 expires 优先于全局 Expires，RFC 3261）。

### D5 — Asterisk 对话桥接控制面（2026-07-16）
- `trunk/dialog.rs`：对话状态 `Idle/Inviting/EarlyMedia/Confirmed/Terminating/Terminated`；UAS 收 INVITE→100→18x→200→ACK + CANCEL(487)/BYE；UAC 发 INVITE→1x/2xx→ACK；Call-ID/tag/CSeq/branch 生成匹配；Timer A/B/D 离线可测抽象。
- `trunk/bridge.rs`：`TrunkBridge` trait 事件驱动两腿联动，Mock 验证。能力门禁：IMS 未就绪时 INVITE 先 100 Trying 再 480/503。
- 来电（Asterisk→SimAdmin→运营商）+ 去电（运营商→SimAdmin→Asterisk 扩展 6108）。
- D5-DTMF：RFC4733 `telephone-event` 协商 + 两侧动态 PT 映射；SIP INFO `application/dtmf-relay` 回退。525 项测试。

### D6 — IMS 双向通话与 RTP 桥接（2026-07-17）
- `882b022`：Asterisk→IMS INVITE/可靠 18x/PRACK/ACK/CANCEL/BYE + SIP INFO + 双腿 Tokio UDP RTP relay。
- `948c75c`：IMS 来电→配置扩展 6108、反向 SDP/PT 映射、18x/200/拒绝、CANCEL/BYE 清理。
- 呼入类型可配：二次拨号/绑定待接/绑定立接；呼入绑定、呼出绑定（From user 门禁/403）；旧 `extension` 自动迁移为 `incoming_binding`。
- IP 接通两选项：`IP 接通(first_rtp)`（运营商腿首个有效 RTP 后自动向 Asterisk 200）与 `GSM 接通时立即接通(gsm_answer)`（运营商应答后立即 200）；早期接通后拒绝 → ACK 后 BYE。`ccabdae` 纠正为两个明确枚举（旧布尔值仅作迁移输入）。537~545 项测试。

### D9 — 本地模块收尾（2026-07-19）
- **VoLTE 有界恢复**：每批最多 5 次 IMS 注册尝试；基带缺失最多 3 次 ModemManager 恢复；耗尽进 `exhausted`，暴露 `/api/volte/lines/{line_id}/retry`。
- **双向 re-INVITE**：Asterisk→IMS 与 IMS→Asterisk 的 in-dialog re-INVITE、2xx/ACK/拒绝、并发 `491 Request Pending` 竞争处理。
- **视频 relay**：`NegotiatedTrunkVideoSeam` 将 Asterisk 协商的视频端点接入 IMS 侧；音频/视频各用独立 Tokio UDP relay，ViLTE 总开关门控。
- **Trunk runtime 诊断页**：SIP endpoint/REGISTER/退避、对话/通话、帧/字节、媒体/视频、DTMF、Operator、RTP 包/字节计数，Web 每 3 秒刷新。
- 稳定 HEAD `dd4eb95`；`be10591`（五轮恢复）、`e65591e`（每线路 Trunk 诊断）、`1a593c3`（双向 re-INVITE + 音视频 relay + ViLTE 门控接入 live）、`76ed7e3`~`2535b06`（物理槽位锚点/持久化/迁移/不稳定节点拒绝）。

### 剩余（D7 真机与安全）
- 强制鉴权 + 内网默认绑定/ACL + 敏感信息复核 + 对外 SIP 安全测试。
- Asterisk/Linphone 外呼/来电、真实语音/视频、双向 re-INVITE、抓包、银行/客服 IVR。
- 多轮 REGISTER 刷新、断网/重启恢复、并发通话与 RTP 资源释放长期 soak。

---

## 十、阶段 E：语音编排（网关模式）

### 已落地
- `voice.rs` 参数化：抽出中立 `VoiceParams`，解开对 `CarrierProfile` 耦合；VoWiFi 10 单测回归全绿。
- `access/volte/voice.rs`：VoLTE 语音编排（呼叫状态机驱动 + SDP offer/answer + 腿就绪）。
- `access/volte/sip.rs`：INVITE/ACK/BYE/CANCEL/200OK 报文构造。
- `access/volte/rtp_relay.rs`：RTP 双向转发（对称 RTP 学习 + 计数器 + `#[cfg(unix)]` UDP relay 循环）。
- `orchestrator/voice_router.rs`：独立语音优先级、腿就绪判定、拒绝原因。
- IMS live voice 双向事件：初始 INVITE、可靠 18x/PRACK、ACK、CANCEL/BYE，接入每线路 live session。
- 双向媒体重协商：operator-originated re-INVITE、2xx/ACK/拒绝、并发 `491`。

真实外呼/来电、双向 RTP、首 RTP/GSM 接通、DTMF/IVR、音质/时延、断网恢复验收待 IMS 恢复。

---

## 十一、阶段 F/G：ViLTE 视频与通话中切换

### 原理：对话内 re-INVITE 媒体重协商
在已建立的 SIP 对话内重发 INVITE 改变 SDP 增删媒体流：
- **升级视频（VoLTE→ViLTE）**：re-INVITE 的 SDP 在 `m=audio` 外新增 `m=video`(H.264)。
- **降回语音（ViLTE→VoLTE）**：re-INVITE 发纯 `m=audio`。
- re-INVITE 复用已建立 Call-ID + 双 tag（in-dialog），**CSeq 对话内递增**（非从 1 重新开始）。

### 代码落点
| 能力 | 位置 |
|------|------|
| re-INVITE 报文构造 | `access/volte/sip.rs::build_reinvite` |
| 媒体模式模型 | `access/volte/voice.rs::CallMediaMode`（AudioOnly/AudioVideo） |
| 升级视频 | `VolteVoiceCall::upgrade_to_video()` → `MediaReoffer` |
| 降回语音 | `VolteVoiceCall::downgrade_to_audio()` |
| 确认切换 | `confirm_media_switch(sip_status)`：对端 2xx 才生效；非 2xx **保持原模式、通话不掉** |
| 视频 SDP | `access/volte/vilte.rs::build_av_sdp`/`build_video_offer` |

### 守卫与失败语义
- 只能在 `Active` 通话上切（否则 `CallNotActive`）。
- 升级视频要求 ViLTE 已启用（门禁链 `volte.feature → volte.voice → vilte.feature`）+ 配了本地视频 relay 端点（否则 `VideoNotEnabled`）。
- 幂等：已在目标模式返回 `AlreadyInMode`。
- **拒绝不掉话**：视频升级被对端拒绝时 `Rejected(code)`，通话保持原语音模式继续。

### ViLTE 配置与 relay
- `VilteConfig`（门禁链 + codec H.264）+ `/api/vilte/control`/`/api/vilte/config`。
- `VideoRelay` 复用媒体无关的 `RtpRelayCore`，纯转发不转码。
- `NegotiatedTrunkVideoSeam`：用 Asterisk SDP 实际视频端点构建视频 relay，音频/视频各独立 UDP relay。
- 前端 ViLTE 配置 UI（功能/编解码/端口/超时）；关闭 VoLTE 语音时自动关闭 ViLTE。

真实 ViLTE 会话、Linphone 视频升级/降级、H.264 RTP 抓包/质量验收待真机。

---

## 十二、真机验证记录（高通 410）

设备：高通 410（MSM8916），Debian 13 arm64，ModemManager 1.24。测试纪律：正式 `simadmin.service` 保持 inactive，候选仅监听 `127.0.0.1:3101`，独立 release 目录 + 独立配置/DB，测试后恢复（`wwan0` DOWN、XFRM 0/0、CID 2 恢复 `IPV4V6,""`、`$QCPDPIMSCFGE` 全 `0,0,0`、临时凭据/候选删除）。

### VoLTE SMS 成功链路（历史）
参考成品 `1.1.6-dev18` 与当前源码 `cfc34b1` 均在同机完成：P-CSCF 发现 → IMS IPv6 bearer → 401/AUTS/USIM AKA → XFRM SA/策略 → 受保护 REGISTER 200 → IPsec 监听 → 真实 MT 两段长短信 → 逐段 RP-ACK SIP 202 → 拼接入库。API：`phase=registered`/`registration_mode=ipsec`/`data_path_mode=dedicated_ims_bearer_ipv6`，内核 2 SA + 入/出各 1 policy。当前源码另完成短 MO、两段 GSM7 MO、单段 MT、入库与重传去重（`duplicate_count=2` 只保留一条）。`14a0df0` 验证服务启动 60s 后自动 REGISTER 200（`VoLTE IMS auto-restore registered attempt=1`）。

### Trunk REGISTER 成功（2026-07-17）
真实 PJSIP Trunk 在 `10.0.0.3:8060`：本地稳定端口 `5062`、账号 `41000`、来电扩展 `6108`、有效期 `3600s`；`registered=true`/`last_sip_status=200`/`register_attempts=2`（Digest challenge 后成功）/`reconnect_count=0`。60s 周期连续刷新多轮无降级；持续约 80 分钟跨过 3060s 计划刷新点无异常。异常退出后同一 5062 端口重启 Digest 注册立即恢复（稳定 Contact URI 可复用）。VoLTE/蜂窝数据全程关闭。

### D8 纯净系统 A/B 对照（2026-07-18，阻塞点定位）
高通 410 恢复 Debian 初始状态后，对最新源码 `0979f33` 与参考成品 `05ea96a` 同基线复验：
- **当前公共阻塞点在 IMS PDP context 激活 / P-CSCF 获取这一层，不是 SIP/IPsec/SMS 应用层。**
- `0979f33` 曾激活 IPv4 IMS CID 2，但 `CGCONTRDP=2` 无 P-CSCF，后续重试引发 packet detached、Modem 重枚举和测试窗口内整机重启。
- 参考成品手动补启 ModemManager 后连续两周期加载 IMSI/IMS 域/SMSC，但每周期三轮 IPv6 `CGACT` 均 `MobileEquipment.Unknown`，同样未进入 bearer/XFRM/REGISTER。
- **历史成功事实仍成立，但不能表述为"参考成品当前必然成功"**——问题与当前网络/SIM/固件状态强相关。

### D7 IMS 侧受控复验（2026-07-17）
`ipv4v6 → Ipv6OnlyAllowed`、`ipv6 → prefix-unavailable`；6.17-rc6 `bam-dmux` 在基带异常窗口还会进入 runtime-PM error，故不能忽略 link-up 失败。`cf2c639` 恢复 IMS raw-IP 网卡激活的严格失败语义（禁止 `wwan0` 未 UP 时伪装配置路由）。

### 多基带只读诊断
ARM64 `inspect-modems` 只读通过：连续两次相同 `line_id`，正确识别 modem 0、`/dev/wwan0qmi0`、UIM slot 1、SIM 0、运营商 46011。

---

## 十三、关键约束、风险与工程规范

### 合规与可验证性边界
- **离线可保证**（Windows + MinGW 单测）：SIP 报文构造/解析、Digest AKA-MD5（对照规范向量）、TPDU/RP-DATA/GSM7/UCS2 编解码、`ip xfrm` 命令拼装、SDP 协商、RTP 帧、编排器选路/回退/去重。
- **离线无法保证**（需真机）：运营商 IMS 注册成败、真实 SIM AKA（需 qmi-proxy + 硬件）、真实 LTE/P-CSCF 收发、运营商报文怪癖与定时器、内核 xfrm 目标行为、RTP relay 实际音质/时延。
- **一句话**：离线正确 ≠ 真机跑通。真机阶段务必抓包（SIP/RTP tcpdump）比对。

### 风险登记册（高危项）
| ID | 风险 | 等级 | 缓解/回退 |
|----|------|:---:|-----------|
| R1 | 回迁三方合并引入回归 | 高 | 逐文件合并；每步编译；回退合并前 tag |
| R4 | AKA/Digest 与运营商不符 | 高 | RFC 向量单测兜底；真机抓包迭代；降级 UDP |
| R5 | IPsec xfrm 与端口绑定不一致 | 高 | 命令层单测；真机 tcpdump；降级明文 UDP |
| R10 | 抽离 ims/ 破坏 VoWiFi 行为 | 高 | 纯搬移+改入参不改逻辑；VoWiFi 全量单测回归门；小 commit |
| R7 | 对外 SIP endpoint 被滥用 | 中 | 强制鉴权 + 默认内网绑定 + 用例覆盖 |
| R9 | async trait 非对象安全 | 低 | enum 分发（见 §3.3） |

### 工程规范（Definition of Done）
每阶段必须全部满足：`cargo check`+`build` 通过（Windows GNU 工具链）；`cargo test` 全绿（最新基线 563）；`cargo clippy --all-targets -- -D warnings` 零告警；`cargo fmt --all --check` 通过；前端 build+type-check 通过；DB 改动幂等（`CREATE TABLE IF NOT EXISTS`/`ALTER TABLE ADD COLUMN`，加列带默认值，只增不改）；新功能默认关闭、可运行时回滚。

**派生版本号**：`1.1.5+lilith.<n>`（不冒用 1.1.6）。分支：trunk-based，`stage/<x>-<slug>` 单阶段开发，一阶段一 PR 小步合入。

### 安全提醒
- 对外 SIP endpoint 必须强制鉴权（SIP Digest / IP 白名单 / TLS-SRTP），**不要照搬** VoLTE 二进制 SMS 端点的开放模式（仅功能开关保护、无端点鉴权）。
- 网关模式默认绑定内网接口，显式配置才对公网开放。
- 敏感值（IMSI/nonce/密钥/Trunk secret）需脱敏，不入日志/API/文档。

---

## 十四、Git 检查点与候选二进制清单

### VoLTE / 多基带
| 节点 | 内容 |
|------|------|
| `8d5141a` | 双栈 bearer |
| `4d10159` | Qualcomm PCO |
| `0f77c95` | bearer/XFRM 参数 |
| `7594d79` | 四端口双 socket |
| `cfc34b1` | 受保护 REGISTER 200 |
| `5e018f9` | MT live |
| `a216742` / `a09ec36` | MO/多路径 / P-Associated-URI 修复 |
| `95f63ff` | 长短信 GSM7 fill bit 修复 |
| `b6957f1` | 预到期重建/自动恢复 |
| `14a0df0` | 跨传输 live 指纹去重 |
| `255d693` | 多基带发现 + 稳定线路身份 + runtime registry |
| `fbaf830` | 每线路 VoLTE live/QMI/UIM/监听 + 按 line_id 发送 |
| `519a245` | 只读 `inspect-modems` 诊断命令 |
| `f4b6da5` / `91beec4` | 严格 Clippy 基线 / rustfmt 排版基线 |

### Trunk / 语音 / 视频
| 节点 | 内容 |
|------|------|
| `b675d32` | D3b 每线路 trunk profile API |
| `2b78d5f` | D3b-runtime + UI |
| `47b57d4` | D4 UDP endpoint + Digest 注册 |
| `6d10cae` / `d2debb3` | Contact 稳定/注销 / 拒绝重复端口 |
| `3ed7628` | Contact 级注册有效期（RFC 3261） |
| `882b022` / `948c75c` | D6 Asterisk→IMS / IMS→Asterisk 双向 |
| `c7281d9` / `ccabdae` | 首 RTP 自动接通 / IP 接通配置模型纠正 |
| `1a593c3` | 双向 re-INVITE + 音视频独立 relay + ViLTE 门控接入 live |
| `e65591e` | 每线路 Trunk runtime 诊断 |
| `be10591` | VoLTE 五轮有界恢复 + 基带恢复上限 + 手动重试 |
| `dd4eb95` | **稳定 HEAD**：关闭态 Trunk 草稿校验 + ViLTE Web 约束 |

### 候选二进制 SHA-256（部分）
- `14a0df0`（VoLTE 最终候选）：`C6428D83B0EF4C22A0A10CB7023C44A3B130ECD8F26DAEC389BCD741711E3AC5`
- `262cfd5`（语音状态机/媒体编解码）：`D2DCD3417C4DB83C407219200C03AFEF91A238ADE84E378D72D82E1C61ABFCE1`
- `05d7d52`（真实短信终验）：`3F42D57507EF49B33CD179ED7BC7333F41452566ED6EF57BC84860EDF7E12832`
- `b675d32`（D3b）：`294D5E95C0AF43A9E290AA9B09DDC9AB688CB050C96A8B6B39B6B4BC50B66B0F`
- `882b022`~`948c75c`（D6）：`455EBA6CB295A3862157734A82006EA165AC68DDC8744450E063E1EC242C17CC`
- D6 IP 接通最终候选：`56DA5B8098BA92BBA77B8F193FDFBC082C8F9ACEE60FDC05823D5D6BE1E41CF6`

交叉编译：`cargo zigbuild --release --target aarch64-unknown-linux-musl`。

---

## 十五、剩余待办

### 真机（依赖 IMS/SIM 恢复）
1. 换/恢复可用 SIM 后受控复验 IMS context→P-CSCF→Bearer→XFRM→REGISTER，及真实 MT/MO/长短信/通知/soak。
2. Trunk/Asterisk 真实语音、双向 RTP、DTMF/IVR、re-INVITE、ViLTE 视频链路（抓包比对）。
3. 对外 SIP 鉴权/默认内网/ACL + 长期稳定性验收。

### 本地/独立任务
- **WIP 每线路 VoWiFi/独立读卡器**：非主线路独立 IKE/IMS live runtime 拆分 + 真实读卡器适配（未提交）。
- **阶段 H 旧筛选实现删除**：Trunk 稳定后以独立 Git 提交删除旧 API/UI/配置/DB 调用（能力转 Asterisk）。
- **网页接听**（最终 Todo）：独立静态页面，Cloudflare Pages/Workers，SIP.js/JsSIP 连 Asterisk WSS。
- **迁移到 1.1.5**：三方合并（含 Email/ServerChan3 等 1.1.5 新功能），按 §2.3 高危点执行。

---

> **本文档整合来源**：`SimAdmin_扩展开发文档_多路径语音短信Trunk_进度更新版_v2_视频切换与Trunk对接.md`（根目录主规划）、`阶段D3b~D9_*.md`（7 份阶段总结）、`多卡化改造记录_本轮.md`。整合后原件建议归档或删除，本文档为唯一权威开发历程记录。
