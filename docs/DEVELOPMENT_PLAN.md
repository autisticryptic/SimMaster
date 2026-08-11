# SimAdmin IMS 后续开发计划 — SIM 覆写 / IMEI / E911 / 视频 / UT

> 用途：后续开发的可勾选实施清单。本文只描述计划和当前静态审阅结果，不表示未勾选能力已经可用于生产。
>
> 审校状态：2026-08-11 已按当前工作树和 QCM410 目标机实测结果重新审校。VoLTE 与 VoWiFi 的公共 IMS REGISTER 抽取已经完成，不再把它列为“重构前置条件”。
>
> 验证基线：2026-08-11 目标机已验证真实 VoLTE REGISTER、VoWiFi ePDG/IKE/EAP-AKA/ESP/REGISTER、双接入同时注册和 SQLite backup/export 一致性。本机 live Asterisk 测试已覆盖 Digest REGISTER、INVITE/ACK、re-INVITE、DTMF 和 BYE；目标机到 WSL Asterisk 受 NAT 隔离，Windows Linphone 端到端仍未完成。VoWiFi 外呼已确认 INVITE 通过注册后的受保护 channel 发出，但观察窗口内没有收到运营商 1xx/最终响应，不能标记为接通验收。未执行 CS 或紧急呼叫。
>
> 路径约定：Rust 源码路径以 `backend/src/` 为根；其他路径以仓库根目录为根。不再记录容易随重构失效的 `file:line`。

## 1. 架构结论：哪些共享，哪些分开

不应在“全部共享”和“全部分开”之间二选一。目标结构是：**共享 IMS 协议核心，分离 VoLTE/VoWiFi 接入适配器**。

| 层次 | 归属 | 职责与边界 |
| --- | --- | --- |
| REGISTER 事务与公共报文 | 共享 | provisional response、401/407、AKA/AUTS 轮次、CSeq、公共 Header 顺序、成功响应解析 |
| 注册租约与业务身份 | 共享为主 | `Expires`、`Service-Route`、`P-Associated-URI`、刷新/失效语义；具体调度和链路恢复由接入侧完成 |
| VoLTE 接入 | 独立适配器 | IMS bearer、P-CSCF discovery、LTE PANI、XFRM/IPsec、QCM410 或通用 modem provider |
| VoWiFi 接入 | 独立适配器 | ePDG、IKEv2/EAP-AKA、ESP、TUN、Wi-Fi PANI、多组端口/SPI/密钥候选 |
| 视频、UT、MWI、主叫隐私 | 共享领域模型 | SIP/SDP/XCAP/MWI 解析与状态模型共享；网络发送通过当前 VoLTE 或 VoWiFi access transport |
| E911 entitlement | 独立服务 | TS.43/HTTPS/websheet，不属于 SIP REGISTER；只复用按线路 SIM identity 和受控 AKA 原语 |

因此，后续不要再复制一份 VoLTE REGISTER 到 VoWiFi，也不要把 ePDG/IKE 或 LTE bearer 塞进共享 REGISTER engine。共享层接受接入侧已经解析好的 policy/identity/security 参数，并返回接入无关的注册结果。

## 2. 当前代码基线

### 2.1 已完成

- [x] `connectivity/core/register.rs`：共享 REGISTER transaction driver，统一 initial REGISTER、provisional、401/407、最多两轮 challenge、CSeq 和终态错误。
- [x] `connectivity/core/register_message.rs`：共享 REGISTER 报文与公共 Header 排序。
- [x] `connectivity/core/digest_aka.rs`：共享 nonce、Digest-AKA 和 AUTS Authorization 原语。
- [x] `connectivity/core/register_response.rs`：共享解析 Contact/Expires、`Service-Route` 和 `P-Associated-URI`。
- [x] `connectivity/modems/ims/volte/live.rs` 与 `connectivity/modems/ims/vowifi/live.rs` 均已接入共享 REGISTER driver。
- [x] VoWiFi 的 ePDG/IKE/ESP/TUN 和 VoLTE 的 bearer/P-CSCF/XFRM 仍保持接入隔离。
- [x] `ProfileStore` 当前是只读 `carrier_Bundles` catalog facade；旧 SQLite operator override 写入已经移除。
- [x] `LineProfileConfig`、`LineRuntimeRegistry`、VoLTE/VoWiFi runtime 和 trunk 基础已按 `line_id` 隔离。
- [x] `SimBindingKey` + `SimOverrideStore` + `EffectiveImsProfile`/source map：按 SIM 隔离覆写，字段级 merge。
- [x] `EffectiveDeviceIdentity`（custom_imei → modem IMEI → unavailable），两线路不串值。
- [x] `services/e911`：TS.43 client、受控 AKA adapter seam、provider registry、websheet operation、SSRF 防护、独立 state/secret store 与地址 provisioning API。
- [x] `ConfigManager` 已支持版本化 SQLite 单例文档和事务写入；生产默认路径为 `/data/config.sqlite3`，空库直接写入当前默认值，不读取旧 JSON。
- [x] IMS 视频 SDP/H.264 与 RTP relay 已迁到共享 core；VoLTE/VoWiFi adapter 均使用共享媒体类型，旧 VoLTE 路径只保留兼容 re-export。
- [x] 共享音频 core 已支持 EVS 的 `EVS/16000` SDP、运营商动态 PT、`br`/`bw` fmtp 保留和双腿 PT 映射；schema-v7 catalog 优先消费 `/media/audio/codecs`，无媒体配置时保持 AMR-WB/AMR/G.711 回退。

### 2.2 尚未完成或仍需收口

- [x] VoLTE/VoWiFi 的 MWI `SUBSCRIBE` 与 INVITE、MESSAGE、OPTIONS 一样消费注册结果中的 `Service-Route`，没有建立第二份 route 缓存。
- [x] VoLTE 与 VoWiFi 已共享租约、refresh 失效分类和显式注销结果：鉴权拒绝、SIP 网络拒绝、信令传输丢失及接入传输丢失均进入同一 `RegistrationRefreshResult`，再由 VoLTE 重建 bearer、VoWiFi 重建 IKE/ESP/TUN；显式停止沿用原 REGISTER dialog/CSeq，在 401/407 后调用各线路 AKA provider 发送 `REGISTER Expires: 0`，只有最终 2xx 映射为 `UnregisterResult::Confirmed`。
- [x] E911 使用 TS.43 EAP relay JSON（不是 HTTP Digest-AKA），复用按线路 QMI/UIM USIM AKA；challenge 的 AT_MAC 会校验，sync failure 最多重试 3 轮。
- [x] per-SIM effective profile 已接入 VoLTE/VoWiFi live boundary：VoLTE bearer/P-CSCF/REGISTER 与 VoWiFi ePDG/DNS/IKE/REGISTER 均消费按 access 固定的会话快照。
- [x] 自定义 IMEI 已接入 VoWiFi IKE_AUTH Vendor ID、VoLTE/VoWiFi policy-gated SIP `+sip.instance`，以及 TS.43 `terminal_id`；永久 NAI/IMPI/IMPU 不变。真实运营商是否接受各位置仍需实机验证。
- [ ] VoWiFi adapter 已具备 IMS 视频 SDP/relay/re-INVITE 基础，但真实运营商、Asterisk/Linphone 与双线路视频矩阵尚未验收。
- [ ] IMS Ut/XCAP 已具备显式 catalog policy、受限 HTTPS transport、规则级原位修改以及 OIP/CLIP、OIR/CLIR 的 GET→条件 PUT→GET 编排；VoLTE/VoWiFi 当前会话的源地址/AKA provider 已接入，但运营商网络回读仍未完成。MWI 订阅/NOTIFY、一次 AKA challenge 重试和 SIM→catalog→override 语音信箱号码解析已接线，仍需运营商回读验证。
- [ ] Asterisk trunk 的本机 live 测试已通过 Digest REGISTER、INVITE/ACK、re-INVITE、DTMF 和 BYE；Windows Linphone、目标机可达性与真实运营商媒体互通仍未验收。

