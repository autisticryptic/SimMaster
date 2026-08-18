# 未使用函数与符号审计

审计基线：2026-08-18，`cargo check --workspace`。

本轮清理后，Linux 默认 binary 构建无 dead-code / unused warning。审计依据是编译器、`rg` 调用点和全量单元测试；没有把“公共 API 可能被外部插件调用”擅自当成死代码。EC20/EC25/EG25 与 USB 读卡器仍只完成静态审阅，相关兼容 seam 是否需要恢复，留在硬件扩展待办中。

## 本轮已删除

- `connectivity/modems/ims/vowifi/profile_import.rs` 及其 AOSP/IPCC importer：项目没有导入入口，完整 carrier profile 仍由 catalog/profile store 提供。
- `services/orchestrator/listener_election.rs`：没有接入真实 SMS listener，实际监听按线路 readiness 和数据库去重。
- `hardware/devices/transport.rs` 的 `DataTransport`、`VoiceTransport`、`SmsTransport`、`RegistrationTransport` 空 trait：上层使用具体 runtime/capability 接口，保留实际 `ImsBearerTransport`。
- `hardware/cellular/qmi_wds.rs` 中未接入生产流程的 retained-CID、旧 IMS session/proxy seam；保留 DATA6 实际使用的 settings/packet handle 解析。
- QCM410 旧的未调用 secondary-QMI helper，以及仅有测试覆盖的 `QmiOpenMode::Proxy`。
- 未接入的 E911 `ssrf_error`/`first_public_ip` 测试辅助、旧 TS.43 XML 兼容解析入口、未使用 setter 和重复 re-export。
- 仅测试使用的 SIM/SMSC AT 解析器、CGCONTRDP/QMI 状态便捷判断改为 `#[cfg(test)]`，不进入 Linux 生产 binary。

## 当前保留的扩展点

- E911 provider registry、TS.43 transport、state store：代码已具备安全边界，但尚未有运营商非紧急 provisioning 验收，不能删除或宣称完成。
- `hardware/cellular/modem_manager.rs` 的 SIM/SMSC AT 兼容测试解析器：未来 EC20/EC25/EG25 可能需要，当前不进入默认运行路径。
- `hardware/devices/qcm410` 的 `ForceQmi`、remoteproc/baseband capability 检查：410 DATA6/IMS 仍依赖，不能按“当前只有一台设备”删除。
- Trunk digest、XCAP、VoWiFi SOCKS/TUN 的实际 runtime 方法：即使某个配置没有在本机启用，也有生产调用或协议测试覆盖。

## 验证

- `cargo fmt --all -- --check` 通过。
- `cargo check --workspace` 通过且无 warning。
- `cargo test --workspace --no-fail-fast`：1041 passed、1 ignored（需要外部 Asterisk/Linphone）、0 failed。
