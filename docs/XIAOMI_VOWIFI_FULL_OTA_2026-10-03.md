# 小米完整固件VoWiFi修复与本地验收

数据库仓库 **7fab7a585b31bc5f6e11d6bac5c6916de8f946ae** 已在本地验证通过后提交并推送，远端main已核验。
本轮未操作410、未更换运行库、未发布GitHub Release；固件和生成数据库没有混入Git提交。

## 原始固件与真实来源

用户授权后已下载完整 `xuanyuan Global / OS3.0.301.0.WOAMIXM / Android16` OTA：
9,035,445,935字节，SHA256 `be2572f4082d294d9b4c25d74133daeba016733406320777d916acbb647c2135`，
与旧数据库记载的原固件一致。保存于WSL `/root/.cache/xiaomi-full-ota-20261003/xuanyuan-full-ota.zip`。

product、mi_ext、system_ext、vendor、odm、system分区经payload校验及EROFS提取。
真实策略来自CarrierConfig.apk、CarrierConfigResCommon_Sys.apk、MiuiCarrierConfigOverlay.apk和
mi_ext的vendor_miui.xml，不是从MCFG抓域名或把APN存在当作VoWiFi可用。

新增有边界检查的APK纯文本/Android二进制XML解析，解码455份XML；按实际DEX验证的覆盖顺序和
mDefaultPb刷新点处理引用。只加载2g_and_3g_sunset.xml、globalization.xml、default_v2.9.6.xml，
不合并所有历史默认版本。MCC/MNC单值数值比较与逗号列表字符串匹配分开，保留原PLMN身份。

未知carrier-ID映射、GID/SPN/IMSI/设备条件的冲突值不推广到整个PLMN；MVNO专属APN也不泛化。
显式false保持禁用。WFC事实与派生ePDG/IKE证据分开记录，混合section不再错误归因。
APK manifest先检查大小再解压，异常覆盖文件/重复ZIP路径/未知包含关系失败关闭。
已知损坏的no-SIM占位文件单独记录且不用于有SIM配置。

## 最终结果

| 项目 | 数量 |
|---|---:|
| Profile | 941 |
| LTE IMS静态ready | 644 |
| NR IMS静态ready | 655 |
| VoWiFi静态ready | **380** |
| VoWiFi明确unsupported | 560 |
| VoWiFi未知 | 1 |

380个WFC=true的最终原始证据：308来自default_v2.9.6，68来自设备覆盖资源，4来自sunset文件。
标准ePDG/AKA/IKE均明确标记standard_derived，**不是380次实网注册或已证明网关可达**。
早期335条候选在审查发现sunset遗漏和数值匹配差异后未提交，已重建并以380条最终结果验收。

## 测试、产物与边界

- 提交前97项数据库Python测试通过。
- 实际SimAdmin消费者逐条加载380 VoWiFi及644 LTE ready配置，858项非ready接入按预期拒绝。
- 新12库：`../carrier_Bundles/data/variants/2026-10-03-xiaomi-vowifi-final/`。
  全部完整性、外键、schema v7/contract v1、SHA256通过。
- 新集合189项删除接入保持现有派生解析，12196项其他投影及NR保持。
- 小米三版均保留380条VoWiFi ready；本地新小米full/no-icons约8.76MiB、minimal约2.66MiB。
  本次重建明确跳过图标同步，不能把full/no-icons相同误称图标提取通过。
- 新真实策略使旧430项小米LTE删除条件不再全部成立；不为维持旧删减率而扔掉新事实。
- 动态opconfig/APEX更新、所有carrier-ID/MVNO组合、实机隧道/通话未验证，不对外宣称全覆盖。

详情与可复现命令在数据库仓库 `android/xiaomi/FULL_OTA_VALIDATION.md`。
证据：`.local/evidence/xiaomi-full-ota-20261003/` 的 `download-verified.json`、
`full-build-verified.log`、`precommit-tests.log`、`consumer-rebuilt-final.log`、
`consumer-variants-final.log`、`artifacts-verified.json`、`delivery.json`。
SimAdmin新增测试仅本地保留；没有为测试部署或重启当前服务。
