# 阶段 D5 开发总结：Asterisk SIP 对话桥接控制面

> 日期：2026-07-16
> 分支：`codex/ims-core-stage-a-b-live`
> 本阶段边界：完成每线路 Trunk 的 SIP UAS/UAC 对话标识、INVITE 事务状态机和事件驱动桥接控制面；真实 IMS INVITE、RTP socket relay、VoLTE/ViLTE 拨号仍需后续阶段接入。

## 一、已完成内容

### 1. 独立 SIP Dialog 与事务状态

- 新增 `backend/src/trunk/dialog.rs`，为 Asterisk 腿维护独立的 Call-ID、From/To tag、INVITE CSeq、远端目标和事务状态。
- 明确区分 `AsteriskOriginated` 与 `OperatorOriginated`，避免把运营商 IMS 的 Call-ID/tag/CSeq 直接复用到 Asterisk 对话。
- 状态迁移覆盖 Proceeding → AcceptedAwaitingAck → Confirmed → Terminated/Failed。
- 对 ACK、CANCEL、BYE 做状态门禁；在错误阶段不会伪造成功。

### 2. SIP 报文构造

- `trunk/sip.rs` 新增带稳定 To-tag、额外头和 SDP body 的响应构造器。
- 新增对话内请求构造器，支持 Contact、CSeq、Call-ID、To-tag 和 `Content-Length`。
- 新增 CANCEL 和 2xx INVITE ACK 构造器；ACK/CANCEL 复用原始 INVITE 的事务身份。
- 所有带 SDP 的响应/请求都按二进制 body 长度写入 `Content-Length`，不把 SDP 当作 Unicode 文本重新编码。

### 3. 事件驱动 TrunkBridge

- 新增 `backend/src/trunk/bridge.rs`。
- Asterisk INVITE 进入桥接表后立即返回 100 Trying，并产生 `OperatorCommand::StartCall`；不在 UDP 收包任务内同步等待 IMS。
- 尚未接入 IMS live voice 时，驱动使用 `OperatorAvailability::Unavailable`，自动返回 480 Temporarily Unavailable；因此不会把“基带未在线”误报为已接通。
- 支持 Asterisk 方向的 CANCEL（200 + 487 + CancelCall）、ACK、BYE（200 + HangupCall）和静态 OPTIONS。
- 事件入口支持运营商侧 180/183、200 + SDP、拒绝、不可用和结束；后续 IMS live 循环只需把事件送回 `handle_operator_event`。
- 预留运营商来电向 Asterisk 发起 UAC INVITE 的 `start_operator_incoming`；目标扩展由每线路 `TrunkProfileConfig.extension` 生成。

### 4. SDP/媒体校验

- 桥接入口复用共享 `ims::voice::parse_audio_sdp` 与 ViLTE `parse_video_sdp`。
- 从 SDP 的连接地址和媒体端口解析音频/视频 RTP endpoint；端口为 0、地址不是 IP 或没有可识别音频 codec 时拒绝。
- ViLTE 的视频 offer 作为独立媒体端点保留，为 D6 的双路 RTP relay 和 `TrunkVideoSeam` 接线提供输入。
- SimAdmin 仍只负责信令映射和 RTP relay，不转码、不承担网页 WebRTC/DTLS-SRTP。

### 5. 通话中数字键（DTMF）桥接

- Asterisk/Linphone 侧优先通过 SDP 协商 `telephone-event/8000`，按 RFC 4733（兼容旧称 RFC 2833）在 RTP 媒体流中传递 `0-9`、`*`、`#`、`A-D`。
- SDP 解析会保存 `telephone-event` 的动态 payload type 与 `fmtp` 事件范围；当 Asterisk 腿与运营商 IMS 腿协商出的 payload type 不同（例如 101↔96）时，relay 只改写 RTP 头部的 7-bit PT 字段，保留 marker、序号、时间戳、SSRC 和 DTMF event payload。
- Asterisk 也可发送通话内 SIP INFO；支持 `application/dtmf-relay` 和 `application/dtmf`，解析后产生 `OperatorCommand::SendDtmf`。合法请求返回 200，非法数字/时长返回 400，未建立对话返回 481，不支持的 Content-Type 返回 415。
- 运营商侧新增通话内 DTMF INFO 构造器，作为 IMS 未协商 RFC 4733 时的兼容回退；数字范围为 `0-9`、`*`、`#`、`A-D`，时长限制 40–5000 ms。
- 不实现带内音频音调检测或重新生成。该方案需要解码 AMR/其他音频并破坏 SimAdmin 的纯 RTP relay 定位；若运营商只接受带内 DTMF，后续应由 Asterisk 的媒体能力处理。
- FreePBX/PJSIP Trunk 建议使用 `dtmf_mode=rfc4733`；SIP INFO 仅作为兼容回退。

## 二、离线验证

- Mock bridge 覆盖：
  - 无 IMS 能力：100 Trying → 480；
  - 事件驱动 200：200 SDP → ACK → BYE；
  - CANCEL：CANCEL 200 + 原 INVITE 487 + CancelCall；
  - 音频 SDP endpoint、`telephone-event/8000`、`fmtp 0-16` 解析及无效请求拒绝；
  - 通话内 SIP INFO 数字键转发、非法 DTMF 拒绝；
  - 运营商 IMS DTMF INFO 构造；
  - RFC 4733 RTP payload type 101↔96 双向改写。
- 后端全量：522 项测试通过。
- `cargo clippy --all-targets -- -D warnings` 通过。

## 三、当前明确未完成项

- `OperatorCommand` 尚未接入 `access/volte/live.rs` 的真实 IMS INVITE/应答事件队列。
- `OperatorCommand::SendDtmf` 与运营商 DTMF INFO 构造器尚未接入真实 IMS voice session；当前只完成控制面、报文和 RTP PT 映射。
- RTP relay 仍复用 `access/volte/rtp_relay.rs` 的纯逻辑骨架，尚未由 Trunk 呼叫建立实际 UDP relay socket。
- 双向 re-INVITE、真实 VoLTE/ViLTE 呼叫、媒体抓包、真机拨号和银行 IVR 数字键验证不在本阶段伪造；需要 D6 完成真实通话桥接后再执行。
- Asterisk 侧 Digest/IP ACL、TLS/SRTP 和 Web 电话仍按后续 D7/D8 Todo 处理。

## 四、下一检查点

1. 将 `OperatorCommand`/`OperatorEvent` 接到每线路 VoLTE live session 的非阻塞事件分发器。
2. 将 `OperatorCommand::SendDtmf` 接到真实 IMS dialog；优先沿 RFC 4733 RTP 事件转发，未协商时生成 SIP INFO。
3. 为每条线路分配音频/视频 relay UDP 端口，并在 SDP answer 生成时写入内部端点，同时应用双方协商出的 `telephone-event` PT 映射。
4. 在高通 410 上仅验证 REGISTER、OPTIONS、INVITE 无 IMS 能力时的 100→480，不执行真实拨号。
5. Trunk 已能稳定桥接后，再由用户安排 Asterisk 6108 的真实语音/视频拨号和银行 IVR 数字键测试。
