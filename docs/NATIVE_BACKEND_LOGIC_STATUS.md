# 原生后端逻辑候选：默认 MM，硬件验收延期

> 2026-09-13，`dev/1.1.5-modem-backends`。
> 用户要求先推进非 MM 逻辑；现阶段继续以 MM 为主，等 IMS 多卡基线收敛后再做接管测试。
> 本轮不部署、不连接测试设备、不切换 owner。代码接线、离线验证和实机支持必须分开记录。

## 1. 默认与接管边界

- 旧配置没有 `cellular_backend` 时仍选择 `modemmanager`；默认值不额外写入配置文件。
- 可以预存 native 设备描述，但 MM 模式不会探测或接管这些端口。
- Native 必须显式设置 `mode: native` 和 `allow_unvalidated_native: true`，缺省拒绝。
- 后端仅在进程启动时选择；没有活动通话/IMS 会话中热切 owner 的 HTTP 开关。
- Native 当前要求 **MM daemon 未运行**，不自动停 MM，不通过探测激活 MM。
- 未知 native 目标、协议失败、能力缺失都不回退到 MM。
- 存活的 native 物理锁或未解决的会话 receipt 会阻止 MM 启动，避免接管未清理资源。

“MM 优先”指默认仍使用 MM，不是 MM/native 同时争用同一个基带。
同机不同设备分别用 MM/native 的混合模式尚未接通；当前是全局后端选择。

## 2. 已有代码接线

| 范围 | 实现 |
| --- | --- |
| 启动配置 | `backends/config.rs`：默认 MM、显式实验 opt-in、设备/端口/网口唯一性检查 |
| 兼容接口 | `cellular/control.rs`：保留原 API/服务签名；MM 分支沿用原实现，native 分支进入独立 controller |
| 物理控制器 | `backends/native.rs` / `io.rs`：按物理设备串行、sysfs/字符设备归属检查、flock、受限参数、超时/输出上限 |
| 发现/身份 | 显式设备表生成 `ModemBinding`，沿用物理锚点 + slot 的 line ID，不制造 MM 对象路径 |
| 射频/驻网 | QMI DMS/NAS、MBIM Radio/Registration、AT CFUN/CEREG/COPS；未知不等于 RF-on 或 home |
| QMI 偏好 | `management.rs`：设备能力约束的 RAT、运营商、LTE/NR band；白名单 NAS/DMS TLV，不改无关紧急/漫游策略 |
| SIM/AKA | QMI UIM 使用同一物理操作门；AT CCHO/CGLA/CCHC 复用 USIM 解码器；保留 PC/SC 路径 |
| IMS 入口 | Native 不再要求先执行 mmcli；PDP/P-CSCF/语音信箱 AT 查询经 facade；MM 路径保留 |
| QMI/MBIM bearer | 显式 endpoint/interface/session、CID 保留/释放、地址族/前缀检查、存活观察、receipt |
| UE 数据面 | 复用 mandatory UE worker/netns、地址/路由及 generation 检查；不提供宿主 bearer fallback |
| 基带短信 | AT PDU 发送/存储轮询；复用 SMS codec、DB 去重和通知；删除前核对索引内容 |
| 基带电话/USSD | AT 查询、拨号、接听、挂机、DTMF、呼叫等待、CUSD；失败不走 MM Voice |
| eSIM | Native lpac 与 SIM/射频共用物理门；按 QMI/AT/MBIM 选择 reader，拒绝越线设备 |
| 退出/取消 | 禁止 shutdown 后新命令；等待 pending setup/物理操作；不确定分配/清理保留 receipt |
| 运维 | `modem-backend-mode` 无硬件解析配置；`GET /api/modem/backend` 只读脱敏；安装/恢复资源按后端选择 |
| 漫游保护 | 禁止漫游时，查询失败/未明确 home 都拒绝数据连接，不把未知当成未漫游 |

### 两种 “native bearer” 不可混淆

- **MM 默认路径**：QCA410 主 QMI IMS bearer 仍由 MM 创建/持有，独占接口进入 UE。
  已实测的 MM 路径没有被原生实现替换。
- **Native 候选路径**：新 controller 管理 QMI/MBIM 会话，没有 MM CreateBearer/Connect。
  旧 SIM-03 的 MM 注册/续期记录不算新 native 路径的验收证据。

## 3. 仍未完成或未覆盖

不能因为代码能够构建，就将以下项目标记完成：

1. **硬件验收全部延期**：BAM-DMUX/data-port 映射、QMI/MBIM 固件差异、SIM/AKA、
   IMS 注册/续期、IPv6、短信/电话、掉线恢复均无本轮实测。
2. **混合 owner**：尚未实现 MM 在线时对不同 modem 的 inhibition/端口隔离式并行管理。
3. **自动代次恢复**：控制节点代次变化目前要求重启/重新核验；不确定 receipt 需要受控
   reconciliation，自动孤儿会话恢复器尚未完成。不能盲删 receipt 后重连冒充续期。
