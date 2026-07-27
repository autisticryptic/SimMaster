# VoLTE beta2 IDA 逆向差异与后续修改指导

> 创建日期：2026-07-27  
> 适用项目：`SimAdmin`  
> 参考成品：`SimAdmin-VoLTE/simadmin beta2`  
> 文档用途：作为下一阶段 VoLTE bearer、P-CSCF、IMS 注册和运行时监督修改的直接指导。

## 1. 文档定位与结论优先级

本文件记录使用 IDA Pro 对已知可用的 beta2 成品进行函数级逆向后，和当前 `SimAdmin`
源码逐项对照得到的结果。对于本文明确给出 IDA 地址和控制流的结论，其优先级高于下列旧文档中的推测性内容：

- `VOLTE_beta2对齐重构计划.md`
- `VOLTE_改进记录与待办.md`
- `VOLTE_逆向与重构总文档.md`

旧文档仍保留历史测试、协议背景和开发过程；若与本文中“已确认”结论冲突，以本文为准。

### 1.1 证据等级

- **A 级：IDA 已确认**。有明确字符串交叉引用、函数地址或分支控制流。
- **B 级：源码已确认**。当前源码调用关系或未调用关系可以静态证明。
- **C 级：待真机确认**。实现方向合理，但需要 410 设备、SIM 卡和运营商网络验证。

### 1.2 逆向对象

| 项目 | 值 |
|---|---|
| 文件 | `SimAdmin-VoLTE/simadmin beta2` |
| IDA image base | `0x400000` |
| image size | `0x859828` |
| SHA-256 | `45571d1033b6914f04be33543e11cb62e04c15280940d14609d2440f323140ab` |
| 当前源码检查点 | `2a4f57b feat(ims): checkpoint QMI bearer and carrier runtime` |

## 2. beta2 的 VoLTE 主流程

根据日志锚点和函数控制流，beta2 的主要连接顺序可以整理为：

```text
线路启用
  -> 等待 QMI/UIM 初始 provisioning
  -> 读取 SIM/USIM 身份和 USIM AID
  -> 计算 data-slot mode
  -> 清理失效但未连接的旧 IMS bearer
  -> 建立 IMS bearer（MM 或原生 QMI WDS）
  -> 取得本地 IPv4/IPv6 地址和 P-CSCF
       1. IMS profile 预取
       2. bearer/PCO
       3. 当前 WDS client 的 current settings
       4. AT CGCONTRDP 兜底
  -> 初始 SIP REGISTER
  -> 401/407 AKA challenge
  -> USIM AUTHENTICATE APDU
  -> 优先尝试 3GPP IPsec 注册
       成功 -> IPsec IMS runtime
       失败 -> 清理 IPsec -> 从头尝试 plain UDP SIP
  -> REGISTER 200
  -> 保存 Service-Route / P-Associated-URI
  -> 进入短信、语音、刷新和 bearer 健康监督
```

当前项目已经具备其中大部分协议组件，但仍有几处关键控制流没有接入，不能仅凭模块文件存在就认为已经对齐。

## 3. 已确认的 beta2 行为

## 3.1 QMI packet-service 健康检查是三态逻辑

**证据等级：A**

相关函数：

- 健康检查：`sub_58C9B4`，地址 `0x58c9b4`，大小 `0x758`
- packet status 解析：`sub_595D4C`，地址 `0x595d4c`

相关字符串：

- `0x93cd07`：`Secondary QMI packet status was inconclusive; retaining live host IMS state`
- `0x93cd52`：`volte_runtime_health_qmi_disconnected`
- `0x93cd77`：`Secondary QMI packet status query failed; retaining live host IMS state`
- `0x93ccdf`：`volte_runtime_health_qmi_address_missing`

控制流结论：

1. 明确解析为 `connected`：保留会话。
2. 明确解析为 `disconnected`：设置 `volte_runtime_health_qmi_disconnected`，触发上层恢复。
3. 查询命令失败：只记录警告，保留现有 IMS 会话。
4. 输出存在但状态不明确：只记录 inconclusive 警告，保留现有 IMS 会话。

因此，普通 QMI 查询失败不能累计为“bearer 已断开”。只有明确的 `disconnected` 或独立的基带失联证据才允许拆除 IMS 会话。

