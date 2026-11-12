# 无忧行原生离线积压：持久消费与运行边界

> 本文描述新增的 Rust 软件通路及其原字节依据。新增源码必须通过自身 SHA 的 Build-Release、Validate Beta Refactor 和正式制品核验；静态分析、夹具或历史托管备份都不是当前服务器认可、手机授信复用或真实短信时延的证明。

## 1. 接在什么位置

```text
本人原 App UUID + 同账号/号码
→ 本次 getCallStatus 明确认可，业务已开启或经过明确开启/读回
→ 取得本次电话凭据、校验安装模板
→ 原 SDK loginSession、原 SipGateway 注册成功
→ 原 receiver 配置仍有效且账号 Hosted
→ 原连接 getOfflineMessages / moreOfflineMessages
→ 原包 + 已支持的 SMS + 通知任务 + 事件 + cursor 事务提交
→ 才允许请求下一页
```

此功能不改变原 UUID、不 enroll、不发 VTD、不制造运营商 token，也不自动切换另一账号、SDK 身份、TLS 根证书或接收域。SDK 在线、业务已开、已配置接收器和真实收件仍是不同状态。

## 2. 显式安装选项

在已核验的私有安装模板 `profile` 内设置：

```json
"offline_reception": true
```

- 默认关闭；旧配置未提供该字段时保持原在线行为，私有往返序列化也省略 false。
- 必须已有正确的 `incoming` 对象、域和 receiver kind；开启选项不创建这些值、不把名字字符串当成真实绑定。
- 接收器只在匹配的原生登录之后安装，取件只由 Hosted 的原账号消费器启动。
- 请求中的 UID、selector 和 ARC token 来自本次匹配的 `loginSession`，不是文件缓存、App token、SIP 密码或用户输入的替代值。
- 本次源码没有替设备配置猜测默认域、ClientEx、AccountAlive 或媒体信任，也没有自动修改已有 410 模板。

## 3. 已核对的原格式

原 `libmtc.so` SHA-256：`57086cd624d8227dc1b1b64a63d7c8b2258f87b261779b681a9bfd306ad9d2e8`。手机当前 8.9.4.0 的该库与原分析样本字节相同；Java 层不同版本不据此混用。

| 原定位 | 行为 |
| --- | --- |
| `0x47A9F8 / 0x58EF34 / 0x5A4940` | `moreOfflineMessages.MessageReceiver.Client` 读取 position64，结果是 bool + Common footer；提示不是短信正文 |
| `0x5967C0` | ClientI 初始离线位置为 signed64 `-1`，不是 32 位 request ID |
| `0x1AFA10 / 0x5B898C` | `getOfflineMessages.AccountEntry.Account`：原版本列表 `[0]`、UID string、selector string、position64、limit32；原 limit 为100 |
| `0x5B57F8 / 0xE67B04` | 回复 bool、count32 × `(position64, raw blob)`、独立 server position64、Common footer |
| `0x58EB58 / 0x4A6384 / 0xE695C0` | 每个 blob 是独立 stream：kind string、属性 map、body blob；不重复方法版本前缀 |
| `0xE86DA0 / 0xE799AC / 0x1745BF8` | type2 构造器解析 JSON，但该消息所用的顺序 string/blob 槽 `0xE77EBC / 0xE77E1C` 抛 not-support；不猜其 JSON 属性 |

有序位置集合、负数初值、完整64位位置和必需 footer 均保留。重复位置、越界、截断、超限、错误 footer 或不可能的 server position 都拒绝，不能返回部分消费成功。

## 4. 数据不会被提前吞掉

- `OwnedOfflinePage` 只能由匹配的原 RPC 回复创建，携原 owner、scope、请求 ID/位置和不含 token 的身份摘要；HTTP 或任意原字节不能重新标注其来源。
- SQLite 游标按账号修订、真实 native UID/selector、原 AccountEntry 和 receiver 上下文隔离。新连接先请求 `-1` 以取得本次真实上下文，再复用相同上下文的持久游标；不借用另一主体的旧位置。
- 原包、已支持的 `Sip.Request` 文本投影、现有通知队列和事件与游标在同一个事务中提交。任何插入、通知规划或 cursor 写入失败都会回滚本页；不会发出后续位置请求。
- 不支持的嵌套格式保存在有界私有 quarantine 中，不落为 SMS、不推进 cursor、不假造 consumer ACK。可解析的未知 kind 保留为未分类原包。
- 非空页只按实际最大条目位置加1推进；空页按本次服务器位置加1。hint 不写入 cursor；无进展或服务器位置回退不自动重置/自旋。
- 只允许一页在途。仅命令尚未准入的 backpressure 可重试；超时、断连或不确定提交关闭原 scope，不重放可能已发出的调用。
- 只有已在先前事务中提交的 cursor，才可压缩先前已持久化且已分类的原包；未知原包不驱逐。删除 SMS 会清除其原包内容，重连重放不能重建已删除短信。
- 旧 direct/batch 唯一性、原行 ID 和 SQLite 自增水位保留。旧 CHECK 约束采用事务迁移；未知附加字段/触发器不猜、不删除，不创建设备回滚备份。
- 原私有收件资源上限仍约束在线、离线原包和 quarantine 总量；满额拒绝而非删未知数据腾空间。

`offline_committed` 仅表示本地消费提交，不代表服务器已收到 ACK。hint 的 bool ACK 仅表示接受取件提示。Router/TLS/batch ACK、原消息消费者完成和厂商离线唤醒不是同一层。

## 5. 验证与尚未完成项

新增回归覆盖独立字节、64位位置、负数初值、错误/未知格式、匹配关系、身份隔离、事务回滚、删除/重放、旧表迁移、自增水位、原内存 TLS 账号闭环及原 Router/SARC/Common 通路。运行测试仅在本次 SHA 的两个 Actions；原回归、单线程门禁、超时和失败证据不削弱。

仍需按真实证据核验：

1. 同本人账号/号码和原 App UUID 的本次服务器认可复用。
2. 实际 MessageReceiver/ClientEx 对象与域、适用 AccountAlive，以及完整媒体/通知装配。
3. 已授权 410 部署后外来短信、来电/双向音频、断连、`stop → logoutSession → Join` 和端到端时延。
4. 服务端是否删除/重放积压、授信 TTL/撤销等服务器行为；客户端事务回归不能证明这些规则。

手机已带走，本轮不再查询 ADB。确需真实收件条件时再请求连接和相应操作确认；不通过新 UUID/VTD、强停/恢复 App 或付费外发补造成功。

相关说明：[手机材料读取](JEGOTRIP_PHONE_BACKUP_IMPORT.md)、[最小配置](JEGOTRIP_MINIMUM_CONFIGURATION.md)、[原生分层交付](JEGOTRIP_NATIVE_HOSTING_DELIVERY.md)。