## P0 — 共享 IMS 注册收尾

本阶段不重写已经完成的 REGISTER driver，只补注册完成后的共享语义。

### P0.1 统一注册结果上下文

- [x] 在共享层定义 `RegisteredImsContext`，包含实际有效期、`Service-Route`、关联公有身份、注册 access 和注册时间。
- [x] 有效期优先使用网络响应中的 Contact `expires`，其次使用 `Expires`，最后才使用 profile 默认值。
- [x] VoWiFi 将 `Service-Route` 保存到已注册 voice/SMS context，并用于 INVITE、MESSAGE、OPTIONS 与已有 dialog 请求。
- [x] MWI `SUBSCRIBE` 消费同一个 `RegisteredImsContext.service_route`，不另建 route 缓存。
- [x] 接入侧安全状态继续独立保存；共享结构不持有 ESP key、XFRM handle、TUN fd 或 modem bearer handle。

### P0.2 生命周期边界

- [x] 抽取共享的租约计算、refresh result、unregister result 和失效原因模型。
- [x] VoLTE 与 VoWiFi 仍各自调度 refresh：VoLTE refresh 失败可重建 bearer；VoWiFi refresh 失败可重建 IKE/ESP/TUN。
- [x] 两条 adapter 实际消费共享 refresh result：共享 REGISTER failure 统一区分鉴权/网络/信令失败，VoWiFi 另将 ePDG/IKE/TUN 失败标为 access transport lost；日志保留 adapter 原始错误码。
- [x] 需要显式注销时，使用同一 dialog identity/CSeq 和 AKA challenge 完成 `REGISTER Expires: 0`，再映射 `UnregisterResult`；网络拒绝、access 丢失和已过期分别收口，后续本地 bearer/TUN teardown 不会反向改写为 `Confirmed`。共享 challenge、VoLTE UDP 和 VoWiFi operator protected-channel 回归已通过，真实运营商注销响应仍待实机记录。
- [x] VoLTE 与 VoWiFi adapter 模块运行同一组共享 REGISTER contract：覆盖 200、401、407、AUTS 后第二轮 challenge、第三次 challenge 拒绝、initial/authenticated provisional、Contact expires、Service-Route 和关联身份。VoLTE 覆盖共享 driver 管理 exchange 的形态，VoWiFi 覆盖 adapter 自管 protected exchange 的形态；真实 USIM 算法和运营商响应仍由各 access 的现有单测/实机验收负责。
- [x] 不把“REGISTER 成功”自动等同于 voice/video/UT/MWI ready；视频按 access gate，MWI/UT 使用独立 capability/readiness。

## P1 — 按 SIM 的用户覆写与有效配置

### P1.1 SIM/eSIM 绑定键

- [x] 新增 `SimBindingKey`：普通 SIM 使用规范化 ICCID；eUICC 使用 EID + 当前启用 profile ICCID。
- [x] ICCID 复用 `platform/utils.rs::normalize_iccid`；EID 只接受规范化的 32 位十进制值。
- [x] 拿不到 ICCID 时返回 `sim_identity_not_ready`，不得退回 `line_id`、modem path、IMEI、IMSI 或“第一张卡”。
- [x] 普通可移除 eSIM 实体卡在拿不到 EID 时使用当前 profile ICCID；切换 eSIM profile 后必须得到不同绑定键。
- [x] `line_id` 只用于运行态；SIM 移到另一台 modem 后仍应读取同一份用户覆写。

### P1.2 覆写存储

- [x] 新增 access-neutral 的 `SimOverrideStore`，建议放在 `connectivity/modems/ims/profile_override.rs`；不要塞进 `vowifi/live.rs`。
- [x] 生产默认在 `/data/config.sqlite3` 的独立 `ims_sim_overrides` 表中每个 binding 保存一行，主键为 binding SHA-256；测试/恢复工具仍可显式使用逐 SIM 文件后端。
- [x] 每行只保存用户明确修改的字段，不复制完整 catalog profile；字段缺失表示继承 catalog。
- [x] schema 至少预留 `ims.common`、`ims.volte`、`ims.vowifi`、`services`、`emergency.e911_address` 和 `schema_version`。
- [x] SQLite 写入使用独立事务、WAL、`synchronous=FULL` 和 `0600` 数据库权限；旧文件后端仍使用 `0700/0600`、唯一临时文件、`sync_all` 和原子 rename。
- [x] 覆写为空时删除当前 binding 行；无行是正常状态，自动连接只读 catalog。
- [x] schema 不支持、binding/hash 不匹配或 JSON 损坏时 fail closed，并返回可诊断错误，不能静默串用旧缓存。
- [x] 生产启动只读取 SQLite override 表，不扫描或导入旧逐 SIM JSON；开发阶段不维护旧配置兼容路径。

### P1.3 解析顺序

统一 live 读取链：

```text
line_id → 当前 SIM identity/SimBindingKey
        → 按 access 从只读 carrier catalog 解析 baseline
        → 读取当前 SIM override（如存在）
        → 字段级 merge + source map
        → access-specific validation
        → immutable EffectiveImsProfile
```

- [x] `LineProfileConfig` 继续保存开关、重试、trunk、代理等线路/主机运行意图；不要把它当作永久 SIM 身份。
- [x] 完成应跟随 SIM 的连接字段迁移。profile pin、IMS/ePDG、DNS、自定义 IMEI、语音信箱和 E911 只保留 SIM override 写入口；`LineVowifiConfig` 只保存开关、自动恢复和主机代理。
- [x] 解析结果使用 owned/`Arc` profile；避免用户每次编辑都通过 `Box::leak` 产生新的永久对象。
- [x] 返回字段级 `source_map`，明确值来自 catalog、SIM override、modem 或网络响应。
- [x] 将 effective profile 接入 VoLTE/VoWiFi live connect boundary；连接开始时固定不可变快照，refresh 沿用该快照，重连/换卡时重新解析。
- [x] 移除 `LineVowifiConfig.profile_id`/DNS 等与 SIM 重叠的旧运行时来源；开发阶段不提供旧字段迁移，同一字段只有 SIM override 一个写入口。

