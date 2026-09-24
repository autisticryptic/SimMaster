# Native SIM/APDU 通道账本

`GET /api/modem/backend` 的每个 native device 增加 `sim_channels`，仅暴露用途、slot、
本项目确认打开的 client/channel ID、状态、计数和是否需要核对。**不记录 AID、APDU、
IMSI/ICCID、RAND/AUTN、AKA 或会话密钥。**

- `capacity=null` 表示未有设备证据，不能用当前分配数量冒充卡的最大逻辑通道容量。
- 原有物理操作门仍保护完整 open/APDU/close，eSIM 与 IMS AKA 不可交错。
- AT CCHO/CGLA/CCHC 与 QMI UIM logical-channel 都在分配前创建
  `session-<line>-sim.json`；确认打开后记录通道，不通过轮询去猜别人的通道。
- 明确的 QMI 协议拒绝可清理 open-pending；超时、损坏回复或未知分配结果保留 receipt。
- 只有本次通道已确认关闭才移除 receipt；Drop 不盲发 CCHC，不以退出作用域冒充释放成功。
  原有失败路径仍尝试关闭已确认的通道；已知 channel ID 的落盘更新失败也不会跳过关闭。
- QMI native open/close 核对 service/client/transaction/message，避免异步 indication
  被误认作已关闭确认；MM 路径不使用 native 账本。
- lpac 具有同一个物理门，并显示 `purpose=esim` 的外部操作范围。外部 helper 的通道 ID
  不可见，因此标记 `external_channels_unknown=true`，不虚构 open/close 数量；正常完成
  释放范围，错误/超时留下待核对记录。
- 未解决的 SIM receipt 在本进程内阻止后续 SIM 分配和专项维护；新进程 native/MM
  启动也拒绝接管，包含写入中留下的 `.PID.tmp` 文件。不自动删除孤儿记录。

这是所有权与诊断增强，不是通道泄漏自动修复器。操作者核对真实设备/卡代次和资源后
才能恢复；native 卡类型、通道容量、拔插和 lpac 固件差异仍需独立硬件验收。
