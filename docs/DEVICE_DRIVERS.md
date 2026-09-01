# 设备驱动边界

SimAdmin 的 IMS、线路、自动化、OTA 与安装逻辑不直接导入具体设备实现。运行时先通过
`backend/src/hardware/devices/mod.rs` 识别平台，再由 `DeviceDriver` 提供能力。

## 通用层可以依赖的接口

- `ImsBearerTransport`：建立和释放设备原生 IMS bearer，并返回设备无关的地址、DNS、
  P-CSCF、接口所有权和失败提示。
- `CellularDataTransport`：建立、保持和释放该线路的数据 bearer。
- `BasebandFaultPolicy`：只观察设备故障状态，不在通用 IMS 层硬编码 sysfs 路径。
- `DeviceDriver::capabilities`：报告 gateway、本地音频和本地视频等不可变硬件能力，API
  不再按设备型号写死这些值。
- `DeviceDriver::initialize_native_bearers`：设备启动期的端点准备。
- `DeviceDriver::install_update_resources`：设备自己的 systemd、脚本和其他发布资源。

未知平台使用安全的 unsupported driver：不建立 native IMS/data bearer，也不会回退到
宿主网络命名空间。

## QCM410 归属

QCM410 的 DATA6/RPMSG 发现与绑定、QMI 会话、netdev 解析、bam-dmux 故障策略和安装资源
全部位于：

```text
backend/src/hardware/devices/qcm410/
deploy/devices/qcm410/
```

通用 ModemManager 解析器和 QMI WDS 协议工具仍保留在 `hardware/cellular/`；它们按协议
复用，不包含 QCM410 的端口、remoteproc、DATA6 或 systemd 资源选择。

## 新增设备

1. 在 `backend/src/hardware/devices/<device>/` 新建设备模块并实现所需 transport。
2. 实现 `DeviceDriver`，把硬件识别证据留在设备模块内。
3. 在 `DeviceKind` 与驱动注册表中加入新设备。
4. 将设备资源放到 `deploy/devices/<device>/`，通用构建会自动复制整个 `devices/`
   目录，再由该 driver 安装；新增设备不需要修改打包脚本。
5. 上层只消费 transport 返回的结构化结果；不要解析该设备的命令输出或错误文本。
6. 为未知/缺失能力返回 unavailable，不增加宿主网络命名空间兜底。

## 仍按协议划分的边界

`ModemBinding::qmi_device` 目前仍是线路发现结果的一部分，因为现有 QCM410 的 USIM AKA、
eSIM 和离线基带恢复确实通过 QMI/UIM 完成。它不是未来设备必须实现的通用能力：接入
MBIM、PC/SC 或厂商 API 的设备时，应分别增加 SIM 鉴权与基带恢复 transport，由 driver
选择实现；不能让新设备伪造一个 QMI 路径，也不能在通用 IMS 编排中增加设备型号判断。