## 3.2 IPsec 注册失败后无条件尝试 plain UDP

**证据等级：A**

主控制函数：`sub_5965BC`，地址 `0x5965bc`，大小 `0x1cf0`。

关键控制流：

```text
0x597920  BL sub_59B9F4   ; 3GPP IPsec 注册尝试
0x597924  检查返回值
           成功 -> 0x598018
           失败 -> 记录回退日志

0x597960 / 0x597ad8
           "Native VoLTE IPsec registration failed,
            falling back to plain UDP SIP"

0x597b30  清理失败尝试的资源
0x597be8  BL sub_59F808   ; 独立的 plain UDP SIP 注册
```

没有发现按 IPsec 错误类型阻止 UDP 回退的业务分支。日志条件分支属于 tracing/callsite 代码，
不是错误分类逻辑。可以认为 beta2 的行为是：**只要 IPsec 注册函数返回失败，就尝试 plain UDP**。

需要注意：回退是独立注册尝试，不是继续复用已经切换到受保护端口的 channel。实现时必须：

1. 卸载本次 XFRM state/policy。
2. 关闭受保护的发送和接收 socket。
3. 重新创建普通 UDP SIP channel。
4. 使用新的 Call-ID/tag/CSeq 或明确定义的重试事务状态，从初始 REGISTER 开始。
5. 保留 bearer、本地地址和已选择的 P-CSCF，避免重新拨 bearer。

## 3.3 P-CSCF 顺序包含 profile 和直接 WDS 查询

**证据等级：A**

确认的日志锚点：

- `Native VoLTE using P-CSCF candidates prefetched from IMS profile`
- `Native VoLTE P-CSCF candidates prefetched from IMS profile`
- `Native VoLTE P-CSCF candidates discovered directly from QMI WDS`
- `Native VoLTE direct QMI WDS P-CSCF query unavailable; keeping AT fallback`
- `Native VoLTE QMI WDS CID is not numeric; skipping direct P-CSCF query`
- `Native VoLTE P-CSCF candidates discovered from active IMS bearer`

`sub_5965BC` 中 `0x596a34` 附近先检查已存在的 P-CSCF 集合；非空时在 `0x596a5c`
附近记录 profile 预取日志。后续才进入 bearer 和其他发现层。

beta2 的有效顺序应实现为：

1. 线路或运营商 IMS profile 中保存的 P-CSCF 候选。
2. 已连接 bearer 直接暴露的 PCO/P-CSCF。
3. 使用建立 bearer 的 WDS client/CID 读取 `--wds-get-current-settings`。
4. 如果 WDS CID 可复用但 P-CSCF 仍不可用，再运行 AT `CGCONTRDP` 路径。
5. DNS 可以作为附加发现手段，但不能取代 WDS current settings。

## 3.4 data-slot mode 是连接前决策，不是连接后的展示标签

**证据等级：A**

相关锚点：

- `event src/volte.rs:1676`
- `data_requested`
- `primary_data_active`
- `secondary_data_active`
- `event src/volte.rs:1687`
- `IMS allocated to primary qmi0; DATA6 is reserved for data`
- `IMS allocated to DATA6; primary qmi0 is reserved for data`
- `volte_data_slot_mode_missing`
- `volte_data_slot_conflict`

相关选择函数为 `sub_58E0C4`，地址 `0x58e0c4`。它根据运行时输入返回离散模式，随后由注册流程决定 IMS 和普通数据各自使用的端点。

beta2 中存在的配置/状态 token：

- `independent_wwan1`
- `secondary_qmi_data`
- `both_data_slots_active`

这些 token 表示数据面分配结果或意图，不能仅根据最终 bearer 类型事后生成。

## 3.5 bearer 健康检查包含路径和地址变化检测

**证据等级：A**

相关字符串：

- `volte_runtime_health_bearer_changed:expected=`
- `volte_runtime_health_bearer_query_failed:`
- `volte_runtime_health_qmi_address_missing`

`sub_58C9B4` 在 `0x58cdd0` 至 `0x58cec0` 附近比较当前 bearer 快照和会话建立时保存的预期值，至少包含地址族、地址和 bearer 标识相关字段。发生变化时返回 `health_bearer_changed`，由 supervisor 负责重建。

这套检查和 QMI packet status 三态检查必须区分：

