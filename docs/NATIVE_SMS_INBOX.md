# Native 直接短信、持久化收件箱与送达报告

代码检查点：`a1be268`。默认 MM 不变；本功能只接在已显式启用的 native 线路上。

## 接收与确认顺序

1. 线路 present/enabled、`sms_reception_enabled` 及 IMS/CS 接收策略准入。
2. 在同一物理操作锁下核验 SIM，绑定稳定 SIM 摘要，再初始化 PDU 模式。
3. `+CMT`/`+CDS` 与命令回复分离，校验 SMSC/TPDU 长度与类型；最多保留 16 个私有 PDU。
   公共事件仍只有 boolean 提示，不包含号码、正文、PDU 或 SIM 摘要。
4. 每个 PDU 先提交 SQLite 私有 inbox，**提交成功后**才允许 CNMA 或移除 modem 存储。
   不等待全部分片才确认某一片；重启后从已持久化的分片继续组装。
5. 完整消息、SIM 作用域去重、`sms.received` 事件和 inbox consumed 状态在一个事务内提交。
   提交后接入现有短信广播/trunk bridge 与通知入口。

普通 MT 仍使用 `AT+CNMI=2,1,0,1,0` 的存储模式，保留 15 秒扫描兜底，不主动启用 direct-only。
存储中的 PDU 也进入同一个 inbox；删除前重新核验 SIM 与索引的精确 PDU 内容。

### CNMA 限制

读取 `AT+CSMS?` 的当前 service；只有 service=1 且 MT 支持明确时考虑 `AT+CNMA=1`。
service=0 不需要 CNMA；unknown 不猜测。

CNMA 没有消息 ID，因此仅接受唯一、完整、当前 SIM/会话、未丢帧且收到不超过 10 秒的 token。
多条待确认、溢出、分帧错误、旧连接或超时都不能授权盲 ACK。写前退休 token，失败不重发它。
丢帧/不明 service 导致的 ACK fence 不自动清除；需重新建立经过确认的接收会话，不能简单
清一个标志继续确认后来消息。10 秒只是本地保守上限，不是所有固件 ACK 定时器的保证。

## 持久化与重组

- 私有表：`native_sms_inbox`、`native_sms_received`；位于现有应用 SQLite 数据库中。
- 待处理/隔离 PDU 每 line 最多 256、全库最多 4096；满时拒绝新提交，不 ACK、不淘汰未消费数据。
- 完成记录保留每 line 最近 2048 个指纹；已完成记录清除原始 PDU。
- 稳定 SIM scope 包含物理 key、slot 和 SIM 身份摘要；不会把前一张 SIM 的队列归给新卡。
- 分片按 SIM、发送者、PID/DCS、UDH（区分 8/16-bit reference 与端口 IE）、reference/total 及
  最多 5 分钟 SCTS 窗口归组。相同分片幂等，冲突/缺片不拼接，无效或截短 PDU 隔离保存。
- native 内去重原子提交，不把旧的孤立 `sms_dedup` claim 当作入库证据。

**边界**：旧 MM/IMS 记录没有可靠的 SIM 作用域，本功能不据其相同号码/正文/时间去丢弃另一张
SIM 的 native 消息。默认 IMS 接管时 native modem 接收仍暂停；显式启用 CS fallback 时，跨
native/旧 IMS 接入面的完全统一 SIM-scoped 去重及通知/trunk 崩溃重放不在本轮实现范围。
应用 SMS/inbox 的持久化不等同于外部通知恰好一次送达。

## 发送与送达

native 发送入口在第一片发送前持久化 outgoing pending 记录和预期总片数。整个发送任务由
物理操作锁及独立任务保护，不因 HTTP 调用者取消而放弃账本。

- native SUBMIT 设置 TP-SRR 请求报告；共享 IMS RP-DATA 编码不变。
- 每片确认的 `+CMGS: <mr>[,<ackpdu>]` 保存实际 MR 和发送时间区间。
- 任一片结果或账本提交不明，返回 `submission_state=unconfirmed`、保留 pending 记录及已提交
  前缀；不自动转其它路径重发。UI 显示“待确认”，提醒勿直接重发。
- 报告必须同 line、SIM、收件人（不猜 national/international 等价）、实际 MR，且 TP-SCTS 落入
  该片发送区间的窄容差内。多候选/未知提交不标送达；报告先到时保留并重试关联。
- 仅 TP-ST=0 是确认送达；全部预期分片均确认后才把 `sms_messages.status` 改为 `delivered`。
  已确认成功不会被后来的临时/失败报告降级；SMS-COMMAND 报告不关联 SMS-SUBMIT。
- 发送账本保留 7 天已确认历史；未确认记录不自动删，达到上限时阻止继续盲发。

## 验证

`a1be268`：Validate `35984847888`、Build `35984847830`、Frontend `35984847857` 全部成功；
amd64-musl/arm64-musl 成功，Publish Release skipped。本地 123 项 Python 守卫通过。
新增 DB 事务/重启、socket-pair ACK、PDU codec、分片、报告先到/碰撞/多段/失败回归实际运行。

未发送实网短信、未拨号、未在 SIM-04 切 native；native 固件的 CNMI/CSMS/报告格式仍需独立实机验收。
