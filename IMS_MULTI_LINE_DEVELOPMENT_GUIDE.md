# IMS 多线路、SIM 覆写、补充业务与 Asterisk 开发指导

> 文档状态：基于 2026-08-09 当前工作树的静态审阅结果，并在 IMS 模块重命名、QCM410 IMS bearer 抽象新增后再次复核。本文是后续开发方案，不表示下述功能已经全部实现或已经通过真机验收。
>
> 本轮重新执行了 `cargo check --tests`，结果通过并产生 25 条现有 dead-code 类警告；`cargo test e911` 通过 1 项定向测试（836 项被过滤）。没有安装 Asterisk、修改 Linphone、连接运营商 IMS、拨打电话或发起任何 E911/紧急呼叫。

## 1. 目标与结论

本轮改造的目标不是继续在 Qualcomm 410 的既有实现上叠加分支，而是形成以下边界：

1. carrier catalog 是只读且可审计的运营商基线。
2. 只有用户明确修改配置时，才为当前 SIM/eSIM profile 生成本地覆写文件。
3. 每次连接都先读取 catalog，再叠加当前 SIM 的覆写；后台自动恢复、重试和探测绝不写覆写文件。
4. `line_id` 标识“当前硬件槽位上的运行线路”，SIM 配置则使用 ICCID 或 EID + profile ICCID 标识，二者不能混用。
5. VoLTE 和 VoWiFi 共用 IMS 业务模型、视频媒体、UT、主叫身份和 trunk 桥接；DATA6 只属于 QCM410 设备驱动。
6. 所有长生命周期运行态均按 `line_id` 隔离，所有用户的运营商差异配置均按 `SimBindingKey` 隔离。

当前代码已经具备较好的每线路运行时、VoLTE 视频与 SIP trunk 基础，但还不能判定“多卡改造完成”。最关键的未完成项是：

- 本地 carrier 覆写仍保存在 `data.db/vowifi_carrier_profiles`，并按 profile/PLMN 全局生效，不按 SIM 隔离。
- `profiles.rs` 仍把覆写发布到进程级 `DB_OVERRIDES`；两张同 PLMN、但配置不同的卡会互相影响。
- 自定义 IMEI 字段已经存在于 carrier profile 模型和 UI，但没有进入 VoWiFi 实际身份生成路径，而且放置层级不正确：IMEI 是 SIM/用户覆写，不应成为整个运营商 profile 的全局属性。
- VoWiFi operator 明确返回 `vowifi_video_not_supported`；当前视频只在 VoLTE 路径启用。
- IMS Ut/XCAP、语音信箱 MWI、完整 CLIP/CLIR/OIP/OIR 尚未实现。
- CS 拨号和 DTMF 可以按 modem 工作，但 CS 还没有能接入 Asterisk 的音频适配器，配置层也明确拒绝 CS trunk 路由。
- QCM410 文件已经移动到 `hardware/devices/qcm410`，并新增 `ImsBearerTransport`、`Qcm410ImsBearer` 与通用 `AT+CGCONTRDP` 解析层；但 VoLTE strategy 仍直接构造 QCM410 driver，设备探测、provider 注入和通用回退尚未接线，`detect_device_kind()` 仍总是返回 `Unknown`。

## 2. 当前实现审阅

### 2.1 已经完成度较高的部分

| 领域 | 当前实现 | 判断 |
| --- | --- | --- |
| 每线路运行时 | `services/line_registry.rs` 为每条线创建独立 VoLTE、VoWiFi、trunk、data proxy、重试锁与 watchdog | 基础完成 |
| 稳定线路身份 | baseband `line_id` 由物理硬件锚点、UIM slot 和 ICCID 生成，不依赖 `/Modem/0` 序号 | baseband 场景可用 |
| VoWiFi 运行态隔离 | TUN、IMS channel、SIM 设备、网络覆写和 operator handle 均以 `line_id` 为键 | 基本完成 |
| VoLTE 视频 | H.264 SDP、视频 RTP relay、初始音视频呼叫、语音到视频/视频到语音 re-INVITE 均有实现和测试 | 代码路径较完整，仍需实机互通 |
| VoLTE/VoWiFi 音频 trunk | Asterisk REGISTER、Digest、主被叫、早期媒体、PRACK、BYE/CANCEL、re-INVITE 和 RTP 中继已有实现 | 代码路径较完整 |
| DTMF | CS 使用 ModemManager/AT；IMS trunk 支持 SIP INFO，并可协商/重写 RFC 4733 telephone-event payload | 代码路径存在，需双模式实测 |
| 按线路配置 | `LineProfileConfig` 已包含 VoLTE、VoWiFi、ViLTE、APN、data、roaming、airplane、SMS/voice policy、trunk | 基础完成 |
| API 线路选择 | modem、call、VoLTE、VoWiFi、trunk 等主要接口显式携带 `line_id` | 基本完成 |
| QCM410 IMS bearer 第一层抽象 | `ImsBearerTransport` 返回设备无关的 `ImsBearerInfo` 与 opaque teardown handle；`Qcm410ImsBearer` 独占 secondary QMI/DATA6，`cgcontrdp.rs` 提供通用 AT context 解析 | 接口和单一实现已完成，运行时 dispatch 未完成 |

### 2.2 仍为部分完成或全局状态的部分

| 位置 | 当前问题 | 目标状态 |
| --- | --- | --- |
| `ims/vowifi/profiles.rs` | `DB_OVERRIDES`、PLMN/IMSI 索引是进程级单例 | resolver 显式接收 SIM scope，不发布可变全局覆写 |
| `LineVowifiConfig.profile_id` | pin 属于 `line_id` 配置；独立读卡器换卡时 `line_id` 不变 | profile pin 放进 SIM 覆写或由 SIM identity 解析 |
| `ProfileIdentityPolicyRecord` | 已有 `device_identity_imei`，但它属于 carrier profile 且运行时未消费 | carrier 只保存“是否/在哪里需要 IMEI”的策略；具体 IMEI 按 SIM 保存 |
| `vowifi/operator.rs` | 音频/re-INVITE 已有，但所有 video offer 都被拒绝 | 复用共享 IMS video 与双 RTP relay |
| call settings handlers | 只有 ModemManager CallWaiting 可读写；forwarding 返回 unsupported | 建立 CS adapter 与 IMS Ut adapter，API 返回明确 capability/source |
| 语音信箱 | 没有 message-summary SUBSCRIBE/NOTIFY，也没有 voicemail number 模型 | 每线路 MWI + 号码发现/拨号 |
| caller ID | IMS 来电可读取 P-Asserted-Identity，但没有完整 privacy/CLIR 控制 | 统一 OIP/OIR 模型并安全映射到 Asterisk |
| `hardware/devices/transport.rs` | `ImsBearerTransport` 已由 QCM410 实现并被 native strategy 调用，但 strategy 仍直接 `let transport = Qcm410ImsBearer`；其余 data/voice/SMS/registration traits 尚无 driver 实现 | 每条线路注入 capability/provider；generic MM 与 QCM410 分开 |
| `LineRuntime.secondary_data` | 所有线路都直接构造 QCM410 `SecondaryDataRuntime` | 只在 QCM410 capability 存在时提供 |
| QCM410 udev fallback | 静态规则按 `wwan*qmi1/qmi2` 匹配，缺少可靠设备型号判断 | 仅被已识别的 QCM410 安装/生成；其他设备不写规则 |
| trunk `codec_allow` | 配置和 UI 已有，但 trunk 代码没有消费 | 在 SDP offer/answer 入口强制执行 |
| trunk INVITE Digest | `with_digest_credentials` 已实现但当前 driver 未接线 | 创建 bridge 时注入或删除无效配置，增加挑战测试 |
| RTP/视频质量 | relay 不转码、不做 jitter buffer，也没有完整 RTCP 对处理 | 明确交给 Asterisk 转码；补 RTP/RTCP 配对或 rtcp-mux |
| CS trunk | CS 调用 API 可用，但 `set_line_voice_path_policy` 明确拒绝 CS | 有音频硬件 adapter 后再宣称 trunk 支持 |

### 2.3 哪些全局对象可以保留

并非所有 `static` 或 `AppState` 字段都应该改成每线路。下列对象可以保留为进程级共享资源：

- 只读 carrier catalog 连接。
- SQLite 连接、认证、系统安全配置、DDNS 和通知通道。
- 以 `line_id` 为键的资源锁表、VoWiFi channel cache 和 operator handle 表。
- 仅用于生成不重复 ID 的原子计数器。
- ModemManager 全局日志级别操作和设备级系统事件。

