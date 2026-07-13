# VoWiFi 语音通话模块改造说明

本文件汇总本次对 SimAdmin 后端 `backend/src/vowifi/` 的语音（接打电话）能力改造，供逐项核查。

改造目标：在原有"仅短信收发"的 VoWiFi 模块基础上，新增拨打/接听电话的信令与编排能力，并为后续媒体（RTP 语音）与对外标准 SIP endpoint 预留接口。整体遵循下面的目标架构：

```
        Linphone / 其他 SIP 软电话
                   SIP + RTP (标准，可 opus/G711)
        Asterisk (PBX：路由/分机/录音/多方)
                   SIP + RTP (标准)
   ┌──────────── SimAdmin GoIP 网关 ────────────┐
   │  对外：一条标准 SIP endpoint (per SIM)       │
   │  内部编排：先 VoWiFi，失败回退运营商           │
   │                                             │
   │   ┌─ VoWiFi 语音腿 ──────────────┐          │
   │   │ INVITE/SDP/AMR/RTP over ePDG │          │
   │   │ (自研，复用现有隧道/注册)      │          │
   │   └──────────────────────────────┘          │
   │                                             │
   │   ┌─ 运营商语音腿 ────────────────┐          │
   │   │ AT命令(ATD/ATA/CLCC) 控制     │          │
   │   │ USB Audio 读写 PCM            │          │
   │   │ (无 USB Audio → 禁用此腿)     │          │
   │   └──────────────────────────────┘          │
   └─────────────────────────────────────────────┘
                   QMI/AT/USB-Audio
              EC20 模块 + SIM
```

---

## 一、改造前的现状

原 `backend/src/vowifi/` 只实现了 **短信（SMS over IMS）** 的收发：

- `sms.rs`：纯状态机 + GSM7/UCS2 编解码 + RP-DATA/TPDU 处理。
- `live.rs`：真实网络 I/O（IKE、ESP、IMS REGISTER、SMS 发送/接收）。
- `ims.rs`：**只有 REGISTER**，没有任何 INVITE/SDP/RTP 代码。
- `executor.rs` / `runtime.rs` / `flow.rs`：阶段编排，最高阶段止步于 `Sms`。

模块的统一设计范式（本次改造严格沿用）：

1. **纯状态机 + 编解码器**（离线可单测），与 **`live.rs` 真实 I/O** 分离。
2. 每个对外可序列化类型都带 `sensitive_values_policy` 字段，敏感值（手机号、SDP、RTP 负载、密钥）一律不序列化。
3. 阶段（stage）通过 `ExecutorStage` 枚举 + 就绪阶梯（readiness ladder）串联，编排层逐级推进。

---

## 二、新增/修改文件清单

| 文件 | 类型 | 说明 |
|------|------|------|
| `backend/src/vowifi/voice.rs` | **新增** | 语音核心：呼叫状态机 + SDP + RTP/AMR 编解码 + 预留接口 |
| `backend/src/vowifi/mod.rs` | 修改 | 注册 `pub mod voice;` |
| `backend/src/vowifi/profiles.rs` | 修改 | 新增 `VoicePolicy`，加到全部 profile |
| `backend/src/vowifi/diagnostics.rs` | 修改 | `VowifiReadiness` 增加 `voice_ready`；事件显示文案 |
| `backend/src/vowifi/executor.rs` | 修改 | 新增 `ExecutorStage::Voice` 及相关映射 |
| `backend/src/vowifi/runtime.rs` | 修改 | 就绪阶梯、apply_stage、事件日志加入 Voice |
| `backend/src/vowifi/flow.rs` | 修改 | 新增 `VoiceReady` 阶段与流程步骤 |
| `backend/src/vowifi/live.rs` | 修改 | INVITE/ACK 构建、呼叫编排、媒体预留 |
| `backend/src/handlers.rs` | 修改 | 新增 `POST /api/voice/call` 处理器 |
| `backend/src/models.rs` | 修改 | 新增 `PlaceCallRequest` |
| `backend/src/main.rs` | 修改 | 注册 `/api/voice/call` 路由 |
| `Cargo.lock` | 重新生成 | 见文末"依赖与编译环境说明" |

---

## 三、核心新文件：`vowifi/voice.rs`