- bearer 对象或地址明确变化：需要重建。
- 明确 packet status disconnected：需要重建。
- 单次查询失败或输出不明确：暂时保留，会在后续周期继续检查。

## 3.6 beta2 使用 USIM logical channel 执行 AKA

**证据等级：A**

相关函数：`sub_5B79A8`，地址 `0x5b79a8`，大小 `0xf68`。

相关字符串：

- `sim_auth_proxy_connect_failed`
- `sim_auth_proxy_open_failed`
- `sim_auth_uim_client_failed`
- `sim_auth_logical_channel_failed`
- `sim_auth_logical_channel_close_failed`
- `sim_auth_apdu_exchange_failed`
- `sim_auth_apdu_security_status`
- `sim_auth_aka_response_parse_failed`
- `sim_auth_aka_sync_failure_parse_failed`
- `sim_auth_apdu_more_data_unhandled`
- `sim_auth_apdu_wrong_length_unhandled`
- `sim_auth_apdu_instruction_not_supported`

反编译结果显示 beta2：

1. 通过 qmi-proxy 获取 UIM client。
2. 打开逻辑通道并取得 channel id。
3. 组装 AUTHENTICATE APDU。
4. 处理 `0x61` more-data 和 `0x6c` wrong-length 状态字。
5. 解析 AKA success 或 AUTS 同步失败响应。
6. 在结束时关闭 logical channel。

当前项目复用 `vowifi::qmi_uim` 执行相同类型的 USIM AKA，方向基本正确。

## 3.7 尚未证明 beta2 读取 ISIM 身份文件

**证据等级：A（否定“已经证明”的说法），功能本身为 C 级待验证**

已搜索但未发现可靠命中的内容：

- `EF_IMPI`
- `EF_DOMAIN`
- `EF_IMPU`
- `uim-read-transparent`
- 对应 ISIM EF 名称字符串

已经确认 beta2 会运行 `--uim-get-card-status`、解析 USIM AID，并从 USIM 执行 AKA。但现有证据不能证明它读取 ISIM 的 IMPI/IMPU/DOMAIN 文件。

因此下一阶段不应把“完整 ISIM EF 读取”列为最优先注册修复。只有在 bearer/P-CSCF/IPsec
控制流对齐后，真机明确出现身份相关 403、realm 或 IMPU 错误时，再实现 ISIM identity provider。

## 4. 当前源码的已确认差异

## 4.1 data-slot 选择器没有进入生产调用链

**证据等级：B，优先级：P0**

相关源码：

- `backend/src/access/volte/data_slot.rs::select_data_slot_mode`
- `backend/src/access/volte/live.rs::connect_live_for_line`
- `backend/src/access/volte/live.rs::connect_inner`

当前 `select_data_slot_mode()` 只有定义和单元测试，没有生产调用者。注册成功后才根据
`session.native_bearer.is_some()` 生成 `data_path_mode`，属于展示逻辑，不会影响 bearer 端点。

同时 `native_ims_bearer_enabled()` 默认返回 false，只有设置
`SIMADMIN_VOLTE_NATIVE_IMS_BEARER=1` 才走原生 QMI。因此默认流程仍固定为 ModemManager bearer。

### 修改要求

连接前构造真实的 `DataSlotInputs`：

- `data_requested`：当前线路 `data_connection_enabled` 或数据代理运行意图。
- `primary_data_active`：主 QMI/MM 默认数据 bearer 的实际状态。
- `secondary_data_active`：DATA6 数据会话的实际状态。
- `secondary_endpoint_available`：对应基带是否存在可用 DATA6。

选择结果必须参与 bearer 端点和普通数据端点的实际分配。`data_path_mode` 只能由已经执行的选择结果生成。

## 4.2 默认 MM 路径缺少完整 P-CSCF 层级

**证据等级：B，优先级：P0**

当前 `connect_inner()` 在 bearer settings 没有 P-CSCF 时，直接调用
`discover_pcscf_via_at_with_context()`。只有 native bearer 内部会使用自身 WDS client 读取 current settings。

缺失项：

1. 没有每线路 IMS profile P-CSCF 数据模型和读取入口。
2. 默认 MM 路径没有在 AT 前进行直接 QMI WDS current-settings 查询。
3. `RUNTIME_PROFILE_PCSCF_MISSING` 错误码已定义但没有实际生产调用。

