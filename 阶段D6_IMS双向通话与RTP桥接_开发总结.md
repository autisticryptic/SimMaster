# 阶段 D6：IMS 双向通话与 RTP 桥接开发总结

> 日期：2026-07-17
> 分支：`codex/ims-core-stage-a-b-live`
> 关键检查点：`882b022`（Asterisk→IMS 外呼）、`948c75c`（IMS→Asterisk 6108 来电）
> 结论：双向音频通话的代码与离线 UDP 集成测试已完成；真实拨号因运营商当前未分配 IMS IPv6 前缀而未执行。

## 一、本阶段完成内容

### 1. Asterisk → 运营商 IMS 外呼

- 将 Asterisk INVITE 的 Request-URI 号码规范化为运营商 IMS `sip:number@home-domain;user=phone`。
- 在已注册的受保护 IMS channel 上建立独立 Call-ID、tag、CSeq 和事务 branch。
- 支持 INVITE、可靠 18x/PRACK、200/ACK、CANCEL、BYE、SIP INFO DTMF。
- 初始 INVITE 与对话内 re-INVITE 使用独立事务，不复用错误的 branch/CSeq。
- 无共同 codec 或不可用 SDP 时先完成必要 ACK，再向 IMS 发 BYE，并向 Asterisk 返回 488，避免半开对话。

### 2. 运营商 IMS → Asterisk 6108 来电

- 识别注册 channel 上不属于现有 MO 对话的新 IMS INVITE。
- 从 `P-Asserted-Identity`/`From` 提取主叫，保留 IMS 对话的 Call-ID、远端 tag、Contact 和初始 INVITE。
- 为每个 MT 呼叫分配 IMS 侧与 Trunk 侧两个 Tokio UDP RTP socket，把 Trunk 侧 relay 地址写入发往配置扩展 `6108` 的 SDP。
- `OperatorEvent::Incoming` 驱动 `TrunkBridge` 生成独立 Asterisk UAC INVITE；Asterisk 18x/200/拒绝映射为 IMS provisional/final response。
- Asterisk 200 后自动 ACK Asterisk 腿，并以 IMS relay 地址和运营商 payload type 生成 IMS 200 SDP。
- IMS CANCEL 映射为 Asterisk CANCEL，同时向 IMS 返回 CANCEL 200 + 原 INVITE 487；任一侧 BYE 均会结束另一腿并释放 relay。
- Trunk 任务将已连接 UDP socket 选出的本地 IP 发布给同线路 IMS 任务；未配置来电扩展时不发布，IMS 来电诚实返回 480。
- 无效/无共同 codec 的 Asterisk early media 或 final answer 返回 IMS 488，并同步取消或结束 Asterisk 腿。

### 3. RTP 与 DTMF

- 每个呼叫使用独立的 `PendingRtpRelay`/`ActiveRtpRelay`，不共享媒体端口。
- 两腿 SDP codec 相同但动态 payload type 不同时，只改写 RTP 头部 7-bit PT 字段，保留 marker、序号、时间戳、SSRC 和 payload。
- RFC 4733 `telephone-event` 可双向映射，例如运营商 96 ↔ Asterisk 101。
- Asterisk SIP INFO 可映射为 IMS INFO；未实现带内 DTMF 检测或音频转码，继续由 Asterisk 承担媒体转换。

## 二、离线与构建验证

- 后端全量：537 项测试通过，0 失败。
- 严格质量门：`cargo clippy --all-targets -- -D warnings` 通过。
- 新增集成覆盖：
  - IMS incoming event → Asterisk 6108 INVITE；
  - Asterisk 180 → `ReportProvisional`；
  - Asterisk 200/SDP → ACK + `AcceptCall`；
  - IMS CANCEL → Asterisk CANCEL；
  - MT RTP relay 激活及 `telephone-event` 101→96 改写；
  - Trunk 连接地址跨任务发布与停机清理。
