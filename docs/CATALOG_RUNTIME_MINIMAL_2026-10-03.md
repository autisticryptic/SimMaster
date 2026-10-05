# 进一步精简及小米VoWiFi提取修正

> 后继已完成完整固件下载/重提取，恢复380条静态ready VoWiFi并测试后推送7fab7a5。
> 见[最新完整OTA修复](XIAOMI_VOWIFI_FULL_OTA_2026-10-03.md)。下文缺输入和零VoWiFi为早先记录。

独立数据库仓库提交 **3442c0b89a01cd82bd11268353efbca19dc697a5** 已推送并核验远端。
本轮没有操作410或替换其运行库；未更新线上Release。

## 运行时精简已完成

新开关 `--runtime-minimal` 配合模拟报告使用，两个数据库发布构建工作流已启用。
在原直接删除策略之后清空 `field_evidence` 的审计记录，保留8张表、索引及schema v7/contract v1。
SimAdmin运行时仅要求该表存在，不读取其中的行。完整版保存全部原始证据，报告绑定其SHA256。
其他依赖在线证据的消费者仍应选full/no-icons。

| 来源 | no-icons MiB | 新minimal MiB | 文件减少 |
|---|---:|---:|---:|
| IPCC | 11.1016 | 5.8125 | 47.64% |
| iPhone | 15.0938 | 7.0195 | 53.49% |
| Pixel | 8.4297 | 3.8398 | 54.45% |
| 小米 | 2.8594 | 1.3594 | 52.46% |
| 合计 | 37.4844 | 18.0313 | 51.90% |

产物：`../carrier_Bundles/data/variants/2026-10-03-runtime-minimal/`，四来源×三变体，共12库。
此减幅是去运行时证据冗余，**不是额外删除51.9%的注册配置**；接入删除仍为614 LTE、4 VoWiFi。

验证：59数据库Python测试；真实消费者618项已删除接入仍走派生、11326其他投影保持、NR保持。
另新增消费者单测证明去证据前后完整策略/身份解析一致。六张运行配置/匹配/来源表与旧minimal
逐项相同，schema SQL相同；full/no-icons与旧产物逐字节一致。12库完整性/外键及所有SHA256通过。

## 小米：确定的提取缺陷已修，实际VoWiFi补齐仍未完成

现有721条记录全部来自APN表（719 apns-conf、2 fiveG-apns-conf），不是解码后的MCFG策略。
提取器原来在product找到任意配置就提前退出，跳过后续mi_ext/system_ext/vendor/odm；还把无关
aconfig/linker/speech protobuf误计作配置。已修复完整分区扫描、路径过滤与旧提取缓存失效，
合成fixture回归通过。缓存manifest版本变化不改变任何数据库格式。

本地仅剩基带镜像，没有原完整OTA或Android分区/APK。基带MCFG确有IWLAN XML，但有禁用静态域名、
紧急专用域名及多版本并存，不能只提取ePDG字符串、从域名猜SIM匹配，再声称导入VoWiFi成功。
APK解码及MCFG选择/优先级解析仍未实现；当前新小米库仍为0个VoWiFi接入。

完整固定OTA镜像HEAD探测HTTP200，长度 **9,035,445,935字节（约8.41GiB）**，本轮没有下载。
下一步需要恢复或下载完整固件/Android分区，安装payload与EROFS工具，重跑修正后的扫描；
若配置位于APK，必须检查真实资源与PLMN/MVNO选择规则后增加解析器。不能用APN存在推断支持VoWiFi。
详细调查：`../carrier_Bundles/android/xiaomi/INVESTIGATION.md`。

证据：`.local/evidence/catalog-runtime-minimal-20261003/` 中的 `delivery.json`、`equivalence.json`、
`consumer-real.log`、`consumer-unit.log`、`python-final.log`、`xiaomi-source-inventory.json`、
`xiaomi-mcfg-findings.json`、`rom-availability.json`。
