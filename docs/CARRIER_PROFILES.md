# 运营商 Profile 来源与维护边界

> 状态：应用侧读取、匹配和本地覆盖已落地；公开 AOSP/IPCC 事实 importer 不属于当前运行时，手机基带固件的深度逆向提取未排期。
> 原始调研记录于 2026-07-29，本页按当前实现更新。

## 目的

VoLTE/VoWiFi 注册依赖运营商的 APN、ePDG、IKE/ESP proposal、IMS domain/realm、SIP
头字段和重试策略。仅按 MCC/MNC 推导 3GPP 默认值无法覆盖所有网络，因此 SimAdmin 将
“可发布的运营商基线”“设备上的用户覆盖”和“明确标记的标准推断兜底”分开管理。

## 当前数据模型

1. **只读 carrier catalog**
   - 由独立的 `carrier_Bundles` 流程收集、规范化、审计并生成 SQLite release。
   - SimAdmin 运行时只接受已封存、schema 与 config contract 兼容的 v7 catalog。
   - catalog 同时包含 LTE/EPC 与 WiFi/ePDG access 配置、公共身份匹配和来源引用。
2. **本地覆盖**
   - 用户修改保存在 SimAdmin 自身的 `data.db`，不改写发布 catalog。
   - 解析时本地覆盖优先；删除覆盖后恢复 catalog 基线。
3. **旧配置迁移**
   - 旧 `vowifi-profiles.conf` 会在存在 catalog 基线时一次性迁移到本地覆盖表，然后重命名
     为 `.conf.migrated`。
4. **标准自动推断**
   - 只在未显式指定 profile 且本地覆盖、carrier catalog 都没有可用 access 配置时启用。
   - LTE 与 VoWiFi 分别生成独立 profile，只推导 `ims` APN、3GPP IMS domain/realm 和 ePDG
     FQDN，并采用保守的通用 IKE/ESP 基线。
   - 不猜测静态 P-CSCF、visited-network、entitlement、XCAP 或 E911 配置。
   - API 和页面始终标记来源为 `derived`，并保留数据库缺失、`unknown`、`partial` 或校验失败
     的原始原因。后续 Bearer、P-CSCF、IKE 或 REGISTER 失败时，页面同时显示推断来源和实际失败阶段。
   - 用户显式指定的 profile 保持严格模式；配置不存在或不可用时直接报错，不自动切换到推断值。

主要实现位置：

- `backend/src/connectivity/modems/ims/vowifi/carrier_catalog.rs`
- `backend/src/connectivity/modems/ims/vowifi/carrier_catalog_v7.rs`
- `backend/src/connectivity/modems/ims/vowifi/profile_store.rs`
- `backend/src/connectivity/modems/ims/vowifi/profile_record.rs`

运行时不解析 AOSP/IPCC 原始文件。标准自动推断只作为未验证的实验性兜底，不表示该运营商已受支持；
正式支持仍需在独立 carrier catalog 流程中完成来源审计、字段校验和封存，再由 SimAdmin 加载兼容的
catalog release。

## 支持的来源

### Android

- AOSP `apns-conf.xml`、CarrierConfig XML 和 Apple plist 仅作为独立 catalog 流程的研究来源；SimAdmin
  不直接解析这些文件。
- 厂商 `/vendor` 配置、Qualcomm MBN 与专有 IMS 数据可能包含更多字段，但格式和授权边界
  不稳定，SimAdmin 运行时不直接解析这些固件资产。

### Apple

- IPCC / carrier bundle 的 XML plist：可提取 APN、VoLTE/VoWiFi 开关和部分 E911 信息。
- SimAdmin 不下载或分发 Apple bundle；导入器只处理用户依法取得的 XML plist 事实。

### 标准兜底

- ePDG FQDN、IMS domain/realm 等少数字段按 3GPP 规则从 MCC/MNC 推导。
- 推断 profile 使用通用保守基线，可能在 IKE、P-CSCF 或 SIP REGISTER 阶段失败；失败不改变
  catalog 状态，也不会被记录成已验证配置。
- catalog 条目标记为 `partial`、`unknown` 或缺少运行时必需字段时，仍保持原状态；自动流程可以
  尝试推断值，但页面必须同时展示“数据库没有可用配置”和实际失败原因。

## 已知限制

1. IKE/ESP proposal、AKA 细节和 SIP 变体经常只存在于 modem 固件或专有 IMS 栈中，公开
   配置不一定足够完成注册。
2. 手机侧字段与 `CarrierProfileRecord` 并非一一对应；导入事实必须叠加在可信 catalog
   基线上，不能用猜测值覆盖未知字段。
3. Android/iOS 和厂商格式会变化，独立 catalog 流程若重新引入解析器，必须配套 fixture、来源引用和 schema 契约测试。
4. E911 元数据目前可进入 catalog，但 SimAdmin 尚未执行完整的紧急呼叫定位流程。
5. 固件、carrier bundle 和运营商配置可能受版权、许可或设备条款约束；收集、使用和分发前
   应分别确认授权，不应把来源不明的原始资产提交到本仓库。

## 维护原则

- 原始资料收集、标准化、去重、来源审计和 release 封存属于 `carrier_Bundles` 项目。
- SimAdmin 只维护受支持 schema 的只读适配器、运行时校验、本地覆盖和用户导入入口。
- catalog 更新应作为独立制品评审；不要把个人测试数据库、未封存 SQLite 或原始厂商资产
  直接放入发布制品。
- 新增字段时先更新 catalog contract 和 fixture，再更新 `CarrierProfileRecord`、v7 adapter、
  运行时使用点、API/前端以及 `docs/DEVELOPMENT_PLAN.md` 中的验收项。
