# 运营商 Profile 来源与维护边界

> 状态：应用侧读取、匹配和导入框架已落地；手机基带固件的深度逆向提取未排期。
> 原始调研记录于 2026-07-29，本页按当前实现更新。

## 目的

VoLTE/VoWiFi 注册依赖运营商的 APN、ePDG、IKE/ESP proposal、IMS domain/realm、SIP
头字段和重试策略。仅按 MCC/MNC 推导 3GPP 默认值无法覆盖所有网络，因此 SimAdmin 将
“可发布的运营商基线”和“设备上的用户覆盖”分开管理。

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

主要实现位置：

- `backend/src/connectivity/modems/softstack/vowifi/carrier_catalog.rs`
- `backend/src/connectivity/modems/softstack/vowifi/carrier_catalog_v7.rs`
- `backend/src/connectivity/modems/softstack/vowifi/profile_store.rs`
- `backend/src/connectivity/modems/softstack/vowifi/profile_record.rs`
- `backend/src/connectivity/modems/softstack/vowifi/profile_import.rs`

## 支持的来源

### Android

- AOSP `apns-conf.xml`：按 PLMN 提取 IMS APN 等事实。
- AOSP CarrierConfig XML：提取运营商是否支持 VoWiFi/IMS 等公开配置。
- 厂商 `/vendor` 配置、Qualcomm MBN 与专有 IMS 数据可能包含更多字段，但格式和授权边界
  不稳定，SimAdmin 运行时不直接解析这些固件资产。

### Apple

- IPCC / carrier bundle 的 XML plist：可提取 APN、VoLTE/VoWiFi 开关和部分 E911 信息。
- SimAdmin 不下载或分发 Apple bundle；导入器只处理用户依法取得的 XML plist 事实。

### 标准兜底

- ePDG FQDN 等少数字段可按 3GPP 规则从 MCC/MNC 推导。
- 推导值只适合作为明确字段的兜底，不能凭空补齐运营商专有 proposal、SIP 变体或重试策略。
- catalog 条目标记为 `partial` 或缺少运行时必需字段时，不应伪装为可直接拨号的 `ready` 配置。

## 已知限制

1. IKE/ESP proposal、AKA 细节和 SIP 变体经常只存在于 modem 固件或专有 IMS 栈中，公开
   配置不一定足够完成注册。
2. 手机侧字段与 `CarrierProfileRecord` 并非一一对应；导入事实必须叠加在可信 catalog
   基线上，不能用猜测值覆盖未知字段。
3. Android/iOS 和厂商格式会变化，解析器必须配套 fixture、来源引用和 schema 契约测试。
4. E911 元数据目前可进入 catalog，但 SimAdmin 尚未执行完整的紧急呼叫定位流程。
5. 固件、carrier bundle 和运营商配置可能受版权、许可或设备条款约束；收集、使用和分发前
   应分别确认授权，不应把来源不明的原始资产提交到本仓库。

## 维护原则

- 原始资料收集、标准化、去重、来源审计和 release 封存属于 `carrier_Bundles` 项目。
- SimAdmin 只维护受支持 schema 的只读适配器、运行时校验、本地覆盖和用户导入入口。
- catalog 更新应作为独立制品评审；不要把个人测试数据库、未封存 SQLite 或原始厂商资产
  直接放入发布制品。
- 新增字段时先更新 catalog contract 和 fixture，再更新 `CarrierProfileRecord`、v7 adapter、
  运行时使用点、API/前端以及真机测试清单。