判定规则是：共享容器可以是全局的，但容器内任何会改变线路行为的值必须以 `line_id` 或 `SimBindingKey` 为键；不可变 catalog cache 可以按 release/profile hash 共享。

## 3. 目标架构

```text
carrier-bundles.sqlite3 (只读)
        │
        │  resolve(access, IMSI/PLMN, optional profile pin)
        ▼
CarrierProfileRecord baseline
        │
        │  overlay only when current SIM has a user file
        ▼
/data/simadmin/ims-overrides/<sim-key-sha256>.json
        │
        │  validate + normalize + build source map
        ▼
EffectiveImsProfile (owned/Arc, immutable)
        │
        ├── VoLTE access provider ── generic MM / QCM410 driver
        ├── VoWiFi access provider ── ePDG/IKE/ESP/TUN
        ├── IMS voice + video + Ut + MWI
        └── per-line Asterisk trunk
```

建议新增四类上下文：

```rust
struct LineContext {
    line_id: String,
    binding: ModemBinding,
    sim: SimIdentitySnapshot,
    capabilities: LineCapabilities,
}

struct SimIdentitySnapshot {
    binding_key: SimBindingKey,
    iccid: String,
    eid: Option<String>,
    profile_iccid: String,
    imsi: Option<String>,
    modem_imei: Option<String>,
}

struct ResolvedImsProfile {
    profile: Arc<CarrierProfileRecord>,
    catalog_release: String,
    origin: ProfileOrigin,
    overridden_fields: Vec<String>,
    sim_binding_hash: Option<String>,
}

struct LineCapabilities {
    cellular_data: bool,
    circuit_voice: bool,
    volte: bool,
    vowifi: bool,
    ims_video: bool,
    ims_ut: bool,
    secondary_qmi: bool,
    local_audio_adapter: bool,
}
```

不要让 `ProfileStore` 再通过全局函数发布可变状态。推荐接口如下：

```rust
trait EffectiveImsProfileResolver: Send + Sync {
    async fn resolve(
        &self,
        line: &LineContext,
        access: CatalogAccessKind,
    ) -> Result<ResolvedImsProfile, ProfileResolveError>;
}
```

短期为了降低改动量，可以继续把最终 record `intern()` 成当前栈需要的 `&'static CarrierProfile`，但必须由每线路 resolve 结果直接传入，不能再写 `DB_OVERRIDES`。长期应把 profile 参数迁移为 `Arc<CarrierProfile>`，消除持续编辑配置时的永久内存泄漏。

## 4. SIM/eSIM 唯一绑定设计

### 4.1 `line_id` 与 `SimBindingKey` 的职责

- `line_id`：运行时、设备操作、锁、TUN、trunk endpoint 和诊断使用。
- `SimBindingKey`：用户覆写、E911 地址、自定义 IMEI 和订阅者业务偏好使用。
- 一张 SIM 从基带 A 移到基带 B 后，`line_id` 可以变化，但必须仍读取同一份 SIM 覆写。
- 同一个独立读卡器换入另一张卡时，reader `line_id` 当前不会变化，因此绝不能用 reader `line_id` 选择覆写。

推荐 key 优先级：

1. eUICC：规范化 EID + 当前启用 profile 的规范化 ICCID。
2. 普通 SIM 或拿不到 EID 的实体 eSIM 卡：规范化 ICCID。
3. ICCID 暂时不可读：不应用任何持久化覆写，返回 `sim_identity_not_ready`；物理槽位 + UIM slot 只能保存内存中的待提交草稿，不能作为永久 key。

不建议把 IMSI 作为主键。IMSI 隐私级别更高，而且运营商 profile 更新时可能变化；也不要使用 modem path、IMEI 或用户可修改的线路标签。

```rust
enum SimBindingKey {
    Iccid { iccid: String },
    EuiccProfile { eid: String, profile_iccid: String },
}
```

规范化要求：

- ICCID：复用 `platform::utils::normalize_iccid`，只保留十进制数字；保存前校验合理长度和校验位策略。
- EID：只保留十进制数字，要求 32 位。
- 哈希输入必须带类型和字段分隔，示例：`euicc-profile\0<EID>\0<ICCID>`。
- 文件名使用 SHA-256 小写十六进制。项目已经依赖 `ring`，可用 `ring::digest::SHA256`，无需为文件名继续使用 MD5。

### 4.2 文件位置与权限

推荐默认目录：

```text
/data/simadmin/ims-overrides/
  7ce1...e91a.json
```

同时支持 `SIMADMIN_IMS_OVERRIDE_DIR`，方便测试隔离。目录权限 `0700`、文件权限 `0600`，owner 与 SimAdmin 服务一致。E911 地址和 IMEI 均不得写入普通 INFO 日志、系统事件 detail 或 API 列表响应。

写入流程必须是：

1. 在同一目录创建随机临时文件，拒绝 symlink。
2. 写入完整 JSON，设置权限，`sync_all()`。
3. 原子 `rename()` 替换目标文件。
4. 对父目录 `sync_all()`。
5. 使用按 binding hash 的锁串行化两个并发用户请求。

读取失败时不能静默回到旧缓存：

- 文件不存在：正常使用 catalog。
- schema 不支持、JSON 损坏、binding 不匹配：阻止该线路自动 IMS 连接并返回可诊断错误。
- 字段校验失败：拒绝保存；不得等到 IKE/REGISTER 阶段才暴露。

### 4.3 覆写文件 schema

文件只保存用户明确修改的字段，不复制完整 catalog。建议 schema：

```json
{
  "schema_version": 1,
  "binding": {
    "kind": "euicc_profile",
    "iccid": "8986...",
    "eid": "8904..."
  },
  "catalog": {
    "base_profile_id": "maxis-50212",
    "last_seen_release": "v7-2026-08"
  },
  "ims": {
    "common": {},
    "volte": {},
    "vowifi": {
      "custom_imei": null
    }
  },
  "services": {
    "video_enabled": true,
    "ut": {}
  },
  "emergency": {
    "e911_address": {
      "address_line_1": "...",
      "address_line_2": null,
      "city": "...",
      "region": "CA",
      "postal_code": "...",
      "country": "US"
    }
  },
  "updated_at": "2026-08-09T00:00:00Z"
}
```

字段语义必须固定：

- key 不存在：继承 catalog。
- key 有具体值：覆盖 catalog。
- `custom_imei: null`：自动使用本机 IMEI；不要用空字符串表示自动。
- `e911_address` 只保存用户输入的地址意图；运营商返回的 validation/provisioning 状态不属于 override，必须放进独立的运行状态存储。
- 用户点击“恢复运营商默认值”时删除对应 override key；文件没有任何有效 override 时删除整个文件。
- catalog 新 release 上线后仍叠加字段级 override，不允许旧覆写中的完整 profile 阻止 catalog 安全更新。

建议为合并结果提供 `source_map`，例如：

```json
{
  "ims.vowifi.epdg.host": "catalog",
  "ims.vowifi.custom_imei": "sim_override",
  "emergency.e911_address": "sim_override"
}
```

这比只返回 `origin=database/catalog` 更适合字段级合并，也便于 UI 明确提示实际来源。

### 4.4 解析和写入边界

统一读取顺序：

```text
识别当前 SIM
→ 按 access 从只读 catalog 解析 baseline
→ 读取该 SimBindingKey 的 override（若存在）
→ 字段级合并
→ access-specific 校验
→ 生成不可变 EffectiveImsProfile
```

只有以下入口可以写文件：

- 用户在 Web UI 明确保存。
- 受认证的 REST API 明确 `PUT/PATCH`。
- 一次性、需要用户确认目标 SIM 的旧配置迁移工具。

以下流程严禁写文件：

- 启动恢复、网络重连、失败重试。
- carrier 自动匹配、ePDG DNS 探测、P-CSCF discovery。
- 从网络响应中学习成功的 REGISTER 变体。
- 自动化拨号、短信、数据消费或 SIM profile 切换。

网络学习值可以写每线路诊断/缓存，但必须与用户覆写类型分开，且不能改变下一次配置优先级。

### 4.5 API 建议

保留 catalog 管理为只读：

```text
GET /api/ims/catalog/profiles
GET /api/ims/catalog/profiles/{profile_id}
```

新增按线路解析的用户接口：