### P1.4 唯一写入边界与 API

- [x] `GET /api/ims/lines/{line_id}/profile`：返回脱敏的 effective profile 与 source map。
- [x] `GET/PATCH/DELETE /api/ims/lines/{line_id}/override`：用户读取、修改和恢复默认。
- [x] `POST /api/ims/lines/{line_id}/override/validate`：只验证，不连接网络。
- [x] PATCH handler 在同一请求的校验前与落盘前各读取一次绑定键；不一致时返回 `sim_binding_changed_during_update`。
- [x] 只有认证后的显式用户保存/删除可以写 override 表。
- [x] 启动恢复、失败重试、网络探测、REGISTER fallback、profile 自动匹配、拨号和短信流程严禁写 override。

## P2 — 自定义 IMEI

原计划中“用 IMEI 组成永久 NAI/替换 SIP 身份”的描述不正确。IMSI/IMPI/IMPU 是订阅者身份，自定义 IMEI 是设备身份，两者必须分开。

- [x] 新增共享 `EffectiveDeviceIdentity`，解析顺序为 SIM override 的 `custom_imei` → 当前 line 的 `ModemBinding.equipment_identifier` → unavailable。
- [x] `custom_imei: null` 或用户留空表示使用本机 IMEI；API 不把空字符串落盘。
- [x] 校验 15 位十进制与 IMEI check digit；日志只记录 `source=custom|modem|unavailable`，不记录原值。
- [x] 不向 modem 发送任何修改硬件 IMEI 的 AT/QMI 命令。
- [x] 不得用 IMEI 替换 IKE EAP-AKA permanent NAI、IMPI、IMPU、ICCID 或 IMSI。
- [x] 仅在 carrier policy 明确要求的位置使用设备身份，例如特定 IKE_AUTH device/vendor identity、TS.43 device information，或 GSMA IMEI 格式的 `+sip.instance`。
- [x] 未经 profile policy 允许，不修改 User-Agent，也不假设所有运营商都接受 IMEI 型 `+sip.instance`。
- [x] VoLTE、VoWiFi 和 entitlement 均消费同一 `EffectiveDeviceIdentity`；各 adapter 只负责协议位置映射。设备身份在 IKE Vendor ID、SIP `+sip.instance` 和 TS.43 `terminal_id` 上均受 carrier policy 门控。
- [x] 测试 custom/modem/unavailable 三态、两线路不串值、换卡重解析以及日志/API 脱敏。

## P3 — E911 地址与 entitlement

E911 不属于共享 REGISTER。地址权威副本通常在运营商 provisioning 系统中，本地 override 行只保存用户输入意图；详细协议依据见 `docs/E911_IMPLEMENTATION_RESEARCH.md`。

- [x] 保留 `connectivity/modems/ims/vowifi/profile_record.rs::E911PolicyRecord` 为只读 carrier policy/evidence，不把 `enabled=true` 展示成“地址已设置”。
- [x] SIM override 可保存用户明确输入的 civic address；地址、ICCID、EID、IMSI、IMEI 均按敏感信息处理。
- [x] entitlement token、cookie、`AddrStatus`、`ProvStatus`、重试时间和 provider reference 写独立 state/secret store，不写回用户 override。
- [x] 新增独立 `services/e911`：TS.43 client、受控 SIM AKA adapter、provider registry、websheet operation 和状态机。
- [x] 接入真实 TS.43 请求参数（EAP_ID/root NAI、`ap2004`、`vers`、token、terminal policy），解析 WAP provisioning XML 的 `EntitlementStatus/ProvStatus/TC_Status/AddrStatus`、token、版本和 `ServiceFlow_URL/UserData`。
- [x] 真实 `ServiceFlow_URL` 与 cookie/token 按 `SimBindingKey` 加密保存；websheet 完成仍必须重新 query，不能把本地地址或页面关闭当成紧急呼叫确认。
- [x] websheet operation 新增一次性同源 `launch_url`；标准 URL-encoded `ServiceFlow_UserData` 通过受控页面 POST，completion 使用独立随机 nonce，JSON 不返回 user data/token/cookie；非标准 body fail closed。
- [x] E911 API 以当前线路 provider 和当前线路 QMI/UIM slot 执行查询；补充 VoWiFi 诊断页的状态、查询和运营商流程入口。
- [x] entitlement URL/redirect 必须来自可信 catalog 并经过 HTTPS、host allow-list、DNS/IP/redirect/response-size 限制，防止 SSRF。
- [x] websheet 完成后必须重新查询 entitlement；不能仅凭页面关闭或 HTTP 2xx 宣称地址已确认。完整跨域 callback bridge/proxy 仍需运营商页面实测后接入。
- [x] API/UI 分开显示：运营商要求、地址已本地保存、运营商已确认、紧急呼叫未验证。
- [x] 所有后台 query/provisioning 只能更新状态存储，不能自动改写用户地址文件。
- [x] 只做非紧急 provisioning 测试；禁止用拨打 911 验收。

## P4 — 共享 IMS 视频与 VoWiFi 视频

严格说 ViLTE 指 LTE 接入下的视频；VoWiFi 上应称“IMS 视频（VoWiFi 接入）”。产品 UI 可以说明它提供与 ViLTE 类似的视频通话体验。

### P4.0 EVS 音频信令与透明转发

- [x] 共享 `AudioCodec`、SDP parser/serializer 和状态快照识别 EVS；RTP 时钟固定为 16000 Hz，payload type 使用运营商配置或动态分配。
- [x] schema-v7 loader 读取 `/media/audio/codecs` 的 codec 顺序、payload type、sample rate、EVS bitrate/bandwidth，并生成 TS 26.445 的 `br`/`bw` fmtp。
- [x] SDP answer 保留对端 EVS fmtp；VoLTE/VoWiFi trunk relay 在两端动态 PT 不同时复用通用映射，并在多个 EVS 变体之间优先按 fmtp 匹配。
- [x] catalog 没有媒体 codec 数据时继续使用现有 AMR-WB、AMR、PCMU、PCMA 回退，不向所有线路默认强制广告 EVS。
- [ ] SimAdmin 仍不包含 EVS 编码、解码、转码、jitter buffer 或音频播放；端到端 EVS 需要后续 Asterisk/Linphone codec 插件和真实运营商协商验收。