### 修改要求

新增统一 `PcscfDiscoveryContext`，至少包含：

```text
line_id
qmi_device
optional_wds_cid
bearer_path
bearer_settings
profile_candidates
ims_cid
```

统一发现函数返回候选列表及来源，而不是只返回第一个地址：

```text
profile -> bearer_pco -> qmi_wds -> at_cgcontrdp -> dns
```

每个来源均写入结构化 attempt，避免只依靠字符串日志判断失败层。

## 4.3 IPsec 失败会直接终止，没有独立 UDP 重试

**证据等级：B，优先级：P0**

当前 `VolteRegisterAuthenticator` 在收到合法 `Security-Server` 后安装 XFRM 并切换 channel。
如果后续 `run_register()` 返回错误，`connect_family()` 只卸载 XFRM 并返回错误。

只有完全没有可解析 `Security-Server` 时，当前代码才把同一认证事务设置为 UDP 模式。这不等价于 beta2 的“IPsec 失败后从头进行 UDP 注册”。

### 修改要求

将单地址族注册拆成两个明确尝试：

```text
register_ipsec_attempt(context)
register_udp_attempt(context)
```

推荐控制流：

```text
match register_ipsec_attempt {
  success => registered(ipsec),
  failure => {
    cleanup_ipsec_attempt();
    record register_ipsec failed;
    register_udp_attempt()
  }
}
```

UDP 尝试失败后，才允许按 `FailureClass` 决定是否切换下一个 IP 地址族。

## 4.4 连接前主动断开所有已连接 IMS bearer

**证据等级：B，优先级：P1（可能影响首次注册）**

当前 `connect_inner()` 在创建 bearer 前调用 `disconnect_existing_ims_bearers()`。该函数会断开所有
`apn=ims` 且 `connected=yes` 的 bearer，随后 `ensure_ims_bearer()` 再尝试重新连接。

这和 beta2 的“删除 stale disconnected bearer、复用有效 bearer、策略不匹配时重建”不一致，
也会增加 PCO 丢失、`prefix-unavailable` 和基带状态抖动的概率。

### 修改要求

改为统一 reconcile：

1. connected 且 APN、地址族、漫游策略匹配：直接复用。
2. disconnected 且属性匹配：尝试连接一次；失败后删除。
3. 属性不匹配：仅删除该 IMS bearer，不影响非 IMS bearer。
4. 只有当前运行时明确拥有的 bearer，才在会话停止时主动断开。

## 4.5 QMI 查询错误被错误累计为会话死亡

**证据等级：B，优先级：P1**

当前 `live_receive_loop()` 使用 `native_health_failures` 累计所有
`packet_service_status()` 错误，达到 3 次后调用 `cleanup_live_session()`。

这与 beta2 明确的 `query failed; retaining live host IMS state` 相反。

### 修改要求

将返回值提升为显式三态或四态：

```text
Connected
Disconnected
Inconclusive
QueryFailed(error)
```

只有 `Disconnected` 或 `error.is_unsafe_to_retry()` 表示的基带硬故障可以立即重建。
普通 `QueryFailed` 应记录连续次数用于诊断，但不能单独触发 teardown。

## 4.6 默认 MM bearer 缺少运行时监督

**证据等级：B，优先级：P1**

当前健康轮询只在 `session.native_bearer` 存在时执行。默认 ModemManager 路径没有周期性检查：

- bearer object 是否仍存在；
- `connected` 是否仍为 yes；
- interface 是否改变；
- IPv4/IPv6 地址和前缀是否改变；
- P-CSCF 所属地址族是否仍和本地地址一致。

结果是 MM bearer 消失后，SIP socket 的一秒读取超时仍会被当作正常空闲，可能要等到约 3300 秒后的 REGISTER 刷新才触发重建。

### 修改要求

会话建立时保存 `BearerHealthFingerprint`，健康周期重新读取并比较。明确变化时返回
`volte_runtime_health_bearer_changed`，查询暂时失败时保留会话并记录诊断。

## 4.7 BearerRequest 没有使用每线路的实际策略

**证据等级：B，优先级：P1/P2**

当前 `connect_inner()` 使用 `BearerRequest::default()`：

