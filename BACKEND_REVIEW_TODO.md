# 后端审阅与待办

## 本轮已完成

- [x] `hardware::devices::detect_device_kind()` 现在会检查 410 的 `4080000.remoteproc`，不会把相邻的 `a204000.remoteproc`（Wi‑Fi/BT）误判为基带。
- [x] VoLTE 在所有 P-CSCF 候选失败时，短暂保留已建立 bearer，再延迟释放，规避 QCM410 `dhcp_client_mgr`/`smd_dsm` 的建立后立即 teardown 竞态。
- [x] 概览页把 SIM、网络、VoWiFi 状态拆成独立请求；网络/IMS 每 5 秒刷新，连接完成后不会继续显示旧的 `starting`。
- [x] 射频模式、小区锁定和频段选择合并为一个“小区、射频与频段”控制区。

## 高优先级

- [ ] **VoWiFi MT call API**：`backend/src/api/handlers.rs` 的 `answer_call_on_line` 对 `vowifi:*` 仍返回 `VoWiFi incoming call answering is not exposed by this API`。底层 operator 已支持 `AcceptCall`，但 HTTP API 没有 SDP/media offer 和 trunk 会话上下文。应复用 `VoiceAccessRouter` 的 trunk 事件通道，或新增带 SDP 的 answer endpoint；不能用空 SDP 强行接听。
- [ ] **统一 API 拨号路径**：`start_call_for_automation` 在有 ModemManager modem path 时直接调用 `make_call_on_modem`，因此普通 410 线路的网页/自动化拨号绕过 VoWiFi→VoLTE IMS 路由。应构造带线路 profile codec 的 `VoiceCallPlan`，交给 `LineRuntime.voice_access.start_call_plan`，CS 只保留为明确的兼容入口。
- [ ] **MT IMS listener 启动时机**：当前 `ensure_vowifi_mt_listener` 由 `VowifiScope::resolve` 惰性启动。自动恢复通常会触发它，但服务启动到第一次恢复之间存在窗口；建议在线路 registry 建立后为启用 VoWiFi 的线路显式启动 listener，并在线路移除时取消。
- [ ] **VoLTE/VoWiFi 接收路径可观测性**：记录运营商下发的传输类型（IMS `MESSAGE`、MM `Messaging.Added`）和最终去重结果，UI 显示“未收到 IMS MT”与“CS fallback 收到”两个不同原因。用户开关只控制发送强制 VoWiFi，不应伪造运营商侧 MT 路由。
- [ ] **QCM410 恢复监督器**：检测 `4080000.remoteproc` state、WWAN 端口和 ModemManager modem 三者不一致时，先等待内核 remoteproc 自动恢复，再按 baseband 归属重建 DATA6/IMS；禁止进程级盲目重启 ModemManager 或跨线路复用 QMI endpoint。

## 中优先级

- [ ] E911 TS.43 provider 尚未连入 VoWiFi 注册/紧急呼叫流程（`carrier_catalog_v7.rs` 仍有 TODO）。完成前 UI 应明确标注“未实现”，不要声称支持紧急呼叫。
- [ ] 清理旧 `qmi_wds`、AOSP/IPCC profile importer 与未使用 trait；见 `UNUSED_FUNCTIONS_AUDIT.md`，先 feature gate 再删除。
- [ ] 把 `services::orchestrator::listener_election` 接入实际 SMS listener，或删除这套未使用的纯决策层。目前实际接收逻辑固定保留 CS fallback 并靠数据库去重。
- [ ] 为多基带增加端到端线路隔离测试：每个 QMI/AT/PCSC endpoint 必须通过 sysfs remoteproc/USB parent 归属验证，拒绝“取第一个 modem”的回退。
- [ ] 对 bearer 操作、DATA6 初始化、ModemManager hotplug 增加结构化 correlation id，方便将 firmware crash 与具体 QMI 操作关联。

## 验证要求

- Rust：`cargo fmt --manifest-path backend/Cargo.toml -- --check`、`cargo check`、`cargo test`。
- 前端：`pnpm --dir frontend lint`、`pnpm --dir frontend type-check`；完整构建交给 GitHub Actions。
- 410：先仅查看 `/sys/class/remoteproc`、`mmcli -L`、WWAN 端口和日志；只有确认 remoteproc 已恢复且无活动通话时才执行线路级恢复。EC20/读卡器本轮不做实机验证。