- [x] 将 `connectivity/modems/ims/volte/vilte.rs` 的 SDP/H.264 类型迁到 `connectivity/core/ims_video.rs`；旧模块仅兼容 re-export。
- [x] 将通用 RTP relay 从 VoLTE 命名空间迁到 `connectivity/core/media.rs`，VoLTE、VoWiFi 与 trunk 均直接消费共享类型；旧模块仅兼容 re-export。
- [x] 每线路统一使用 `ImsVideoConfig`，分别门控 VoLTE 和 VoWiFi；开发阶段不提供旧 `VilteConfig` 导入流程。
- [x] VoWiFi `operator.rs` 不再用固定的 `vowifi_video_not_supported` 拒绝视频，并为 audio/video 分别持有 pending/active relay。
- [x] 覆盖双方初始视频 INVITE、audio→video、video→audio、488/491/超时回滚以及 BYE/CANCEL 资源释放；re-INVITE 使用 per-call deadline，超时只回滚 pending relay、保留已确认语音 dialog。
- [x] `sync_line_video_capabilities()` 分别设置 VoLTE/VoWiFi backend，不能用一条 access 的 ready 状态替代另一条。
- [x] REGISTER Contact 只在 carrier catalog 明确声明且本地 access capability 开启时保留 `video` feature；本地 capability 关闭时过滤该参数。
- [x] H.264 bitstream 不在 SimAdmin 内转码；共享 relay 只做 RTP payload type 映射，真正转码交给 Asterisk。
- [x] 测试双线路 socket/SSRC/PT 隔离、RTP 与严格校验的 RTCP-mux 透明转发、RTCP 不触发 RTP answered 事件，以及 VoLTE/VoWiFi 对端以 488 拒绝视频升级后仍保留既有音频 relay。当前 SDP/socket 模型不分配独立 RTCP 端口，非 mux RTCP 继续明确不支持。

## P5 — UT、呼叫等待、语音信箱与 Caller ID

### P5.1 共享模型与 access transport

- [x] 新增 `connectivity/core/supplementary.rs` 和 `services/supplementary/*`，统一 capability、call waiting、forwarding、identity presentation、MWI 与错误模型。
- [x] 共享 MWI classifier 统一识别 NOTIFY、subscription response、dialog Call-ID 和 To-tag；VoLTE/VoWiFi adapter 只负责各自通道 I/O。
- [x] IMS Ut/XCAP 的领域模型、命名空间 XML 解析和 GET/条件 PUT 请求描述已在共享 core 实现；未知扩展在只读 GET/parse/GET 路径保留。
- [x] `services/supplementary/ut.rs` 统一 GET、带 ETag 的 `If-Match` PUT、再次 GET 和 semantic readback；PUT 2xx 本身不代表配置成功。
- [x] 共享 `HttpXcapTransport` 强制 HTTPS、禁止 redirect、支持可选源地址绑定、ETag/Content-Type、一次 401/407 Digest provider 重试，并以流式上限拒绝超大响应；不会从 registrar 猜测 XCAP 地址。
- [x] VoLTE/VoWiFi live adapter 已分别提供当前已注册会话的源地址和按线路 Digest-AKA provider；线路级 UT GET/PUT API 只选择当前 IMS access，并复用同一共享事务。
- [x] 同一订阅的 UT 规则不写入 VoLTE/VoWiFi 本地副本；线路 API 每次按当前注册 access GET 网络权威值，成功更新后再次 GET 回读。
- [x] CS/ModemManager/AT 作为独立 provider，只暴露 modem 真正支持的能力：当前仅使用 ModemManager 已声明的 `CallWaitingQuery`/`CallWaitingSetup`；呼叫转移、来显、音频控制和 CS trunk 均明确报告不支持，不发送推测性的 MMI/USSD/AT 命令。

### P5.2 IMS Ut/XCAP

- [x] catalog 可显式提供 XCAP root、`digest_aka` 鉴权、document selector 和 namespace；缺失或未启用时 fail closed 为不支持，HTTPS/完整性校验在 profile import 时完成。
- [x] catalog 已按文档类型显式建模 XCAP partial-update selector，并支持带 `{rule-id}` 的 communication-diversion 规则 selector；只有明确启用且当前文档存在 selector 时才发 element PUT，否则保留整文档条件 PUT。TLS policy 可限制 TLS 1.2/1.3 范围、启停内置可信根并提供额外 carrier CA，证书/主机名校验始终开启。
- [x] 首批支持 communication-waiting、communication-diversion、OIP/CLIP、OIR/CLIR；OIP 的 `active=true` 表示允许显示，OIR/CLIR 的 `active=true` 表示启用限制。OIR 使用规范 XCAP 文档名 `originating-identity-presentation-restriction`，旧 API 别名仍兼容。
- [x] 写入编排采用 GET → parse → `If-Match` 更新 → GET 回读确认；409/412、401/407、PUT 失败和回读不一致分别返回明确错误。
- [x] call-waiting、identity active 与 communication-diversion 规则级修改均保留未知 XML 扩展；新增/更新规则只改目标 rule，不覆盖整份运营商文档。
- [x] 共享模型已拒绝非 E.164 `tel:` 目标并接受 `sip:`/`sips:` URI；完整号码和 XML 不进入事务层普通日志。
- [x] `REFER` 只表示 dialog 内通话转接，不替代网络侧呼叫转移配置；后者走 Ut/XCAP 或 CS provider。共享模型已使用独立的 `DialogTransfer` 命名和状态，不复用 forwarding API。

### P5.3 呼叫等待和通话转接

- [x] SIP dialog 层支持至少两个同时存在的 call、第二路 180/183/200/486、hold/resume 和独立 RTP/DTMF/re-INVITE；VoLTE/VoWiFi 本地矩阵均已直接经过各自受保护 channel 与生产 dialog handler，真实运营商/Asterisk/Linphone 双通话互操作仍属于外部验收。
- [x] 每个 IMS access 最多保留两个独立 dialog；第三路外呼向 trunk 返回 486、第三路入呼向网络返回 486，且不会覆盖或释放既有 call。hold/resume、早期媒体和真实双通话互操作仍在上一项验收范围内。
- [x] 两条 IMS adapter 对跨 leg 的 SDP `sendonly`/`recvonly`/`inactive` 方向进行对端反转；共享 RTP relay 按最终两端 direction gate RTP，hold 时仍透明保留严格校验的 RTCP-mux。网络侧与 Asterisk 发起的 re-INVITE 都复用这条路径。
- [x] 共享 core 已实现 `Refer-To`/`Referred-By` 安全校验、in-dialog REFER 构建、`message/sipfrag` refer-event NOTIFY 解析和单向终态状态机；两条 access 使用同一协议边界。
- [x] 共享 REFER core 已接入 Asterisk B2BUA、VoLTE 和 VoWiFi dialog：仅 confirmed dialog 接受 blind transfer，按 call 独立透传 202/失败响应和 refer-event NOTIFY，并以 32 秒超时收口。两条 access 按 REFER CSeq 校验显式 `Event: refer;id=`，B2BUA 向 Asterisk 重写为其原始 REFER CSeq；底层 command consumer 消失只结束 REFER transaction，不误报整通电话失败。由于 Asterisk leg 与运营商 IMS leg 的 dialog 标识不同，携带 `Replaces` 的 attended transfer 当前明确返回 501；真实运营商、Asterisk 和 Linphone 的转接互操作仍属于外部验收，不据此宣称实机转接已验证。

