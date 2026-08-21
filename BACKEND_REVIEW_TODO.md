# 后端审阅与待办

## 本轮已完成

- [x] `hardware::devices::detect_device_kind()` 现在会检查 410 的 `4080000.remoteproc`，不会把相邻的 `a204000.remoteproc`（Wi‑Fi/BT）误判为基带。
- [x] VoLTE 在所有 P-CSCF 候选失败时，短暂保留已建立 bearer，再延迟释放，规避 QCM410 `dhcp_client_mgr`/`smd_dsm` 的建立后立即 teardown 竞态。
- [x] 概览页把 SIM、网络、VoWiFi 状态拆成独立请求；网络/IMS 每 5 秒刷新，连接完成后不会继续显示旧的 `starting`。
- [x] 射频模式、小区锁定和频段选择合并为一个“小区、射频与频段”控制区。
- [x] 网页、自动化和 Trunk 的普通语音入口统一进入每线路 `VoiceAccessRouter`，IMS 失败且飞行模式关闭时才允许 CS 兼容回退。
- [x] IMS 来电统一使用 `ims:<call_id>`，按线路记录、接听、挂断、DTMF、全部挂断和通话历史；无 Trunk 时提供 180 Ringing 与 loopback RTP answer。
- [x] 路由器记录每通话实际 VoLTE/VoWiFi owner；来电 SDP answer 使用 owner 对应的 carrier codec profile，并正确反转 hold/resume 媒体方向。
- [x] Asterisk 401/407 INVITE challenge 已将 Trunk profile 的 username/secret 接入 bridge，自动生成 ACK 与认证重试 INVITE。
- [x] 删除未接入的 `profile_import`、旧 listener election、旧 QMI WDS seam 和四个空 transport trait；测试辅助解析器改为 test-only。
- [x] 配置维护测试补齐 Windows 文件句柄释放和 SQLite flush 的平台边界。

## 高优先级

- [x] **VoWiFi MT call API**：IMS 来电通过统一路由器保存 offer，HTTP 接听使用带媒体的 `AcceptCall`，不再返回旧的未暴露错误。
- [x] **统一 API 拨号路径**：网页、自动化都构造每 access 的 `VoiceCallPlan`；CS 只在非飞行模式下作为明确兼容入口。
- [x] **MT IMS listener 启动时机**：call monitor 在服务启动后的首个周期为每条线路挂接 listener，后续按原子状态避免重复监听。
- [x] **VoLTE/VoWiFi 接收路径可观测性**：SMS listener 已按线路与 IMS readiness 暂停 CS 扫描并使用跨传输去重；剩余运营商侧路由证据仍需真实网络日志验收。
- [x] **UE 生命周期失效保护**：线路刷新先准备再发布 binding、SIM 映射和 worker/socket context；DATA6/代理拒绝复用已退出或已重建 namespace 的旧 worker，并清理宿主残留网络状态。
- [ ] **QCM410 恢复监督器**：检测 `4080000.remoteproc` state、WWAN 端口和 ModemManager modem 三者不一致时，先等待内核 remoteproc 自动恢复，再按 baseband 归属重建 DATA6/IMS；禁止进程级盲目重启 ModemManager 或跨线路复用 QMI endpoint。

## 中优先级

- [ ] E911 TS.43 provider 尚未连入 VoWiFi 注册/紧急呼叫流程（`carrier_catalog_v7.rs` 仍有 TODO）。完成前 UI 应明确标注“未实现”，不要声称支持紧急呼叫。
- [x] 清理旧 `qmi_wds`、profile importer 与未使用 trait；默认构建已无 dead-code warning，保留的 QMI/AT seam 均有明确测试或未来设备用途。
- [x] 删除未接入的 `services::orchestrator::listener_election`；实际 SMS listener 以线路 readiness 和数据库去重为准。
- [ ] 为多基带增加端到端线路隔离测试：每个 QMI/AT/PCSC endpoint 必须通过 sysfs remoteproc/USB parent 归属验证，拒绝“取第一个 modem”的回退。
- [ ] 对 bearer 操作、DATA6 初始化、ModemManager hotplug 增加结构化 correlation id，方便将 firmware crash 与具体 QMI 操作关联。

## 验证要求

- Rust：`cargo fmt --all -- --check`、`cargo check --workspace`、`cargo test --workspace --no-fail-fast`。
  当前结果：1041 passed、1 ignored（需外部 Asterisk/Linphone），0 failed。
- 前端：`pnpm --dir frontend lint`、`pnpm --dir frontend type-check`；完整构建交给 GitHub Actions。
- 410：先仅查看 `/sys/class/remoteproc`、`mmcli -L`、WWAN 端口和日志；只有确认 remoteproc 已恢复且无活动通话时才执行线路级恢复。EC20/读卡器本轮不做实机验证。
