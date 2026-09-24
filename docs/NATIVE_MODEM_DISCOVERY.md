# 原生 modem 只读发现

`simadmin discover-native` 帮助调查 native 配置所需的物理锚点、控制口、串口与网口。
**发现不等于接管**：不打开设备节点、不发 AT/QMI/MBIM、不连接 D-Bus、不修改配置，
也不停止 ModemManager；可在 MM 运行时使用。默认后端仍是 MM。

```sh
./simadmin discover-native
# 用于离线 sysfs fixture 或 chroot；输出仍使用目标机器的 /sys、/dev 路径。
./simadmin discover-native --sys-root /snapshot/sys --dev-root /snapshot/dev
```

输出为 JSON 数组；没有可识别 modem 时为 `[]`，根目录不存在时报错。
这是一次快照，不是热插拔监控，拔插期间的缺项应重新扫描，不应据此发起自动修复。

## 输出与限制

- `sysfs_anchor`：物理 USB 设备、PCI function 或 SoC WWAN 父设备。
- `hardware_key` / `line_id`：**建议值**，由 sysfs 和默认 slot 1 派生。
  切换现有线路前须核对 `inspect-modems` 的物理键及实际 slot。MM 可能使用自定义 UID：
  例如 SIM-04 返回 `qcom-soc`，其 `physdev:qcom-soc` 不等于这里的 sysfs key。
  不核对就复制会生成另一条线路。
- `usb_generation`：`busnum:devnum` 运行时观察值，不是跨重启稳定身份或 native receipt 代次。
- `qmi_controls` / `mbim_controls` / `serial_ports`：内核暴露的候选端口。
  驱动绑定和路径存在不证明协议可用；native 激活时仍执行字符设备/物理归属/owner 检查。
- `at_port_hint`：仅角色提示。Quectel 接口 2 与 WWAN AT 类型可能提供提示，但多 AT 口
  不任意挑选；候选配置中 **永远不写入 AT 口**。必须在独占维护窗口探测确认后补齐。
- `net_interfaces`：仅列宿主当前可见且属于同一物理祖先的网口。
  已转移到 UE namespace 的网口可能不在列表；不会由列表次序推断 IMS/data。
- `candidate`：仅当 QMI 或 MBIM 控制口/协议唯一时生成的不完整 `devices` 条目。
  `at_device` / `ims` / `data` 均为空，短信存储消费默认关闭。
  纯 AT、多个控制口或多个协议只报观察结果，不生成未经确认的可执行端点。
- `issues`：缺控制口、AT 未探测、驱动未绑定、歧义，以及必须核对的物理键/slot。
  没有自动写 `new_id`、切 USB 模式、重启模块或猜测 QMAP/BAM-DMUX 映射的路径。

只读发现不会输出 IMSI/ICCID/IMEI，不读取 SIM；JSON 中的设备名和拓扑仍属于本机诊断信息，
分享前应自行检查。

## 验证层次

1. fake-sysfs Rust 回归：USB QMI/MBIM/ACM、Quectel ECM、DJI 未绑定、MHI 兄弟通道、
   USB/WWAN 去重、缺口/多端点拒绝、串口别名目标核对。仅在 Actions 编译执行。
2. Python 守卫：被动文件系统调用、无猜测候选、CLI 提前返回、两套 CI 实际运行测试。
3. SIM-04 只读运行可以验证该设备的拓扑扫描；**不能**作为 native IMS/短信/电话的实机通过。
4. EC20/EC25 等设备的 native 接管仍需独立授权窗口与归属、SIM/AKA、承载、注册/续期验收。

审计与后续项目见 [原生硬件接口审计](NATIVE_BACKEND_AUDIT_2026-09-24.md)。
