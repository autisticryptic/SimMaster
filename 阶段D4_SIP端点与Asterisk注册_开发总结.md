# 阶段 D4 开发总结：每线路 SIP UDP Endpoint 与 Asterisk REGISTER

> 日期：2026-07-16  
> 分支：`codex/ims-core-stage-a-b-live`  
> 本阶段边界：完成 Asterisk 方向 SIP UDP endpoint、双模式运行时和 REGISTER；D5 已补上事件驱动的 INVITE 对话控制面，真实 IMS 接线、RTP relay 和语音/视频仍属于 D6-D7。

## 一、已完成内容

### 1. 每线路独立运行时

- 每个 `LineRuntime` 持有独立 `TrunkRuntime`、配置快照、任务代次和驱动任务。
- 启动、热插拔刷新、API 保存和开关变更都会按 `line_id` 独立协调，不共享 SIP socket。
- 运行态对外报告 `phase/stage`、peer、注册状态、SIP 状态码、注册/到期/重试时间、尝试和重连次数。
- 配置变化会取消旧代次；旧任务不能覆盖新配置的状态。

### 2. SIP UDP endpoint

- DNS/IPv4/IPv6 地址解析与 connected UDP peer 校验。
- UDP REGISTER 重传、CSeq 匹配、临时 1xx 忽略、最终响应处理。
- `static_peer` 不发送 REGISTER，但会打开双向 SIP socket、发送 CRLF keepalive 并应答 OPTIONS。
- D5 前，INVITE 明确返回 503；D5 控制面接入后，INVITE 先返回 100 Trying，再由 IMS 能力决定 480/200；无效对话的 BYE/CANCEL 返回 481。

### 3. 主动 REGISTER 与鉴权

- 标准 SIP Digest MD5、MD5-sess。
- 支持 `qop=auth` 和无 qop，支持 401 `WWW-Authenticate` 与 407 `Proxy-Authenticate`。
- 支持 423 `Min-Expires`、服务端返回的 Expires、85% 周期刷新。
- 失败采用 5 秒起步、上限 300 秒的指数退避；配置变更/关闭可中断退避。
- 密码只存在持久化配置和内存；API、运行态、日志和本文档均不输出明文。

### 4. Contact 稳定性与注销

- 所有启用的 Trunk 都必须配置稳定、每线路唯一的本地 SIP UDP 端口；不再允许主动注册使用随机端口。
- 多线路建议按 `5062、5064、5066...` 分配，避免端口冲突并保证进程重启后 Contact URI 不变。
- 配置切换或关闭时，已注册驱动会先用同一 socket 发送 `REGISTER Expires: 0`，等待短暂注销窗口后才强制终止。
- Asterisk AOR 推荐 `max_contacts=1` + `remove_existing=yes`，用于处理设备异常退出后遗留的旧 Contact。

### 5. 配置与 UI

- `TrunkProfileConfig` 新增 `local_port`。
- 配置弹窗展示本地 SIP 监听端口、稳定 Contact 的说明和逐线路唯一性提示。
- 线路卡可区分“已注册”“静态 Peer 已监听”“退避/异常”和禁用状态。
- 新增 `SIMADMIN_CONFIG_PATH`，允许真机候选版本完全绕开正式 `/data/config.json`。

## 二、验证

- 后端全量：516 项测试通过（包含 D5 Bridge Mock）。
- `cargo clippy --all-targets -- -D warnings` 通过。
- 前端 lint、TypeScript 类型检查通过；此前 D3b/D4 UI 已完成完整构建和桌面/390px 浏览器验证，零横向溢出、零控制台错误。
- Mock Asterisk 覆盖：401 Digest→鉴权 REGISTER→200、MD5-sess、静态 Peer OPTIONS、REGISTER UDP 回环、稳定本地端口、关闭时 Expires 0 注销。

## 三、Git 检查点

- `2b78d5f feat(trunk): add per-line runtime and configuration UI`
- `47b57d4 feat(trunk): add UDP endpoint and digest registration`
- `d97f37f fix(config): allow isolated candidate config path`
- `6d10cae fix(trunk): keep contacts stable and unregister cleanly`
- `d2debb3 fix(trunk): reject duplicate per-line SIP ports`

## 四、高通 410 / Asterisk 实测进度

设备保持正式 `simadmin.service` inactive，候选仅监听 `127.0.0.1:3101`，配置和数据库位于独立 release 目录，`data_enabled=false`，未建立蜂窝数据或 XFRM。

