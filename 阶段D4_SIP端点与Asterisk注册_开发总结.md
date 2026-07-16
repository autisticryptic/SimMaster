# 阶段 D4 开发总结：每线路 SIP UDP Endpoint 与 Asterisk REGISTER

> 日期：2026-07-16  
> 分支：`codex/ims-core-stage-a-b-live`  
> 本阶段边界：完成 Asterisk 方向 SIP UDP endpoint、双模式运行时和 REGISTER；INVITE 对话桥接、RTP relay、真实语音/视频仍属于 D5-D7。

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
- D5-D6 桥接前，INVITE 明确返回 503，不伪报通话能力；无效对话的 BYE/CANCEL 返回 481。

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

- 后端全量：506 项测试通过。
- `cargo clippy --all-targets -- -D warnings` 通过。
- 前端 lint、TypeScript 类型检查通过；此前 D3b/D4 UI 已完成完整构建和桌面/390px 浏览器验证，零横向溢出、零控制台错误。
- Mock Asterisk 覆盖：401 Digest→鉴权 REGISTER→200、MD5-sess、静态 Peer OPTIONS、REGISTER UDP 回环、稳定本地端口、关闭时 Expires 0 注销。

## 三、Git 检查点

- `2b78d5f feat(trunk): add per-line runtime and configuration UI`
- `47b57d4 feat(trunk): add UDP endpoint and digest registration`
- `d97f37f fix(config): allow isolated candidate config path`
- `6d10cae fix(trunk): keep contacts stable and unregister cleanly`

## 四、高通 410 / Asterisk 实测进度

设备保持正式 `simadmin.service` inactive，候选仅监听 `127.0.0.1:3101`，配置和数据库位于独立 release 目录，`data_enabled=false`，未建立蜂窝数据或 XFRM。

第一版候选向 `10.0.0.3:8060` 发出 REGISTER：

- 首次请求直接得到 SIP 200，运行态进入 `registered`，Expires 3600 秒。
- 服务端没有返回 401/407，因此该次注册没有使用 Digest 密码；需要检查 PJSIP endpoint 的 `auth=` 绑定。
- 将注册周期改为 60 秒时，旧实现因随机本地端口产生第二个 Contact，Asterisk 返回 403；随后 8060 返回 UDP connection refused。
- 候选 Trunk 已关闭，停止继续重试。
- 针对上述问题已完成 `6d10cae`：强制稳定本地端口并在关闭/切换时主动注销。等待 Asterisk 清理旧 Contact、启用 `remove_existing=yes` 并恢复 8060 UDP 监听后复测。

## 五、剩余 D4 联调项

- [ ] 在 Asterisk 正确绑定 userpass auth 后，验证真实 401/407→Digest→200。
- [ ] 使用稳定本地端口 5062 完成 60 秒 REGISTER refresh。
- [ ] 验证关闭时 Asterisk Contact 立即删除，重新启用不会产生第二个 Contact。
- [ ] 验证进程重启后同一 Contact URI 可恢复。
- [ ] static peer 真机 OPTIONS/来源地址匹配。

## 六、后续阶段

- D5：Asterisk 侧 INVITE/UAS-UAC 对话、运营商 IMS live voice 接线。
- D6：音频/视频 RTP relay、SDP 对接和 `TrunkVideoSeam`。
- D7：双向 re-INVITE、真实 VoLTE/ViLTE 通话、鉴权/ACL/抓包安全验收。
- Web 电话仍为最终 Todo，只连接 Asterisk WSS，不进入 SimAdmin 管理前端。