仿照 `sms.rs` 的拆分方式，约 1700 行，全部为 SimAdmin 自有命名。分为以下部分：

### 3.1 枚举（均带 `as_str()`，`Serialize`）

- `CallDirection`：`MobileOriginated` / `MobileTerminated`。
- `SipInviteState`：SIP INVITE 事务状态 —— `Idle → Queued → InviteSent → Ringing → EarlyMedia → Confirmed → Terminated / Failed`。
- `CallState`：面向 API/UI 的聚合状态 —— `Dialing / Ringing / Active / Ended / Failed`，含 `api_status()`。
- `VoiceLegKind`：`Vowifi` / `Carrier` / `None`（哪条腿承载媒体）。
- `MediaTransportKind`：`RtpAvp`（标准，VoWiFi ESP 内层承载）/ `RtpSavp`（预留 SRTP）。
- `AudioCodec`：`Amr` / `AmrWb` / `Pcmu` / `Pcma`，含 `rtpmap_encoding()`、`clock_rate()`、`static_payload_type()`、`is_amr_family()`、`from_token()`。
- `CallEndReason`：`LocalHangup / RemoteHangup / RemoteBusy / NoAnswer / Declined / NetworkFailure / MediaFailure / Canceled`，含 `from_sip_status()`（如 486→RemoteBusy）。

### 3.2 错误类型

- `VoiceRuntimeError`：`InvalidTransition` / `SipRejected(u16)` / `NoCommonCodec` / `InconsistentState`。
- `VoiceEncodingError`：`EmptyCallee` / `InvalidAddress` / `EmptySdp` / `SdpMalformed` / `NoAudioMedia` / `UnsupportedCodec`。

### 3.3 SDP 模型 / 构建 / 解析（纯函数，RFC 4566）

- `SdpCodec`：编解码 + payload type + fmtp（如 `octet-align`、`mode-set`）。
- `MediaDirection`：`SendRecv / SendOnly / RecvOnly / Inactive`。
- `SdpAudioDescription`：完整音频描述，`to_sdp()` 生成 SDP body，`common_codecs()` 求编解码交集。
- `parse_audio_sdp(body)`：宽容解析 `c=`/`m=audio`/`a=rtpmap`/`a=fmtp`/`a=ptime`/方向属性；静态 PT 0/8 自动识别为 PCMU/PCMA。
- `build_mo_audio_offer(profile, addr, addr_type, port)`：按 profile 首选编解码顺序生成 MO offer。
- `build_profile_codec_offer(profile)`：AMR 族分配动态 PT（96/97），G.711 用静态 PT。
- `build_sdp_answer(profile, offer, ...)`：与 offer 求交集生成 answer（保留 offer 方的 payload 编号）。

### 3.4 RTP 包框帧（纯函数，RFC 3550 / 4867）

- `RtpPacket { payload_type, marker, sequence, timestamp, ssrc, payload }`：`encode()` / `parse()`（跳过 CSRC 与扩展头，处理 padding）。
- `build_amr_rtp_payload(frame_type, speech_bits, octet_aligned)` / `parse_amr_frame_type(...)`：单帧 AMR 载荷框帧（多帧聚合与 CRC 留待实媒体实现）。

### 3.5 Wire I/O DTO（供 `live.rs` 使用）

- `MoCallInvite`：待发 INVITE（trace_id、call_id、callee、sdp_offer 等）。
- `MoCallSipOutcome`：INVITE 同步结果（sip_status、invite_state、call_state、negotiated_codec），含 `api_status()`。
- `MtCallInvite`：观察到的入站 INVITE。

### 3.6 对外可序列化状态（不含任何敏感值）

`CallSdpSummary` / `CallSipSummary` / `CallMediaSummary` / `CallRecord` / `CallPublicRecord` / `VoiceRuntimePublicState`。均带 `sensitive_values_policy`。

### 3.7 呼叫状态机 `VoiceCallStateMachine`

- `new(profile)`：按 profile 的 `voice.vowifi_enabled` / `carrier_fallback_enabled` 选择首选腿。
- `mark_registration_ready()` / `queue_mo_call(leg)` / `submit_invite(n)` / `accept_provisional(status)` / `accept_final_answer(status, codec)` / `record_media_progress(sent, recv)` / `terminate(reason)`。
- `assert_state_consistency()`：如"Active 必须已 Confirmed 且已协商编解码"等不变式。
- `snapshot()`：产出 `VoiceRuntimePublicState`。
- `build_dry_run_voice_snapshot(profile)`：离线演示，完整走一遍 register→INVITE→180→183→200(AMR)→media→hangup。