```text
GET    /api/ims/lines/{line_id}/profile
GET    /api/ims/lines/{line_id}/override
PATCH  /api/ims/lines/{line_id}/override
DELETE /api/ims/lines/{line_id}/override
POST   /api/ims/lines/{line_id}/override/validate
```

handler 必须先通过 `line_id` 取得当前 SIM identity，再在锁内二次确认 ICCID/EID 没有因热插拔变化。请求体不允许通过提交另一张卡的 ICCID 绕过当前线路绑定。离线导入应设计单独的管理员流程。

响应至少包含：

- 脱敏的 binding 标识。
- catalog profile/release。
- effective profile。
- 覆写文件是否存在。
- 字段来源映射。
- `restart_required` 或需要重新 IMS 注册的提示。

### 4.6 旧数据迁移

当前 SQLite `vowifi_carrier_profiles` 是全局覆写，无法可靠判断原本属于哪张 SIM，禁止自动复制到所有相同 PLMN 的卡。推荐迁移：

1. 发布版本先停止新的 SQLite profile 写入，但继续只读旧表。
2. UI 显示“未绑定的旧覆写”，要求管理员选择具体 SIM。
3. 选择后以 catalog baseline 重新计算字段差异，写入该 SIM 文件。
4. 对每张需要相同修改的 SIM 分别确认，不做隐式 fan-out。
5. 全部确认后停止运行时读取旧表；保留只读备份一个 release 周期。
6. 原有 `vowifi-profiles.conf.migrated` 仅作为历史备份，不重新引入运行时。

如果旧 row 实际是希望贡献给所有用户的运营商事实，应进入 `carrier_Bundles` 的审核/封存流程，而不是变成本地全局覆写。

## 5. 自定义 IMEI 与 E911

### 5.1 自定义 IMEI

当前模型已有 `device_identity_enabled` 和 `device_identity_imei`，前端也能编辑 15 位值，但 `live.rs` 实际仍只用 IMSI 构造 NAI/IMPI/IMPU，并随机生成 UUID 型 `+sip.instance`。因此现状不能视为“自定义 IMEI 已实现”。

应拆成两部分：

- catalog policy：该运营商是否需要设备身份、使用位置以及 SIP instance 格式。
- SIM override：用户选择的 `custom_imei: Option<String>`。

运行时解析：

```text
custom_imei 有值 → 使用自定义值
custom_imei 为 null → 使用当前 line 的 modem IMEI
两者都没有且 carrier 要求 IMEI → 阻止连接并返回 device_imei_unavailable
carrier 不要求 IMEI → 省略相关字段，不阻止注册
```

校验建议：

- 必须为 15 位数字。
- 推荐校验 IMEI Luhn check digit，并在 API 返回明确错误。
- 不允许把 ICCID、IMSI 或 EID 当 IMEI 使用。
- 不向 modem 下发改写设备 IMEI 的 AT/QMI 命令；这里只改变完全用户态 IMS 报文中的身份值。

需要检查并按 catalog policy 接线的使用点：

1. 运营商要求的 IKE_AUTH 设备身份/vendor attribute。
2. E911/entitlement 请求中的设备标识。
3. SIP REGISTER Contact 的 `+sip.instance`；有的网络要求 UUID，有的要求 `urn:gsma:imei:...`，不能一刀切。
4. 特定运营商 user-agent/template 字段；只有 catalog 明确要求时使用。

不得把 IMEI替换进 EAP-AKA NAI、IMPI 或 IMPU；这些仍由 SIM 的 IMSI/MSISDN 和运营商域生成。日志只记录 `imei_source=custom|modem|omitted`，不得记录原值。

IMEI 使用可能受当地法律、运营商条款和设备所有权约束。UI 应提示该能力仅用于用户拥有并有权测试的设备/实验环境。

### 5.2 E911 地址

结论先说清楚：**当前项目不能替美国 SIM 完成 E911 地址登记，也不能证明紧急呼叫可用；但已经有一部分可以复用的 catalog、导入、SIM 认证和 HTTPS 基础，适合继续实现。** 目前 UI 中的“启用紧急呼叫配置”只是在编辑运营商 profile 元数据，不是把地址提交给运营商。

静态审阅得到的能力边界如下：

| 层次 | 当前实现 | 能否视为 E911 设置 |
| --- | --- | --- |
| 美国订阅提示 | `CarrierProfileRecord::e911_expected()` 用 MCC 310–316 判断，并由 API/UI 显示提示 | 否，只是提示；也不能代替运营商 capability |
| catalog 模型 | `E911Policy{enabled, provider, entitlement_url, websheet_host_policy}` | 否，只描述可能的入口，没有地址、订阅者状态或提交结果 |
| 数据导入 | AOSP CarrierConfig/IPCC importer 可提取 emergency flag 或 entitlement URL，并叠加到完整 catalog baseline | 部分，只导入事实，不执行 URL |
| 本地保存 | `PUT /api/vowifi/carrier-profiles` 把完整 profile 写入 `data.db/vowifi_carrier_profiles` | 否；它按 profile/PLMN 全局生效，也没有地址字段 |
| 前端 | 可编辑 enabled/provider/URL/host policy，并对美国 MCC 显示警告 | 否；没有街道地址表单、地址校验、提交、回读或撤销 |
| provisioning executor | 没有 provider adapter、HTTP entitlement/websheet 会话、callback 或状态机 | 未实现 |
| 紧急 IMS 呼叫 | 没有 `urn:service:sos` 路由、emergency registration、PIDF-LO/Geolocation、紧急 callback 或 CS fallback 策略 | 未实现 |
| 测试 | 只有元数据、MCC 判断和 importer unit test | 未实现运营商 sandbox/lab 验证 |

后端 `profile_record.rs` 还特意让 E911 元数据不阻塞普通 IMS profile 校验，并在测试中将其命名为 `read_only_metadata`。这证明当前代码的产品语义就是“展示信息”，而不是“已经 provision”。前端却在 `e911.enabled=true` 时强制要求 `websheet_host_policy`，与后端的非阻塞规则不一致；在 executor 落地前应删除这项前端伪校验，或把表单明确改名为“E911 catalog 元数据（不执行）”。

### 5.3 当前 carrier catalog 的美国数据覆盖

对工作树内四份 sealed v7 catalog 做只读统计后，结果如下。统计口径是 `carriers.country_iso2='US'`；`emergency=true` 只是 feature flag，不能当作地址登记入口：

| catalog | 美国 profile | VoWiFi ready | emergency=true | provider/entitlement URL |
| --- | ---: | ---: | ---: | ---: |
| `carrier-bundles-ios-ipcc.sqlite3` | 264 | 34 | 9 | 0 |
| `carrier-bundles-iphone16promax-26.6.sqlite3` | 285 | 35 | 3 | 0 |
| `carrier-bundles-pixel-mustang.sqlite3` | 531 | 485 | 288 | 0 |
| 当前 release 的 `carrier-bundles.sqlite3`（`catalog-23g71`） | 285 | 35 | 3 | 0 |

因此，即使现在补一个通用 HTTP client，当前发布 catalog 也没有足够的 provider/endpoint 信息可以直接替多数美国 SIM 提交地址。`profiles.rs` 中确实存在 T-Mobile 和 AT&T 的示例 URL，但这些 profile 被 `#[cfg(test)]` 包围，只用于测试编译，不进入生产 resolver，也不代表相应运营商当前协议已经验证。

Importer 可以从用户合法取得的 CarrierConfig/IPCC XML 中带入 URL，但当前只检查字符串大致以 `http` 开头。一旦未来开始真正请求这些 URL，这个宽松导入规则会从“无害元数据”变成 SSRF/凭据泄漏风险，必须先收紧。

### 5.4 E911 应拆成两个独立 capability

不要用一个 `e911.enabled` 同时表示两个完全不同的结果：

1. `emergency_address_provisioning`：能否向当前 SIM 的运营商登记、查询或更新地址。
2. `emergency_calling`：当前 access 是否能正确建立紧急 IMS 呼叫，包括必要的注册、服务 URN、位置信息、callback 与 fallback。

第一项成功不自动证明第二项成功。反过来，运营商也可能在自己的网页或账户 App 中登记地址，SimAdmin 只需要读取/提示状态，而不应该复制一个未获支持的私有协议。API 和 UI 应分别显示：

```text
address_state = unsupported | unconfigured | needs_user_action | validating |
                provisioned | rejected | stale | unknown
emergency_calling_state = unsupported | not_evaluated | carrier_declared |
                          lab_verified
```

禁止仅凭 `MCC=310..316`、`e911.enabled=true`、HTTP 2xx、VoWiFi REGISTER 成功或本地地址格式正确显示“E911 可用”。

