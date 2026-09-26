# IMS 只读诊断与日志投影

规范脚本：[`scripts/ims-readonly-evidence.sh`](../scripts/ims-readonly-evidence.sh)。
当前任务/设备状态见 [接手入口](HANDOFF.md)，本文只维护工具契约，不另存一份进度清单。

## 1. 覆盖的证据

- 初始 REGISTER 请求、AKA challenge、认证请求、实际发送端口。
- 未完整收到响应、终止 SIP 响应、注册成功、原通道 refresh 和重试。
- restore 失败、P-CSCF 观察/轮换、承载结束、UE worker 生命周期。
- `/opt/simadmin/meta.json` 安装信息与 `/proc/<MainPID>/exe` 实际运行哈希分开输出。
- 在日志采样前后复核 PID/start ticks，不能把安装 metadata 自动当作运行版本。

只投影明确事件中的类型化白名单：地址族、端口、布尔存在标志、认证轮数、SIP 状态码、
CSeq、已知错误码、租期/重试时间。不回显原始日志行或自由文本错误详情。

真实 `%error` 可能是未加引号的 `code:detail`，并含嵌入引号或假字段。脚本只取白名单 code，
把后续整段当作不透明详情，禁止把其中的 `sip_status=` / `request_cseq=` 当成元数据。
未知错误为 `unlisted_error_redacted`，不能猜测原码或用它判断具体根因。

## 2. 不输出与不执行的内容

- 不输出号码、SIM 身份、PDU、IP/P-CSCF 地址、SIP URI、Cookie、RAND/AUTN/CK/IK/RES 等认证材料。
- 不执行 AT/QMI、API 写入、承载创建、启停服务、owner 切换、NV/USB 写入或重启。
- 不读取应用配置/数据库，不更改密码，不自动 retry/reconnect，不伪造硬件恢复。
- 不需要设备安装 Python/jq/JSON::PP；依赖 POSIX sh、Perl、systemctl/journalctl 与常规 coreutils。

## 3. 使用

在已有授权入口恢复后，可通过 SSH exec 的 `sh -s` 从 stdin 发送脚本，不必在设备安装文件。
设备操作者审阅后也可本地执行：

```sh
sh scripts/ims-readonly-evidence.sh
```

只对已安全取得的日志做离线投影，不查询服务/设备：

```sh
sh scripts/ims-readonly-evidence.sh --filter-log < private-journal.txt
```

原始 journal 保持私有，不提交或直接粘贴。当前本机 SSH 入口在 `.local/active/ims/`，
位置与凭据规则见 `.local/README.md`；旧 `.codex-*` 生成器/部署脚本在本地 archive，不能重放。

## 4. 输出上限与解释

- 仅读本次开机最近 6000 行 journal，最多输出 120 条匹配记录。
- 单行超过 8192 字节整行丢弃并计数，不让超长无换行输入耗尽内存。
- journal 读取失败显式 `journal_read_failed=true` 并以非零退出，不伪装成空白成功。
- `matched=0` 可能表示时间窗口、日志级别、格式或版本不匹配，**不证明从未尝试注册**。
- `oversized>0` 说明证据不完整。
- `running_process_stable_during_sample=true` 只证明采样期间主进程没变。
  `journal_scope=current_boot_all_service_processes` 可能包含当前开机内旧程序或子进程日志，
  不能都归到当前哈希，也不能未经 API/时间/实例核对就归到某张 SIM。

## 5. 与线路状态联合定位

另行经授权应用登录，只 GET 线路列表/详情和 `/api/modem/backend`，核对真实 line ID、
SIM 作用域、backend、runtime 的 `phase/stage/last_error`、尝试记录及下一重试时间。

- `radio` / `bearer`：核对附着、IP 族、owner 和未解决 receipt；不写 NV 或猜 CID。
- `pcscf`：核对本次承载与实际发现来源；不能直接复用旧卡地址/APN。
- `REGISTER_NO_COMPLETE_RESPONSE`：结合认证轮数区分初始/认证后传输失败，不当作收到 403。
- `REGISTER_CHALLENGE`：只证明走到挑战处理，不证明 AKA 已通过；后续失败需核对实际错误码。
- `REGISTER_TERMINAL_RESPONSE`：保留真实 SIP 状态和事务阶段，但状态码本身仍不证明根因。
- `REGISTER_SUCCESS`：初始与 refresh 分开；重新建会话不是原通道续期，注册也不是业务验收。

## 6. 回归

`.github/scripts/test_ims_readonly_evidence.py` 的 28 项测试覆盖实际源日志字段、
`aka_empty_uri_first` 授权枚举、SIP/无响应、引号与未引用 error 注入、类型与重复字段、
上限/超长行、journal 失败以及模拟采样期间 PID 变化。

本地只做 Python/语法检查，Rust 编译与回归仍交给 Actions；最新实际执行结果见接手记录。
