# 原生硬件接口审计与增强计划（2026-09-24）

> 范围：检查 1.1.4 → 1.1.5 “ModemManager 可选 / 本项目直接操作硬件接口”的完成情况，
> 并对照用户指定的 5 个参考项目，给出能增强直接硬件接口完备性的具体条目。
> 分支 `dev/1.1.5-modem-backends`，基线 `71513ea`。
> 每项按“代码 / CI / 实机”分别记录，不因编译通过而勾选实机。

## 1. 完成情况结论

**默认 MM 与显式 native 选择已接线，但 native 端到端实机验收仍为零；本轮补充只读发现。** 具体：

| 方面 | 状态 | 依据 |
|---|---|---|
| 后端选择 | 已完成（代码+CI） | `backends/config.rs`：默认 MM；`mode: native` 需 `allow_unvalidated_native: true`；未知目标不回退 MM |
| 协议控制器 | 已完成（代码+CI） | QMI DMS/NAS/UIM/WDS、MBIM、AT；按物理设备串行、flock、超时与输出上限（`native.rs` 853 行、`io.rs` 623 行、`bearer.rs` 1438 行） |
| SIM/AKA、承载、UE 数据面 | 已完成（代码+CI） | QMI UIM / AT CCHO-CGLA；QMI/MBIM 会话、receipt、namespace 归还确认 |
| 短信/电话/USSD | 部分 | 仅 AT **轮询**（存储、`CLCC`）；只有 `+CUSD` 走 URC（`at_session.rs`），无统一 URC 事件源 |
| 设备发现 | 本轮新增（待 CI） | `discover-native` 只读扫描 sysfs，输出端口/物理锚点建议与不完整配置；不自动启用 native，不猜 IMS/data 映射 |
| Quectel | 专用驱动仅分类 | `devices/quectel/` 主要提供型号分类；native 已可走通用 AT/QMI 控制，但没有 Quectel MBN/USB composition 专用管理 |
| 混合 owner | 未实现 | 同机 MM/native 分设备并行未接通；当前全局二选一 |
| 代次恢复 | 未实现 | 控制节点代次变化需重启；无自动孤儿会话 reconciliation |
| 实机验收 | **无** | 已实测的 IMS 注册/续期（SIM-03、SIM-04 T03/T04/T05、`71513ea`）全部走 **MM 路径**，不能算 native 证据 |
| 路线图 | M0–M5 全部未勾 | `MODEM_BACKEND_ROADMAP_1.1.5_1.1.6.md` §5.2/§5.3 |

因此可以说“**代码提供显式实验性 native 选项**”，不能说“无 MM 全能力已经验收”。
只读发现降低配置调查成本，但不能证明候选端口可用、不会与已有 owner 冲突或支持 IMS。

## 2. 参考项目与可用方式

SimAdmin 为 GPLv3。以下判断决定“能拿代码”还是“只能借鉴思路（净室重写）”：

