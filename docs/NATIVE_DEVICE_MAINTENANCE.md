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

计划返回读取到的状态和绑定“线路＋控制器实例＋动作＋状态”的 `expected_revision`；
控制器重建后旧计划失效。执行请求必须
带同一 `action`、该 revision 和 `confirm_line_id`。执行时在同一物理门内重新核对状态，
拒绝过期计划、有活动 bearer、未确认空闲通话或类型不匹配的设备。

- 写入前在 `/var/lib/simadmin/native-control` 保存 `session-<line>-maintenance.json` receipt，
  带原 owner/控制代次，文件及目录同步；HTTP 取消不会中断物理事务。
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

## DJI 一代 USB 模块（2ca3:4006）

独立维护 CLI，默认只输出 sysfs 计划：

```sh
simadmin dji-prepare --usb-device 1-2
# 下列值必须来自刚才的计划，不能复制别台设备的拓扑或代次：
simadmin dji-prepare --usb-device 1-2 --apply \
  --confirm-usb-device 1-2 --expected-generation <busnum:devnum>
```

这是维护窗口工具，不是开机自动修复器。执行条件：

- Linux，精确 VID/PID、恰好接口 0–4，且当前仅有一台匹配的 DJI 模块。
- MM 必须由操作者事先停止，不存在原生 owner 或未解决 receipt；持有与 native 相同的
  物理锁。其他 AT/读卡/PPP 程序也必须由操作者关闭。
- QMI 接口 4 必须尚未绑定，不能为修复而自动拆掉活动数据面。
- `qmi_wwan` 与 `option` 必须已由操作者加载，工具不自动 modprobe、停服务或改 USB 身份。

执行核验 USB 字符设备及 bus/dev 代次，发送接口 4 的 CDC DTR 控制，注册动态 ID，
绑定接口 4 的 QMI 与 0–3 的串口，逐项回读，最后用有界 `qmicli --dms-get-operating-mode`
检查可读取的 DMS 模式。它**不会**将模式改为 online，也不把 DMS 就绪称作 IMS 注册成功。

注意 `new_id` 是内核驱动级规则，作用到该 VID/PID，通常直到驱动卸载；并非永久单端口规则。
因此拒绝同型号多设备窗口，并显式在计划中披露该副作用。只有本次从未绑定状态新产生的
串口误绑定 QMI 才会被纠正，既有或陌生驱动状态要求人工处理。

中途出错在 `/var/lib/simadmin/native-control` 保留 `session-dji-<usb>-maintenance.json`
和已完成步骤，不自动回滚或重试；旧 `/run` 记录同样阻止接管。
控制节点/代次改变立即停止。此实现只取得代码/fixture 证据，未在 SIM-04 或真实 DJI 硬件执行。

通用 native reset 不能绕过本页显式维护计划。账本检查/终态归档入口及不支持自动清理的
情况见 [资源恢复](NATIVE_RESOURCE_RECOVERY.md)；DJI 专用/未知格式仍需人工核对，不套用通用恢复证明。