### 3.8 预留接口（架构要求的"seam"，当前不做 I/O）

- `AudioSource` / `AudioSink`（媒体面帧收发）+ 惰性默认实现 `SilentAudioSource` / `NullAudioSink`。
- `CarrierVoiceLeg`：运营商腿控制面 —— `usb_audio_available()` / `dial`(ATD) / `answer`(ATA) / `hangup`(AT+CHUP) / `poll_state`(AT+CLCC)。默认实现 `DisabledCarrierVoiceLeg`（**无 USB Audio 时禁用该腿**，符合架构图）。
- `SipEndpointBridge`：**对外标准 SIP endpoint（per SIM）** 的接入 seam，`is_exposed()` / `local_aor()` / `on_external_invite()`。默认实现 `UnexposedSipEndpointBridge`（未启用）。
- `select_voice_leg(profile, vowifi_ready, carrier_usb_audio_available)`：双腿编排 —— **先 VoWiFi，失败回退运营商（仅当 USB-Audio 可用）**，否则 `None`。

### 3.9 单元测试

覆盖：MO 呼叫达到 Active、拒绝(486)标记失败、Active 无编解码判定不一致、dry-run 全流程、SDP offer 往返、answer 编解码交集、RTP 往返、AMR 帧类型往返（两种对齐）、双腿选择优先级。

---

## 四、扩展点改动（加性，编译器可校验）

### 4.1 `profiles.rs`

新增 `VoicePolicy` 并加到 `CarrierProfile.voice`：

```rust
pub struct VoicePolicy {
    pub vowifi_enabled: bool,           // VoWiFi 语音腿是否启用
    pub carrier_fallback_enabled: bool, // 运营商回退腿是否允许（运行时再看 USB-Audio）
    pub preferred_codecs: &'static [&'static str], // 首选编解码顺序: amr / amr-wb / pcmu / pcma
    pub amr_octet_align: bool,          // AMR 是否 octet-aligned
    pub ptime_ms: u16,                  // SDP ptime
    pub sip_endpoint_exposed: bool,     // 是否对外暴露标准 SIP endpoint（默认 false）
}
```

- `DEFAULT_VOICE_POLICY`：VoWiFi 开、允许回退、`["amr-wb","amr","pcmu","pcma"]`、`ptime=20`、不暴露 endpoint。
- 6 个内置 profile + 动态生成器 `generate_standard_3gpp_profile` 均填入 `voice: DEFAULT_VOICE_POLICY`。
- `validate_builtin_profiles()` 增加校验：`preferred_codecs` 非空且 token 合法、`ptime_ms != 0`。

**核查点**：`preferred_codecs` 的 token 校验（`amr`/`amr-wb`/`pcmu`/`pcma`）与 `voice.rs` 的 `AudioCodec::from_token` 一致。

### 4.2 `diagnostics.rs`

- `VowifiReadiness` 结构体与 `Default` 均新增 `voice_ready: bool`。
- 事件显示映射新增 `voice_binding` / `voice_ready` / `voice_binding_failed` 中文文案（非穷尽 match，带 `_` 兜底）。

### 4.3 `executor.rs`

- `ExecutorStage` 新增 `Voice` 变体；`as_str()`→`"voice"`，`component()`→`"voice_over_ims"`。
- `EXECUTOR_STAGES` 常量数组追加 `Voice`。
- `RuntimeExecutorReport` 新增 `voice_dry_run: Option<VoiceRuntimePublicState>`；三个执行器构造（Noop/DryRun/Live）均已填。
- `DryRunRuntimeExecutor::build_voice_snapshot()` 调用 `voice::build_dry_run_voice_snapshot`。
- `dry_run_stage_enabled` / `dry_run_stage_reason` / `dry_run_stage_run_reason` / `soak_observation_for_stage` / `readiness_key_for_stage`（→`"voice_ready"`）均已覆盖 Voice。
- 相关测试列表已加入 `(ExecutorStage::Voice, "voice_ready")`。