### 5.5 推荐的 provider 架构

新增独立服务，而不是把 E911 HTTP 逻辑塞进 `vowifi/live.rs`：

```text
services/e911/
  mod.rs                 # orchestration、状态机、审计事件
  model.rs               # 地址、capability、provider result
  registry.rs            # sealed catalog policy → provider adapter
  websheet.rs            # 需要用户浏览器交互的安全跳转/callback
  providers/<verified>.rs
  state_store.rs         # 非用户 intent 的 provisioning 状态
```

建议 provider 契约至少表达：

```rust
trait E911Provider {
    fn capability(&self, context: &E911Context) -> E911Capability;
    fn begin_validation(&self, request: ValidateAddressRequest) -> ProviderOperation;
    fn begin_provisioning(&self, request: ProvisionAddressRequest) -> ProviderOperation;
    fn query_status(&self, context: &E911Context) -> ProviderOperation;
}
```

`ProviderOperation` 应能返回“已同步完成”“需要用户打开运营商 websheet”“等待 callback/poll”或“该运营商不支持”，不要假设所有运营商都是同一种 JSON API。只有经过 fixture 和真实非紧急流程确认的 provider 才能注册 native adapter；未知运营商默认走 `metadata_only/unsupported`，如果 catalog 提供可信 websheet，则允许受控的用户交互式流程。

现有 `reqwest + rustls` 可以复用为 HTTPS 基础，现有 SIM/AKA 模块也可以通过窄接口提供运营商明确要求的认证材料。但 EAP-AKA、IMS AKA 和某个运营商 entitlement API 不能因为都用了 SIM 就直接共用报文格式；每个 adapter 必须按实际挑战协议实现，且不得把 AKA secret 暴露给浏览器。

### 5.6 SIM 覆写与运行状态必须分离

E911 地址跟订阅者/SIM 绑定，不跟 MCC/MNC 全局绑定：

- 物理 SIM 使用 ICCID。
- eUICC profile 使用 EID + profile ICCID；切换 profile 后必须重新选择地址。
- ICCID 尚不可读时不允许保存或提交，不能退回 slot/line_id。

SIM override 只保存用户明确输入的地址。后台 validation/provisioning 产生的值单独保存，例如：

```rust
struct E911ProvisioningState {
    binding_hash: String,
    address_fingerprint: String,
    provider_id: String,
    state: E911AddressState,
    carrier_reference: Option<String>,
    confirmed_at: Option<DateTime<Utc>>,
    expires_at: Option<DateTime<Utc>>,
    last_error_code: Option<String>,
}
```

这样网络重试、状态轮询和过期刷新可以更新 state store，却不会改变 override 文件的 mtime/hash，继续满足“只有用户写配置时才生成或修改覆写”。用户改地址后 fingerprint 变化，旧的 `provisioned` 状态立即变成 `stale`，不得沿用。

地址属于高敏感个人信息。override 目录 `0700`、文件 `0600` 只是最低要求；条件允许时应使用设备密钥做 envelope encryption。完整地址只在受认证的详情/编辑接口返回，普通 line/profile 列表只返回 `configured/state/updated_at`。carrier reference 可留在 state store，session cookie、CSRF token、OAuth token 或 AKA 派生材料必须进入 secret storage，并设置短期过期，不得混入可导出的 JSON。

### 5.7 URL、websheet 与提交安全边界

真正执行 entitlement URL 前必须增加以下约束：

- 只允许 HTTPS，正常验证证书和 hostname；禁止关闭 TLS 校验。
- URL 必须来自 sealed catalog 中有 evidence 的 provider policy，或者管理员明确审批的本地 provider 配置；不能直接执行普通 profile 编辑器提交的任意 URL。
- 拒绝 localhost、IP literal、私网、link-local、metadata service 和解析后落入这些网段的地址；每次 redirect 后重新校验。
- redirect 只能落到 provider allow-list；限制次数、body 大小、总时长，并禁止自动把 SIM/IMEI/token 转发到跨域目标。
- `websheet_host_policy` 改成强类型 enum/allow-list，不继续接受任意字符串。
- 所有请求日志只记录 provider、stage、HTTP class 和 request ID，不记录地址、ICCID、IMSI、IMEI、cookie、token 或响应正文。

### 5.8 API 与 UI

建议新增按线路接口：

```text
GET    /api/ims/lines/{line_id}/e911/capability
GET    /api/ims/lines/{line_id}/e911/address
PUT    /api/ims/lines/{line_id}/e911/address
DELETE /api/ims/lines/{line_id}/e911/address
POST   /api/ims/lines/{line_id}/e911/address/validate
POST   /api/ims/lines/{line_id}/e911/address/provision
GET    /api/ims/lines/{line_id}/e911/operations/{operation_id}
POST   /api/ims/lines/{line_id}/e911/operations/{operation_id}/callback
```

`PUT/DELETE` 是用户 intent 写入口；validate/provision/callback 只能更新独立 state store。每个 handler 都必须在开始和提交前二次确认当前 `SimBindingKey`，防止验证期间热插拔后把地址提交给另一张卡。

UI 至少区分“运营商声明需要 E911”“地址已保存在本机”“运营商已确认地址”和“紧急呼叫能力未验证”。现有 carrier profile 页只保留只读 policy/evidence；地址编辑必须放到当前线路/SIM 详情页。provider 要求 websheet 时，由后端生成一次性 operation，前端只打开经过 allow-list 的 URL，不直接接触 SIM 密钥。

### 5.9 非紧急测试方案

测试顺序应为：

1. mock provider 覆盖 accepted/rejected/needs-user-action/timeout/redirect/换卡竞争等状态。
2. 使用无真实地址的固定 fixture 验证序列化、脱敏、fingerprint 和 override/state 分离。
3. 若运营商提供 sandbox，使用其专用测试订阅和测试地址跑 validate/provision/query。
4. 若没有 sandbox，只能由持卡人明确批准后，对其自己的账户和真实地址执行运营商的**非紧急**登记流程；默认不在自动化测试中执行。
5. 验证服务重启、eSIM profile 切换、同 PLMN 双卡、地址更新和撤销不会串卡。

不得为了验收实际拨打 911 或其他紧急号码，也不得用普通测试号码证明 E911。只有运营商提供的测试呼叫流程、合规实验室或当地主管机构明确授权的方式，才可以把 `emergency_calling_state` 提升到 `lab_verified`。

## 6. VoWiFi 视频通话

### 6.1 当前差距

VoLTE 路径已经有可复用的能力：

- `ims/volte/vilte.rs`：H.264 SDP 解析、生成和协商。
- `ims/volte/live.rs`：音频和视频分别创建 `PendingRtpRelay`/`ActiveRtpRelay`。
- `services/trunk/bridge.rs`：识别 audio + video SDP，并转发双方 re-INVITE。
- `services/trunk/access_router.rs`：视频呼叫只选择 `video_enabled` 的 access backend。

VoWiFi 的 `ims/vowifi/operator.rs` 虽然已有音频呼叫、网络侧/Trunk 侧 re-INVITE、DTMF 和 RTP relay，但当前：

- `StartCall` 和 `Renegotiate` 遇到 `offer.video.is_some()` 直接返回 `vowifi_video_not_supported`。
- incoming INVITE 和 network re-INVITE 固定构造 `video: None`。
- `sync_line_video_capabilities()` 只给 VoLTE 设置 video capability。

### 6.2 共享媒体模块

先把接入无关的视频代码从 `ims/volte/vilte.rs` 移到例如：

```text
backend/src/connectivity/core/ims_video.rs
backend/src/connectivity/core/media_relay.rs
```

`services/trunk` 不应继续反向依赖 `modems::ims::volte::vilte`。共享模块至少包含：

- `VideoMediaDescription`、`VideoOffer`。
- SDP `m=video`/`a=rtpmap`/`a=fmtp`/`a=sendrecv` 解析和生成。
- H.264 payload/profile-level-id/packetization-mode 协商。
- audio-only、audio+video、inactive/rejected video 的状态表达。
- 视频 RTP payload type 映射和独立的 RTP/RTCP 地址。

配置层建议把只属于 VoLTE 命名的 `VilteConfig` 迁移为每线路 `ImsVideoConfig`：

```rust
struct ImsVideoConfig {
    enabled: bool,
    volte_enabled: bool,
    vowifi_enabled: bool,
    codec: String,
    video_payload_type: u8,
    h264_fmtp: String,
}
```

