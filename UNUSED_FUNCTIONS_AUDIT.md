# 未使用函数与符号审计

审计基线：2026-08-18，`cargo check --manifest-path backend/Cargo.toml`。
清理确实无用的 VoWiFi TUN 导入和非 Unix 文件权限参数后，编译器还报告 74 个 warning，其中包含未使用导入、函数、方法、常量、结构体和 trait。前端使用 `pnpm --dir frontend lint` 与 `pnpm --dir frontend type-check` 检查通过。`cargo-udeps` 未安装，因此本报告不把“没有文本调用”直接等同于可以删除；尤其是公共 API、测试 seam、平台/feature 条件代码，需要在确认产品范围后再清理。

## 优先确认后可删除的代码

这些符号在当前默认 binary 编译中没有调用方，且没有明显的运行时注册入口：

- `backend/src/connectivity/modems/ims/effective_profile.rs:resolve_effective_common`
- `backend/src/connectivity/modems/ims/vowifi/diagnostics.rs:match_profile_from_identity`
- `backend/src/connectivity/modems/ims/vowifi/profile_import.rs` 中的 `ImportedCarrierFacts`、`plmn`、`to_record`、`parse_aosp_apns`、`parse_aosp_carrier_config`、`parse_ipcc_carrier_plist` 及其 XML 辅助函数
- `backend/src/connectivity/modems/ims/vowifi/profiles.rs:DEFAULT_UT_POLICY`、`publish_database_profiles`
- `backend/src/hardware/devices/qcm410/secondary_qmi.rs:remoteproc_for_primary`、`discover_spare_qmi_ports`、`udev_ignore_rule`
- `backend/src/hardware/cellular/qmi_wds.rs` 中从 `QmiOpenMode`、`WdsEndpoint`、`WdsClient` 到 `start_ims_session`、`start_single_shot_session`、`probe_services`、解析辅助函数的整套旧 WDS seam
- `backend/src/hardware/devices/transport.rs` 中 `DataTransport`、`VoiceTransport`、`SmsTransport`、`RegistrationTransport`（当前上层直接使用具体 live/runtime 类型）
- `backend/src/services/e911/orchestrator.rs:ssrf_error`、`services/e911/ssrf.rs:first_public_ip`，以及未接入产品流程的 TS.43 解析入口

删除前应先确认没有外部插件、集成测试或后续 EC20/读卡器分支依赖；建议先移动到 `legacy/` 或加 feature gate，再做一个 release 周期的构建验证。

## 仅在特定 feature/测试/未来接入时使用

- `vowifi/socks5.rs:Socks5UdpClient::with_max_datagram_bytes`、`relay_addr`：UDP relay 插件启用时可用，当前 UI 禁用了该入口。
- `hardware/cellular/cgcontrdp.rs:CgcontrdpSettings::is_empty`：旧 CGCONTRDP 解析路径使用；切换到 QMI native path 后默认构建未引用。
- `hardware/cellular/modem_manager.rs` 的 `decode_hex`、`parse_crsm_fcp_record_length`、`decode_bcd_digits`、`decode_smsc_from_ts_sca`、`parse_smsc_from_crsm_record`：SIM/SMSC 的 AT 兼容兜底，当前设备走 QMI/ModemManager 属性，换设备时可能重新启用。
- `qcm410/secondary_qmi.rs:QmiOpenMode::Proxy`：当前 410 只走 `ForceQmi`，保留给不同内核的 qmi-proxy 模式。
- `services/trunk/bridge.rs:TrunkBridge::with_digest_credentials`、`services/supplementary/ut.rs:HttpXcapTransport::with_max_response_bytes`：公共构造器，属于可插拔后端 seam。
- `services/e911` 的 provider registry、`E911Operation*`、`E911Secrets`、`E911StateStore`、`Ts43Transport`：E911 尚未接入主流程，不应在功能完成前删除。

## 非函数 warning

- `services/e911/mod.rs` 的 re-export/import 当前只为未接入模块服务；若暂不删除功能，建议改为模块内部引用或加 `#[allow(unused_imports)]` 并在 E911 TODO 完成时移除。

## 建议清理顺序

1. 先清理未使用导入和 `_path`，保持 warning 数量下降且无行为变化。
2. 将 `qmi_wds`、`profile_import`、旧 SMSC AT 解析分别标记为 `legacy` feature，运行默认构建与 410 smoke test。
3. 确认 EC20/EC25/EG25 支持进入开发周期后，再决定是否恢复或彻底删除 Quectel/AT 兼容 seam。
4. 每次删除后执行 `cargo check`、`cargo test`、前端 lint/type-check；发布前补充 `cargo-udeps`。