### 4.4 `runtime.rs`

- `RuntimeLiveReadiness` 新增 `voice_ready`；`normalize_protocol_prerequisites()` 中 `voice_ready` 回填全部前置阶段。
- `apply_stage_result` 新增 `ExecutorStage::Voice` 分支（完成时回填 sim_auth/epdg/ike/child_sa/esp/ims 前置）。
- `readiness()` 输出 `voice_ready`。
- `live_refresh_stages_for` 在 `Sms` 之后追加 `Voice`（即 voice 位于就绪阶梯末端，依赖 ims_registered）。
- 连接过程的 start/success/failed 三处事件日志 match 均加入 Voice 映射（`voice_binding` / `voice_ready` / `voice_binding_failed`）。

### 4.5 `flow.rs`

- `RuntimeStage` 新增 `VoiceReady`（`as_str()`→`"voice_ready"`）。
- `STEP_DEFINITIONS` 追加 voice 步骤（component `voice_over_ims`，readiness_key `voice_ready`）。
- `highest_ready_stage()` 最高阶段改为优先判定 `voice_ready`。
- 现有 `full_sms_ready_flow_reports_userspace_esp` 测试补上 `voice_ready: true`（因此 UI 最高阶段变为 voice_ready，测试断言已相应更新）。

### 4.6 `live.rs`

新增常量 `LIVE_VOICE_INVITE_TOTAL_TIMEOUT`（32s）、`LIVE_VOICE_MMTEL_ICSI`。新增：

- `LiveCallResult` / `LiveCallFollowupFrame`：同步结果 + 异步 dialog/媒体进度通道（仿 `LiveSmsSendResult`）。
- `run_live_voice_until(profile)`：Voice 阶段就绪检查（校验至少一条腿启用 + IMS 已注册 + 状态机一致）。
- `place_live_voice_call(callee)`：对外入口。解析 SIM 身份/profile → 确保 IMS 注册 → 取 ESP TCP 路由 → 生成 SDP offer → 发 INVITE。总超时保护，超时清会话。
- `place_live_voice_call_for_profile(...)`：装配 offer、request-URI、`MoCallInvite`。
- `send_live_invite(...)`：复用现有 ESP 隧道发送 INVITE，循环读 1xx（180→Ringing，183→EarlyMedia），收到 >=200 终态；2xx 则解析 SDP answer 协商编解码并发送 ACK，非 2xx 标记失败。
- `start_live_call_followup_task(...)`：先上报同步结果，再对已接通的呼叫在媒体窗口内监听 in-dialog BYE，正常终止后上报 ended。**RTP 媒体循环在此预留**（绑定音频后端后即在此驱动，当前为静音）。
- `build_live_invite_request(...)` / `build_live_ack_request(...)`：仿 `build_live_sms_message_request`，但 `Content-Type: application/sdp`，并带 MMTel ICSI（`Contact`/`Accept-Contact`/`P-Asserted-Service`）、`Supported: 100rel,timer`、`Allow` 等语音相关头。
- `SystemLiveDatagramAdapter` / `LiveNetworkStageAdapter` 的 stage 分发、`live_stage_implemented`、`stage_requires_live_network` 均已加入 `Voice`（注意：`stage_requires_device_change` **不含** Voice）。
- 顶部 `use super::{... voice, ...}`。

**核查点**：INVITE/ACK 走的是与 SMS 相同的 ESP-保护 TCP 路由（`gateway.ims_client_tcp_route()`），复用了 SIP wire helpers（`read_sip_frame_buffered` / `parse_sip_status` / `sip_body` / `build_sip_ok_response_for_request` / `write_sip_frame` / `connect_tcp_from_inner` / `sip_host` / `hex_token`）与身份/Security-Verify 缓存。

### 4.7 `handlers.rs` / `models.rs` / `main.rs`

- `models.rs`：`PlaceCallRequest { phone_number: String }`。
- `handlers.rs`：
  - `place_call_handler`（`POST /api/voice/call`）：校验 VoWiFi 已启用 → 检查/推进 `voice_ready` → 调 `place_live_voice_call` → 返回 call_id/trace_id/call_state/invite_state/negotiated_codec/sip_status，并后台 drain 进度通道。
  - `spawn_vowifi_call_followup`：记录 dialog/媒体进度（仿 `spawn_vowifi_sms_followup_persist`）。
  - import 增加 `place_live_voice_call`。