旧 `vilte` 字段作为一次 schema migration 输入，迁移后只写新字段。carrier catalog 还应提供 access-specific `video_supported`，最终 capability 是：

```text
用户启用
AND catalog 声明当前 access 支持
AND IMS 注册声明 MMTel video feature
AND trunk/Asterisk SDP 有共同 H.264 参数
```

### 6.3 VoWiFi operator 改造

参照 VoLTE `LiveOperatorCall` 的做法，为 `vowifi::operator::VoiceCall` 增加：

```text
pending_video_relay
active_video_relay
operator_video_local
internal_video_local
```

所有对话方向都要覆盖：

1. Asterisk 发起初始音视频 INVITE。
2. 运营商发起初始音视频 INVITE。
3. Asterisk 发起 audio → audio+video re-INVITE。
4. 运营商发起 audio → audio+video re-INVITE。
5. 任一方发起 audio+video → audio downgrade。
6. 488/491/超时后保留已确认的原媒体会话。
7. BYE/CANCEL/线路断开时同时释放两组 relay。

实现要点：

- 从 ePDG/TUN 侧地址绑定 operator video socket，从 Asterisk 可达地址绑定 internal video socket。
- SDP 中 audio 和 video 可以有不同的 media-level `c=`，不可假设共用 session address。
- H.264 不在 SimAdmin 内转码；如果 payload type 不同，只改 RTP PT，不改 H.264 bitstream。
- 先完成双方 SDP 协商再 activate relay，避免 RTP 发往尚未确认的地址。
- REGISTER Contact 在 carrier policy 允许时声明 `video` 与 MMTel service capability。
- `sync_line_video_capabilities()` 分别设置 VoLTE/VoWiFi，不把一条 access 的 ready 状态误当另一条。

### 6.4 视频验收条件

- Asterisk/Linphone 可以初始发起音视频呼叫。
- 通话中升级视频和降级语音均成功，Call-ID/tags/dialog 不变化。
- 对端拒绝视频时语音保持。
- VoWiFi 视频关闭时 router 不选择 VoWiFi；可按策略选择 VoLTE，不能发送后才报错。
- 两条线路同时视频时 socket、SSRC、payload mapping 和计数完全隔离。
- RTP 和 RTCP 都有明确处理；若第一版只支持 RTP，状态和文档必须报告 RTCP unsupported，而不是宣称完整视频支持。

## 7. UT、语音信箱与来电显示

### 7.1 不要把所有能力继续塞进 ModemManager handler

呼叫等待、呼叫转移等是订阅者业务。CS 网络可能通过 modem/AT 提供，IMS 网络通常通过 Ut/XCAP 提供。建议新增统一 service：

```text
backend/src/connectivity/core/supplementary.rs
backend/src/services/supplementary/mod.rs
backend/src/services/supplementary/cs.rs
backend/src/services/supplementary/ims_ut.rs
backend/src/services/supplementary/mwi.rs
```

```rust
trait SupplementaryServiceProvider {
    async fn capabilities(&self) -> SupplementaryCapabilities;
    async fn get_call_waiting(&self) -> Result<CallWaiting, UtError>;
    async fn set_call_waiting(&self, value: CallWaiting) -> Result<CallWaiting, UtError>;
    async fn get_forwarding(&self) -> Result<ForwardingRules, UtError>;
    async fn set_forwarding(&self, value: ForwardingRules) -> Result<ForwardingRules, UtError>;
    async fn get_identity_presentation(&self) -> Result<IdentityRules, UtError>;
    async fn set_identity_presentation(&self, value: IdentityRules) -> Result<IdentityRules, UtError>;
}
```

provider 由当前线路和 access 决定：

- `auto`：当前已注册 IMS access 优先；否则查询 CS capability。
- `volte`：通过 IMS bearer 访问 Ut/XCAP。
- `vowifi`：通过 ePDG child SA/TUN 访问 Ut/XCAP。
- `cs`：只调用 modem 真正声明支持的接口；不要默认发送 MMI/USSD 字符串。

这些设置通常是运营商侧的订阅者状态，不是“每个 access 各有一份”。VoLTE 与 VoWiFi 应共享同一个领域模型，只是传输路径不同。切换 access 后重新读取网络状态，不将旧本地缓存当权威值。

### 7.2 IMS Ut/XCAP

按照 3GPP TS 24.623/相关 XCAP 规范实现，并让 catalog 提供：

- XCAP root/Ut server URL。
- authentication scheme、realm、TLS policy。
- simservs document selector 和运营商 namespace 差异。
- 是否允许 partial document update/ETag。

首批服务映射：

| 用户功能 | IMS simservs/XCAP 领域 | API 模型 |
| --- | --- | --- |
| 呼叫等待 | communication-waiting | enabled |
| 无条件转移 | communication-diversion | unconditional rule |
| 遇忙转移 | communication-diversion | busy condition |
| 无应答转移 | communication-diversion | no-answer + timer |
| 不可达转移 | communication-diversion | not-reachable condition |
| 来电显示 | originating-identity-presentation | OIP/CLIP |
| 隐藏主叫 | originating-identity-presentation-restriction | OIR/CLIR |

写入必须使用 GET → parse → 条件更新 → GET 回读确认：

- 保存 ETag，PUT/PATCH 使用 `If-Match`，冲突返回 409 风格的业务错误。
- 保留未知 XML namespace/运营商扩展，不用重新序列化时删掉未知节点。
- 转移号码规范化为 E.164，但允许 catalog 明确要求的 URI 格式。
- no-reply timer 在运营商允许范围内校验。
- 不在日志输出完整 XCAP 文档、IMPU、转移号码或鉴权头。

### 7.3 呼叫等待的媒体行为

只修改网络开关还不够。SIP 对话层还要能同时维护至少两个 dialog，并支持：

- 第二个来电的 180/183/200/486。
- 当前通话 hold/resume（SDP `sendonly`/`inactive`/`sendrecv`）。
- Asterisk/Linphone 的 waiting UI 与选择接听。
- 每个 dialog 独立 DTMF、re-INVITE 和 RTP relay。

当前 operator 内部使用 call map，具备并发对话的结构基础，但需要补资源上限和双通话测试。资源不足时应明确返回 486/503，不能覆盖第一个 call。

### 7.4 语音信箱

语音信箱应拆成三项，避免把“号码”和“消息”混为一谈：

1. MWI：IMS REGISTER 后发送 `SUBSCRIBE`，`Event: message-summary`，处理 `NOTIFY` 的 `Messages-Waiting` 与 `Voice-Message`。
2. 语音信箱号码：优先读取 SIM EF/ModemManager，次选 catalog，最后允许 SIM 覆写。
3. 拨打语音信箱：走当前 `VoiceAccessRouter`，记录为普通通话，但 UI 标记 voicemail。

Asterisk 自带 voicemail 是 PBX 本地能力，与运营商语音信箱不同；测试文档和 UI 必须分别命名。MWI 订阅状态按 `line_id` 保存，号码配置按 `SimBindingKey` 保存。

### 7.5 Caller ID 与隐私

统一解析顺序建议为：

```text
Privacy:id/anonymous
→ P-Asserted-Identity
→ P-Preferred-Identity（仅本端发出）
→ From
→ Remote-Party-ID（兼容 Asterisk）
```

- 收到 `Privacy: id` 时，即使 PAI 有真实号码，也只能向普通 UI/Linphone显示“匿名”；真实值不得进入 call history 或普通日志。
- IMS → Asterisk：根据隐私状态映射 `P-Asserted-Identity`、`Privacy` 和 caller ID presentation。
- Asterisk → IMS：只有当前 trunk endpoint 被允许时才接受其 caller ID；最终身份仍受 SIM/运营商策略约束。
- CLIR/OIR 是网络状态，不应只在本地改 From header 后假装成功。

### 7.6 新 API

```text
GET /api/ims/lines/{line_id}/supplementary/capabilities
GET/PUT /api/ims/lines/{line_id}/supplementary/call-waiting
GET/PUT /api/ims/lines/{line_id}/supplementary/forwarding
GET/PUT /api/ims/lines/{line_id}/supplementary/identity
GET /api/ims/lines/{line_id}/voicemail
POST /api/ims/lines/{line_id}/voicemail/dial
```

响应统一返回 `source=ims_ut|modemmanager|at|unsupported`、`access=volte|vowifi|cs`、`network_confirmed` 和 `last_error`。不要再对 unsupported 功能返回 HTTP 200 + 模糊英文字符串；至少提供稳定业务错误码供前端区分。