第一版候选向 `10.0.0.3:8060` 发出 REGISTER，随后使用 `6d10cae` 候选完成终验：

- 首次请求直接得到 SIP 200，运行态进入 `registered`，Expires 3600 秒。
- 将注册周期改为 60 秒时，旧实现因随机本地端口产生第二个 Contact；FreePBX 的 AOR 只允许一个 Contact，已有 `sip:41000@10.0.0.116:59448`，因此返回 403。
- 复用旧 Contact 的 59448 端口后，真实 401/407→Digest→200 成功，`register_attempts=2`，确认 InAuth、用户名和密码均实际生效。
- 60 秒注册周期连续刷新多轮，每轮均完成 challenge + authenticated REGISTER，`registered_at/expires_at` 持续前移，重连计数保持 0。
- 新候选先在 59448 接管现有 Contact；关闭时约 0.7 秒完成 Digest `Expires: 0` 注销；随后固定端口 5062 可立即注册，证明旧 Contact 已从 AOR 删除。
- 对候选进程执行强制终止后，使用同一 5062 端口重启，Digest 注册立即恢复，无 403，证明异常重启时稳定 Contact URI 可复用。
- 多线路配置层新增端口冲突门禁：另一条已启用线路不能占用相同本地 SIP 端口。
- 测试结束后完成正常注销并停止候选；正式服务 inactive、3101 关闭、`wwan0` DOWN、XFRM state/policy 0/0、ModemManager active。含凭据的隔离配置、数据库和日志均已删除，只保留校验过的候选二进制。

### 2026-07-17 外部 Asterisk 3600 秒注册复验

- 使用用户给定的真实 PJSIP Trunk 参数复验：远端 `10.0.0.3:8060`、本地稳定端口 `5062`、账号 `41000`、来电扩展 `6108`、注册有效期 `3600` 秒；凭据明文未写入日志或文档。
- 候选继续仅监听 `127.0.0.1:3101`，正式 `simadmin.service` 保持 inactive，`data_enabled=false`、VoLTE runtime disabled，未建立 IMS bearer。
- 运行态确认 `registered=true`、`last_sip_status=200`、`register_attempts=2`（Digest challenge 后成功）、`expires_at-registered_at=3600s`、`reconnect_count=0`；设备 UDP `10.0.0.116:5062` 已连接远端 PJSIP 端口。
- 候选持续运行约 80 分钟，跨过 3600 秒有效期在 85%（3060 秒）处的计划刷新时点，期间未出现 Trunk 降级或重连告警；停止前未再次采集 API 状态，因此该条仅记为长窗口无异常观察，不替代后续多轮 refresh/断网 soak。
- 修复 RFC 3261 注册有效期优先级：200 响应同时包含 Contact `expires=` 与全局 `Expires` 时，按 Contact 级绑定有效期刷新；新增回归测试与无凭据的注册成功日志。Git 检查点：`3ed7628 fix(trunk): honor contact registration expiry`。
- 本地质量基线更新为 523 项测试全绿，`cargo clippy --all-targets -- -D warnings` 通过。
- 测试结束后正常停止候选并清理设备与本地临时凭据；确认 5062/3101 均释放、正式服务 inactive、`wwan0` DOWN、XFRM state/policy 为 0。

## 五、剩余 D4 联调项

- [x] 在 Asterisk InAuth 下验证真实 401/407→Digest→200。
- [x] 使用稳定本地端口 5062 完成 60 秒 REGISTER refresh。
- [x] 验证关闭时 Asterisk Contact 立即删除，重新启用不会产生第二个 Contact。
- [x] 验证进程异常退出后同一 Contact URI 可恢复。
- [ ] static peer 真机 OPTIONS/来源地址匹配。

## 六、后续阶段

- D5：✅ Asterisk 侧 INVITE/UAS-UAC 对话控制面、事务状态机、Mock bridge；详见 `阶段D5_Asterisk对话桥接控制面_开发总结.md`。
- D6：运营商 IMS live voice 事件接线、音频/视频 RTP relay、SDP 对接和 `TrunkVideoSeam`。
- D7：双向 re-INVITE、真实 VoLTE/ViLTE 通话、鉴权/ACL/抓包安全验收。
- Web 电话仍为最终 Todo，只连接 Asterisk WSS，不进入 SimAdmin 管理前端。
