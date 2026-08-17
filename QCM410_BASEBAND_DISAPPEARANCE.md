# QCM410 基带偶发消失诊断记录

设备：410（jz02v10 4G Modem Stick），采集时间：2026-08-18，地址 `192.168.100.13`。

## 已确认的现象

设备当前的基带 DSP 是 `4080000.remoteproc`，Wi‑Fi/BT 协处理器是 `a204000.remoteproc`。当前采集时两者都为 `running`，`mmcli -L` 能看到 `/org/freedesktop/ModemManager1/Modem/0`。

跨越最近多个 systemd boot 的内核日志显示：

```text
qcom-q6v5-mss 4080000.remoteproc: fatal error received: smd_dsm_memcpy.c:297:
remoteproc ... crash detected ... type fatal error
remoteproc ... recovering 4080000.remoteproc
wwan0at0/at1/qmi0/at2 disconnected
```

这不是偶发一次：多个 boot 都出现同一 `smd_dsm_memcpy.c:297`；另一次在建立 IMS bearer 后出现：

```text
fatal error received: dhcp_client_mgr.c:263:
ModemManager bearer connected -> disconnecting
WWAN ports disconnected
```

因此“基带消失、重启 ModemManager 仍找不到”是合理结果：ModemManager 只是上层观察者，真正的 Q6 modem subsystem 已经 crash/recover，WWAN/QMI 设备节点在 remoteproc 恢复期间不存在或尚未重新绑定。单独重启 ModemManager 无法修复内核/固件状态。

## 与 SimAdmin 的关系

- 多次启动的第一次 fatal 发生在 SimAdmin IMS 操作之前，不能归因于 SimAdmin。
- `dhcp_client_mgr.c:263` 那次与 IMS bearer 建立后立即释放在时间上重合，说明 QCM410 固件对 PDP/WDS 快速 teardown 存在竞态。
- 代码已串行化每条线路的 bearer 操作，并拒绝在 baseband-wedged 错误上重复激活；本轮又把“所有 P-CSCF 失败”纳入 3 秒 failed-bearer 保留窗口，减少立即断开触发固件崩溃的概率。这是通用的时序缓解，不是针对某张 SIM 的硬编码修复。

## 建议的现场恢复顺序

1. `cat /sys/class/remoteproc/remoteproc*/name` 与 `state`，确认 `4080000.remoteproc` 是否 `running`。
2. 检查 `/dev/wwan0qmi0`、`/dev/wwan0at*` 和 `mmcli -L`；等待内核 remoteproc 自动恢复并重新发布端口。
3. 端口全部回来后再刷新 ModemManager/SimAdmin 线路 inventory；不要在端口消失时反复执行 `mmcli --enable`、IMS bearer activate 或 DATA6 qmicli。
4. 若 remoteproc 长时间不是 `running`、端口不再出现，只能执行设备级重启或厂商提供的 modem subsystem recovery；进程级重启不够。

## 后续可实现的自动化

- 增加只读健康快照：remoteproc name/state、端口列表、ModemManager object、最近一次 kernel crash 时间和原因。
- 增加按 baseband 归属的恢复状态机：`observed_crash → wait_ports → refresh_mm → rebind_data6 → restore_intents`，每步限次并写入 system event。
- 将 `4080000.remoteproc` 的 crash reason 与当前 QMI correlation id 写入数据库，区分启动固件故障、IMS teardown 竞态和 DATA6 操作故障。
- 优先升级厂商 modem firmware/kernel；若升级后仍出现 `smd_dsm_memcpy.c:297`，应向设备供应商提供上述 boot 日志和 firmware 版本。

本次没有直接写入 remoteproc `state` 或执行设备重启，避免在有活动线路时造成不可逆的无线中断。

