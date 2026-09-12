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

## 后续版本目标（完整后端能力尚未实现）

- **1.1.5**：通过统一设备能力接口兼容 MM/native；原生 QMI/MBIM/AT 适配器由设备控制器
  协调。同一物理 modem 的全部槽位和端口只能由一个 backend owner 管理。
- **1.1.6**：移除 MM provider、运行调用与依赖，原生接管所有声明支持的设备能力，
  不保留隐藏 MM fallback；未适配硬件继续明确返回 unsupported。
- 复用现有 driver、transport、UE 和身份边界；不能因迁移后端把临时 MM 对象路径、
  QMI 端点或网卡编号当成跨设备统一的持久身份。

详细执行与验收统一见 [1.1.5 / 1.1.6 设备后端版本规划](MODEM_BACKEND_ROADMAP_1.1.5_1.1.6.md)。

## 1.1.5 第一阶段：设备观察接口

`hardware/cellular/observations.rs` 的 `ModemObservationProvider` 提供设备列表与驻网
snapshot，`LineRuntimeRegistry` 在构造时注入它。注册表及其 API 调用者不再为刷新传入
D-Bus connection，也不直接解析 MM 的错误字符串。
该查询调用链已通过 `e013684` 的 Actions 编译、回归和双架构构建；未部署实机，
不代表后面的控制/原生接入阶段已经完成。

- `bindings.rs` 承载原有 `ModemBinding`、稳定线路/迁移别名与 reader 绑定算法；
  原 JSON 字段和 ID 值保留。`modem_path` 等旧字段暂作为兼容 selector，不能要求未来
  原生 provider 伪造 MM 对象路径。
- `mm_observations.rs` 复用已有 D-Bus connection 与既有 MM 查询。构造 adapter 不新增
  连接，也不执行 Enable/Connect、切卡或修改开机策略。
- 驻网 snapshot 的明确不可用会立即清除，瞬时查询失败按原 TTL 保留；
  对整个设备列表查询失败的既有策略暂不改变，不能混为同一种缓存策略。
- 当前唯一生产实现仍是 MM；fake provider 只用于无硬件测试。这不是完整 MM 后端剥离，
  也不是原生 QMI/MBIM/AT 已实现。设备控制、AT/UIM、短信/呼叫、bearer 及启动副作用
  还需逐步迁移；飞行模式和冷启动离线保证需要独立策略及实机验证。
