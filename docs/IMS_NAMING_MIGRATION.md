# IMS 命名、兼容接口与持久化迁移

> 2026-09-26 合并第一阶段说明与第二阶段计划的现行结论。
> 命名迁移已经完成；不要看到历史 `volte_*` 字符串就再次机械改名。
> 原始清单、决策演进和提交记录见文末档案。

## 1. 语义

- `cellular_ims` 表示蜂窝 IMS 接入/注册，语音、短信和补充业务共享它。
- **VoLTE/VoNR 是语音能力**，不是 IMS 注册开关。实际语音标志、第三方原始字段、历史记录中的
  VoLTE/VoNR 命名不应全部改成 IMS。
- `carrier_Bundles` 注册字段为 `lte_ims_status` / `nr_ims_status`；readiness 已与语音能力解耦，
  完整 SMS-only IMS 配置不应仅因 `services.volte=false` 被否决。

## 2. 当前接口约定

| 范围 | 规范写出/调用 | 兼容 |
|---|---|---|
| HTTP | `/api/cellular-ims/*` | 保留 `/api/volte/*` 旧别名，鉴权和响应契约不能分叉 |
| 运行态与 profile 字段 | `cellular_ims`、`cellular_ims_profiles`、`cellular_ims_ready` | serde 兼容旧字段读取 |
| 线路配置 | `cellular_ims_connection_enabled`、`cellular_ims_auto_restore`、`cellular_ims_profile_selection`、`cellular_ims_ip_families` 等 | 旧 `volte_*` 作为 alias，保存写新名；同时给新旧拼写仍拒绝歧义 |
| SIM 覆写 | `ims_cellular` / `ims.cellular_ims` | 旧 `ims_volte` / `ims.volte` 兼容读取 |
| 接入种类 / UT access | `cellular_ims` | 读取旧编码的兼容边界保留 |
| 环境变量 | `SIMADMIN_CELLULAR_IMS_PCSCF`、`SIMADMIN_CELLULAR_IMS_CID` | `SIMADMIN_VOLTE_*` 仍作回退 |
| 前端/Bruno | 使用规范路由、新 JSON 字段及错误码 | 与后端同包发布，不混搭新旧资源 |

模块位于 `backend/src/connectivity/modems/ims/cellular_ims/`。保留兼容路由并不是迁移未完成。

## 3. 错误码

- 后端固定码集中在 `cellular_ims/errors.rs::code`，规范前缀为 `cellular_ims_*`。
- 前端 `cellularImsErrorCodes.ts` 由 `.github/scripts/gen_cellular_ims_error_codes.py` 生成，
  `cellularImsErrorFormat.ts` 按完整 token 查表，不靠 `includes()` 或含糊的前缀匹配。
- 守卫检查声明/`code::ALL`/前端生成表一致、无重复/子串歧义，调用方不得重新散落码值。
- `last_error` 的 IMS runtime 错误码不是原数据库的持久化列。其他业务里的 `last_error`、
  通话 `failure_code` 等仍是独立词表，不可因为同名而一起改写。
- 断连原因、格式化前缀、配置键、短信 transport 不是同一语义，不能把它们放进一张字符串替换表。

## 4. 已完成的数据库联动（不是待办）

用户后续将原先延期的持久化名称纳入第二阶段第 7 步，代码已有迁移与兼容读取：

1. `volte_refresh_stats` 合并到 `cellular_ims_refresh_stats` 后删除旧表；旧版重新建表写入后，
   再升级可以重新迁移，不丢失已有记录。
2. `sms_messages`、`sms_dedup`、`app_events` 的 transport 从 `volte_ims` 归一到 `cellular_ims`。
   后端归一化、通知/诊断标签和前端仍读取旧值。
3. `app_events` 的 `volte.*` 类型迁移到 `cellular_ims.*`。
4. `volte-mt:` 短信标记迁移到 `cellular-ims-mt:`；内容指纹去重语义保留，历史相关行一起迁移。
5. `volte_enabled` 原本是线路配置 JSON 键，不是 SQL 列；由 serde 兼容处理。

旧文档“永不修改 `volte_ims`”的决定已经被上述**带迁移的实现**取代。正确做法是保留迁移与旧值读取，
不是回退数据库，也不是将所有历史文件/第三方字段全局替换。

## 5. 验证与边界

- `71513ea` 完成配置、JSON、持久化迁移；`2129282` 将数据库迁移用例加入两个实际执行的 CI 过滤器。
- `legacy_volte_persisted_names_migrate_to_cellular_ims` 覆盖旧数据、降级后再升级、二次启动无副作用。
- `2129282`：Validate `35947763715` / Build `35947763717` success，双架构成功，Publish skipped；
  `carrier_Bundles:558a505` 的配套 CI 通过。具体历史记录见归档，不能当新提交的 CI 结果。
- 旧 sealed catalog 不原地改写；重新提取/封存/发布是独立流程。
- 这是一轮命名/兼容性变更，不证明所有卡、native 后端、真实语音或双注册已通过。

## 6. 档案

- [第一阶段原文](archive/2026-09/IMS_NAMING_MIGRATION_PHASE1.md)
- [第二阶段计划、完成清单与提交](archive/2026-09/IMS_NAMING_PHASE2_PLAN.md)
- [当前接手](HANDOFF.md) / [开发总计划](DEVELOPMENT_PLAN.md)
