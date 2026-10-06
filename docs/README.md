# 文档导航

项目文档按主题收敛为 **15份受控Markdown**，不再用几十份日期报告重复描述“当前状态”。
用户本地私有eSIM报告不在公开文档集合内，不会随整理提交。

## 先看什么

| 文档 | 用途 |
|---|---|
| [HANDOFF](HANDOFF.md) | 唯一当前交接：用户约束、已确认状态、正在排查的问题、下一步 |
| [开发计划](DEVELOPMENT_PLAN.md) | 剩余能力与验收门槛，不重复已完成流水账 |
| [历史摘要](archive/README.md) | 关键版本/设备/卡/验证边界，以及旧全文的Git和本地备份位置 |

## 使用与开发

| 文档 | 用途 |
|---|---|
| [安装与运行](INSTALL.md) | 同版本校验包、部署/回滚、配置路径、运行环境及服务 |
| [开发指南](DEVELOPER.md) | master工作流、Actions构建/测试、API与产物验证 |
| [架构](ARCHITECTURE.md) | 线路/SIM身份、原工作台、强制namespace、路由域和DNS |
| [设备驱动](DEVICE_DRIVERS.md) | 设备能力边界与新增适配 |
| [原生后端](NATIVE_BACKEND_STATUS.md) | native发现、事件、短信、资源/SIM账本、维护和MEP边界 |
| [更新记录](CHANGELOG.md) | 版本变化及合并后的发布说明 |

## IMS与运营商配置

| 文档 | 用途 |
|---|---|
| [IMS注册与兜底](IMS_REGISTRATION_POLICY.md) | 全局注册、共存、三态字段、续期、资费及兼容契约 |
| [MM资源生命周期](IMS_MM_EXACT_FAMILY_LEASE_DESIGN.md) | SIM校准、临时profile、换卡屏障、归属及恢复事务 |
| [IMS诊断](IMS_DIAGNOSTICS.md) | 分层排查、只读采证、脱敏、失败与未验证边界 |
| [运营商配置](CARRIER_PROFILES.md) | 来源/派生、四源三变体、裁剪证据与发布边界 |
| [QCM410固件故障](QCM410_BAM_DMUX_MODEM_CRASH.md) | 独立硬件问题的因果证据和恢复风险，不当作通用IMS失败解释 |

API请求样例另见[Bruno集合](../bruno-api/README.md)。

## 维护规则

- 新结论更新对应主题文档；HANDOFF保持简短，不追加数百行重复历史。
- 不再新增日期命名的交接/Prompt模板。详细原始证据留Git历史或忽略的`.local`。
- 旧文档在精简前提交`0502395`及本地`docs-before.zip`中保留，可按历史摘要定位。
- 日期、commit、运行程序、SIM和接入必须对应；测试/构建/发布/实网成功分开报告。
- 用户要求：不本机编译；不以运营商特例代码或特例测试替代全局兜底修复。