- APN 固定为 `ims`；
- `allow_roaming=false`；
- `profile_id=None`。

线路已有 `roaming_allowed`、数据连接状态等配置，但没有传入 IMS bearer 请求。beta2 则有
`recreating IMS bearer to match roaming policy` 和 `3gpp-profile` 相关路径。

### 修改要求

新增每线路 IMS bearer 配置，至少支持：

- `apn`，默认 `ims`；
- `allow_roaming`，默认继承线路漫游策略；
- 可选 `3gpp_profile_id`；
- 可选预置 P-CSCF 列表；
- 地址族偏好。

没有配置 profile id 时仍允许 APN-only 拨号，不能把 profile id 变成强制项。

## 5. 已对齐或暂时不应重写的部分

## 5.1 每线路启用状态已经成为连接门

当前 `connect_live_for_line()` 不再检查全局 `feature_enabled`、`sms_enabled` 或旧的
`connection_enabled`。实际连接由每线路：

```text
profile.enabled && profile.volte_connection_enabled
```

决定。兼容 API 中仍存在同名全局字段，但响应已经映射为线路状态，不应在后续修复中重新引入全局注册门。

仍然从 `VolteConfig` 读取的内容包括 `ip_family_preference` 和 `voice_enabled`。如果以后需要每张 SIM
独立设置地址族或 IMS 语音开关，再把这两项下沉到 `LineProfileConfig`。

## 5.2 双栈到单栈回退模型基本存在

`bearer.rs` 已实现：

```text
dual-stack -> 根据明确拒绝选择单栈
dual-stack -> 信息不明确时按偏好依次尝试 IPv4/IPv6
```

下一阶段主要工作不是重写回退队列，而是让 MM/native 两种 bearer 路径、data-slot 选择和 P-CSCF
发现共同使用同一 `ImsConnectionPlan`，并把每次尝试的真实结果记录完整。

## 5.3 USIM AKA 主体可以保留

当前 `identity::run_usim_aka()` 复用 VoWiFi QMI UIM 实现，与 beta2 的 logical channel + APDU
方向一致。除非真机日志证明 APDU 状态字、AID 选择或 AUTS 处理不一致，否则不应先重写这一层。

## 6. 建议实施阶段

## 阶段 A：修复初始注册控制流（最高优先级）

目标：消除当前最可能导致“bearer 已建立但 IMS 仍注册失败”的差异。

1. 把 data-slot selector 接入生产流程。
2. 引入统一 P-CSCF 候选列表和来源记录。
3. 补 profile P-CSCF 和默认 MM 路径的直接 WDS 查询。
4. 实现独立的 IPsec -> UDP fallback。
5. 删除连接前无条件断开 connected IMS bearer 的行为。

完成标准：

- 单元测试覆盖每个 data-slot 输入组合。
- 单元测试证明 P-CSCF 来源顺序固定。
- 集成测试证明 IPsec 模拟失败后确实创建新的 UDP channel。
- 日志可区分 `register_ipsec` 和 `register_udp` 两次尝试。

建议 Git 节点：

```text
feat(volte): wire data-slot allocation into bearer selection
feat(volte): complete beta2 pcscf discovery chain
fix(volte): retry registration over plain udp after ipsec failure
fix(volte): reconcile and reuse existing ims bearers
```

## 阶段 B：修复运行时健康与恢复

1. 把 native packet status 改成三态/四态处理。
2. 新增 MM bearer fingerprint 和周期检查。
3. 明确 `bearer_changed`、`qmi_disconnected`、`query_failed` 的恢复边界。
4. 让 supervisor 只对确定性故障消耗五次重试额度。

完成标准：

- 连续普通 QMI 查询失败不会拆除已注册会话。
- 明确 disconnected 会进入 degraded 并触发重连。
- MM bearer 地址变化会触发重建。
- 查询暂时失败在 Web 诊断页可见，但不会显示成已确认断线。

## 阶段 C：线路级 IMS profile 和配置完善

1. 将地址族偏好按需下沉到线路。
2. 增加可选 3GPP profile id。
3. 增加每线路预置 P-CSCF 候选。
4. 关联线路漫游策略。
5. 保持敏感身份和 AKA 材料不进入 API 响应或普通日志。