4. **协议/设备扩展**：AT-only PPP/ECM/NCM 数据面、MBIM/AT 的厂商 RAT/band/reset、
   多槽/MEP 和自动槽位切换仍需专门适配。缺失能力明确 unsupported。
5. **事件与业务长稳**：native 短信/通话当前主要用 AT 存储/CLCC 轮询；原生 WMS/VOICE、
   完整 URC 事件源、索引复用、补充业务等仍需完善及验收。
6. **接口归属证据**：显式配置不代表端点一定能收到 SIP；有 IP 或 netdev 不等于 IMS 成功。
   主 QMI IMS 与 DATA6 普通数据的映射必须分别核对，不能复制其他端口/型号的数字。

因此当前还不能声称已完成所有替换项，更不能据此删除 MM 默认实现或发布 1.1.6。

## 4. 配置与诊断——目前只阅读，不在测试机执行

默认等价配置：

```yaml
cellular_backend:
  mode: modemmanager
  allow_unvalidated_native: false
  devices: []
```

未来 native 维护窗口需要核对：

- `hardware_key` 沿用现有物理槽位锚点，不能使用临时 `/Modem/N`。
- `sysfs_anchor` 是精确物理祖先，不是整个 `/sys/devices` 等宽泛目录。
- `protocol` 为 qmi/mbim/at；`control_device`、可选 `at_device` 必须属于同一物理设备。
- `uim_slot` 必须真实匹配；QMI 会核对 primary GW slot，MBIM/AT 多槽需驱动扩展。
- `ims` / `data` 分别指定控制端点、独占网口及 session ID；需要时填写有设备证据的
  `qmi_data_port` / `qmi_binding`。不提供可以直接复制到真实设备的猜测映射。
- Native 普通数据 APN 为空时不猜 `internet`，需要明确配置。

`modem-backend-mode` 只解析配置；`--require-mm` 可作为服务条件。
API 的 `hardware_validated: false`、`native_hardware_validation: deferred` 是真实验收状态。

## 5. 验证记录

- 本地仅运行 Python 边界检查、rustfmt、shell 语法检查。
- Rust 编译、离线用例、私有 D-Bus API 回归及双架构构建通过 Actions 执行。
- 新用例覆盖默认 MM 不碰预存 native 设备、协议解码、参数/slot 校验、AKA/PDU、
  NAS 字段白名单、session 释放/回滚和超时保留 receipt。
- 首轮 `9aafa06` 的 Actions 编译发现 DNS 地址解析类型推导错误（E0283），由
  `32d4c0d` 修正。后者的
  [Validate Beta Refactor](https://github.com/autisticryptic/SimMaster/actions/runs/34739474728)
  和 [Build-Release](https://github.com/autisticryptic/SimMaster/actions/runs/34739474668)
  均 success，包含后端/前端回归与 arm64/amd64 构建；发布任务 skipped。
- 后续收尾增加：拨号调用者取消后的已确认呼叫清理；协议明确拒绝与结果不确定的
  区分；SMS 删除在同一物理门内核对 SIM/内容，保留部分清理进度；
  lpac 超时后等待子进程退出再释放操作门。
- 安全收尾提交为 `b1caafe`、`b94c9c2`。最新 **`b94c9c2`** 的
  [Validate Beta Refactor](https://github.com/autisticryptic/SimMaster/actions/runs/34740891966)
  与 [Build-Release](https://github.com/autisticryptic/SimMaster/actions/runs/34740891971)
  均 success，包含新增/原有离线回归、私有 D-Bus API 测试、前端和 arm64/amd64 构建；
  `Publish Release` 已核对为 skipped。中间 `b1caafe` 两套 workflow 也均 success。
- 本地49项 Python检查与 Rust格式/语法、shell语法检查通过；没有本地 Rust 构建，
  没有部署、发布或 native 硬件测试。

**当前检查点**：本轮逻辑候选已提交并通过 CI，但第 3 节的逻辑缺口仍未完成。
下一名开发 agent 应从混合 owner / 代次恢复等项目继续，不把本轮当作完整替代已完成，
也不提前在 IMS 验证设备启用 `native`。

## 6. 后续 agent 接续

1. 检查本节提交/CI 状态，继续使用独立 `SimAdmin-1.1.5` worktree。
2. 继续第 3 节的逻辑缺口，不能把安全拒绝当作全能力覆盖。
3. IMS 派生兜底仍在原 `fix/sim02-catalog-aka-baseline` 分支验证。
   真实凭据仍仅在本地私密交接文件，不经本文或 Git 分发。
4. 用户明确安排硬件窗口后，再备份、确认归属、释放已确认资源，分 MM/native 测试；
   不同时控制同一物理 modem。
5. 仅一名开发 agent 修改/部署，其他 agent 只读汇总。
