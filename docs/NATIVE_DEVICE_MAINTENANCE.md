# Native 设备专项维护

默认后端仍是 MM。本页 API **仅**对已经显式配置并由 native 独占的线路生效；不停止 MM、
不切 owner、不猜端口，也不会由注册重试或设备发现自动调用。没有在 SIM-04 执行这些写操作。

## Quectel EC20 / EC25 / EG25

认证后的接口（`line_id` 必须来自现有 native 线路）：

- `GET /api/modem/lines/{line_id}/native/quectel/diagnostics`
- `POST /api/modem/lines/{line_id}/native/quectel/plan`
- `POST /api/modem/lines/{line_id}/native/quectel/apply`

诊断通过已确认 AT 口读取 CGMM、QCFG ims/usbnet/usbcfg、QMBNCFG AutoSel/List。
缺失/拒绝字段明确报告，未知型号不发送 Quectel 设置。这里的“只读”指不改变 modem
配置，并非 `discover-native` 那种完全不打开设备的 sysfs 扫描。

先关闭该线路 IMS/data intent、释放承载及通话，再请求计划。例如：

```json
{"action":{"kind":"set_ims","mode":0}}
```

动作范围：`set_ims` 的 0/1/2（MBN 默认/启用/禁用）、`set_usb_network` 的 0/1
（EC2x QMI/RMNET / ECM）、`select_mbn` 的 **List 中精确名称**、独立的 `reboot`。
不按 HPLMN 猜 MBN，不将关闭基带 IMS 当作用户态 IMS 必需步骤。

计划返回读取到的状态和绑定“线路＋动作＋状态”的 `expected_revision`。执行请求必须
带同一 `action`、该 revision 和 `confirm_line_id`。执行时在同一物理门内重新核对状态，
拒绝过期计划、有活动 bearer、未确认空闲通话或类型不匹配的设备。

- 写入前保存 `session-<line>-maintenance.json` receipt；HTTP 取消不会中断物理事务。
- 模式/MBN 设置写后回读；MBN 必须来自设备清单，先显式关闭 AutoSel，再设置选中项。
- `verified_setting` 仅表示设置回读一致，**不表示重启、重新枚举、驻网或 IMS 成功**。
  部分固件需重启/激活配置，必须再次明确请求，不自动执行 `CFUN`。
- `unconfirmed` / `reboot_requested` 保留 receipt，禁止本进程再复用旧控制代次；
  不进行逆向回写或自动重试。新 native/MM 启动也会被未解决 receipt 阻止。
- 维护未确认时，应由操作者核实设备复位、物理路径/端口映射、实际设置后处理 receipt，
  不能以删除 receipt 当作故障修复。自动孤儿恢复不是本接口的功能。
- 不写 USB VID/PID、APN、初始 EPS 或 NV，不提供任意 AT 命令入口。

代码回归使用注入式 IO，检查计划过期、跨线路确认、活动承载拒绝、写后回读、超时保留
receipt 与后续 IO 禁止；Rust 仅在 Actions 运行。真实 Quectel 固件行为仍需独立设备验收。
