# Native SIM/APDU 通道账本

`GET /api/modem/backend` 的每个 native device 增加 `sim_channels`，仅暴露用途、slot、
本项目确认打开的 client/channel ID、状态、计数和是否需要核对。**不记录 AID、APDU、
IMSI/ICCID、RAND/AUTN、AKA 或会话密钥。**

- `capacity=null` 表示未有设备证据，不能用当前分配数量冒充卡的最大逻辑通道容量。
- 原有物理操作门仍保护完整 open/APDU/close，eSIM 与 IMS AKA 不可交错。
- AT CCHO/CGLA/CCHC 在通道分配前创建 `session-<line>-sim.json`；QMI UIM 从 **CTL client
  分配之前**开始记录，覆盖 client/open/APDU/close/release 全过程，不轮询猜别人的通道。
- 明确的 QMI 协议拒绝可清理 open-pending；超时、损坏回复或未知分配结果保留 receipt。
- AT 本次通道确认关闭才移除 receipt；QMI 的 channel close 只更新通道状态，**CTL client
  release 成功之后**才清除。拒绝把 client 仍在使用的记录当作已释放。
  Drop 不盲发 CCHC，不以退出作用域冒充成功；已知 channel ID 落盘失败仍尝试关闭已知通道。
- QMI native open/close 核对 service/client/transaction/message，避免异步 indication
  被误认作已关闭确认；MM 路径不使用 native 账本。
- lpac 具有同一个物理门，并显示 `purpose=esim` 的外部操作范围。外部 helper 的通道 ID
  不可见，因此标记 `external_channels_unknown=true`，不虚构 open/close 数量；正常完成
  释放范围，错误/超时留下待核对记录。
- 未解决的 SIM receipt 在本进程内阻止后续 SIM 分配和专项维护；新进程 native/MM
  启动也拒绝接管，包含写入中留下的 `.PID.tmp` 文件。不自动删除孤儿记录。

新通用 receipt 使用 `/var/lib/simadmin/native-control` 的持久化 schema-2 envelope，带原 owner
实例和控制代次；`/run` 保留 flock，并继续识别旧记录。确认清理后先持久化终态再删除。
[显式恢复 CLI](NATIVE_RESOURCE_RECOVERY.md) 可以处理“已清理、删除账本前退出”的遗留，
不重放旧 CID，不把外部 helper 退出或设备重插本身当作通道释放证明。

这是所有权与诊断增强，不是所有型号的通道泄漏自动修复器。native 卡类型、通道容量、
拔插、lpac 固件差异及未知资源的核对仍需独立硬件验收。
