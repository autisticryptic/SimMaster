# Native 资源账本与显式恢复

> 接续原待办 #26。默认 MM、实验性 native 的准入条件不变。
> **本功能不是任意 modem 重置后自动恢复；不重放旧 CID、不自动接管设备。**

## 1. 持久化与代次

新 native SIM、QMI/MBIM bearer 和 Quectel 维护 receipt 存放于：

- `/var/lib/simadmin/native-control/session-*.json`：持久化账本，目录 0700、文件 0600。
- `/run/simadmin/native-control/`：仍保留物理/端口 flock；旧版本运行时账本也在这里。

新 DJI 维护意图也改为持久化目录；其专用旧格式仅供人工核对，不伪造通用恢复证据。
备份/迁移 native 测试系统时，应连同 `/var/lib/simadmin/native-control` 保存，不能仅搬应用数据库。

通用 schema-2 envelope 记录：原 owner 的 boot ID、PID、进程起始 tick；物理 key、slot、
canonical sysfs 锚点；所有控制节点的 canonical/sysfs/rdev/inode 与代次摘要；操作载荷及
`cleanup_confirmed`。不记录 IMSI、ICCID、APDU、AKA 材料或 APN 密码。

每次更新核对原 owner/代次；创建、替换与清除均同步文件及父目录。正常释放在删除 receipt
之前，先持久化 `cleanup_confirmed=true`。这样“硬件已释放、但程序在删除账本前退出”
可以恢复，而不需要新进程猜测旧数字 ID 的含义。

MM/native 启动都检查两个目录的全部 pending receipt，包含其它 line、DJI、损坏文件、
`.PID.tmp`；更改 `hardware_key` 或 slot 不能绕开遗留记录。无法归属的记录保守阻断。

## 2. 恢复 CLI

此入口在数据库、native fleet、UE worker、网络命名空间扫尾或服务启动**之前**运行。

```sh
# 默认只列元数据；不打开 modem，也不连接 D-Bus
simadmin native-recovery

# 对一个 schema-2 完整文件生成只读计划；配置必须准确描述该物理线路
simadmin native-recovery \
  --receipt session-<line-id>-sim.json \
  --config /path/to/config.yaml
```

计划绑定 receipt 字节摘要、当前控制代次、原 owner 存活状态、配置物理归属。
若 `eligible=true`，可在独立维护窗口显式执行：

```sh
simadmin native-recovery \
  --receipt session-<line-id>-sim.json \
  --config /path/to/config.yaml \
  --apply \
  --expected-revision <计划返回的完整 revision> \
  --confirm-line-id <计划中的 line_id> \
  --confirm-physical-key <计划中的 physical_key>
```

执行时重新检查计划、取得原/新端口与物理 flock、确认 MM 未运行，再写入 resolution 证据，
精确归档原 receipt 到同目录的 `resolved/`。不覆盖不同内容的归档，不批量删除记录。
执行不会停止 MM、发送 AT/QMI/MBIM、重启模块、移动网口或自动重启 SimAdmin。
之后须另行显式启动服务，取得全新租约。

### 可以处理

- 原进程已确认完整清理，但在 receipt 删除前退出或删除失败。
- 上述记录跨进程、跨控制节点代次或跨宿主重启遗留；旧 owner 已消失、物理配置一致、
  当前端点可核验，且执行者精确确认当前计划。

### 不能处理（保持阻断）

- 分配、关闭、复位结果不明，或者仅凭 USB 重新出现/用户口头“已重启”。
- schema-1/未知格式、损坏/部分文件、无法确认物理目标。
- 尚存 namespace/网口归还义务或未确认的固件会话。
- 外部 helper 仅证明进程结束、不能证明其内部通道清理的遗留记录。
- 仍在运行的原 owner、占用中的 flock、MM owner 或过时的 revision。
- 未反映到控制节点/proxy 生命周期的固件内部复位；本记录的控制代次不是所有型号的固件 boot ID。

这些情况需要具体型号支持的资源核对/复位证据与独立授权窗口。本实现不把“不知道”
升级为“资源已经释放”，也不提供 `--force`/“清空所有 receipt”开关。

## 3. 生命周期补强

- **QMI UIM**：分配 CTL client **之前**写账本；logical-channel close 只记录 channel 关闭，
  client release 成功后才允许清除。分配/关闭/释放结果未知均保留账本。
- **Bearer**：每个已确认释放的 CID/session 即刻从内存列表移除并持久化。即使 namespace
  仍待归还，重复清理也不会再次发送该旧 CID；失败写账本不恢复已释放 ID。
- **Reset**：通用 native reset 不再绕过专项维护。Quectel 使用显式 Reboot 计划；未实现的
  SIM-only/其它驱动复位报告 `native_reset_requires_explicit_maintenance_plan`。
- **维护 revision**：Quectel 计划包含 controller 实例，重建控制器后旧计划失效。
- **启动**：native 不再运行 MM 路径的全局 stranded-link 自动回收；需核对明确归属。

## 4. 验证范围

新增硬件无关测试覆盖：完整/部分/旧格式账本、owner/代次变化、陈旧计划、终态归档、
QMI socket-pair 分配/关闭/释放成功与失败、重复 bearer 清理不重放 ID、禁止通用 reset。

`a4a83c2` 的 Validate `35989431881` / Build `35989431842` 全部通过，含新增 recovery、
QMI socket-pair 与原有回归；amd64/arm64 成功、Publish skipped。后继最终检查点见接续计划。
**未执行 native 真机故障恢复、
Quectel/DJI 写操作或 SIM-04 native 接管**；已有 SIM-04/SIM-05 验收不能替代这些测试。