- `main.rs`：注册路由 `.route("/api/voice/call", post(place_call_handler).options(options_handler))`。

---

## 五、请求/响应示例

```
POST /api/voice/call
Content-Type: application/json

{ "phone_number": "+8613800138000" }
```

成功响应（`data` 字段）：

```json
{
  "path": "vowifi_ims",
  "transport": "vowifi_ims",
  "call_id": "....@simadmin",
  "trace_id": "voice-mo-....",
  "call_state": "active",
  "invite_state": "confirmed",
  "negotiated_codec": "amr",
  "sip_status": 200,
  "media_followup": "background"
}
```

---

## 六、就绪阶梯（readiness ladder）

```
identity → profile → sim_auth → epdg → ike → child_sa → esp → ims_registered → sms_ready → voice_ready
```

`voice_ready` 位于末端，依赖 `ims_registered`。SMS 与 Voice 是 IMS 注册之上的两个并列能力（编排时先 SMS 后 Voice）。

---

## 七、安全说明（需你决策）

- `POST /api/voice/call` 目前**沿用 SMS 端点的开放模式**：仅受 VoWiFi 功能开关（`feature_enabled && connection_enabled`）保护，端点层没有额外的认证/授权。若该端点会暴露到网络，**建议显式增加鉴权**，不要照搬 SMS 的开放模式。
- 运营商语音腿（AT + USB-Audio）与对外 SIP endpoint 目前都是**预留 trait，未启用**，不会在无 USB-Audio 时误开腿。

---

## 八、依赖与编译环境说明（重要）

1. **编译验证结论：已通过**。使用 Windows 的 GNU 工具链（`stable-x86_64-pc-windows-gnu`，rustc 1.97.0）配合本机 MinGW-W64 GCC 16.1.0（位于 `D:\Program\Dev\Languages\GCC\mingw64`）完成本机目标编译。GNU 版 Rust 自带链接器，MinGW GCC 负责编译需要 C 工具链的依赖（`rusqlite` 内置的 SQLite、`ring` 的 C/汇编），因此**不再需要 MSVC/`link.exe`**。

   > 注：早前"未能编译"是因为当时只有 MSVC 版工具链且缺 `link.exe`。切换到 GNU 版 Rust + MinGW GCC 后此问题消失，与本次代码改动无关。

2. `Cargo.lock` 已重新生成并锁定（约 258 包），在 rustc 1.97 下正常解析。

3. **本机复现验证命令**（在 `backend/` 目录下，需确保 MinGW `bin` 在 `PATH` 中，并设置 `RUSTUP_HOME` / `CARGO_HOME` 指向对应目录）：

   ```
   cargo check          # 本机目标编译检查
   cargo test --bin simadmin vowifi::voice   # 运行 voice 模块单元测试
   ```

   注意：本 crate 是纯二进制包（无 lib target），`cargo test -p simadmin` 会报 "no library targets"，需用 `--bin simadmin`。

4. **本机 vs 生产目标的区别**：
   - 本机（`x86_64-pc-windows-gnu`）可完成 **编译检查 + 纯逻辑单元测试**，足以验证本次语音改造的核心逻辑正确性。
   - 生产目标（`aarch64-unknown-linux-musl`，见 `.cargo/config.toml`）用于 Debian ARM 蜂窝设备，需在带交叉工具链的 Linux 构建机上产出最终可部署二进制。`live.rs` 的真实网络 I/O、D-Bus（zbus）、TUN 网关等 Linux 专属能力只能在真设备/Linux 环境运行，但这些**不影响** voice 模块纯逻辑层的单测。

---

## 八·补、测试验证记录

本次改造遵循"纯状态机/编解码器（离线可单测） 与 `live.rs` 真实 I/O 分离"的设计范式，因此**接打电话的核心语音逻辑无需 Linux 即可在本机完整单测验证**。

### 验证过程（三步）