- ARM64 musl 静态候选：9,506,368 bytes。
- 候选 SHA-256：`455EBA6CB295A3862157734A82006EA165AC68DDC8744450E063E1EC242C17CC`。
- 候选启动日志准确标识提交 `948c75c`。

## 三、高通 410 实机结果

### 1. 已通过

- 正式 `simadmin.service` 全程保持 inactive；候选仅监听 `127.0.0.1:3101`。
- 当前候选向真实 FreePBX/PJSIP `10.0.0.3:8060` 完成 Digest REGISTER 200。
- Trunk 本地端口为 5062，服务端有效期 3599 秒，challenge + authenticated 共两次尝试，零重连。
- 整机重启后候选仍能发现稳定 `line_id`，并正确重新绑定 ModemManager modem 0。

### 2. IMS bearer 当前阻塞

此次没有发起真实呼叫，因为 VoLTE runtime 未达到 `registered=true`。受控对照如下：

1. 当前候选先请求 `ipv4v6` IMS bearer，网络明确返回 `Ipv6OnlyAllowed`。
2. 地址族修复 `6639a27` 删除失败 bearer，并按网络指示创建 `ipv6` bearer。
3. IPv6 bearer 随后返回 `MobileEquipment.Unknown: Call failed: ipv6 error: prefix-unavailable`，`connected=no`。
4. 重启 ModemManager 后仍复现。
5. 完整重启高通 410、重新驻网后仍复现。
6. 使用 2026-07-15 在同机真实 REGISTER 200 的历史候选 `cfc34b1` 做回归：它当前同样在 `ipv4v6` bearer 上收到 `Ipv6OnlyAllowed`。

因此，历史成功事实仍成立：`cfc34b1` 曾完成 IMS IPv6 bearer、AKA、XFRM、REGISTER 200 和真实 VoLTE 短信收发；但在本次测试时刻，旧候选也无法连接双栈 bearer，新候选进一步遵从网络提示后又拿不到 IPv6 prefix。现有证据排除 D6 语音代码回归，阻塞位于当前运营商/基带的 IMS IPv6 PDN 分配状态。

### 3. 为什么没有强行拨号

- Trunk REGISTER 200 只代表 Asterisk 腿在线，不代表运营商 IMS 腿在线。
- IMS bearer `connected=no` 时没有专用 IPv6 地址、P-CSCF 路由、XFRM SA/策略和受保护 REGISTER。
- 此时发起拨号只能得到本地失败，不能验证真实 INVITE、RTP、DTMF 或通话释放；因此坚持 `registered=true` 前不拨号。

## 四、清理结果

- 停止并删除 `simadmin-d6-test`、`simadmin-d6-oldtest` 和 `/tmp/simadmin-d6-test`。
- 删除本地临时 SSH askpass 与含凭据配置；凭据未写入 Git 或文档。
- 正式服务 inactive；3101/5062 无监听。
- ModemManager active，IMS bearer 列表为空。
- `wwan0` 无 IMS IPv6 地址；XFRM state/policy 为 0/0。
- CID 2 已停用，`$QCPDPIMSCFGE` 1–16 全部为 `0,0,0`；失败测试产生的 CID 2 PDP 定义已删除。

## 五、下一步

1. 保持一段网络冷却时间后，只重试一次 IMS bearer；先确认 `registered=true`。
2. 注册恢复后由用户从 Asterisk 6108 发起一通可控外呼，验证 180/183/PRACK/200/ACK、双向 RTP、RFC 4733、BYE。
3. 再从运营商侧拨入本 SIM，确认 Asterisk 6108 振铃、接听、双向 RTP、CANCEL/BYE。
4. 真实语音稳定后验证 Asterisk re-INVITE 与银行/客服 IVR 数字键。
5. 双向 operator-originated re-INVITE、ViLTE 视频 relay、TLS/SRTP 与 WebRTC 仍属于 D7/D8，不在本检查点宣称完成。