## 阶段 D：真机验证后再考虑 ISIM EF

只有出现以下证据时才进入此阶段：

- REGISTER 返回明确的 private identity/realm 错误；
- IMSI 派生 IMPI 被拒绝，但卡内 ISIM 可读；
- 运营商要求非 IMSI IMPU；
- P-Associated-URI 不能在注册后修正业务身份。

届时再实现：ISIM AID 选择、EF_IMPI、EF_DOMAIN、EF_IMPU、EF_IST 读取和 IMSI fallback。

## 7. 真机测试矩阵

真机测试必须保存完整结构化 attempts，同时避免记录 IMSI、AKA RAND/AUTN/RES、CK/IK 和 SIP 鉴权响应。

| 编号 | bearer | 地址族 | P-CSCF 来源 | SIP 安全 | 预期 |
|---|---|---|---|---|---|
| T1 | MM | IPv4 | bearer/PCO | IPsec | REGISTER 200 或明确错误 |
| T2 | MM | IPv4 | direct WDS | IPsec | 不运行 AT 即取得 P-CSCF |
| T3 | MM | IPv6 | direct WDS | IPsec | IPv6 REGISTER 成功或明确拒绝 |
| T4 | MM | 双栈 | 任意 | IPsec | 双栈失败后按策略单栈回落 |
| T5 | native QMI | IPv4 | retained CID | IPsec | CID 可跨命令复用 |
| T6 | native QMI | IPv6 | retained CID | IPsec | 获得前缀、网关和 P-CSCF |
| T7 | 任意 | 任意 | 任意 | IPsec 模拟失败 | 自动从头尝试 UDP |
| T8 | 已注册 | 任意 | 任意 | 任意 | packet query error 时保留会话 |
| T9 | 已注册 | 任意 | 任意 | 任意 | explicit disconnected 时重建 |
| T10 | MM 已注册 | 任意 | 任意 | 任意 | bearer 地址变化时重建 |

### 7.1 每次测试必须采集的字段

```text
line_id
data_slot_mode
qmi_device
bearer_backend (mm/native)
bearer_path 或 WDS CID（可脱敏）
requested_ip_type
assigned_ip_family
pcscf_source
pcscf_family
registration_mode
registration_stage
SIP status code
failure_class
cleanup result
```

### 7.2 禁止出现在普通日志中的内容

```text
完整 IMSI / ICCID / IMEI
完整 IMPI / IMPU
RAND / AUTN / AUTS / RES
CK / IK
Authorization / Proxy-Authorization 完整值
SIM APDU 原始鉴权响应
```

## 8. 推荐的数据结构边界

为了防止后续继续把显示状态和实际控制混在一起，建议保持下列边界：

```text
LineImsIntent
  - enabled
  - family_preference
  - roaming_allowed
  - optional profile_id
  - optional pcscf candidates

DataSlotDecision
  - mode
  - ims_endpoint
  - data_endpoint
  - reason

EstablishedImsBearer
  - backend (mm/native)
  - owner
  - path/cid/handle
  - interface
  - settings
  - health fingerprint

PcscfCandidate
  - address
  - source
  - family
  - priority

RegistrationAttempt
  - family
  - pcscf
  - mode (ipsec/udp)
  - stage
  - result
```

`runtime.data_path_mode`、Web 页面阶段和诊断日志均应从这些真实对象派生，不能反向决定底层行为。

## 9. 当前最终判断

当前项目不是“完全没有 VoLTE 实现”，而是协议组件已经较完整，但几个关键控制流仍停留在
“模块存在、运行时未接入”或“只实现成功路径”的状态。最值得优先修复的不是短信编解码或重新编写 AKA，
而是：

1. **实际执行 data-slot/bearer 选择。**
2. **补齐 profile -> bearer -> WDS -> AT 的 P-CSCF 链。**
3. **实现 beta2 明确存在的 IPsec 失败后 plain UDP 重注册。**
4. **复用有效 IMS bearer，避免连接前主动断开。**
5. **按 beta2 三态语义修复健康检查。**

在这些差异修复并完成真机测试前，当前版本仍可能无法连接 VoLTE；即使失败阶段表现为
`prefix-unavailable`、P-CSCF 缺失或 REGISTER 超时，也不能直接归因于 SIM 卡或运营商停机。