| 项目 | 语言 | 许可证 | 可用方式 | 与本项目最相关的内容 |
|---|---|---|---|---|
| [VoCat](https://github.com/MengMengCode/VoCat) | Go | 自定义研究/评估许可（禁商用） | **仅思路** | 无 MM 的 Quectel EC20/EC25 面板：sysfs 驱动绑定发现、端口角色、`AT+QCFG="ims"`、`AT+QMBNCFG` MBN 选择、DJI `2ca3:4006` 驱动绑定修复、原生 QMI NAS/飞行模式、QMI 端口租约 |
| [mdd-sim-gateway](https://github.com/MddIdd/mdd-sim-gateway) | Python | GPLv3 | 代码许可兼容；但其架构依赖 MM，价值在思路 | 模块 SIM 的逻辑通道容量/分配/失败释放；DJI/EC25 经 AT 建虚拟读卡通道 |
| [DJIModeSwitcher](https://github.com/hiwangchuan/DJIModeSwitcher) | Swift (macOS) | Apache-2.0 | 可引用（署名） | DJI 一代 4G 模块：批量端点上 `AT`/`OK` 探测 AT 接口；`AT+QCFG="usbnet",0/1` 切换并重启、等待重枚举、回读验证 |
| [EC25Toolbox](https://github.com/skyrocketingHong/EC25Toolbox) | Swift (macOS) | AGPLv3 | **本轮仅借鉴思路**（不引入额外许可证义务） | 单读者 AT transport：行分帧、`>` 提示、终结码收集、URC 分类与可重连事件分发；`QCFG usbcfg` 身份切换带备份/重启/验证/回滚 |
| [DJOneHub](https://github.com/ZenGeekLabs/DJOneHub) | Go (macOS) | PolyForm Noncommercial | **仅思路** | `apduarbiter`：多方（eSIM/短信/读卡）APDU 访问仲裁；换卡/拔插处理 |

macOS/libusb transport、模块 PCM 语音（`AT+QPCMV`）、MaVo/ADB 注入与本项目的 Linux 服务
形态无关，不纳入。

## 3. 增强条目（按收益/风险排序）

### N1 只读原生设备发现 `simadmin discover-native` — 最高优先

原因：手写设备表是启用 native 的主要障碍；本命令只读文件系统，不打开设备节点或改变 owner。
实现与使用说明见 [原生设备只读发现](NATIVE_MODEM_DISCOVERY.md)。

- [x] USB 按 `qmi_wwan` / `cdc_mbim` / `option` / `qcserial` 驱动发现；
      `cdc_acm` 只在同设备另有 modem 证据或已知厂商身份时纳入，避免把 Arduino 当基带
- [x] Quectel `2c7c` 的 ECM/RNDIS/未绑定组合，以及 DJI `2ca3:4006` 保留诊断条目
- [x] WWAN 严格解析 qmi/mbim/at 端口；MHI 兄弟通道归并到 PCI function，USB/WWAN 视图去重
- [x] Quectel 惯例端口只作 hint；多 AT 口不选第一个，候选 `at_device` 留空等待独占窗口探测
- [x] USB sysfs 锚点及 `busnum:devnum` 观察；硬件键/slot **必须人工核对**，不宣称切换后
      line ID 必然不变。SIM-04 的 MM `Device=qcom-soc` 与 sysfs key 就是不同的
- [x] 缺 QMI/MBIM 控制口、未绑定驱动、AT 缺失/未探测、多控制口/协议显式报告；
      多控制口不生成任选第一项的配置
- [x] JSON 输出可审阅的 `candidate`，只在协议/控制口唯一时生成；`ims`、`data`、`at_device`
      留空，`sms_reception_enabled=false`，不打开端点，不写配置
- [x] 12 项 fake-sysfs Rust 回归接入两套 CI；4 项 Python 被动边界守卫
- [ ] CI 与两架构构建通过（不得将本地格式检查当成 Rust 测试）
- [ ] 实机只读发现验证（不等于 native owner 接管或注册验收）

### N2 URC 驱动的 AT 会话（后续路线图，本轮不切换业务事件源）

- [ ] 每个 AT 口单一读者：行分帧、`>` 提示、终结码（`OK`/`ERROR`/`+CME ERROR`/`+CMS ERROR`）归属当前事务
- [ ] URC 分类并广播：`+CMTI`/`+CMT`/`+CDS`、`RING`/`+CLIP`/`+CRING`、`NO CARRIER`、`+CUSD`、
      `+CREG`/`+CEREG`/`+C5GREG`、`+QIND`
- [ ] 以事件替换原生短信存储轮询和 `CLCC` 轮询（保留低频轮询兜底以防 URC 丢失）
- 对应状态文档 §3 缺口 5

### N3 Quectel 设备驱动（EC20/EC25/EG25）

先只读诊断，再做受确认保护的写入：

- [ ] 只读：`AT+QCFG="ims"`（MBN 默认/强制开/强制关 + 基带 VoLTE 可用位）、
      `AT+QMBNCFG="List"` / `"AutoSel"`、`AT+QCFG="usbnet"`、`AT+QCFG="usbcfg"`
- [ ] 写入需显式确认、写后回读、必要时 `AT+CFUN=1,1` 并按代次作废既有 QMI 会话：
      IMS 模式、按 HPLMN 选择 MBN（关闭 AutoSel 以免被拉回其他运营商 MBN）
- [ ] 设计问题待实机确认：用户态 IMS 注册时，基带自带 IMS 客户端是否会与本项目争用同一
      IMS PDN/注册（EC20 CEFA 固件差异正是这一类问题）。候选做法是由本项目注册时
      强制关闭基带 IMS（`QCFG ims=2`），但必须在实机上先验证不影响承载与 P-CSCF 下发

### N4 DJI 一代 4G 模块（`2ca3:4006`）驱动绑定修复

- [ ] 显式维护命令（不自动执行）：接口 0–3 经 `option` 的 `new_id` 绑定为串口，接口 4 绑定
      `qmi_wwan`，拉起 DTR，随后 `qmicli --dms-get-operating-mode` 就绪检查；不写 NV、不改 USB 身份
- [ ] `usbnet` 模式切换沿用 N3 的“写入—重启—等待重枚举—回读验证”流程

### N5 SIM 逻辑通道与 APDU 仲裁

- [ ] 逻辑通道容量/已分配/用途可见；部分分配失败时主动释放已打开通道
- [ ] eSIM(lpac)、IMS AKA、短信读卡共用的 APDU 仲裁已有物理操作门，补齐按通道的归属账本

## 4. 边界

- VoCat / EC25Toolbox / DJOneHub 的代码不复制，只按思路净室实现；引用 DJIModeSwitcher
  （Apache-2.0）时保留署名。
- SIM-04 继续作为 **MM 路径** IMS 测试机；native 实测需另一台设备（用户计划采购的 EC20 类设备）。
- N1 只读；N3/N4 的写操作必须有显式确认、回读与失败报告，不做静默兜底。
- 实机未验证前，文档与 UI 不得宣称“MM 可选已完成”。

## 5. 变更记录

| 条目 | 提交 | 说明 |
|---|---|---|
| — | `71513ea` | 审计基线；该版本 SIM-04 MM 路径已实机注册及自然续期 |
| N1 | 本轮待提交 | 被动发现、12 项 Rust 回归、4 项 Python 守卫；无硬件写入 |