1. **环境配对**：GNU 版 Rust 工具链（自带链接器） + MinGW-W64 GCC 16.1.0（编译 `rusqlite` / `ring` 的 C 代码）。
2. **`cargo check`**：258 个依赖 + 项目本体全部通过，仅 1 处无害警告（`live.rs:1945` 未使用变量 `profile`）。类型、导入、`match ExecutorStage` 穷尽性、各结构体字面量字段均由编译器校验通过。
3. **单元测试**：`cargo test --bin simadmin vowifi::voice` —— **10 passed; 0 failed**。

### voice 模块单元测试清单（10 项，全部通过）

测试位于 `backend/src/vowifi/voice.rs` 的 `#[cfg(test)] mod tests`，覆盖呼叫状态机、SDP 协商、RTP/AMR 编解码、双腿选路四大核心语音能力：

| # | 测试函数 | 验证的语音能力 |
|---|----------|----------------|
| 1 | `mo_call_reaches_active_with_negotiated_codec` | **拨出电话完整流程**：注册就绪 → 发 INVITE → 对方振铃(180) → 接通(200 OK) → 协商 AMR 编解码 → 媒体收发。断言最终 `call_state=active`、`api_status=active`、`negotiated_codec=amr`，且状态机一致性校验通过 |
| 2 | `rejected_invite_marks_call_failed` | **拒接/占线处理**：对方返回 SIP 486，呼叫标记 `call_state=failed`、`end_reason=remote_busy`，错误类型为 `SipRejected(486)` |
| 3 | `active_call_without_codec_is_inconsistent` | **状态不变式**：若呼叫号称接通(200)却未协商出编解码，`assert_state_consistency()` 必须报错，防止非法状态 |
| 4 | `dry_run_snapshot_walks_full_call` | **离线全流程演练**：`build_dry_run_voice_snapshot` 走完 注册→INVITE→180→183→200(AMR)→通话→挂断，断言 `call_state=ended`、`negotiated_codec=amr`、`voice_ready=true` |
| 5 | `sdp_offer_round_trips_through_parser` | **SDP offer 往返**：`build_mo_audio_offer` 生成的 SDP 体经 `parse_audio_sdp` 解析后，媒体端口(40000)、连接地址、首选编解码均保持一致 |
| 6 | `sdp_answer_intersects_codecs` | **SDP answer 编解码交集**：对入站 offer 求交集生成 answer，正确保留 offer 方的 payload 编号（AMR=96） |
| 7 | `rtp_packet_round_trips` | **RTP 包编解码**（RFC 3550）：含 marker/sequence/timestamp/ssrc/payload 的 RTP 包 `encode()` 后 `parse()` 原样还原 |
| 8 | `amr_payload_frame_type_round_trips_octet_aligned` | **AMR 帧（octet-aligned 对齐）**打包/解包，帧类型(7)正确还原 |
| 9 | `amr_payload_frame_type_round_trips_bandwidth_efficient` | **AMR 帧（带宽高效对齐）**打包/解包，帧类型(5)正确还原 |
| 10 | `leg_selection_prefers_vowifi_then_carrier` | **双腿选路优先级**：VoWiFi 就绪时选 VoWiFi；VoWiFi 失败时仅在 USB-Audio 可用且允许回退时才选运营商腿，否则 `None` |

> 说明：测试均针对不碰硬件的纯逻辑层，因此在 Windows 本机即可通过。`live.rs` 中真实收发 SIP/RTP、D-Bus 读卡、TUN 转发等属于 Linux/真设备专属，须在目标设备上做集成验证。

---

## 九、后续可继续的工作（本次已留接口）

- 绑定真实媒体后端：在 `start_live_call_followup_task` 的媒体窗口内驱动 RTP 收发循环，使用 `voice::RtpPacket` + `AudioSource`/`AudioSink`。
- 实现 `CarrierVoiceLeg`（AT + USB-Audio PCM）并接入 `select_voice_leg` 的回退路径。
- 实现 `SipEndpointBridge`，对外暴露 per-SIM 标准 SIP endpoint，桥接外部 UA（Asterisk/Linphone）的 INVITE 到内部 VoWiFi/运营商腿。
- 入站呼叫（MT）：在 dialog loop 中处理网络侧 INVITE，用 `voice::build_sdp_answer` 生成 answer 并回 200 OK。
- 通话记录持久化：参考 SMS 的 `upsert_vowifi_sms_delivery`，为 `vowifi_voice_call` 增加 DB 表与写入。