## 8. 多线路改造完成度与检查清单

### 8.1 当前判断

当前可以判断为“主要运行时已按线路化，但配置身份、IMS profile resolver 和若干外围持久化仍未完全按卡化”。在以下门槛全部通过前，不建议标记多卡项目完成。

### 8.2 必须逐项检查

| 检查域 | 当前状态 | 完成标准 |
| --- | --- | --- |
| line registry | 已按线 | 同线复用 runtime，拔卡标 absent，不把 B 卡操作落到 A 卡 |
| baseband line identity | 较好 | 物理 slot + UIM + ICCID 稳定，hotplug/renumber 测试通过 |
| standalone reader | 部分 | reader 换卡后 profile/override 按新 ICCID/EID 切换 |
| VoWiFi SIM auth | 已按线 | 所有 QMI/UIM/ModemManager identity lookup 必须显式 line mapping |
| VoWiFi caches | 已按线 key | 清除 A 不影响 B；禁止空 `line_id` 写入 |
| carrier override | 未完成 | 无全局可变 overlay；相同 PLMN 两张卡可有不同 effective profile |
| config | 大部按线 | 所有业务 intent 在 `LineProfileConfig` 或 SIM override；真正设备级配置留全局 |
| SQLite runtime tables | 混合 | 新写入的 SMS/call/diagnostic 必须有 line；历史 nullable row 只作为 legacy |
| trunk | 已按线 runtime | AOR/auth/local port/Call-ID/RTP socket 不碰撞，可两线并发 |
| active calls | 部分 | key 必须包含 line 或保证 call object 全局唯一，任何查询都校验 ownership |
| notifications | 部分 | SMS/call/automation 通知带 line；设备级通知允许 line 为 null |
| automation | 较好 | 非设备级任务必须有 target，运行锁按 target scope |
| supplementary | 未实现 | cache/订阅/dialog 全部按 line，网络订阅状态按 SIM 解释 |
| device driver | 部分 | 已有 QCM410 `ImsBearerTransport` seam；generic provider 不引用 DATA6，QCM410 provider 独占 DATA6，runtime 按探测结果注入 |

### 8.3 自动化审计建议

在 CI 增加静态/契约测试：

- 所有 `/api/modem`、`/api/ims`、`/api/vowifi`、`/api/volte` 变更接口必须包含 `{line_id}`，catalog 等真正全局只读接口除外。
- 新的 line-scoped DB insert 拒绝空 `line_id`。
- `LineRuntime` 中不得直接出现设备型号模块类型；只能保存 provider trait object/capability。
- 禁止业务代码调用“第一个 modem”“modem 0”或空 line fallback。
- 对 static map 检查 value 是否不可变或 key 是否包含 line。
- 两线路 fixture 使用相同 PLMN、不同 override，覆盖所有 resolver/连接入口。

### 8.4 QCM410 半脱钩

最新改动已经不只是移动文件，以下第一层边界是正确的，应当保留：

- `hardware/devices/transport.rs` 定义设备无关的 `ImsBearerInfo`、`ImsBearerHandle` 和 `ImsBearerTransport`。
- `hardware/devices/qcm410/ims_bearer.rs` 负责 secondary QMI/DATA6、retained WDS session、netdev 配置和 teardown。
- `hardware/cellular/cgcontrdp.rs` 把 `AT+CGCONTRDP` 的地址、网关、DNS、prefix 和 P-CSCF 解析放到设备无关层。
- `ims/volte/native_bearer.rs` 只负责 family attempt strategy、错误分类和投影到 `BearerConnection`。

但 `native_bearer.rs` 仍直接 import 并构造 `Qcm410ImsBearer`，所以 protocol strategy 仍然知道具体设备；`LineRuntime.secondary_data` 也仍是 QCM410 类型。下一步应在 line 创建时完成 detection + capability 装配，并复用已经存在的 trait，而不是再造一套平行 IMS bearer 接口：

```rust
struct DeviceCapabilities {
    ims_bearer: Option<Arc<dyn ErasedImsBearerTransport>>,
    data: Option<Arc<dyn DataProvider>>,
    circuit_voice: Option<Arc<dyn CircuitVoiceProvider>>,
    sim_identity: Arc<dyn SimIdentityProvider>,
}

trait CellularBearerProvider {}
trait CircuitVoiceProvider {}
trait SimIdentityProvider {}
```

当前 `ImsBearerTransport` 带 associated error 且使用 async method，不能直接作为这里的 trait object；实现时可以增加 object-safe erased adapter，或让 `DeviceCapabilities` 保存一个小型 enum dispatcher。无论选哪种，都应把 concrete `Qcm410ImsBearer` 的选择移出 protocol strategy。

每个 `LineRuntime` 持有当前设备实际支持的 provider。推荐 backend：

- `GenericModemManagerProvider`：普通 QMI/MBIM/USB modem，data 与可用的 CS 能力走 ModemManager。
- `Qcm410Provider`：注册现有 `Qcm410ImsBearer`，额外实现 secondary QMI/DATA6 与该固件的数据 bearer 分配策略。
- `ReaderVowifiProvider`：只有 SIM auth + VoWiFi，无 cellular bearer/CS/VoLTE。

`detect_device_kind()` 必须用可测试的 sysfs/DT compatible/udev fact 判断，并允许只读配置 override；未知设备只能进入 generic 路径。`secondary-qmi-init`、systemd ExecCondition、udev 规则生成和 `SecondaryDataRuntime` 创建都必须要求明确 `Qcm410` capability。

特别是静态 `99-simadmin-secondary-qmi.rules` 不应在所有设备上无条件安装。安装阶段应先运行无副作用的设备探测，只有确认 QCM410 才安装；运行时规则只写实际由 QCM410 initializer 成功绑定的端口。未知设备必须“不写规则、不绑 DATA6、不隐藏 ModemManager 端口”。

## 9. SIP trunk 与通话链路优化

### 9.1 当前可复用能力

当前 trunk 已具备：

- 每线路 `TrunkRuntime` 和独立 UDP local port。
- static peer / outbound REGISTER。
- REGISTER Digest challenge、refresh、unregister 和 bounded backoff。
- 双向 INVITE、CANCEL、ACK、BYE、re-INVITE。
- operator ↔ Asterisk 的音频/video offer 模型。
- SIP INFO DTMF 和 RFC 4733 telephone-event payload mapping。
- VoLTE/VoWiFi access router；呼叫开始前可按 policy 选择 access。

这些代码应继续保留，而不是改为通过 Asterisk 命令行控制通话。

### 9.2 必须补齐的点

1. **消费 `codec_allow`**：保存配置时校验 codec 名；生成/转发 SDP 时只保留 allow-list 与运营商共同支持项。无交集返回 488。
2. **接线 INVITE Digest**：创建 `TrunkBridge` 时按配置调用现有 `with_digest_credentials`，或删除无效分支；增加 Asterisk 对 INVITE challenge 的集成测试。
3. **每线标识**：AOR、auth username、incoming/outgoing binding、local port 必须唯一。保存时除 port 外再检查 endpoint/binding 冲突。
4. **传输安全**：当前是 UDP + MD5 Digest。局域网实验可用；生产需要明确是否支持 TCP/TLS、证书验证和更强 Digest，不能在 UI 标记为加密。
5. **RTP/RTCP**：现有 relay 只按 RTP v2 处理数据报。视频和质量统计应增加 RTCP pair 或明确支持 `rtcp-mux`。
6. **媒体资源上限**：每线路最大 dialogs、audio relays、video relays 应配置并可观测，防止多来电耗尽端口/内存。
7. **路由保持**：access router 在 call answered 后固定 owner 是正确的。不要在已建立 dialog 中静默把 VoWiFi 切到 VoLTE；跨 access continuity 是另一项完整功能。
8. **可观测性**：状态增加选中 access、codec、audio/video endpoints（脱敏）、DTMF method、re-INVITE result、RTP/RTCP counters。

### 9.3 CS、VoLTE、VoWiFi 的真实边界

- CS：当前可以通过 ModemManager/AT 拨号、接听、挂断和发 DTMF，但没有 baseband PCM/USB audio/ALSA 到 RTP 的适配器。因此只能称为“直接 CS 通话控制”，不能称为“Asterisk CS trunk”。
- VoLTE：operator SIP channel、RTP relay 和视频 relay 已接入 trunk，仍需运营商实机验证。
- VoWiFi：音频和 DTMF 已接入 trunk；视频需按第 6 节补齐。