### P5.4 MWI 与语音信箱

- [x] 注册成功后通过共享模块发送并续订 `SUBSCRIBE Event: message-summary`；VoLTE/VoWiFi 在各自受保护通道解析 `NOTIFY` 中的 `Messages-Waiting` 和 `Voice-Message`，未知 subscription dialog 返回 481。
- [x] 不把普通 `MESSAGE` 当作 MWI；只有 `NOTIFY Event: message-summary` 且 Content-Type 为 `application/simple-message-summary` 才更新 MWI，`MESSAGE` 继续走原 SMS/即时消息分流。
- [x] MWI subscription/runtime 按 `line_id` 保存并记录当前 access owner；语音信箱覆写号码按 `SimBindingKey` 保存，旧 access 的延迟 teardown 不会清掉新 access 状态。
- [x] 新增只读 `GET /api/ims/lines/{line_id}/supplementary`，返回该线路 capability/readiness 与 MWI snapshot，不写回配置库。
- [x] 对 MWI `SUBSCRIBE` 的 401/407 challenge 实现一次性、按线路的 Digest-AKA 鉴权重试；不复用 REGISTER nonce，也不跨线路取卡。运营商实际 challenge 兼容性仍需实机验证。
- [x] 号码来源按 SIM override → SIM/ModemManager (`AT+CSVM?`) → catalog 解析；`POST /api/ims/lines/{line_id}/voicemail/call` 以按 access 的 `MediaOffer` 交给当前 `VoiceAccessRouter`，复用 `StartCall`、选路、故障切换和既有 lifecycle。`*86` 一类服务码由两条 IMS adapter 使用同一安全 dial-string 规范化。HTTP 接口目前只有预留的本地 RTP sink，尚不提供浏览器/本地音频收听或语音信箱交互 UI。
- [x] MWI snapshot 携带 `source=operator_ims|asterisk_local`；当前解析得到的状态明确标为 `operator_ims`，不冒充 Asterisk 本地 mailbox。

### P5.5 Caller ID 与隐私

- [x] 共享解析 `Privacy`、`P-Asserted-Identity`、From 和兼容性的 `Remote-Party-ID`。
- [x] 收到 `Privacy: id` 时，VoLTE/VoWiFi 到 trunk 的入口强制替换为 `sip:anonymous@anonymous.invalid`；trunk 回归测试确认不会转发 P-Asserted-Identity 或原始号码。
- [x] VoLTE/VoWiFi 入站 Caller ID 已统一隐私解析并在 trunk 入口匿名化；出站 caller ID 继续由注册身份生成，不接受 Asterisk `caller` 字段覆盖。UI/call-history 的专门隐私审计和 Linphone 实测仍待完成。
- [x] CLIR/OIR 写操作不修改本地 From header 冒充成功，而是走共享 XCAP GET→条件 PUT→GET，并对 OIP/OIR 的相反 `active` 语义执行网络回读比对；真实运营商接受情况仍属于外部验收。

## P6 — 多线路与 trunk 回归门槛

- [x] 已审计会改变线路行为的全局容器：VoWiFi live/TUN/REGISTER/security/operator cache 均以 `line_id` 为键；baseband restart 以硬件 key 为键；SIM override 以 `SimBindingKey` 为键；只读 carrier catalog 共享。
- [ ] 独立读卡器换卡、两张相同 PLMN SIM、SIM 移动 modem、eSIM profile 切换均不得串用覆写、IMEI、E911、UT、MWI 或 RTP 状态。
- [x] 每个 `LineRuntime` 持有独立 `TrunkRuntime`、`VoiceAccessRouter`、REGISTER/Digest/dialog generator、profile、relay metrics、generation 与 driver backoff；回归测试覆盖单线 teardown、generation、视频 gate 和 RTP 计数不影响另一线。
- [ ] 本机 live Asterisk 已覆盖 Digest REGISTER、主叫 INVITE/ACK、re-INVITE、DTMF 和 BYE；VoLTE/VoWiFi 真实接入下的被叫、早期媒体/PRACK、CANCEL、Windows Linphone 与视频矩阵仍待完成。
- [ ] CS trunk 只有在找到真实双向音频数据面 adapter 后才能标记支持；仅有 ModemManager 呼叫控制不等于可接入 Asterisk。
- [ ] 本地 contract 已确认一线清理不会删除另一线的 VoWiFi REGISTER variant 或 operator session，另一线仍可发 INVITE；真实双 modem 下的 TUN、bearer、活动 RTP/视频 relay 故障隔离仍待验收。

## P7 — 项目配置 SQLite 化

这里的“项目配置”指用户可修改、重启后仍应保留的配置。只读运营商 catalog、短信/通话历史等运行数据、E911 entitlement token/cookie 等 secret state 继续保持独立所有权，不能因为都使用 SQLite 就混成一套 schema。

- [x] `ConfigManager` 保持强类型内存快照和现有 getter/setter API，生产持久化改为 `/data/config.sqlite3`。
- [x] 使用 `app_config` 单例行保存版本化 `AppConfig` 文档；写入采用 `BEGIN IMMEDIATE` + UPSERT + commit，开启 WAL、`synchronous=FULL`、busy timeout 和启动时 quick check。
- [x] SQLite 行同时校验 storage schema version、line config version、JSON 有效性和严格 `AppConfig` schema；异常时 fail closed，不回退默认值覆盖原数据。
- [x] SQLite 尚无 canonical row 时直接写入当前 `AppConfig::default()`；同目录旧 `config.json` 即使存在也不会被读取、备份或导入。
- [x] 默认数据库文件权限设为 `0600`，拒绝数据库路径符号链接；`SIMADMIN_CONFIG_DB` 是唯一生产配置路径覆盖入口。
- [x] `SimOverrideStore` 使用同一配置库的独立 `ims_sim_overrides` 表；生产启动不扫描旧逐 SIM 文件或写 migration marker。
- [x] systemd 使用 `UMask=0077` 保护 SQLite WAL/SHM 与其他运行期敏感文件；卸载脚本区分保留/清理配置库、sidecar 和 E911 state。
- [x] 已实现 `config backup/export/import/restore`：在线备份使用 `VACUUM INTO`，显式 JSON 导入校验 typed config、SIM binding hash 和 override document，维护 schema journal，并在 import/restore 前保留 SQLite rollback snapshot。
- [x] 已覆盖并发写期间在线备份、只读 source、SQLite 损坏、未来 schema 拒绝、非法 import 不改目标和目标不覆盖。
- [ ] 真实掉电、磁盘满及只读目标文件系统 fault-injection 仍待完成。

