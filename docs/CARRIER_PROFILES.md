# 运营商配置、派生与数据库变体

## 1. 三种来源，不混淆标签

- `database`：SimAdmin应用数据库中用户保存的profile，不是下载的carrier SQLite。
- `carrier_catalog`：独立`carrier_Bundles`项目生成、封存的schema-v7只读catalog。
- `derived`：缺少可用接入配置时，按可信SIM归属身份生成的标准推断；不是运营商认证配置。

线路三个来源槽独立保存。requested来源是槽位标签；实际origin/fallback_reason必须另外记录。
database/catalog缺项或不可投影后可以在该槽内回退derived，不能把requested=database当成专属数据库参数已使用。
旧通用pin接口的严格行为，与线路candidate接口允许的缺失行回退，须按各自API契约处理。

归属PLMN不能拿驻网PLMN替代。标准推导只覆盖有依据的IMS域/realm、APN/ePDG等默认值；
不猜静态P-CSCF、用户身份、开通、XCAP、E911、访问网差异或安全特例。
注册流程见[IMS协议](IMS_REGISTRATION_POLICY.md)，资源校准见[MM生命周期](IMS_MM_EXACT_FAMILY_LEASE_DESIGN.md)。

## 2. 存储与导入边界

SimAdmin不在运行时解析Apple/AOSP/厂商固件。收集、提取、来源审计、归一化和封存属于独立catalog项目。
运行时只读取契约兼容、sealed的SQLite，用户覆盖保存在自己的data.db，不改写发布catalog。
旧配置迁移必须可追踪/幂等，不把未知ready状态改成支持，也不能冒用另一个MVNO条目。

主要入口：`carrier_catalog.rs`、`carrier_catalog_v7.rs`、`profile_store.rs`、`profile_record.rs`。
LTE/EPC与Wi-Fi/ePDG分别投影；存在NR字段不等于实现了NR/5GC独立注册适配。

覆盖优先级应区分profile源选择与按SIM字段覆盖。删除用户覆盖后回归基线，不能删掉原catalog证据。
配置未知/partial/disabled/unsupported必须保留原状态，同时报告派生回退和实际失败阶段。

## 3. 四来源与三种变体

IPSW、在线IPCC、Pixel、小米各自产出full/no-icons/minimal-no-icons，共12份独立数据库。
不把不同固件的字段混合拼成“全能运营商配置”，不新增私有解码格式。

| 变体 | 保留/删除 |
|---|---|
| full | 封存输入的字节等同副本，保留图标和审计证据 |
| no-icons | 只删除视觉资源及指针，配置/匹配保留 |
| minimal-no-icons | 只移除有明确模拟覆盖的接入配置；可选runtime-minimal再清字段审计行 |

全部仍为SQLite schema v7、8表及`carrier-bundles-ims-v1`契约，schema/index不因精简被替换。
原始证据保留在full。不能以“标准运营商”标签批量删行，不能以体积目标扩大裁剪条件。

## 4. 裁剪条件与能力边界

LTE和VoWiFi独立判定：一种可派生、另一种不可派生时只删除已覆盖的接入。
存在其他业务/共享策略或NR时不能整行删除；全接入覆盖且无其他保护项才允许删除整行及级联引用。

未知/更严格策略、私有域、APN认证、单族要求、显式隐私/加密、非默认SIP头、媒体/开通/额外服务均需保留。
父级证据只有在与原配置子树完全一致时才能裁剪；不可解释的原值保留。
“保留”只表示没有充分等价证据，不代表派生一定失败。

删除后的接入依赖消费者已有的derived回退，不引入隐藏重建标记。缺少同等派生能力的其他消费者应选full/no-icons。
模拟假定订阅/承载/P-CSCF可达和有效SIM材料，不证明真实运营商、全部卡、IKE/IPsec或5G网络都能注册。

证据必须绑定源文件、测试程序和日志摘要。行为修改后旧冻结证据不自动升级为新版本证明；
新增全局兜底回归必须验证协议状态和请求继承，不能用运营商特例测试授权扩大删库。

## 5. 当前已发布集合的事实

独立catalog项目已发布 [v0.3.1-catalog-v7](https://github.com/autisticryptic/carrier_Bundles/releases/tag/v0.3.1-catalog-v7)，
目标`814b057`，构建run37201477372、发布run37206201366；20公开文件和12只读SQLite曾独立下载校验。
这些是带日期的制品事实，不等于当前任意设备已安装该库。

| 来源 | no-icons字节 | minimal字节 | 减幅 |
|---|---:|---:|---:|
| Pixel mustang | 8,839,168 | 4,026,368 | 54.45% |
| iPhone16ProMax 27.0.1 | 15,826,944 | 7,360,512 | 53.49% |
| Apple IPCC | 12,267,520 | 6,336,512 | 48.35% |
| Xiaomi15Ultra | 9,453,568 | 2,826,240 | 70.10% |
| 合计 | 46,387,200 | 20,549,632 | 55.70% |

主要收益来自`field_evidence`及载荷瘦身，不是删除同等比例的注册策略。
旧v0.3.0资源使用更早构建、未启用runtime-minimal，曾只减少约0.626%；Release创建日期与资源上传日期不同。
旧Release/tag/assets未覆盖，不能把GitHub页面日期当成当前构建算法的证据。

小米full OTA经固定SHA256验证，真实WFC策略由APK/XML和明确覆盖顺序提取，不从MCFG字符串猜配。
三变体均保留380条**静态**VoWiFi ready；本轮跳过图标同步，不把full/no-icons相等说成图标提取完成。
380条静态配置不代表380次实网验收，动态APEX/opconfig、carrier-ID/MVNO组合仍需验证。

## 6. Pixel/iOS对照的已知审查点

曾有“Pixel可注册但呼入转语音信箱，iOS可接打”的用户反馈。必须控制同卡/同版本/同接入/同有效profile，
不能把几份库最终都回退derived解释成各自专属配置通过。

历次审查涉及：user-agent模板映射、security_agreement路径、接入专属SIP覆盖、显式Contact/MMTEL参数、
身份/Service-Route和媒体路径。旧空Contact导致漏MMTEL的缺陷已有修正；不可重复宣称它必是新故障根因。
这些是回归关注点，不是对当前源码的未经核验缺陷结论。

## 7. 维护要求

新增字段需同步catalog契约、fixture、v7投影、有效profile、运行时、API/前端和全局兼容测试。
不将个人数据库或原始厂商资产提交到SimAdmin；收集和发布前分别确认许可/条款。
生产换库与程序升级应尽量分开，以免无法归因。不要通过降低安全、重写realm或伪造ready状态得到绿色结果。

本页合并了早期变体/裁剪审计、runtime-minimal、小米重建和Actions发布报告。
完整旧正文在Git历史与本机文档备份；主要验证时间线见[历史摘要](archive/README.md)。