要让 CS 进入 trunk，必须新增 `CircuitVoiceProvider` 和 `CircuitMediaAdapter`，明确音频来源，例如 USB audio、PCM/I2S、厂商 voice tunnel 或可用的 modem audio interface。只有信令控制而没有双向音频数据面时，不得让 UI 显示 CS trunk ready。

### 9.4 VoLTE/ViLTE 转换

这里的“转换”应定义为同一 IMS dialog 的媒体升级/降级，而不是新建第二通电话：

```text
audio established
→ re-INVITE(audio + video)
→ 2xx + ACK
→ activate video relay
→ re-INVITE(audio only)
→ 2xx + ACK
→ release video relay, keep audio relay
```

必须处理 glare（491）、488、超时和对端先发 re-INVITE。失败时保留原 media，不把整个 call 标为 ended。相同逻辑应同时服务 VoLTE 和 VoWiFi。

### 9.5 DTMF 策略

优先级建议按实际协商决定：

1. 双方 SDP 都有 telephone-event：RTP RFC 4733，必要时映射 payload type。
2. 任一方没有 telephone-event、但双方支持 INFO：`application/dtmf-relay`。
3. CS：ModemManager `SendDtmf` 或受控 AT `+VTS`。

测试数字覆盖 `0-9`、`*`、`#`、`A-D`，校验 duration 和重复按键。不要同时发送 RTP event 和 SIP INFO，否则 IVR 会收到双键。

## 10. 分阶段实施顺序

### Phase 0：冻结基线与增加证据

目标：在改变 resolver 前，建立可比较基线。

- 保留当前 `cargo check --tests` 通过状态。
- 给现有全局 profile resolver、VoWiFi video reject、UT unsupported 和 CS trunk reject 增加显式契约测试。
- 状态响应增加 config/catalog release、line_id 和脱敏 SIM binding hash。
- 记录现有 API/前端/Bruno 契约，不同时进行无关 UI 重构。

完成条件：失败能够定位到 line/access/stage，日志不含 IMSI、ICCID、IMEI、EID、E911 地址、Digest/AKA 材料。

### Phase 1：SIM identity 与只读 override store

目标：只实现安全的文件读写和 merge，不立即替换 live path。

- 新增 `SimBindingKey`、identity provider、`SimOverrideStore`、schema 和原子写测试。
- 使用临时目录测试权限、损坏文件、schema 版本、symlink、并发写和断电式中断。
- 新增 effective profile dry-run API，和当前 live resolver 并排比较。
- eUICC EID 通过 line-scoped `EsimSupervisor` 读取并短期缓存；profile ICCID 取当前启用 profile。

完成条件：移动普通 SIM 到另一 modem 后 hash 不变；同一实体 eSIM 卡切换 profile 后 hash 变化；身份未知时 fail closed。

### Phase 2：切换 profile 解析和用户 API

目标：catalog → SIM override 成为唯一 live 解析链。

- 替换 `ProfileStore.save/delete` 的 SQLite 写路径。
- 移除 `DB_OVERRIDES` 对 live 连接的影响。
- carrier profile 页面改为“查看 catalog + 编辑当前 SIM 覆写”。
- profile import 必须要求选择 SIM；全局事实只能导出给 carrier_Bundles。
- 实现“恢复 catalog 默认”删除 key/文件。

完成条件：同 PLMN 的 line A/B 使用不同 ePDG/IMEI 时互不影响；自动重试不会改变任何 override 文件 mtime/hash。

### Phase 3：IMEI 与 E911

目标：让用户态 IMEI 真正进入指定协议位置，并建立 E911 数据安全边界。

- 实现 `EffectiveDeviceIdentity`，空值回退本机 IMEI。
- 按 carrier policy 接入 IKE/entitlement/`+sip.instance`，禁止替换 IMSI身份。
- E911 地址 CRUD 写 SIM override；运营商非紧急 validation/provisioning 状态写独立 state store，普通响应只返回脱敏状态。
- 增加“SIM 拔出/换卡后不读取上一张卡地址”的测试。

完成条件：抓包只在 policy 指定字段看到选定 IMEI；日志/API 普通页面无法看到原值；不进行紧急呼叫测试。

### Phase 4：多线路硬门槛与设备 provider

目标：完成 QCM410 代码位置和运行行为的脱钩。

- `LineRuntime` 注入 provider/capabilities，不直接持有 QCM410 类型。
- 实现 generic MM、QCM410 和 reader provider。
- 接线设备探测；未知设备不运行 secondary-QMI。
- udev/install/systemd 三处都增加 QCM410 判断。
- 清理不带 line 的业务 API、写路径和 fallback。

完成条件：generic MBIM/QMI fixture 完全不访问 DATA6；QCM410 fixture 保持 IMS + data 分配逻辑；两 baseband 并发操作各自串行、相互不阻塞。

### Phase 5：共享 IMS video 与 VoWiFi video

目标：VoLTE/VoWiFi 使用同一视频领域和 trunk bridge。

- 移动共享 SDP/video 类型到 `connectivity/core`。
- 给 VoWiFi 增加视频 relay 和所有 re-INVITE 路径。
- 扩展 per-access capability 和前端配置。
- Asterisk/Linphone 增加 H.264 lab 配置。

完成条件：第 6.4 节全部通过，并且语音-only 回归不增加视频 socket。

### Phase 6：UT、MWI、caller ID

目标：实现真正的网络确认业务，而非本地假状态。

- 先做 read-only capability 和 GET。
- 再做 call waiting、forwarding、identity PUT + 回读。
- 最后做 MWI SUBSCRIBE/NOTIFY 与 voicemail dial。
- 给 VoLTE/VoWiFi transport 分别做相同 contract tests。

完成条件：access 切换后读取同一网络规则；未知 XML 扩展不丢失；隐私来电不泄露号码。

### Phase 7：trunk hardening 与 CS adapter

目标：把当前可工作的 trunk 路径变成可验收产品能力。

- 执行 codec policy、INVITE Digest、RTCP、资源上限和 per-line generator。
- 建立两线 Asterisk lab。
- CS adapter 仅在找到真实双向音频接口后开发；否则继续返回明确 unsupported。

完成条件：第 11 节完整矩阵通过，且每条线路的失败只影响自身 trunk。

## 11. 测试与验收方案

### 11.1 测试层级

按以下顺序执行，前一层失败时不要直接用实机绕过：

1. 纯逻辑/unit：merge、identity、SDP、XCAP XML、dialog、DTMF、路由。
2. backend integration：mock D-Bus/QMI/HTTP/XCAP/SIP，验证 ownership 和并发。
3. WSL Asterisk：真实 PJSIP REGISTER/dialog/RTP，但不连接运营商。
4. Windows Linphone：真实软电话音频/视频/DTMF互通。
5. 目标设备 + 运营商：真实 ePDG/IMS bearer、注册和普通号码通话。

### 11.2 建议的仓库检查

```bash
cd backend
cargo fmt --check
cargo check --tests
cargo test
cargo clippy --all-targets -- -D warnings

cd ../frontend
pnpm type-check
pnpm lint
pnpm build
```

当前工作树 `cargo check --tests` 已通过，但 24 条 dead-code 警告说明新的 device transport seam 仍未接线，因此此时不应加 `-D warnings` 作为发布门槛，直到 Phase 4 清理完成。

必须新增的核心用例：

- catalog record 不被 merge 修改。
- 无 override 文件时不创建文件。
- 自动 restore/retry 后目录内容和 mtime 不变。
- 同 PLMN、不同 ICCID 的两张卡得到不同 effective profile。
- 同一 ICCID 移动 modem 后覆写仍生效。
- reader 换卡、eSIM 切 profile 后不继承旧配置。
- custom IMEI value/auto/missing-required 三态。
- 两条线同时 REGISTER、呼叫、DTMF、re-INVITE。
- 清理 line A runtime/cache 不改变 line B。
- XCAP ETag 冲突、401/407、TLS 失败、未知 XML 节点保留。

### 11.3 WSL Asterisk lab

仓库已有 `scripts/asterisk/`，但当前模板是单线路 `41000`、Linphone `6108`，主要覆盖音频与 SIP INFO DTMF。不要把现有 ignored test 当作多线/视频/运营商验收。

开发机准备步骤应由用户在维护窗口显式执行：

```bash
sudo apt-get update
sudo apt-get install -y asterisk tcpdump sngrep
sudo scripts/asterisk/install-simadmin-lab.sh
sudo asterisk -rx 'core show version'
sudo asterisk -rx 'pjsip show endpoints'
sudo asterisk -rx 'pjsip show contacts'
```

