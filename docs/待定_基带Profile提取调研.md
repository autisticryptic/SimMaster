# 待定：从 iOS / Android 基带提取运营商 VoWiFi Profile（调研）

> 状态：**待定 / 未排期**。不在「VoWiFi/VoLTE 四点改造」本轮实现范围内。
> 记录日期：2026-07-29

## 动机

WiFi Calling 的「指定运营商 profile」下拉只列**数据库**里的 profile。数据库是懒填充的：
- SIM 通过动态生成的 profile 成功连上 VoWiFi 后自动落库；
- 或用户手动在「运营商 Profile」页写入。

新设备刚装好时数据库可能几乎是空的。若能从手机基带侧把厂商已经调好的运营商 VoWiFi
参数（ePDG、IKE proposal、IMS domain/realm、Contact 形态等）提取出来导入本数据库，
就能一次性填充大量可用 profile，省去逐个真机试错。

## 目标产物

一段能把「手机侧运营商配置」转换成本项目 `CarrierProfileRecord`（见
`backend/src/connectivity/modems/softstack/vowifi/profile_record.rs`）的映射，
经 `ProfileStore::save()` 校验后落库。

## 可能的来源（待逐一验证可行性 / 合法性）

### Android
- **运营商配置来源**：
  - CarrierConfig（`CarrierConfigManager`，`KEY_*` 键值，部分与 VoWiFi/IMS 相关）。
  - `carrier_list` / carrier settings APK、`/vendor` 下的运营商 XML。
  - IMS 相关：`ImsManager`、`vendor.qti` 的 IMS 配置、`mbn`（Qualcomm Modem 配置二进制）。
- **ePDG/IKE**：多数走 3GPP 标准 FQDN（`epdg.epc.mncXXX.mccYYY.pub.3gppnetwork.org`），
  IKE/ESP proposal 往往在 modem `mbn` 或 IMS 栈内，未必以明文配置暴露。
- **取数手段**：`adb shell dumpsys carrier_config`、读 `/vendor` 分区（需 root）、解析 mbn（难）。

### iOS
- **运营商配置来源**：Carrier Bundle（`.ipcc` / `.bundle`，含 `carrier.plist`、
  `Overrides.plist` 等），部分含 IMS/VoWiFi 开关与域名。
- **取数手段**：从固件/ipsw 或设备提取 carrier bundle，解析 plist。ePDG/IKE 细节同样
  多在基带侧，bundle 未必完整。

## 已知难点 / 风险

1. **ePDG 的 IKE/ESP proposal、AKA 细节**通常**不在**手机明文配置里，而在 modem 固件
   （mbn / baseband）内部，提取难度高，未必可得。可提取的多是「域名/APN/开关」层面。
2. **合法性**：解析厂商固件 / carrier bundle 可能涉及版权与逆向条款，需先确认用途边界
   （自用调试 vs 分发）。
3. **字段映射不完全**：手机侧字段与本项目 `CarrierProfileRecord` 不是一一对应，缺失项
   仍需用 3GPP 标准派生填默认（等于回到现有动态生成逻辑）。
4. **维护成本**：Android/iOS 每代格式会变，提取脚本需持续跟。

## 建议的调研步骤（若排期）

1. 先只做 **Android CarrierConfig + carrier XML** 的「域名/APN/IMS 开关」层，验证能否稳定拿到
   `epdg host / apn / ims domain-realm`，映射进 `CarrierProfileRecord` 的 epdg/ims 部分，
   IKE/ESP 仍用标准默认。
2. 评估 iOS carrier bundle 能补充多少 Android 拿不到的字段。
3. 明确合法用途边界后，再决定是否做 mbn / baseband 深度提取。

## 关联

- 数据库 profile 结构：`backend/.../vowifi/profile_record.rs`（`CarrierProfileRecord`）。
- 落库入口：`backend/.../vowifi/profile_store.rs`（`ProfileStore::save`）。
- 动态派生（无提取时的兜底）：`backend/.../vowifi/profiles.rs`（`generate_standard_3gpp_profile`）。
- 本轮四点改造计划：`../VOWIFI_VOLTE_四点改造计划.md`。