## 验证与回归

- [x] `cargo fmt --manifest-path backend/Cargo.toml -- --check`
- [x] `cargo check --manifest-path backend/Cargo.toml`
- [x] `cargo clippy --manifest-path backend/Cargo.toml --all-targets`（通过；保留既有 warning）
- [x] `cargo test --manifest-path backend/Cargo.toml --no-fail-fast`（1027 passed; 0 failed; 1 ignored）
- [x] override store 使用临时目录测试权限、symlink、损坏 JSON、未知 schema、原子写和并发写。
- [x] 自动读取/重试 contract 重复执行 SQLite load 和 VoLTE/VoWiFi/IMEI/E911 effective resolution，前后比较 override `document_json` 与 `updated_at` 完全一致；生产写入口审计只保留显式用户 override/E911 地址 API 和维护工具。
- [x] 两线路 contract tests 使用同一 carrier catalog 的两个独立 SIM binding，覆盖不同 VoLTE/VoWiFi/IMEI/E911 配置、重新打开 store 的热插拔式二次解析、VoLTE↔VoWiFi access 分支和同 EID 的 eSIM profile ICCID 切换；真实硬件矩阵仍属于 P6 外部验收。
- [ ] Asterisk/Linphone 外部实验单独记录环境、codec、SIP trace 和脱敏结果；不得把 ignored test 当作已验收。
- [ ] 真实运营商测试只拨打已授权的普通测试号码；E911 仅走运营商提供的非紧急验证流程。

## 实机/外部环境完成条件

下列未勾选项不能只用 mock 或代码审阅完成：

1. 运营商 IMS/Ut/MWI/E911：需要对应 SIM、catalog policy、受控 AKA 和脱敏抓包；E911 禁止拨打 911，只能使用运营商非紧急 provisioning/validation 流程。
2. Asterisk/Linphone：需要可达的 Asterisk 配置和 Windows Linphone 人工配合，记录 REGISTER、主被叫、早期媒体、re-INVITE、DTMF、BYE/CANCEL 与视频协商结果。
3. 多 SIM/硬件：至少需要两条真实线路覆盖同 PLMN、热插拔、SIM 移动 modem、eSIM profile 切换和一线断网场景。
4. 普通电话：只在确认测试线路授权、余额/资费和目标号码无误后拨打 `+60 1112023012`；测试记录不得包含完整 ICCID、IMSI、IMEI、AKA、地址或 SIP 鉴权材料。

### 2026-08-11 WSL/Asterisk 实测

- WSL Asterisk 版本为 `22.10.1`；live test `services::trunk::driver::tests::live_asterisk_digest_register_and_linphone_call` 通过，覆盖 Digest REGISTER、INVITE、ACK、re-INVITE、DTMF 和结束流程。
- 目标机不能直连 WSL NAT 地址；Windows 主机当前没有可用的管理员 UDP NAT 转发，因此该 live test 证明 trunk/Asterisk 状态机与真实 Asterisk 互通，不等同于目标机 + Windows Linphone 的端到端验收。

### 2026-08-11 目标机 `192.168.100.13` 实测

- 最新 ARM64 musl 二进制已部署到 systemd 实际使用的 `/root/simadmin-codex/simadmin`，并同步 `/opt/simadmin/simadmin`；启动日志确认读取 `/data/config.sqlite3`，不读取旧 JSON。
- 在无活动通话时执行一次受控 `ModemManager.service` 重启后，QCM410 恢复 `/Modem/0`、SIM ready、LTE home registered 与 packet attached。
- VoLTE 真实完成 bearer、P-CSCF、IPsec 和 IMS REGISTER；运行态为 `phase=registered`、`registered=true`、`recovery_state=registered`，服务重启后自动恢复成功。
- VoWiFi 真实完成 ePDG DNS、IKE_SA_INIT、NAT-T、EAP-AKA、CHILD_SA、userspace ESP 和 IMS AKA REGISTER `200 OK`；`sms_ready=true`、`voice_ready=true`、`degraded_reason=null`。
- 同一线路已同时观察到 VoLTE `registered=true` 与 VoWiFi `voice_ready=true`，证明双接入可并存；这不是两张物理 SIM 的多线路验收。
- SQLite override validation 拒绝非法 IMEI；在线 backup、export 与从 backup 再 export 的 JSON SHA-256 一致。
- 当前 Maxis profile 的 E911 provider 为 `metadata_only`，`operator_requires=false`、`query_supported=false`；未执行 provisioning 或紧急呼叫。
- MWI capability 为 supported，但 readiness 停留在 `mwi_subscribe_pending`；UT/call waiting/forwarding/identity 仍为 `supplementary_not_connected`，不能标记为运营商验收。
- 修复了 direct VoWiFi 外呼在 REGISTER channel 已占用受保护 UDP 端口后再次 bind 导致的 `ims_udp_bind_failed`：HTTP 外呼现在复用 per-line operator registered channel，并在建链 32 秒无最终进展时发送 CANCEL、报告 `voice_invite_response_timeout`。定向测试覆盖事件映射及复用 channel 的 INVITE/ACK/re-INVITE/DTMF/BYE。
- 只对授权普通号码执行一次 VoWiFi IMS 外呼，未启用 CS fallback。API 返回 `path=vowifi_ims`、`call_state=dialing`、`invite_state=queued`；日志确认 INVITE 通过注册 channel 进入 TUN 并由 ESP 发出。观察窗口内未收到运营商 180/183/200 或拒绝响应，因此只证明 bind 问题已消失和出站信令已发出，不证明振铃、接通、媒体或挂断成功。
- 实测期间发现目标机 systemd drop-in 开启 ESP key/frame 明文调试；已删除 drop-in、daemon-reload 并重启服务，确认环境变量不再存在。历史 journal 已包含敏感调试记录，未擅自执行全局 vacuum。

## 推荐实施顺序

1. P0 注册结果收尾，避免后续 MWI/UT/视频重复处理 route 和 identity。
2. P1 SIM binding、override store、effective profile，这是防止多卡串配置的基础。
3. P2 IMEI 与 P3 E911，共用已稳定的 SIM binding 和敏感信息边界。
4. P4 共享视频，再给 VoWiFi 接线。
5. P5 UT/MWI/Caller ID，复用共享注册结果和 access transport。
6. P6 两线路 trunk/Asterisk/Linphone 完整回归。
7. P7 配置 SQLite 化先迁 `ConfigManager`，再迁按 SIM override；运行数据、只读 catalog 和 secret state 保持独立。

