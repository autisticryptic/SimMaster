# eSIM MEP 预留接口与后续实现计划

> 状态：预留阶段（2026-08-30）。本文件只描述 MEP 的接口、数据模型、模拟测试和后续实机适配边界。
>
> 当前没有支持 MEP 的 eUICC 和读卡器，因此在真实硬件到位前，不把任何 MEP 项目标记为实机完成。普通 eSIM Profile 管理、外置读卡器 VoWiFi 和现有基带 IMS 行为必须保持不变。

## 目标范围

本项目的优先目标是**读卡器/eUICC 作为 SIM AKA 来源的 WiFi-only VoWiFi**，不要求读卡器提供蜂窝联网，也不把基带 MEP 作为读卡器 VoWiFi 的前置条件。

预留的未来场景包括：

```text
内置 eUICC / 基带 MEP Port 0 -> Profile A -> 蜂窝 -> VoLTE
内置 eUICC / 基带 MEP Port 1 -> Profile B -> Wi-Fi -> ePDG -> VoWiFi

外置 PC/SC 读卡器 MEP Port 0 -> Profile C -> Wi-Fi -> ePDG -> VoWiFi
外置 PC/SC 读卡器 MEP Port 1 -> Profile D -> Wi-Fi -> ePDG -> VoWiFi
```

MEP Port、QMI/MBIM UIM slot、PC/SC reader index、物理基带槽和 SimAdmin `line_id` 必须保持为不同概念；不能把 `uim_slot` 直接当成 `mep_port`。

## 任务清单

### P0：领域模型和能力探测

- [ ] 新增 `MepCapability`、`MepMode`、`MepPort`、`ProfilePortBinding` 和 `MepTransport` 数据模型。
- [ ] 为能力状态区分 `supported`、`unsupported` 和 `unknown`；能力未知时不得自动启用 MEP 或发送自定义 APDU。
- [ ] 探测 eUICC 是否存在、EID、Profile Version、MEP 模式、逻辑 Port 数量和当前 Profile-to-Port 映射。
- [ ] 将 `mep_port` 作为独立的可选配置字段预留；它不能改变现有普通 Profile enable 行为，直到真实能力探测确认支持。
- [ ] 保持 eUICC 级 APDU/LPAC 互斥，同时预留 Port 级运行状态和线路级 IMS/VoWiFi 状态隔离。

### P0：后端适配接口

- [ ] 新增可插拔的 MEP backend seam，至少覆盖 `probe`、Port 状态读取、Profile-to-Port 绑定、Port 释放和 Port 级 SIM AKA/APDU 上下文。
- [ ] 预留 `pcsc`/独立读卡器适配层；读卡器只承担 APDU/SIM AKA，不要求蜂窝数据能力。
- [ ] 预留 QMI、MBIM、AT/厂商接口适配层，分别允许 410、724ug、EC20、EM05-G、EM7430 等设备按实际暴露的接口接入；不能根据型号名称猜测 MEP 能力。
- [ ] 预留 `IntegratedEuicc` 和 `IntegratedEuiccMepPort` SIM 来源，使内置 eUICC 的第二个 Profile 可以在不建立蜂窝承载的情况下作为 VoWiFi SIM AKA 来源。
- [ ] 对没有 MEP backend 或没有 MEP 能力的设备返回明确的 `unsupported`/`unknown`，继续使用现有单 Profile 模式。
- [ ] 不在没有标准或厂商证据时硬编码 MEP APDU；真实 APDU 适配必须附带版本、厂商和抓包/实机证据。

### P1：模拟后端和自动化测试

- [ ] 新增 Mock MEP backend，可构造一个 eUICC、多个 Port、多个 Profile 和任意绑定状态。
- [ ] 覆盖启用、解绑、端口占用、Profile 不存在、能力未知、APDU 不支持、操作失败回滚等测试。
- [ ] 覆盖同一读卡器下多条 WiFi-only VoWiFi 线路的 IMSI、AKA、IMS profile、ePDG、TUN、Call-ID、CSeq 和重试状态隔离。
- [ ] 覆盖“一个内置 eUICC Port 0 走蜂窝 VoLTE，Port 1 只走 WiFi VoWiFi”的模拟线路组合。
- [ ] 覆盖普通单 Profile 设备回退路径，确保现有 eSIM/VoWiFi 测试和行为不改变。