随后运行现有单线 live test：

```bash
sudo scripts/asterisk/run-live-trunk-test.sh
```

Phase 7 应把 lab 生成器扩展为每线路一组对象，例如：

```text
sim-line-a: AOR/auth/endpoint 41001, local port 5062
sim-line-b: AOR/auth/endpoint 41002, local port 5064
linphone:   AOR/auth/endpoint 6108
```

PJSIP endpoint 要求：

- `direct_media=no`。
- `rtp_symmetric=yes`、`force_rport=yes`、`rewrite_contact=yes`。
- audio 至少对齐 `ulaw/alaw`；运营商 AMR/EVS 如需转换由 Asterisk codec 能力决定。
- video 添加 `h264`，并确认 Asterisk build 和 Linphone 都支持共同 profile；无共同 fmtp 时返回 488。
- DTMF 分别测试 `rfc4733` 和 `info`，不要只保留当前模板的 `dtmf_mode=info`。
- 配置固定 RTP 端口范围并限制 WSL/Windows 防火墙范围。

WSL2 网络需要先确认是 mirrored 还是 NAT：

```bash
hostname -I
ip route show default
ss -lunp | grep -E ':(8060|5060)\b'
```

Windows 到 WSL 需开放实验 SIP 端口和 RTP 范围；只对本机/实验网开放，不要把无 TLS 的 Asterisk 暴露到公网。WSL IP 在 NAT 模式重启后可能变化，现有 `configure-linphone-lab.ps1` 可重新写 Linphone account。

### 11.4 Windows Linphone

可以使用现有脚本，也可以在 Linphone UI 手工建立：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\asterisk\configure-linphone-lab.ps1 -Distro debian -Restart
```

验收时记录：

- Linphone 注册到 Asterisk 的 contact。
- Linphone → Asterisk → SimAdmin → operator 的 outgoing call。
- operator → SimAdmin → Asterisk → Linphone 的 incoming call。
- 180/183、PRACK、200、ACK、BYE/CANCEL 的顺序。
- 双向音频 RTP packet 增长且主观可听。
- 初始视频、audio → video、video → audio。
- Linphone DTMF 发出一次，operator/IVR 只收到一次。
- 匿名来电显示为匿名，普通来电显示正确号码。

证据建议：

```bash
sudo asterisk -rvvv
pjsip set logger on
rtp set debug on
sudo tcpdump -i any -s 0 -w /tmp/simadmin-lab.pcap 'udp portrange 5000-20000'
```

pcap 含号码、SIP URI、SDP 和媒体元数据，必须作为敏感测试制品保存、脱敏和定期删除，不得提交仓库。

### 11.5 多线路并发矩阵

| 场景 | line A | line B | 通过条件 |
| --- | --- | --- | --- |
| 不同运营商 | VoLTE | VoWiFi | 两条线各自注册，profile 不串线 |
| 相同 PLMN | override A | override B | ePDG/IMEI/E911 各自独立 |
| 同时呼叫 | audio | audio | dialog/RTP/DTMF 无碰撞 |
| 同时视频 | VoLTE video | VoWiFi video | 双视频 relay 和 payload 独立 |
| 一线断网 | 通话中断 | 正常通话 | B 不重注册、不释放 RTP |
| 热插拔 | 拔出/换卡 | 保持在线 | A 旧配置不落到新卡，B 无影响 |
| 服务重启 | 自动恢复 | 自动恢复 | 各自 intent 恢复，自动化不写 override |

### 11.6 UT/语音信箱矩阵

每项分别在 VoLTE 和 VoWiFi access 执行：

- 查询/开关呼叫等待，切换 access 后状态一致。
- 无条件、遇忙、无应答、不可达转移；验证取消规则。
- no-reply timeout 边界。
- CLIP/OIP、CLIR/OIR 与 Asterisk caller presentation。
- MWI 从无消息 → 有消息 → 已清除。
- 语音信箱号码来源优先级和拨号。
- 401/403/404/409/5xx、超时和 access 断开时错误可理解且不写假状态。

### 11.7 真实普通号码通话

用户提供的测试号码为 `+60 1112023012`。该步骤只能由有权使用设备和 SIM 的操作者手工批准执行，执行前确认号码所有权/同意、漫游状态、资费和当地法规。自动化测试不得循环拨打该号码。

建议只在前述实验室测试全部通过后执行：

1. 单次 VoLTE outgoing call：拨号、振铃、接听、双向音频、DTMF、远端/本端挂断。
2. 单次 VoWiFi outgoing call：同上。
3. 对端明确支持视频时再做视频；普通语音号码不支持时，验证 488 后语音保持即可。
4. 由对端回拨，验证 incoming caller ID、接听、拒接和未接记录。
5. 每项完成后立即检查 line-specific call history、trunk metrics 和运营商注册状态。

日志和报告只写掩码号码 `+60 11****3012`。不要把完整号码写入公开 issue、仓库 fixture 或普通诊断包。

### 11.8 发布验收门槛

满足以下全部条件后才可以标记功能完成：

- 所有 unit/integration/frontend 检查通过，无新增 warning。
- 两条相同 PLMN 的 SIM 可同时使用不同覆写。
- 普通 SIM 跨 modem、实体 eSIM 卡切 profile 的绑定行为正确。
- VoLTE/VoWiFi 音频、视频和双向 DTMF 完成 lab + 实机验证。
- UT 状态由网络回读确认，不由本地 UI 猜测。
- caller privacy 在 API、日志、Asterisk、Linphone 四处一致。
- Asterisk 两线路并发完成，停用/断开一线不影响另一线。
- generic 非 QCM410 设备不写 udev DATA6 规则、不绑定 secondary QMI。
- E911 只完成非紧急 provisioning 验证，没有实际紧急呼叫。

## 12. 建议修改的文件映射

| 改造 | 主要文件 |
| --- | --- |
| SIM identity/binding | `hardware/cellular/modem_manager.rs`、`hardware/sim/esim.rs`、新 `services/sim_identity.rs` |
| override store/merge | 新 `connectivity/modems/ims/profile_override.rs`、`vowifi/profile_store.rs`、`vowifi/profiles.rs` |
| config/API/UI | `platform/config.rs`、`api/models.rs`、`api/handlers.rs`、`main.rs`、`frontend/src/api/*`、carrier/line 页面 |
| custom IMEI | `vowifi/profile_record.rs`、`vowifi/live.rs`、IKE/entitlement 使用点、override schema |
| E911 | `vowifi/profile_record.rs` 的只读 policy、新 `services/e911/*` provider/state store、SIM override、按线路 API/UI |
| shared video | 新 `connectivity/core/ims_video.rs`、`volte/vilte.rs`、`volte/live.rs`、`vowifi/operator.rs`、trunk bridge/router |
| UT/MWI | 新 `connectivity/core/supplementary.rs` 与 `services/supplementary/*`、两个 IMS access transport |
| caller ID/privacy | 两个 IMS operator、trunk bridge/SIP、call history/API/UI |
| device decoupling | `hardware/devices/*`、`services/line_registry.rs`、data/VoLTE handlers、secondary-qmi service/udev/install |
| trunk hardening | `services/trunk/{driver,bridge,sip,runtime}.rs`、`scripts/asterisk/*` |
| contract tests | backend 各模块 test、frontend type/API、`bruno-api/` |

## 13. 实施时应避免的捷径

- 不要把整个 catalog profile 复制到每张 SIM 的文件中。
- 不要让自动连接“学习成功配置”后改写用户覆写。
- 不要按 PLMN 全局发布可变 override。
- 不要在 ICCID 不可用时退回第一张卡、第一台 modem 或某个默认文件。
- 不要把 `line_id` 当成永久 SIM 身份，尤其是独立读卡器场景。
- 不要把 `device_identity_imei` 只接到 UI 而不接协议使用点。
- 不要把 SIP REGISTER 成功等同于语音、视频、Ut、MWI 都可用；分别报告 capability/readiness。
- 不要仅因为有 CS 信令控制就宣称 CS trunk 已实现；必须有双向音频数据面。
- 不要用紧急呼叫验证 E911。
- 不要自动拨打真实号码或在日志中保留完整号码。

按此顺序实施，可以先解决最容易造成跨卡污染和紧急信息误绑定的配置问题，再复用现有 IMS/trunk 能力扩展视频与补充业务，最后进入 Asterisk、Linphone 和运营商实机验收。