## 已确定的设计决策

1. REGISTER 相同逻辑继续放在共享 core；ePDG/IKE 与 LTE bearer 不合并。
2. 用户覆写采用 `ims_sim_overrides` 每个 `SimBindingKey` 一行，不采用所有 SIM 共用的大 JSON 文档，也不提供旧逐 SIM 文件导入。
3. `line_id` 管运行态，ICCID 或 EID + profile ICCID 管永久 SIM 配置。
4. 视频配置迁移为 access-aware 的 `ImsVideoConfig`，不再让 `VilteConfig` 同时暗指 VoWiFi。
5. UT 使用共享领域模型和两个 IMS access transport；CS 是第三个独立 provider。
6. E911 entitlement 独立于 SIP REGISTER，紧急地址 provisioning 与紧急呼叫能力分别报告。
7. 生产 `AppConfig` 使用 SQLite；使用同一种数据库技术不代表合并 catalog、配置、历史数据和 secret state 的 ownership。

## 变更记录

- 2026-08-10：初稿创建，当时共享 REGISTER 重构尚未完成。
- 2026-08-10：按当前架构重审；标记共享 REGISTER 已完成，补充注册收尾边界、E911、多线路/trunk 阶段，并修正 IMEI、REFER/呼叫转移和 MWI 协议描述。
- 2026-08-11：E911 改为公开 TS.43 EAP relay JSON 流程；接入按线路 QMI/UIM AKA、WAP provisioning XML、AT_MAC 校验、token/cookie/真实 ServiceFlow URL 的加密保存和前端 websheet 入口。当前仍未执行真实运营商 E911 provisioning 或紧急呼叫认证。
- 2026-08-10：P4 完成 `VilteConfig` → `ImsVideoConfig` 配置迁移：`volte/voice.rs` 字段改为 `ims_video`/`video_enabled` 并以 `volte_enabled` 门控，`start_line_volte_restore` 改用 `get_line_ims_video_config`；`cargo check --all-targets` 通过，`volte::voice` 12 个测试全部通过。SDP/relay 迁移与 VoWiFi 视频接线仍待做。
- 2026-08-10：完成 P7 的配置持久化主体。生产 `ConfigManager` 默认使用 `/data/config.sqlite3`，支持事务持久化和版本/fail-closed 校验；`SimOverrideStore` 使用独立逐 binding 表。按开发阶段要求移除全部旧 JSON 自动导入。catalog、运行历史和 E911 secret state 按 ownership 继续独立。
- 2026-08-10：P4 继续完成共享 `ims_video`/`media` core 和 VoWiFi 双媒体 relay/re-INVITE 接线；旧 VoLTE 模块降为兼容 re-export。审校 P1/P2 后明确 effective profile 与自定义 IMEI 尚未进入 live wire，取消端到端完成表述。
- 2026-08-10：完成 P1 live 收口。删除 `LineVowifiConfig.profile_id`/DNS 双写入口；VoWiFi 在新连接锁内发布 ePDG/DNS/IKE/IMS owned snapshot，活动会话不受后续数据库编辑影响；VoLTE 改为只读取 `ims_volte` pin，并让 APN、P-CSCF、domain、realm、registrar 在初始 REGISTER、AKA 和 refresh 中共享同一快照。自定义 IMEI 的协议位置仍按 P2 保持未完成。
- 2026-08-10：继续收口 P5。共享 MWI classifier 统一两条 access 的 dialog/NOTIFY/To-tag 判断；新增按线路补充服务只读状态 API。MWI 续订沿用同一 Call-ID/From-tag 并递增 CSeq，旧 access refresh 不会抢占新 access owner。
- 2026-08-10：开始 P5。新增共享补充服务模型、RFC 3842 parser 和按线路 runtime；VoLTE/VoWiFi 注册通道自动发送/续订 MWI SUBSCRIBE，复用 `RegisteredImsContext.service_route`，处理 NOTIFY 并隔离 access handover。两条 IMS 来电入口改用共享 `Privacy`/PAI/From/RPID 解析，受限身份进入 trunk 前匿名化。Ut/XCAP、SUBSCRIBE challenge 鉴权、语音信箱号码来源和完整 UI/history 审计仍待完成。
- 2026-08-10：新增共享 Ut/XCAP domain model：使用 `quick-xml` 解析 communication-waiting、communication-diversion 和身份呈现文档，校验转移目标 URI，生成带 ETag 的 GET/条件 PUT 请求描述，并保留未修改文档原始 XML。尚未连接 VoLTE/VoWiFi HTTP transport，也未宣称运营商回读成功。
- 2026-08-11：部署最新 SQLite 版本并完成 QCM410/Maxis 实测：VoLTE 与 VoWiFi 均真实 REGISTER 成功且可同时保持注册；SQLite backup/export 一致性通过；本机 live Asterisk 测试通过。记录 E911 `metadata_only`、MWI pending、UT 未连接和 Windows Linphone 网络阻塞。
- 2026-08-11：修复 direct VoWiFi 外呼重复 bind 已注册 IPsec protected port 的问题，改由 API 复用 per-line operator channel，映射 provisional/answered/rejected/ended/cancelled 状态，并为静默网络增加 32 秒 CANCEL/失败收口。单元与 operator dialog 定向测试通过；实机唯一一次 INVITE 已经 ESP 发出，但无运营商响应，未宣称通话接通。
- 2026-08-11：完成 SimAdmin 侧 EVS 信令与透明 RTP relay 支持。共享 audio policy 消费 schema-v7 的媒体 codec/PT/sample-rate/bitrate/bandwidth，VoLTE/VoWiFi 复用 EVS SDP 与动态 PT 映射；不包含 EVS 编解码或 Asterisk/Linphone 插件，也未宣称运营商 EVS 实机验收。
- 2026-08-11：完成后续代码收口：自定义 IMEI 接入 VoWiFi IKE Vendor ID、VoLTE/VoWiFi SIP `+sip.instance` 和 TS.43 terminal identity；MWI `SUBSCRIBE` 401/407 按线路 AKA 一次重试；`communication-diversion` 规则级原位 XML 更新；视频 re-INVITE 32 秒超时回滚；REGISTER `video` Contact feature 同时受 catalog 与本地 capability 门控；语音信箱 effective number 支持 `AT+CSVM?`、catalog、SIM override 来源链。以上仍不能替代真实运营商 Ut/MWI/E911、Linphone 和多硬件验收。
- 2026-08-11：补齐 UT catalog/transport 基础：schema-v7 profile 可显式携带 XCAP HTTPS root、document selector、namespace 和 `digest_aka` policy；缺失时保持关闭。共享 transport 禁止 redirect、支持源地址 bind 与一次 challenge provider 重试，并改为边读边执行 512 KiB 上限。尚未把当前 VoLTE bearer 或 VoWiFi TUN 的源地址/AKA provider 接到 API，因此未宣称真实 UT 可用。
- 2026-08-11：完成 UT 线路级接入：VoLTE 从当前 session 获取 IMS bearer 源地址/QMI/UIM/AID，VoWiFi 从已注册 ePDG/TUN channel 获取源地址/IMPI；两者通过同一 `XcapAccessContext` 和 Digest-AKA provider 服务 `GET/PUT /api/ims/lines/{line_id}/ut/{document}`，supplementary readiness 按 access owner 更新并隔离旧 access teardown。仍需真实运营商网络回读。
- 2026-08-11：修正 OIR/CLIR 的 `active` 反向语义：`active=true` 现在表示限制已启用，而 OIP/CLIP 的同一字段仍表示允许显示。API 采用规范 `originating-identity-presentation-restriction` 文档名并兼容旧别名；定向 XML/UT 回归测试通过，真实运营商回读仍待完成。
- 2026-08-11：重新审计 CS supplementary provider：它只使用 ModemManager 明确公开的呼叫等待 D-Bus 方法；其他补充服务和 CS trunk 继续 fail closed，未发现自动发送 MMI/USSD/猜测性 AT 的路径。
- 2026-08-11：VoLTE 与 VoWiFi 各自限制为最多两个并发 IMS dialog；第三路 MO/MT 呼叫明确以 SIP 486 收口。VoWiFi 定向回归确认第三路不会影响前两路 dialog；hold/resume 与真实双通话媒体仍待外部验收。
- 2026-08-11：完成 P4 媒体回归收口。共享 relay 严格识别并透明转发完整 RTCP-mux compound packet，不把其计入 RTP 指标或首 RTP answered 事件；双 relay 测试验证 socket、SSRC、动态 PT 与 RTCP-mux 不串线。VoLTE/VoWiFi re-INVITE 改为暂存媒体快照，488、超时或发送失败只释放 pending relay 并恢复已确认音频；VoLTE 补齐与 VoWiFi 一致的 32 秒网络 504/trunk 408 超时收口。独立 RTCP 端口、真实视频互操作与 hold/resume 仍不在此项完成范围内。
- 2026-08-11：完成 P5.4 语音信箱拨号契约。新增按线路 `POST /api/ims/lines/{line_id}/voicemail/call`；号码从同一 SIM binding 的 override、当前 SIM `AT+CSVM?`、只读 catalog 解析，完整号码不写入响应或日志。`VoiceAccessRouter` 新增 access-aware call plan，在实际 policy 选路后才向对应 adapter 投递 `StartCall` 并返回初始 access，避免 VoWiFi/VoLTE codec policy 混用；后续事件和故障切换继续走原 trunk route。接口只使用预留 loopback RTP sink，实际听取语音信箱仍需要已接入的本地音频或 Asterisk media backend。
- 2026-08-11：P5.3 增加 hold/resume 媒体方向收口。VoLTE/VoWiFi 的跨 leg SDP 方向统一取对端反转值；共享 relay 根据 offer/answer 的方向只转发允许的 RTP，`inactive` 不转发 RTP，RTCP-mux 仍可双向通过。完整第二路状态矩阵和真实双通话互操作仍未完成。
- 2026-08-11：P5.3 完成共享 blind REFER 协议核心及 dialog 接线：统一目标 URI 注入防护、Service-Route 请求构建、refer-event `message/sipfrag` NOTIFY 解析与单向终态状态机；Asterisk B2BUA、VoLTE、VoWiFi 均已接入按 call 隔离的响应、通知和超时处理。attended `Replaces` 因跨 B2BUA leg 缺少 dialog 映射而明确返回 501；真实运营商、Asterisk 和 Linphone 互操作仍待外部验收。
- 2026-08-11：P5.3 补齐连续 REFER 的订阅关联与失败边界。VoLTE/VoWiFi 以各自发出 REFER 的 CSeq 校验运营商 `Event: refer;id=`，显式错配或非法 id fail closed；B2BUA 重新使用 Asterisk REFER CSeq 构造 NOTIFY，避免跨 leg 复用标识。access command consumer 消失时返回本次 REFER 的 503，未知 route 返回 481，不再用 call-level `Unavailable` 错误终止已确认通话。
- 2026-08-11：P5.3 完成本地双 dialog 状态矩阵。VoWiFi TCP operator session 与 VoLTE UDP protected-channel 测试均覆盖独立 Call-ID/relay、180/183/200/486、拒绝后槽位复用、hold/resume、一路 hold 时另一路实际 RTP 转发与 SIP INFO DTMF，以及分别 BYE；外部双通话互操作仍未据此标记完成。
- 2026-08-11：完成 P0.2 双 adapter 共享 REGISTER contract。相同矩阵分别从 VoLTE shared-driver exchange 和 VoWiFi adapter-owned protected exchange 入口运行，覆盖 200/401/407、AUTS 两轮、重复拒绝、provisional 与成功 artifacts；同时修复 VoWiFi 认证后只读一帧的问题，初始 socket、AUTS 和 protected candidate 现在都会有界跳过 provisional 后再处理终态。
- 2026-08-11：接通共享注册生命周期结果。VoLTE refresh 改用保留终态响应的 REGISTER driver，VoLTE/VoWiFi 统一分类鉴权拒绝、网络拒绝、信令与 access transport 丢失，再交回各 adapter 清理 bearer 或 IKE/ESP/TUN；未把未实现的 AKA `Expires: 0` 冒充确认注销。新增 SQLite override 只读不改 `updated_at`/document、同 catalog 双 SIM/eSIM access 切换，以及一线 operator teardown 后另一线继续 INVITE 的 contract tests。
- 2026-08-11：完成 P0.2 显式 IMS 注销。共享 driver 复用有界 401/407/AUTS challenge 状态机并区分 Confirmed/Rejected/AccessLost/AlreadyExpired；VoLTE 在释放 XFRM/bearer 前复用 REGISTER identity 和按线路 QMI/UIM AKA，VoWiFi 通过持有 protected socket 的 operator task 调用按线路 AKA factory，再清理 IKE/ESP/TUN。内部故障恢复保留立即 abort 路径，避免已断链时等待注销超时；6 个注销定向测试通过，真实运营商响应仍待实机记录。
- 2026-08-12：完成 XCAP partial-update 与 TLS policy 收口。线路 PUT 改用类型化 `UtMutation`，按 catalog 明示的 document selector 选择 element PUT，并继续执行 GET→`If-Match` PUT→GET 权威回读；未配置当前文档 selector 时保持整文档更新。HTTPS transport 强制仅 HTTPS、无 redirect、TLS 1.2/1.3 policy、可信根策略和可选 carrier CA，且不提供关闭证书或主机名校验的开关。真实运营商 element selector 和私有 CA 仍需网络验收。
