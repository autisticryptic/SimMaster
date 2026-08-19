# VoWiFi 通话实机测试记录（2026-08-19）

## 测试范围与环境

- 设备：QCM410，线路 `line-50ad5391cd09c09936f1081bd479139c`。
- SIM/PLMN：Hotlink/Maxis `50212`，profile `profile-maxis-my-base-50212-12150a2817`，来源为数据库。
- 测试条件：飞行模式开启，基带状态 `disabled`，VoLTE 关闭，VoWiFi `voice_ready` 且 IMS 已注册。
- SIP：SimAdmin `192.168.100.13:5080`，WSL Asterisk `192.168.100.5:8060`，Linphone 账号 `7201`。
- 授权被叫号码：`+601112023012`。

## 已验证结果

1. Linphone `7201` 通过 TCP 注册到 Asterisk，Asterisk 与 SimAdmin trunk 均保持注册。
2. 直接使用 Anonymous From 的 Asterisk INVITE 被 SimAdmin 以 `403 trunk_outgoing_binding_mismatch` 拒绝，证明 outgoing binding 不能被绕过。
3. 使用 `7201` 身份发出的 INVITE 通过线路绑定校验并进入 VoWiFi IMS；Asterisk 收到 `100 Trying` 和带 PCMU SDP 的 `183 Session Progress`。
4. 运营商随后返回 `480 Temporarily Unavailable`，原始 Warning 为 `Release Call received from CAP`。项目将其记录为：
   - `failure_code=carrier_service_control_release`
   - `failure_category=carrier_policy`
   - `failure_retryable=false`
5. 失败后 Asterisk 为 0 channel/0 call，SimAdmin 为 0 active call/0 active dialog/0 active media relay，未发现 SIP 或 RTP 资源泄漏。
6. 本地 Linphone 来电振铃和取消清理正常；未将这项本地 SIP 行为误记为运营商 VoWiFi 来电验收。
7. 部署提交 `8a3c808` 后再次通过 HTTP API 单次外呼：历史记录由 3 条增至 4 条，仅新增 `id=4`，约 1 秒内以 `480 / carrier_service_control_release / carrier_policy / retryable=false` 完整结束，`carrier_reason` 为 `Release Call received from CAP`。10 次逐秒轮询中活动呼叫均为 0，未再产生重复记录或悬空 `dialing` 记录。

## 本轮发现并修复的代码问题

统一语音路由器此前没有发布 `StartCall` 的线路级生命周期事件：

- Asterisk trunk 发起的呼叫无法稳定创建线路通话记录。
- HTTP/API 发起呼叫时，快速的 provisional/final 响应可能早于历史记录创建，导致失败呼叫偶发残留为 `dialing`。

修复后，路由器在后端接受 `StartCall` 后立即发布携带 caller/callee 的 `OperatorEvent::Started`。线路监听器以 `line_id + call_id` 创建或复用记录，后续 `Rejected`、`Unavailable`、`Ended`、`Cancelled` 统一结束记录。Asterisk bridge 将该事件作为元数据 no-op，不改变 SIP 对话或 VoWiFi/VoLTE 选路。

新增测试覆盖：

- trunk `StartCall` 发布 caller/callee 元数据；
- HTTP/本地 call plan 发布相同元数据；
- bridge 在没有 Asterisk dialog 时安全忽略 `Started`。

## 尚未完成的真实网络测试

本次运营商在被叫振铃/接通前执行 CAP 释放，因此以下项目不能伪报为通过：

- VoWiFi 外呼 200 OK、被叫接听、远端拒接、未接和正常 BYE。
- 双向 RTP、测试音频注入、SIP INFO/RFC 4733 DTMF。
- hold/resume、失败 re-INVITE 后保留原媒体、音频与视频切换。
- H.264 视频 RTP、拒绝视频升级后保留语音、VoWiFi 视频和 ViLTE 实网互操作。
- 真实运营商来电、接听、拒接和未接记录。
- 多线路、EC20/EC25/EG25/EG600 与 USB SIM 读卡器实机矩阵（缺少硬件）。

## 下一次复测门槛

需先确认该 SIM 的 VoWiFi 语音业务、预付费余额/资费和 CAMEL O-CSI/CAP 配置允许外呼。获得运营商 `180/183 -> 200 OK` 后，再按以下顺序继续：

1. 语音接通、PCMU/PCMA/AMR 协商和双向 RTP。
2. 音频注入与 DTMF。
3. BYE、拒接、未接与历史记录。
4. hold/resume 和失败 re-INVITE。
5. H.264 视频、音视频切换和拒绝升级。