### P1：API 和前端预留

- [ ] 增加线路级 MEP capability 查询 API。
- [ ] 增加 Port 列表和 Profile-to-Port 映射只读 API。
- [ ] 只有 capability 明确为 `supported` 时，才显示 MEP 管理入口；`unknown` 和 `unsupported` 只显示原因和当前单 Profile 模式。
- [ ] 预留 Profile 绑定到指定 Port、释放 Port 和 Port 级测试 AKA 的 API，但在真实 backend 未接入前不得让前端误报操作成功。
- [ ] 显示 SIM 来源为“内置 eUICC MEP Port”或“外置读卡器 MEP Port”，并明确 `cellular`/`wifi_only` 连接模式。

### P1：混合 VoLTE/VoWiFi 线路

- [ ] 支持一个物理基带下 `Port 0 -> 蜂窝 VoLTE`、`Port 1 -> WiFi-only VoWiFi` 的线路建模。
- [ ] 确认第二个 MEP Port 的 SIM AKA 能够独立读取后，才允许它进入 VoWiFi IMS REGISTER；不能通过临时切换 Profile 伪造 MEP。
- [ ] 验证 WiFi-only 线路不会启动蜂窝数据、不改变 Port 0 的当前 Profile、不触发整张 eUICC 重启。
- [ ] 验证一个 Port 的 IMS refresh、重试、Profile 兜底和失败恢复不会影响另一个 Port。
- [ ] 明确 MEP 不等于双蜂窝、双射频或双 IMS；基带业务能力仍需单独探测和验收。

### P2：真实硬件适配与验收

- [ ] 获得支持 MEP 的 eUICC 后，记录 EID、Profile Version、MEP 模式和 Port 数量。
- [ ] 分别验证 410、724ug、EC20、EM05-G、EM7430 的实际 modem 固件、QMI/MBIM/AT 能力；型号清单只是适配目标，不代表已经支持。
- [ ] 使用外置 PC/SC 读卡器验证 MEP APDU 透传、Extended APDU、逻辑会话和 Profile-to-Port 映射。
- [ ] 验证内置 eUICC 通过基带接口访问第二个 Port 的 APDU/SIM AKA；若系统只暴露单一 UIM context，记录为底层限制。
- [ ] 完成“蜂窝 VoLTE + WiFi-only VoWiFi”双线路真实注册、重连、通话、短信和资源清理验收。
- [ ] 真实硬件未到位前，不把上述项目标记为完成；模拟测试通过只能标记软件接口和隔离逻辑完成。

## 完成定义

MEP 预留接口只有在以下条件全部满足后，才能从“预留阶段”进入“可实机适配”：

1. 普通 eSIM 和读卡器 VoWiFi 回归没有退化；
2. MEP capability、Port、Profile 映射和 SIM 来源模型已经稳定；
3. Mock backend 覆盖无能力、未知、成功、失败和回滚状态；
4. API 不会在 backend 未接入时误报 MEP 已启用；
5. 真实 MEP eUICC 和读卡器到位后，可以只替换具体 APDU/modem backend，而不重写 IMS/VoWiFi 业务层。

## 当前明确结论

- 读卡器不需要联网；读卡器线路使用 Wi-Fi 作为 VoWiFi 承载。
- 读卡器不需要蜂窝基带；eUICC MEP 能力和 APDU 传输兼容性才是前置条件。
- 410 只有在其内置 eUICC、基带固件和系统接口共同暴露 MEP Port 时，才可能实现内置 Profile 的 MEP。
- 没有真实支持 MEP 的 eUICC/读卡器时，可以先完成接口、模型、Mock 和隔离测试，但不能完成最终实机验证。
