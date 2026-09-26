# 历史档案索引

这些文件保留当时的事实、失败、提交与验收范围，**不是当前操作手册**。
当前状态只维护在 [HANDOFF](../HANDOFF.md)；active 功能指南见 [文档导航](../README.md)。
旧 worktree、版本、PID、Cookie、测试回滚或“下一步”不能未经复核直接执行。

## 项目与分支快照

- [旧项目逐卡交接](2026-09/PROJECT_HANDOFF_2026-09-12.md)
- [9/24 主分支整理](2026-09/BRANCH_CONSOLIDATION_2026-09-24.md)
- [beta1–beta3 旧总计划](2026-09/LEGACY_BETA_PLAN.md)
- [历史发布说明](../releases/)

## SIM / IMS 调查与参考

- [SIM-04 后续验收](2026-09/SIM04_CONTINUATION_2026-09-24.md)
- [SIM-04 MM 修复](2026-09/IMS_SIM04_MM_REPAIR_2026-09-17.md)
- [SIM-04 MM 双栈与 exact-family 对照](2026-09/IMS_SIM04_MM_DUAL_STACK_2026-09-18.md)
- [P-CSCF / beta8 对照](2026-09/IMS_PCSCF_BETA8_COMPARISON_2026-09-15.md)
- [beta8 深度派生/注册流程分析](2026-09/IMS_DERIVATION_BETA8_COMPARISON_2026-09-17.md)
- [朋友提供的三网源码审计](2026-09/IMS_REFERENCE_VOLTE_AUDIT_2026-09-21.md)
- [eSIM IMS Profile 历史测试](2026-09/ESIM_IMS_PROFILE_TEST_2026-09-01.md)
- [VoWiFi 历史修复任务](2026-09/VOWIFI_REPAIR_TASK_2026-09-02.md)

## 已合并的阶段文档

现行 native 状态统一在 [NATIVE_BACKEND_STATUS](../NATIVE_BACKEND_STATUS.md)：

- [早期逻辑候选](2026-09/NATIVE_BACKEND_LOGIC_STATUS.md)
- [早期设备窗口](2026-09/NATIVE_BACKEND_DEVICE_VALIDATION_2026-09-13.md)
- [9/24 原生硬件审计及参考项目边界](2026-09/NATIVE_BACKEND_AUDIT_2026-09-24.md)
- [短信/恢复接续计划](2026-09/NATIVE_SMS_AND_RECOVERY_PLAN_2026-09-24.md)

现行命名/兼容约定统一在 [IMS_NAMING_MIGRATION](../IMS_NAMING_MIGRATION.md)：

- [第一阶段原文](2026-09/IMS_NAMING_MIGRATION_PHASE1.md)
- [第二阶段执行计划和 CI](2026-09/IMS_NAMING_PHASE2_PLAN.md)

DNS 的当前实现和已验证范围见 [DNS_HICKORY](../DNS_HICKORY.md)：

- [原 DNS 重构任务](2026-09/DNS_RESOLVER_HICKORY_MIGRATION_TASK.md)

## 本地证据位置变化

- 旧根目录 `.codex-*`、`.tmp-*` → `.local/archive/root/` 下同名条目。
- 旧 `.tmp/` → `.local/archive/legacy-tmp/`。
- 会话 JSONL → `.local/archive/sessions/`。
- 原本地接手文档/私有交接 → `.local/archive/legacy-docs/`。
- 当前只读工具与精简证据分别在 `.local/active/ims/`、`.local/evidence/`。

归档中的命令块和时间点保留历史含义；只修正文档阅读链接，不把旧命令自动改成可执行方案。
指向 `.local/` 的少量历史材料仅在本机存在，不随普通 clone 分发。
整理前完整原文另保留于 `.local/cleanup-2026-09-26/before/`，可按 SHA-256 核对。
