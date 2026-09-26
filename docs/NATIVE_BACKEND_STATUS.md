# 原生后端当前状态

> 2026-09-26 整合。默认 ModemManager（MM）；native 仍是显式实验选项。
> 本文合并原逻辑候选状态与 9/24 审计的现行结论，原始分阶段证据见 [历史档案](archive/README.md)。
> 代码/CI、只读发现、完整硬件验收是三个不同层次。

## 1. 已完成的代码范围

| 范围 | 实现与证据 |
|---|---|
| 后端选择 | 旧配置默认 MM；native 需要 `mode: native` + `allow_unvalidated_native: true`；未知目标不回退 MM |
| 物理控制与协议 | QMI DMS/NAS/UIM/WDS、MBIM、AT；物理门/flock、端口/代次核验、超时与输出上限 |
| 发现 | `discover-native` 被动扫描 sysfs，只给候选/歧义信息，不打开设备、不猜映射、不启用 native |
| SIM/AKA 与承载 | SIM/APDU、QMI/MBIM 会话、UE worker/netns、明确归还与保留 receipt |
| AT 事件与业务 | 单读者会话、URC 分流、有界广播唤醒通话/驻网权威核对，不广播号码/PDU |
| 短信 | 私有 inbox、持久化后 ACK/删除、SIM 隔离、分片/去重、逐片发送引用和严格送达关联 |
| 生命周期与显式恢复 | 持久化 owner/代次、UIM CTL client 分配到释放、不重放已释放 CID；只归档确认完整清理的 receipt |
| 专项维护 | Quectel 诊断与显式 revision 计划；DJI USB/驱动准备；保护写操作，不自动停 MM 或回滚 |
| API/运维 | `/api/modem/backend` 脱敏状态，按后端选择安装/恢复资源，拒绝不明确 owner |

最终功能检查点为 `302b70e`；Validate `36003900244`、Build `36003900240` success，
双架构成功、Publish skipped。它覆盖此前 `a1be268` 的短信及 `a4a83c2` 的恢复增强。
已有实机只读发现通过，**native 端到端接管仍无验收通过记录**。

## 2. 不能勾成完成的部分

- **native 真机矩阵**：SIM/AKA、IMS/自然续期、IPv6、短信、电话/USSD、掉线/代次故障和长稳；
  需真实 QMI/MBIM/AT 组合，不能由 SIM-04/05 的 MM 反馈或 mock 替代。
- **混合 owner 未实现**：当前全局二选一；native 要求 MM daemon 未运行，应用不自动停止 MM。
  同机不同 modem 分别由 MM/native 管理尚未接通，更不能两个 owner 争同一 modem。
- **未知孤儿资源自动恢复尚不支持**：设备重插/重启、原进程退出、helper 结束不等于固件资源已释放。
  未确认、损坏或旧格式 receipt 仍阻断；没有 `--force`、批量清账或旧 CID/session 重放入口。
- **真实厂商维护待验收**：Quectel 固件、DJI DTR/驱动绑定/重枚举；基带自带 IMS 与用户态 IMS
  是否争用需独立设备验证，不能默认关闭基带 IMS 来猜修复。
- **协议/型号扩展**：AT-only PPP/ECM/NCM 数据面、厂商 RAT/band/reset、多槽/MEP/自动槽位切换等，
  按实际能力适配，未实现返回 unsupported，不泛化为所有硬件可用。
- **短信边界**：旧 MM/IMS 记录缺可靠 SIM 作用域；跨新 native/旧 IMS 的完全统一去重，
  外部通知或 Trunk 的恰好一次崩溃重放不在本轮范围；存储入库不等于通知恰好一次。
- **冷启动/发布**：应用启动后的射频门不证明从上电起零射频。完整 1.1.5/1.1.6 的支持/发布矩阵
  仍按路线图，不能因版本字符串调整就自动通过。

## 3. 不变的使用边界

1. 同一物理 modem 同时一个 owner，覆盖全部端口/槽位；不热切正在注册/续期/通话的会话。
2. `line_id` 是物理槽位，SIM 覆写另有 SIM 作用域；临时 `/Modem/N`、CID、网口编号不是稳定身份。
3. IMS 数据面仍在 per-UE namespace；不能退回宿主网络制造成功。
4. QCA410 主 QMI 承载 IMS、DATA6 供普通数据是该设备契约，不推广到所有设备。
5. 只读发现不验证端口实际可用；缺失/多候选/未知映射须显式确认，不自动选第一个。
6. 维护需独立授权、空闲核对、revision/owner/代次核验、写前意图与写后回读；未知结果保留账本。
7. 用户已取消测试窗口自动回滚；当前不做 SIM-04 native 接管，不自动发短信或拨号。

## 4. 详细契约与参考

- [发现](NATIVE_MODEM_DISCOVERY.md)、[AT/URC](NATIVE_AT_EVENTS.md)、[短信](NATIVE_SMS_INBOX.md)。
- [SIM 通道账本](NATIVE_SIM_CHANNEL_LEDGER.md)、[资源恢复](NATIVE_RESOURCE_RECOVERY.md)、[专项维护](NATIVE_DEVICE_MAINTENANCE.md)。
- [开发总计划](DEVELOPMENT_PLAN.md)、[设备后端路线图](MODEM_BACKEND_ROADMAP_1.1.5_1.1.6.md)。
- [9/24 原始审计](archive/2026-09/NATIVE_BACKEND_AUDIT_2026-09-24.md)：包含五个参考项目的许可证、
  采用方式、提交与 CI 索引。VoCat/EC25Toolbox/DJOneHub 仅借鉴思路，不复制受不同许可约束的实现。
- [早期逻辑候选记录](archive/2026-09/NATIVE_BACKEND_LOGIC_STATUS.md)：保留各历史日期的限制与验证，
  不能将其旧 worktree 或旧 SIM-04 失败状态当作当前结论。
