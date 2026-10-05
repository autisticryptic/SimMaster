# carrier 精简产物的 Actions 验证（2026-10-04）

## 最终公开发布

用户明确要求继续完成后，已发布 **[v0.3.1-catalog-v7](https://github.com/autisticryptic/carrier_Bundles/releases/tag/v0.3.1-catalog-v7)**。
Release/tag指向已验证的 `814b057ea9c0982bd636f3e4908117d72bd509be`，复用下文原Actions产物，未重新构建。

- [安全dry-run / 37206088071](https://github.com/autisticryptic/carrier_Bundles/actions/runs/37206088071)通过。
- [正式发布 / 37206201366](https://github.com/autisticryptic/carrier_Bundles/actions/runs/37206201366)通过。
- 隔离发布分支 `release-staging/catalog-set/20261004T133100Z`，发布工作流快照 `028439898ec460c9414d70041f1b53813ad0ac77`。
- 从公开Release独立下载 **20个文件**，逐一校验大小/SHA，12份SQLite只读校验通过。
- 公开SHA256SUMS自身摘要：`626081a6485171bc3f365cbed3df57e9ddc1fb20a488a2a127091f48b5daccbf`。
- 旧 `v0.3.0-catalog-v7` Release/tag/assets未动，main仍2966036，用户HEAD/索引/既有脏源码未改。
- 没有安装新数据库到410；Globe修复仍使用设备原来的运行catalog，避免同时改变来源混淆结论。

证明：`.local/evidence/ims-switch-actions-20261004/carrier/release-v0.3.1-catalog-v7/`
中的 `release-proof.json`、`preservation-proof.json`及公开下载文件。

## 结论

`minimal` 与普通 `no-icons` 接近的直接原因是**公开Release仍使用旧构建结果**，
不是当前 `--runtime-minimal` 实现无效。

旧 `v0.3.0-catalog-v7` Release 对象创建于8月8日，但数据库资源实际在10月3日重新上传，
来自Actions `37095019351 / c445d53`。其manifest只有19场景证据、没有runtime-minimal策略，
总减幅约0.626%。不能以Release创建日期认定资源一直没更新，也不能用本地旧manifest冒充最新发布。

本轮远程重建后，四来源 **44.24 MiB → 19.60 MiB，减少55.70%**。
主要节省来自审计证据 `field_evidence` 行及其载荷，不是删除相同比例的运营商注册策略。

## 已验证产物

- [Actions 37201477372](https://github.com/autisticryptic/carrier_Bundles/actions/runs/37201477372)：success。
- 源码快照：`814b057ea9c0982bd636f3e4908117d72bd509be`。
- 验证分支：`dev/runtime-minimal/20261004T121214Z`。
- artifact：`11303097227 / runtime-minimal-validation-37201477372`。
- 官方ZIP摘要：`4471b902945a6f17f7a928d1c108d74d673f9c5f62c2387c1656a8c3429aa090`。
- 已实际下载、核验官方摘要、包内全部SHA清单，并以只读连接复核四源12份SQLite。

| 来源 | no-icons 字节 | minimal 字节 | 减幅 |
|---|---:|---:|---:|
| Pixel mustang | 8,839,168 | 4,026,368 | 54.45% |
| iPhone 16 Pro Max / 27.0.1 | 15,826,944 | 7,360,512 | 53.49% |
| Apple IPCC | 12,267,520 | 6,336,512 | 48.35% |
| Xiaomi 15 Ultra | 9,453,568 | 2,826,240 | 70.10% |
| **合计** | **46,387,200** | **20,549,632** | **55.70%** |

新数值与此前本地集合不同，原因是IPCC/固件来源及生成内容来自本轮明确固定的输入；
上表只引用本轮已核验产物，不拼接不同集合的大小。

## 输入、构建与保持项

- IPCC、Pixel及IPSW复用旧成功Actions的**完整库**，先按其manifest核验大小/SHA和原输入一致性。
  不把minimal再当full输入，不重新下载Google/Apple固件或代替用户接受新的服务条款。
- 旧Xiaomi库为零WFC，故不复用；在Actions重新下载此前已授权的固定完整OTA并核验SHA256
  `be2572f4082d294d9b4c25d74133daeba016733406320777d916acbb647c2135`，使用当前提取器重建。
- 小米显式跳过图标同步；三变体都保持 **380条静态VoWiFi ready**，不是380张卡实网成功。
- 使用当前裁剪器及已冻结的24场景证据；**没有扩大裁剪条件或改写旧证据为新测试结果**。
  carrier冻结证据源树`fefddf12…`与本轮SimAdmin的新`4955e8d5…`是两份独立记录，不冒充同一报告。
- 保持schema v7 / `carrier-bundles-ims-v1`、8表及索引；完整库字节保持输入，no-icons保留配置，
  minimal清审计证据但不额外删除未覆盖策略/匹配/NR。构建器既有保持项测试及本轮新门禁在Actions通过。
- 本轮没有本机数据库构建、Rust或前端编译；本机下载与SQLite验证均为只读。

## 工作流与发布安全

新增 `../carrier_Bundles/.github/workflows/validate-runtime-minimal.yml`，内容/actions权限均只读，
仅上传验证artifact，不创建Release/tag。新增 `tools/rebuild_runtime_minimal_ci.py` 对固定输入、
四源12库、runtime-minimal、摘要和小米WFC数量实行失败关闭。

旧 `build-catalog-set.yml` 在任何一个来源成功后都可能删旧Release全部资产。本轮已在验证快照中改为：

1. 默认 `publish_release=false`，部分来源成功仅留artifact。
2. 发布必须为main上的显式手动授权，且四来源全部成功、当前测试通过。
3. 要求新tag；存在同名tag或Release则拒绝，不再删除旧资产。

**现有main仍为2966036，工作索引保持；改动推送至隔离验证/发布分支，尚未合并main。**
公开新Release已完成，见首节；原v0.3.0保持，数据库未安装到410。
本轮发布通过单独的固定artifact发布流程完成，不重跑来源提取、不开启旧main的自动覆盖流程。

## 本地证据位置

- `.local/evidence/ims-switch-actions-20261004/carrier-snapshot.json`
- `.local/evidence/ims-switch-actions-20261004/carrier/verified.json`
- 同目录 `artifact.zip`、`catalog-set/`、Actions日志及 `actions-verification.json`。

下载时WSL到GitHub曾出现No route to host；随后使用Windows Python/PowerShell断点下载并核验成功。
这是产物传输问题，不是构建失败。没有为此更改网络配置、关闭校验或接触410。
