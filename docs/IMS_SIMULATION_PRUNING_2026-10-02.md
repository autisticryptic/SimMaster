# 派生注册模拟、原格式直接裁剪与410安装

> 2026-10-03 更新：下文为10月2日安装现场。数据库`c445d53`已通过SimAdmin现有SSH认证推送，
> 后继报告统计修复`f9cc1d3`也已推送。410重启恢复和EID修复已部署，正式IPsec注册及自然续期通过；
> 详情见[最新部署记录](IMS_REBOOT_RECOVERY_2026-10-03.md)。
> minimal实际减幅及判定局限见[精简差距审查](CATALOG_PRUNING_AUDIT_2026-10-03.md)，没有再次扩大删库范围。

## 已交付

- `offline-registration-sim/` 复用真实派生工厂、两接入请求构造/候选、共享 REGISTER 和 Digest-AKA，
  使用内存对端与合成SIM结果。19场景通过：12注册正例、7预期拒绝；无真实网络/硬件证明。
- 独立 `../carrier_Bundles` 中的 `simulations/ims_registration/` 包含可移植运行器；
  `simulation_pruning/` 包含直接筛选逻辑和源码/测试摘要绑定的报告。
- 按用户澄清保持原 `schema v7 / carrier-bundles-ims-v1`。v2格式/生成规则/解码器已全部撤回。
  LTE和VoWiFi分别直接删除可覆盖的接入配置，其他策略与NR保留；全部接入可覆盖才整行删除。
- 最终产物：`../carrier_Bundles/data/variants/2026-10-02-direct-final/`，共12个库。
  删除614个LTE IMS接入、4个VoWiFi接入，其中2条Profile整行删除。
- 46项数据库Python测试通过，12库摘要/完整性/外键/格式通过；实际消费者确认618项通过现有
  来源绑定路径进入派生解析，11326项其他投影保持，NR不变。

模拟通过仅说明所建模的条件。部分iOS/VoWiFi配置含媒体、开通、加密、专属域名或未知策略，
仍保留，不能把未覆盖误称成必然无法派生注册。更多细则在数据库项目 `docs/CATALOG_VARIANTS.md`。

## Git提交

数据库独立仓库已提交：`c445d5327e721407505643d328595378e8849a2a`，本地main、工作区干净。
**推送未成功**：WSL Git和Windows Git均返回 `fatal: unable to get password from user`。
远端main仍为 `558a5053c43f4c078512cebe05c12b97bb3c2136`。需恢复GitHub凭据后正常push，不能
说已经提交到了GitHub。提交使用skip-ci以避免push自动触发大固件下载和覆盖历史Release；
本次验证为本地执行，不是新的线上CI。

## 410安装结果

原WLAN地址192.168.100.13不可达；通过历史有线地址 **http://192.168.68.1:3000** 连接，
验证原SSH host key和machine reference确认是同一设备。

- 全部12库、manifest/报告安装到 `/opt/simadmin/catalogs/direct-pruned-c445d53/set/`。
- 当前启用 `carrier-bundles-pixel-mustang-minimal-no-icons.sqlite3`，复制到正常运行路径
  `/opt/simadmin/carrier-bundles.sqlite3`。
- API确认 `installed=true / usable=true / sealed=true`，可读1166个cellular IMS、980个VoWiFi配置。
- 配置和SQLite一致性快照备份在 `/opt/simadmin/catalogs/direct-pruned-c445d53/backup/`。
- 没有重启主服务/MM/基带；主PID527保持，全部线路配置指纹不变。前端仍是此前已部署版本。

### IMS验收未通过，且问题在本次安装前已存在

安装前后均显示 `disabled`、`registered=false`，`cellular_ims_connection_enabled`仍为true，
错误为 `cellular_ims_bearer_session_lost:mm_ims_profile_runtime_recovery_unresolved`。
只读检查确认持久化v2 lease仍指向旧PID93382、runtime active、`same_boot=false`。
这说明设备在此前UI部署之后经历过重启，旧会话账本尚未安全结案；不是本次数据库安装造成。

未直接删除账本、伪造清理完成或反复激活。须按严格库存/租约恢复流程另行处理并重新验收IMS。
数据库可加载不等于IMS注册已恢复。

证据：`.local/evidence/catalog-410-final/{preflight,install,ims-recovery-readonly}.json`；
最终本地测试日志在 `.local/continuation-20261001/{pruning-final-python,direct-final-consumer}.log`。
